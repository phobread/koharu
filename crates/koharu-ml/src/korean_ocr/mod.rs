use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use image::{DynamicImage, GenericImageView, GrayImage, imageops::FilterType};
use ndarray::Array4;
use once_cell::sync::OnceCell;
use ort::{inputs, session::Session, value::TensorRef};
use serde::Deserialize;

use koharu_runtime::RuntimeManager;

const INPUT_HEIGHT: u32 = 48;
const DEFAULT_INPUT_WIDTH: u32 = 320;
const MAX_INPUT_WIDTH: u32 = 3200;

static ORT_INITIALIZED: OnceCell<()> = OnceCell::new();
static ORT_INIT_LOCK: Mutex<()> = Mutex::new(());

pub struct KoreanOcr {
    session: Session,
    characters: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LineRecognition {
    pub text: String,
    /// Mean of the emitted characters' scores (Paddle's CTC convention).
    pub confidence: f32,
    /// One score per `text` character: the CTC probability at the timestep
    /// that emitted it. Outlined dots and hearts drag the line mean down, so
    /// per-character scores let a confident syllable stand on its own.
    pub char_confidences: Vec<f32>,
}

#[derive(Deserialize)]
struct Metadata {
    #[serde(rename = "PostProcess")]
    post_process: PostProcess,
}

#[derive(Deserialize)]
struct PostProcess {
    character_dict: Vec<String>,
}

impl KoreanOcr {
    pub async fn load(runtime: &RuntimeManager) -> Result<Self> {
        let assets = runtime.ensure_korean_ocr_assets().await?;

        // ORT's environment is process-global. The lock prevents two pipeline
        // workers from racing the first lazy initialization.
        if ORT_INITIALIZED.get().is_none() {
            let _guard = ORT_INIT_LOCK
                .lock()
                .map_err(|_| anyhow::anyhow!("ONNX Runtime initialization mutex poisoned"))?;
            if ORT_INITIALIZED.get().is_none() {
                ort::init_from(assets.runtime_library.to_string_lossy())
                    .commit()
                    .context("failed to initialize ONNX Runtime")?;
                let _ = ORT_INITIALIZED.set(());
            }
        }

        let metadata: Metadata = serde_yaml::from_reader(
            std::fs::File::open(&assets.metadata)
                .with_context(|| format!("failed to open `{}`", assets.metadata.display()))?,
        )
        .with_context(|| format!("failed to parse `{}`", assets.metadata.display()))?;
        let mut characters = Vec::with_capacity(metadata.post_process.character_dict.len() + 2);
        characters.push("blank".to_owned());
        characters.extend(metadata.post_process.character_dict);
        // Paddle's CTCLabelDecode(use_space_char=true) appends this class but
        // does not serialize it into character_dict.
        characters.push(" ".to_owned());

        let session = Session::builder()?
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)?
            .commit_from_file(&assets.model)
            .with_context(|| format!("failed to load `{}`", assets.model.display()))?;

        Ok(Self {
            session,
            characters,
        })
    }

    pub fn recognize_line(&mut self, image: &DynamicImage) -> Result<LineRecognition> {
        let input = preprocess(image)?;
        let outputs = self
            .session
            .run(inputs![TensorRef::from_array_view(input.view())?])?;
        let output = outputs[0].try_extract_array::<f32>()?;
        let shape = output.shape();
        if shape.len() != 3 || shape[0] != 1 {
            bail!("unexpected Korean OCR output shape: {shape:?}");
        }
        if shape[2] != self.characters.len() {
            bail!(
                "Korean OCR dictionary has {} classes but model emitted {}",
                self.characters.len(),
                shape[2]
            );
        }

        let mut previous = usize::MAX;
        let mut text = String::new();
        let mut char_confidences = Vec::new();
        let mut score_sum = 0.0_f32;
        let mut score_count = 0_usize;
        for timestep in 0..shape[1] {
            let mut best_index = 0_usize;
            let mut best_score = f32::NEG_INFINITY;
            for class in 0..shape[2] {
                let score = output[[0, timestep, class]];
                if score > best_score {
                    best_score = score;
                    best_index = class;
                }
            }
            if best_index != 0 && best_index != previous {
                let emitted = &self.characters[best_index];
                text.push_str(emitted);
                char_confidences.extend(emitted.chars().map(|_| best_score));
                score_sum += best_score;
                score_count += 1;
            }
            previous = best_index;
        }

        Ok(LineRecognition {
            text,
            confidence: if score_count == 0 {
                0.0
            } else {
                score_sum / score_count as f32
            },
            char_confidences,
        })
    }

    pub fn recognize_block(&mut self, image: &DynamicImage) -> Result<Vec<LineRecognition>> {
        split_text_lines(image)
            .iter()
            .map(|line| self.recognize_line(line))
            .collect()
    }

    /// Like [`Self::recognize_block`], but on a dark bubble a line whose
    /// Hangul reading is below `minimum_confidence` is re-read with inverted
    /// polarity, and that reading is used when it clears the bar
    /// ([`prefer_trusted_polarity`]).
    pub fn recognize_block_with_fallback(
        &mut self,
        image: &DynamicImage,
        dark_bubble: bool,
        minimum_confidence: f32,
    ) -> Result<Vec<LineRecognition>> {
        split_text_lines(image)
            .iter()
            .map(|line| {
                let original = self.recognize_line(line)?;
                if !dark_bubble || hangul_confidence(&original) >= f64::from(minimum_confidence) {
                    return Ok(original);
                }
                let inverted = self.recognize_line(&invert(line))?;
                Ok(prefer_trusted_polarity(
                    original,
                    Some(inverted),
                    minimum_confidence,
                ))
            })
            .collect()
    }
}

fn invert(image: &DynamicImage) -> DynamicImage {
    let mut rgb = image.to_rgb8();
    for pixel in rgb.pixels_mut() {
        for channel in pixel.0.iter_mut() {
            *channel = 255 - *channel;
        }
    }
    DynamicImage::ImageRgb8(rgb)
}

fn preprocess(image: &DynamicImage) -> Result<Array4<f32>> {
    let (source_width, source_height) = image.dimensions();
    if source_width == 0 || source_height == 0 {
        bail!("cannot recognize an empty image");
    }

    let scaled_width =
        ((INPUT_HEIGHT as f64 * source_width as f64 / source_height as f64).ceil() as u32).max(1);
    let target_width = scaled_width.max(DEFAULT_INPUT_WIDTH).min(MAX_INPUT_WIDTH);
    let resized_width = scaled_width.min(target_width);
    let rgb = image.to_rgb8();
    let resized = image::imageops::resize(&rgb, resized_width, INPUT_HEIGHT, FilterType::Triangle);

    let mut input = Array4::<f32>::zeros((1, 3, INPUT_HEIGHT as usize, target_width as usize));
    for (x, y, pixel) in resized.enumerate_pixels() {
        // inference.yml specifies BGR, CHW, scaled from [0,255] to [-1,1].
        for (channel, value) in [pixel[2], pixel[1], pixel[0]].into_iter().enumerate() {
            input[[0, channel, y as usize, x as usize]] = value as f32 / 127.5 - 1.0;
        }
    }
    Ok(input)
}

/// Split a detected block into horizontal lines by finding ink whose
/// brightness opposes the dominant background. This handles both ordinary
/// dark-on-white bubbles and BadEnd's white-outlined lettering on black.
///
/// Rows with enough ink form natural spans. Two failure modes are handled
/// without discarding real lines:
/// - Tightly set lines (or artwork running between them) can fuse into one
///   tall span. When the bubble has at least two consistent line pitches
///   between normal-height neighbours, a span clearly taller than one line is
///   cut at the deepest projection valley near each expected boundary. With
///   no such evidence nothing is cut, so a single tall glyph is never halved.
/// - Detector boxes can clip a neighbouring caption. Only specks and partial
///   rows touching the crop's top or bottom edge are dropped; short interior
///   lines (e.g. after one fused span) are kept.
///
/// Each line is trimmed to its ink columns plus a margin.
pub fn split_text_lines(image: &DynamicImage) -> Vec<DynamicImage> {
    line_rects(&image.to_luma8())
        .into_iter()
        .map(|rect| {
            image.crop_imm(
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
            )
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineRect {
    top: u32,
    bottom: u32,
    left: u32,
    right: u32,
}

fn line_rects(gray: &GrayImage) -> Vec<LineRect> {
    let (width, height) = gray.dimensions();
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let mean = gray.pixels().map(|pixel| u64::from(pixel[0])).sum::<u64>() as f64
        / f64::from(width * height);
    let dark_background = mean < 128.0;
    let is_ink = |luma: u8| {
        if dark_background {
            luma >= 180
        } else {
            luma <= 75
        }
    };
    let profile = (0..height)
        .map(|y| {
            (0..width)
                .filter(|&x| is_ink(gray.get_pixel(x, y)[0]))
                .count() as u32
        })
        .collect::<Vec<_>>();

    let min_ink = (width / 50).max(4);
    let mut spans = Vec::new();
    let mut start = None;
    let mut last_active = 0_u32;
    for (y, &count) in profile.iter().enumerate() {
        let y = y as u32;
        if count >= min_ink {
            start.get_or_insert(y);
            last_active = y;
        } else if let Some(top) = start
            && y.saturating_sub(last_active) > 1
        {
            if last_active + 1 - top >= 8 {
                spans.push((top, last_active + 1));
            }
            start = None;
        }
    }
    if let Some(top) = start
        && last_active + 1 - top >= 8
    {
        spans.push((top, last_active + 1));
    }
    if spans.is_empty() {
        return Vec::new();
    }

    let span_height = |(top, bottom): (u32, u32)| f64::from(bottom - top);
    // Lower median, so one fused span cannot inflate the typical line height.
    let typical = lower_median(spans.iter().map(|&span| span_height(span)).collect());
    let normal = spans
        .iter()
        .map(|&span| (0.5 * typical..=1.3 * typical).contains(&span_height(span)))
        .collect::<Vec<_>>();
    let pitches = (0..spans.len().saturating_sub(1))
        .filter(|&i| normal[i] && normal[i + 1])
        .map(|i| f64::from(spans[i + 1].0 - spans[i].0))
        .collect::<Vec<_>>();
    let consistent = pitches.len() >= 2
        && pitches.iter().copied().fold(f64::MIN, f64::max)
            <= 1.35 * pitches.iter().copied().fold(f64::MAX, f64::min);
    // (line pitch, line height) measured from the bubble's own clean lines.
    let pitch = consistent.then(|| {
        let line = median(
            spans
                .iter()
                .zip(&normal)
                .filter(|(_, is_normal)| **is_normal)
                .map(|(&span, _)| span_height(span))
                .collect(),
        );
        (median(pitches.clone()), line)
    });

    let mut pieces = Vec::new();
    for &(top, bottom) in &spans {
        let span = f64::from(bottom - top);
        let fused = pitch.and_then(|(pitch, line)| {
            let parts = round_half_up((span + pitch - line) / pitch);
            (span > 1.7 * line && parts >= 2).then_some((pitch, line, parts))
        });
        let Some((pitch, line, parts)) = fused else {
            pieces.push((top, bottom));
            continue;
        };
        let segment = profile[top as usize..bottom as usize]
            .iter()
            .map(|&count| f64::from(count))
            .collect::<Vec<_>>();
        let base = percentile10(segment.clone());
        let middle = median(segment);
        let mut cuts = Vec::new();
        let mut previous = top;
        for i in 1..parts {
            let center = f64::from(top) + f64::from(i) * span / f64::from(parts);
            let low = (previous + (0.6 * line) as u32).max((center - 0.35 * pitch) as u32);
            let high = bottom
                .saturating_sub((0.6 * line) as u32)
                .min((center + 0.35 * pitch) as u32);
            if high <= low {
                continue;
            }
            let valley = (low..high)
                .min_by_key(|&y| (profile[y as usize], y))
                .expect("non-empty window");
            // Only a real valley separates two lines.
            if f64::from(profile[valley as usize]) - base <= 0.5 * (middle - base) {
                cuts.push(valley);
                previous = valley;
            }
        }
        let mut edges = vec![top];
        edges.extend(cuts);
        edges.push(bottom);
        pieces.extend(edges.windows(2).map(|pair| (pair[0], pair[1])));
    }

    let reference = pitch.map_or(typical, |(_, line)| line);
    let several = pieces.len() > 1;
    pieces
        .into_iter()
        .filter(|&(top, bottom)| {
            let piece = f64::from(bottom - top);
            let at_edge = top <= 1 || bottom >= height - 1;
            !(several && (piece < 0.25 * reference || (at_edge && piece < 0.6 * reference)))
        })
        .map(|(top, bottom)| {
            let columns = (0..width)
                .filter(|&x| (top..bottom).any(|y| is_ink(gray.get_pixel(x, y)[0])))
                .collect::<Vec<_>>();
            let margin = round_half_up(0.3 * reference.min(f64::from(bottom - top)));
            let (left, right) = match (columns.first(), columns.last()) {
                (Some(&first), Some(&last)) => {
                    (first.saturating_sub(margin), (last + 1 + margin).min(width))
                }
                _ => (0, width),
            };
            LineRect {
                top: top.saturating_sub(5),
                bottom: (bottom + 5).min(height),
                left,
                right,
            }
        })
        .collect()
}

fn round_half_up(value: f64) -> u32 {
    (value + 0.5).floor().max(0.0) as u32
}

fn sorted(mut values: Vec<f64>) -> Vec<f64> {
    values.sort_by(f64::total_cmp);
    values
}

fn lower_median(values: Vec<f64>) -> f64 {
    let values = sorted(values);
    values[(values.len() - 1) / 2]
}

fn median(values: Vec<f64>) -> f64 {
    let values = sorted(values);
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

/// numpy-style (linear interpolation) 10th percentile.
fn percentile10(values: Vec<f64>) -> f64 {
    let values = sorted(values);
    let position = 0.1 * (values.len() - 1) as f64;
    let low = position.floor() as usize;
    let high = (low + 1).min(values.len() - 1);
    values[low] + (values[high] - values[low]) * (position - low as f64)
}

pub fn is_dark_panel(image: &DynamicImage) -> bool {
    let gray = image.to_luma8();
    if gray.width() == 0 || gray.height() == 0 {
        return false;
    }
    let mean = gray.pixels().map(|pixel| u64::from(pixel[0])).sum::<u64>() as f64
        / f64::from(gray.width() * gray.height());
    mean < 128.0
}

pub fn contains_lexical_hangul(text: &str) -> bool {
    text.chars().any(is_lexical_hangul)
}

/// Replace PaddleOCR-VL's Hangul with the dedicated Korean recognizer's, by
/// aligning the two Hangul sequences and applying only confident substitutions.
///
/// On BadEnd's outlined lettering the dedicated recognizer reads the glyphs far
/// more reliably than the VL model, but the VL model also drops or merges whole
/// words, so the two Hangul streams differ in length. Rather than refuse the
/// whole block on a length mismatch, we align VL's Hangul against the verifier's
/// (Needleman–Wunsch) and overwrite a VL character only where it aligns
/// one-to-one with a verifier character from a line that clears
/// `minimum_confidence`. Insertions and deletions are left untouched: we never
/// add or remove a word, only swap an individual syllable the verifier is
/// confident about. Equal-length blocks align on the diagonal, so this reduces
/// to a straightforward per-position overwrite in the common case; a verifier
/// line the confidence gate rejects contributes nothing.
///
/// Alignment can be ambiguous around repeated syllables, but because we only
/// ever substitute (never indel) and only from trusted lines, the worst case is
/// swapping one already-uncertain syllable, not corrupting sentence structure.
///
/// A line is trusted by the mean confidence of its Hangul only
/// ([`hangul_confidence`]): outlined dots and hearts score low and would
/// otherwise sink a line whose syllables were read confidently.
///
/// VL sometimes emits ASCII junk where a syllable is (`8-?` for `응-?`,
/// `....CI?` for `...에?`). A run of ASCII letters/digits the verifier does not
/// also read counts as one substitutable slot: a trusted verifier syllable
/// aligned to it replaces the whole run. An unaligned run stays, so real Latin
/// text (`OK`, `TV`) is never deleted.
pub fn repair_hangul(
    paddle_vl: &str,
    dedicated: &[LineRecognition],
    minimum_confidence: f32,
) -> String {
    if dedicated.is_empty() {
        return paddle_vl.to_owned();
    }

    // Ordered verifier Hangul, each tagged with whether its source line is
    // confident enough to overwrite a VL character.
    let verifier = dedicated
        .iter()
        .flat_map(|line| {
            let trusted = hangul_confidence(line) >= f64::from(minimum_confidence);
            line.text
                .chars()
                .filter(|character| is_lexical_hangul(*character))
                .map(move |character| (character, trusted))
        })
        .collect::<Vec<_>>();
    let verifier_text = dedicated
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let vl = paddle_vl.chars().collect::<Vec<_>>();
    let units = repair_units(&vl, &verifier_text);
    if verifier.is_empty() || units.is_empty() {
        return paddle_vl.to_owned();
    }

    // Junk slots align as a character no verifier syllable can match.
    let unit_seq = units
        .iter()
        .map(|unit| unit.hangul.unwrap_or('\0'))
        .collect::<Vec<_>>();
    let verifier_seq = verifier
        .iter()
        .map(|(character, _)| *character)
        .collect::<Vec<_>>();
    let mut replacements = vec![None; units.len()];
    for (i, j) in align_sequences(&unit_seq, &verifier_seq) {
        // Only aligned pairs (matches/substitutions) apply; gaps are skipped so
        // no word is inserted into or deleted from the VL text.
        if let (Some(i), Some(j)) = (i, j) {
            let (replacement, trusted) = verifier[j];
            if trusted {
                replacements[i] = Some(replacement);
            }
        }
    }
    let mut output = String::with_capacity(paddle_vl.len());
    let mut position = 0;
    for (unit, replacement) in units.iter().zip(replacements) {
        output.extend(&vl[position..unit.start]);
        match replacement {
            Some(character) => output.push(character),
            None => output.extend(&vl[unit.start..unit.end]),
        }
        position = unit.end;
    }
    output.extend(&vl[position..]);
    output
}

/// A VL character range `repair_hangul` may overwrite: one Hangul character,
/// or (`hangul == None`) a junk run of ASCII letters/digits.
struct RepairUnit {
    start: usize,
    end: usize,
    hangul: Option<char>,
}

fn repair_units(vl: &[char], verifier_text: &str) -> Vec<RepairUnit> {
    let mut units = Vec::new();
    let mut i = 0;
    while i < vl.len() {
        if vl[i].is_ascii_alphanumeric() {
            let mut end = i;
            while end < vl.len() && vl[end].is_ascii_alphanumeric() {
                end += 1;
            }
            let run = vl[i..end].iter().collect::<String>();
            if !verifier_text.contains(&run) {
                units.push(RepairUnit {
                    start: i,
                    end,
                    hangul: None,
                });
            }
            i = end;
            continue;
        }
        if is_lexical_hangul(vl[i]) {
            units.push(RepairUnit {
                start: i,
                end: i + 1,
                hangul: Some(vl[i]),
            });
        }
        i += 1;
    }
    units
}

/// Mean per-character confidence over a line's Hangul (0 when it has none).
pub fn hangul_confidence(line: &LineRecognition) -> f64 {
    let (sum, count) = line
        .text
        .chars()
        .zip(&line.char_confidences)
        .filter(|(character, _)| is_lexical_hangul(*character))
        .fold((0.0_f64, 0_usize), |(sum, count), (_, &score)| {
            (sum + f64::from(score), count + 1)
        });
    if count == 0 { 0.0 } else { sum / count as f64 }
}

/// Which reading of a line to repair with on a dark bubble: the original,
/// unless it is untrusted and the inverted-polarity reading is trusted.
/// Plain inversion helps PP-OCRv5 on some white-outlined glyphs, but it is
/// only consulted when the original reading is unsure.
pub fn prefer_trusted_polarity(
    original: LineRecognition,
    inverted: Option<LineRecognition>,
    minimum_confidence: f32,
) -> LineRecognition {
    let bar = f64::from(minimum_confidence);
    match inverted {
        Some(inverted)
            if hangul_confidence(&original) < bar && hangul_confidence(&inverted) >= bar =>
        {
            inverted
        }
        _ => original,
    }
}

/// Restore word breaks VL dropped at the bubble's line breaks. Korean bubbles
/// break lines between words, and VL often runs lines together
/// (`리한을흉보는것만은싫어`). Where two adjacent syllables of `text` align to
/// the last syllable of one verifier line and the first of the next — or to
/// syllables the verifier itself separates with a space — and both agree with
/// the verifier, a space is inserted. Existing spaces are never removed.
pub fn space_at_line_breaks(text: &str, dedicated: &[LineRecognition]) -> String {
    // (syllable, line index, space before it within its line)
    let mut verifier = Vec::new();
    for (line_index, line) in dedicated.iter().enumerate() {
        let mut gap = false;
        for character in line.text.chars() {
            if is_lexical_hangul(character) {
                verifier.push((character, line_index, gap));
                gap = false;
            } else if character.is_whitespace() {
                gap = true;
            }
        }
    }
    let chars = text.chars().collect::<Vec<_>>();
    let positions = chars
        .iter()
        .enumerate()
        .filter_map(|(index, character)| is_lexical_hangul(*character).then_some(index))
        .collect::<Vec<_>>();
    if positions.len() < 2 || verifier.is_empty() {
        return text.to_owned();
    }
    let text_seq = positions.iter().map(|&i| chars[i]).collect::<Vec<_>>();
    let verifier_seq = verifier
        .iter()
        .map(|(character, ..)| *character)
        .collect::<Vec<_>>();
    let mut aligned = vec![None; positions.len()];
    for (i, j) in align_sequences(&text_seq, &verifier_seq) {
        if let (Some(i), Some(j)) = (i, j) {
            aligned[i] = Some(j);
        }
    }
    let mut breaks = vec![false; chars.len()];
    for i in 0..positions.len() - 1 {
        let between = &chars[positions[i] + 1..positions[i + 1]];
        if between.iter().any(|character| character.is_whitespace()) {
            continue;
        }
        let (Some(a), Some(b)) = (aligned[i], aligned[i + 1]) else {
            continue;
        };
        let (first, first_line, _) = verifier[a];
        let (second, second_line, gap) = verifier[b];
        if b != a + 1 || first != text_seq[i] || second != text_seq[i + 1] {
            continue;
        }
        if second_line == first_line + 1 || (second_line == first_line && gap) {
            breaks[positions[i + 1]] = true;
        }
    }
    let mut output = String::with_capacity(text.len() + 8);
    for (character, space_before) in chars.into_iter().zip(breaks) {
        if space_before {
            output.push(' ');
        }
        output.push(character);
    }
    output
}

/// Needleman–Wunsch global alignment of two character sequences. Returns the
/// aligned columns in order: `(Some(i), Some(j))` for a match/substitution,
/// `(Some(i), None)` for a deletion from `a`, `(None, Some(j))` for an insertion
/// from `b`. Scoring favours the diagonal, so equal-length inputs align 1:1.
fn align_sequences(a: &[char], b: &[char]) -> Vec<(Option<usize>, Option<usize>)> {
    const MATCH: i32 = 2;
    const MISMATCH: i32 = -1;
    const GAP: i32 = -2;
    let (n, m) = (a.len(), b.len());
    let mut score = vec![vec![0i32; m + 1]; n + 1];
    for (i, row) in score.iter_mut().enumerate() {
        row[0] = i as i32 * GAP;
    }
    for j in 1..=m {
        score[0][j] = j as i32 * GAP;
    }
    for i in 1..=n {
        for j in 1..=m {
            let diag = score[i - 1][j - 1]
                + if a[i - 1] == b[j - 1] {
                    MATCH
                } else {
                    MISMATCH
                };
            let up = score[i - 1][j] + GAP;
            let left = score[i][j - 1] + GAP;
            score[i][j] = diag.max(up).max(left);
        }
    }

    let mut aln = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0
            && j > 0
            && score[i][j]
                == score[i - 1][j - 1]
                    + if a[i - 1] == b[j - 1] {
                        MATCH
                    } else {
                        MISMATCH
                    }
        {
            aln.push((Some(i - 1), Some(j - 1)));
            i -= 1;
            j -= 1;
        } else if i > 0 && score[i][j] == score[i - 1][j] + GAP {
            aln.push((Some(i - 1), None));
            i -= 1;
        } else {
            aln.push((None, Some(j - 1)));
            j -= 1;
        }
    }
    aln.reverse();
    aln
}

fn is_lexical_hangul(character: char) -> bool {
    ('\u{ac00}'..='\u{d7a3}').contains(&character) || ('\u{3131}'..='\u{314e}').contains(&character)
}

#[cfg(test)]
mod tests {
    use image::{DynamicImage, GrayImage, Luma};

    use super::*;

    fn lines(values: &[(&str, f32)]) -> Vec<LineRecognition> {
        values
            .iter()
            .map(|(text, confidence)| LineRecognition {
                text: (*text).to_owned(),
                confidence: *confidence,
                char_confidences: vec![*confidence; text.chars().count()],
            })
            .collect()
    }

    #[test]
    fn repairs_badend_hangul_without_touching_layout_or_punctuation() {
        let cases = [
            (
                "동굼기리 구하주는건 당연한 일이잖아ㅋ",
                vec![
                    ("동료끼리", 0.987),
                    ("구해주는건", 0.998),
                    ("당연한", 0.972),
                    ("일이잖아ㅋ", 0.969),
                ],
                "동료끼리 구해주는건 당연한 일이잖아ㅋ",
            ),
            (
                "어—이! 거기 쓰라져 있는 악골!",
                vec![
                    ("어이", 0.911),
                    ("거기 쓰러져", 0.971),
                    ("있는 약골!", 0.912),
                ],
                "어—이! 거기 쓰러져 있는 약골!",
            ),
            (
                "시과하야 하는건 너 아니냐?!",
                vec![
                    ("사과해야'", 0.929),
                    ("하는건", 0.998),
                    ("너아니냐?!", 0.904),
                ],
                "사과해야 하는건 너 아니냐?!",
            ),
            (
                "그것도 데이 같은 미냐가 말이야!!",
                vec![
                    ("그것도", 0.999),
                    ("레이같은", 0.999),
                    ("미녀가", 0.963),
                    ("말이야!", 0.930),
                ],
                "그것도 레이 같은 미녀가 말이야!!",
            ),
        ];

        for (paddle_vl, dedicated, expected) in cases {
            assert_eq!(repair_hangul(paddle_vl, &lines(&dedicated), 0.90), expected);
        }
    }

    #[test]
    fn low_confidence_line_is_not_applied() {
        // A single below-threshold line changes nothing — every aligned
        // position is untrusted, so the VL text stands.
        let original = "쓰라져 있는 악골!";
        assert_eq!(
            repair_hangul(original, &lines(&[("쓰러져 있는 약골!", 0.89)]), 0.90),
            original
        );
    }

    #[test]
    fn aligns_across_length_mismatch_and_skips_untrusted_line() {
        // The verifier over-reads the last line (extra syllable, low
        // confidence), so its Hangul count (12) exceeds the VL text's (11).
        // Alignment still applies the three trusted lines and leaves the VL
        // characters for the untrusted, mis-counted last line — no wholesale
        // refusal (this is the real BadEnd-017 block 1).
        let vl = "아하하기 전자 존나 끝리니";
        let dedicated = lines(&[
            ("아하하ㅋ", 0.99),
            ("진짜", 0.99),
            ("존나", 0.99),
            ("끌리네스", 0.70),
        ]);
        assert_eq!(
            repair_hangul(vl, &dedicated, 0.80),
            "아하하ㅋ 진짜 존나 끝리니"
        );
    }

    #[test]
    fn threshold_admits_borderline_correct_line() {
        // BadEnd-017 block 3: the verifier's correct 변태 scored 0.771. At 0.80
        // it is skipped (VL's 년타 stands); at 0.77 it applies.
        let vl = "년.타자식이.";
        let dedicated = lines(&[("변.태", 0.771), ("자식이.", 0.821)]);
        assert_eq!(repair_hangul(vl, &dedicated, 0.80), "년.타자식이.");
        assert_eq!(repair_hangul(vl, &dedicated, 0.77), "변.태자식이.");
    }

    #[test]
    fn keeps_vl_words_the_verifier_dropped() {
        // The verifier dropped a word ("있는"), so its count (5) is below the VL
        // text's (7). Alignment substitutes the trusted syllables it does cover
        // (쓰라져→쓰러져, 악골→약골) and leaves the dropped word intact rather
        // than deleting it.
        let vl = "쓰라져 있는 악골!";
        let dedicated = lines(&[("쓰러져 약골!", 0.99)]);
        assert_eq!(repair_hangul(vl, &dedicated, 0.90), "쓰러져 있는 약골!");
    }

    #[test]
    fn repairs_confident_lines_and_keeps_vl_for_unsure_ones() {
        // VL misreads every line; the verifier is confident on lines 1 and 3
        // but unsure on line 2. Total Hangul counts match (2+4+3), so positions
        // align and repair proceeds per line.
        let paddle_vl = "하하 리브리브 하줄까";
        let dedicated = lines(&[("헤헤", 0.95), ("러브러브", 0.60), ("해줄까", 0.99)]);
        // At 0.80 the unsure middle line keeps the VL word; the others are fixed.
        assert_eq!(
            repair_hangul(paddle_vl, &dedicated, 0.80),
            "헤헤 리브리브 해줄까"
        );
        // Drop the bar and every line is trusted.
        assert_eq!(
            repair_hangul(paddle_vl, &dedicated, 0.50),
            "헤헤 러브러브 해줄까"
        );
    }

    #[test]
    fn splits_white_outlines_on_black_and_ignores_neighbor_fragments() {
        let mut image = GrayImage::from_pixel(120, 100, Luma([0]));
        for (top, bottom) in [(3, 10), (25, 43), (61, 80)] {
            for y in top..bottom {
                for x in 20..100 {
                    image.put_pixel(x, y, Luma([255]));
                }
            }
        }

        let split = split_text_lines(&DynamicImage::ImageLuma8(image));
        assert_eq!(split.len(), 2);
        assert!(split.iter().all(|line| line.height() >= 28));
    }

    fn scored(text: &str, scores: &[f32]) -> LineRecognition {
        LineRecognition {
            text: text.to_owned(),
            confidence: scores.iter().sum::<f32>() / scores.len() as f32,
            char_confidences: scores.to_vec(),
        }
    }

    #[test]
    fn trusts_a_line_by_its_hangul_not_its_punctuation() {
        // BadEnd 017: the outlined heart reads as `V` at 0.14, sinking the line
        // mean to 0.68 although both syllables were read at 0.95.
        let line = scored("헤헤V", &[0.95, 0.95, 0.14]);
        assert!(line.confidence < 0.77);
        let rest = scored("그럼", &[0.98, 0.98]);
        assert_eq!(
            repair_hangul("하하♡ 그럼", &[line, rest], 0.77),
            "헤헤♡ 그럼"
        );
    }

    #[test]
    fn replaces_ascii_junk_standing_in_for_a_syllable() {
        assert_eq!(
            repair_hangul("8-?", &lines(&[("응-?", 0.94)]), 0.77),
            "응-?"
        );
        assert_eq!(
            repair_hangul("....CI?", &lines(&[(".에?", 0.80)]), 0.77),
            "....에?"
        );
        // Latin the verifier also reads is text, not junk.
        assert_eq!(
            repair_hangul("OK 좋아", &lines(&[("OK 좋아", 0.99)]), 0.77),
            "OK 좋아"
        );
        // An unaligned run is never deleted.
        assert_eq!(
            repair_hangul("TV 봤어", &lines(&[("봣어", 0.99)]), 0.77),
            "TV 봣어"
        );
    }

    #[test]
    fn falls_back_to_inverted_reading_only_when_it_is_trusted() {
        let original = scored("뭐야", &[0.60, 0.70]);
        let inverted = scored("뭐야", &[0.99, 0.99]);
        let unsure = scored("뮤야", &[0.50, 0.60]);
        assert_eq!(
            prefer_trusted_polarity(original.clone(), Some(inverted.clone()), 0.77),
            inverted
        );
        assert_eq!(
            prefer_trusted_polarity(original.clone(), Some(unsure), 0.77),
            original
        );
        let confident = scored("짜증나네", &[0.9; 4]);
        assert_eq!(
            prefer_trusted_polarity(confident.clone(), Some(inverted), 0.77),
            confident
        );
    }

    #[test]
    fn restores_word_breaks_at_the_bubbles_line_breaks() {
        let dedicated = lines(&[
            ("..리한을", 0.9),
            ("흉보는", 0.9),
            ("것만은", 0.9),
            ("싫어", 0.9),
        ]);
        assert_eq!(
            space_at_line_breaks("..리한을흉보는것만은싫어....", &dedicated),
            "..리한을 흉보는 것만은 싫어...."
        );
        // The verifier's own in-line space counts too.
        let dedicated = lines(&[("입으로 직접", 0.9)]);
        assert_eq!(
            space_at_line_breaks("입으로직접", &dedicated),
            "입으로 직접"
        );
        // Syllables the verifier reads differently get no break inserted.
        let dedicated = lines(&[("모습이랑", 0.9), ("비교해서", 0.9)]);
        assert_eq!(
            space_at_line_breaks("모습이당비고하서", &dedicated),
            "모습이당비고하서"
        );
        // Existing spaces are kept, never removed.
        let dedicated = lines(&[("그자식", 0.9)]);
        assert_eq!(space_at_line_breaks("그 자식", &dedicated), "그 자식");
    }

    /// White bars on black: `rows` are (top, bottom) text lines spanning
    /// columns 20..100; `bridges` are (top, bottom, left, right) extra ink.
    fn bars(height: u32, rows: &[(u32, u32)], bridges: &[(u32, u32, u32, u32)]) -> DynamicImage {
        let mut image = GrayImage::from_pixel(120, height, Luma([0]));
        for &(top, bottom) in rows {
            for y in top..bottom {
                for x in 20..100 {
                    image.put_pixel(x, y, Luma([255]));
                }
            }
        }
        for &(top, bottom, left, right) in bridges {
            for y in top..bottom {
                for x in left..right {
                    image.put_pixel(x, y, Luma([255]));
                }
            }
        }
        DynamicImage::ImageLuma8(image)
    }

    #[test]
    fn cuts_fused_lines_using_the_bubbles_own_pitch() {
        // BadEnd 010: two tightly set lines touch and fuse into one span. The
        // other lines give a consistent 30 px pitch, so the fused span is cut.
        let rows = [(10, 30), (40, 60), (70, 90), (100, 120), (130, 150)];
        let image = bars(170, &rows, &[(30, 40, 60, 66)]);
        assert_eq!(split_text_lines(&image).len(), 5);
    }

    #[test]
    fn never_halves_a_single_span_without_pitch_evidence() {
        // One tall span with a faint valley but no other lines to measure a
        // pitch from: it may be a single tall glyph, so it stays whole.
        let image = bars(90, &[(10, 40), (46, 76)], &[(40, 46, 60, 65)]);
        assert_eq!(split_text_lines(&image).len(), 1);
    }

    #[test]
    fn keeps_short_interior_lines_after_a_fused_span() {
        // The old rule dropped every span shorter than 60% of the tallest, so
        // one fused span threw the real lines below it away.
        let image = bars(140, &[(10, 70), (80, 100), (110, 130)], &[]);
        assert_eq!(split_text_lines(&image).len(), 3);
    }

    #[test]
    fn drops_partial_rows_clipped_at_the_crop_edge() {
        let image = bars(110, &[(0, 12), (30, 55), (70, 95)], &[]);
        let split = split_text_lines(&image);
        assert_eq!(split.len(), 2);
        assert!(split.iter().all(|line| line.height() >= 25));
    }

    #[test]
    fn trims_each_line_to_its_ink_columns() {
        let mut image = GrayImage::from_pixel(120, 80, Luma([0]));
        for y in 20..50 {
            for x in 40..80 {
                image.put_pixel(x, y, Luma([255]));
            }
        }
        let split = split_text_lines(&DynamicImage::ImageLuma8(image));
        assert_eq!(split.len(), 1);
        assert_eq!(split[0].width(), 40 + 2 * 9);
    }
}
