use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use image::{DynamicImage, GenericImageView, imageops::FilterType};
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
    pub confidence: f32,
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
                text.push_str(&self.characters[best_index]);
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
        })
    }

    pub fn recognize_block(&mut self, image: &DynamicImage) -> Result<Vec<LineRecognition>> {
        split_text_lines(image)
            .iter()
            .map(|line| self.recognize_line(line))
            .collect()
    }
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
pub fn split_text_lines(image: &DynamicImage) -> Vec<DynamicImage> {
    let gray = image.to_luma8();
    if gray.width() == 0 || gray.height() == 0 {
        return Vec::new();
    }
    let mean = gray.pixels().map(|pixel| u64::from(pixel[0])).sum::<u64>() as f64
        / f64::from(gray.width() * gray.height());
    let dark_background = mean < 128.0;
    let min_ink = (gray.width() / 50).max(4);
    let active = (0..gray.height())
        .map(|y| {
            (0..gray.width())
                .filter(|&x| {
                    let luma = gray.get_pixel(x, y)[0];
                    if dark_background {
                        luma >= 180
                    } else {
                        luma <= 75
                    }
                })
                .count() as u32
                >= min_ink
        })
        .collect::<Vec<_>>();

    let mut spans = Vec::new();
    let mut start = None;
    let mut last_active = 0_u32;
    for (y, is_active) in active.into_iter().enumerate() {
        let y = y as u32;
        if is_active {
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

    // Detector boxes can overlap a neighboring caption. Small partial rows
    // are not useful recognition lines and would poison the confidence gate.
    if let Some(max_height) = spans.iter().map(|(top, bottom)| bottom - top).max()
        && spans.len() > 1
    {
        spans.retain(|(top, bottom)| (bottom - top) * 5 >= max_height * 3);
    }

    spans
        .into_iter()
        .map(|(top, bottom)| {
            let top = top.saturating_sub(5);
            let bottom = (bottom + 5).min(image.height());
            image.crop_imm(0, top, image.width(), bottom - top)
        })
        .collect()
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
            let trusted = line.confidence >= minimum_confidence;
            line.text
                .chars()
                .filter(|character| is_lexical_hangul(*character))
                .map(move |character| (character, trusted))
        })
        .collect::<Vec<_>>();
    let mut output = paddle_vl.chars().collect::<Vec<_>>();
    let vl_positions = output
        .iter()
        .enumerate()
        .filter_map(|(index, character)| is_lexical_hangul(*character).then_some(index))
        .collect::<Vec<_>>();
    if verifier.is_empty() || vl_positions.is_empty() {
        return paddle_vl.to_owned();
    }

    let vl_seq = vl_positions.iter().map(|&i| output[i]).collect::<Vec<_>>();
    let verifier_seq = verifier
        .iter()
        .map(|(character, _)| *character)
        .collect::<Vec<_>>();
    for (i, j) in align_sequences(&vl_seq, &verifier_seq) {
        // Only aligned pairs (matches/substitutions) apply; gaps are skipped so
        // no word is inserted into or deleted from the VL text.
        if let (Some(i), Some(j)) = (i, j) {
            let (replacement, trusted) = verifier[j];
            if trusted {
                output[vl_positions[i]] = replacement;
            }
        }
    }
    output.into_iter().collect()
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
}
