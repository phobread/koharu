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

pub fn repair_hangul(
    paddle_vl: &str,
    dedicated: &[LineRecognition],
    minimum_confidence: f32,
) -> String {
    if dedicated.is_empty()
        || dedicated
            .iter()
            .any(|line| line.confidence < minimum_confidence)
    {
        return paddle_vl.to_owned();
    }

    let replacements = dedicated
        .iter()
        .flat_map(|line| line.text.chars())
        .filter(|character| is_lexical_hangul(*character))
        .collect::<Vec<_>>();
    let mut output = paddle_vl.chars().collect::<Vec<_>>();
    let positions = output
        .iter()
        .enumerate()
        .filter_map(|(index, character)| is_lexical_hangul(*character).then_some(index))
        .collect::<Vec<_>>();
    if replacements.len() != positions.len() {
        return paddle_vl.to_owned();
    }
    for (position, replacement) in positions.into_iter().zip(replacements) {
        output[position] = replacement;
    }
    output.into_iter().collect()
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
    fn refuses_low_confidence_or_length_mismatch() {
        let original = "쓰라져 있는 악골!";
        assert_eq!(
            repair_hangul(original, &lines(&[("쓰러져 있는 약골!", 0.89)]), 0.90),
            original
        );
        assert_eq!(
            repair_hangul(original, &lines(&[("쓰러져 약골!", 0.99)]), 0.90),
            original
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
