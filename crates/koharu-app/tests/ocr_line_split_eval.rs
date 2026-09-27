//! Opt-in experiment: raw reader outputs for the Korean line-splitting and
//! repair work, so candidate rules can be scored offline against a blind key.
//!
//! Per block (production geometry): the hinted PaddleOCR-VL text (raw and
//! single-lined), and PP-OCRv5 readings — with per-character confidences — on
//! the lines of BOTH the legacy splitter (copied below) and the current
//! `split_text_lines`, each in original and inverted polarity. Also the
//! production result (legacy lines + alignment repair @0.77 + emoji safeguard)
//! so the offline scorer can prove it reproduces Rust.
//!
//! Env: KOHARU_OCR017_ROOT (output dir; must not contain results.json; resumes
//!      from progress.json), KOHARU_LEVERS_FIXTURE (dir with fixture.json and
//!      the source images it names), KOHARU_OCR017_DATA (runtime data root;
//!      default %LOCALAPPDATA%/koharu).
//! Run: bun cargo test -p koharu-app --features cuda --test ocr_line_split_eval \
//!        -- --ignored --nocapture

use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Instant};

use anyhow::{Context, Result};
use image::DynamicImage;
use koharu_app::pipeline::support::{is_degenerate_ocr_text, single_line_ocr_text};
use koharu_llm::{
    paddleocr_vl::{PaddleOcrVl, PaddleOcrVlGenerateOptions, PaddleOcrVlTask},
    safe::llama_backend::LlamaBackend,
};
use koharu_ml::{
    TextRegion,
    comic_text_detector::{crop_text_block_deskewed, crop_text_block_exact},
    korean_ocr::{KoreanOcr, LineRecognition, repair_hangul, split_text_lines},
};
use koharu_runtime::{ComputePolicy, RuntimeManager};
use serde_json::{Value, json};

const REPAIR_MIN_CONFIDENCE: f32 = 0.77;
const HINT: &str = "Korean";

fn verifier_crop(image: &DynamicImage, region: &TextRegion) -> DynamicImage {
    let mut tight = region.clone();
    let pad = (tight.width.min(tight.height) * 0.03).max(2.0);
    tight.x -= pad;
    tight.y -= pad;
    tight.width += pad * 2.0;
    tight.height += pad * 2.0;
    crop_text_block_exact(image, &tight)
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

/// The splitter as shipped before this change (row projection, then every
/// span shorter than 60% of the tallest dropped), kept here for A/B.
fn legacy_split_text_lines(image: &DynamicImage) -> Vec<DynamicImage> {
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

/// Hinted VL call as in production (empty hinted result retried plain).
fn vl_raw(paddle: &mut PaddleOcrVl, crop: &DynamicImage) -> Result<String> {
    let options = PaddleOcrVlGenerateOptions {
        max_new_tokens: 256,
        language: Some(HINT.to_owned()),
        ..Default::default()
    };
    let mut text = paddle
        .inference_with_options(crop, PaddleOcrVlTask::Ocr, &options)?
        .text;
    if text.trim().is_empty() {
        let plain = PaddleOcrVlGenerateOptions {
            max_new_tokens: 256,
            ..Default::default()
        };
        text = paddle
            .inference_with_options(crop, PaddleOcrVlTask::Ocr, &plain)?
            .text;
    }
    Ok(text)
}

fn read_lines(korean: &mut KoreanOcr, lines: &[DynamicImage]) -> Result<Vec<LineRecognition>> {
    lines
        .iter()
        .map(|line| korean.recognize_line(line))
        .collect()
}

fn lines_json(lines: &[LineRecognition]) -> Value {
    json!(
        lines
            .iter()
            .map(
                |l| json!({"text": l.text, "confidence": l.confidence, "chars": l.char_confidences})
            )
            .collect::<Vec<_>>()
    )
}

#[test]
#[ignore = "requires the local model cache, a GPU, and a prepared fixture"]
fn korean_line_split_eval() -> Result<()> {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(run())
                .inspect_err(|e| eprintln!("EVAL FAILED: {e:#}"))
        })?
        .join()
        .expect("eval thread panicked")
}

async fn run() -> Result<()> {
    let root = PathBuf::from(std::env::var("KOHARU_OCR017_ROOT")?).canonicalize()?;
    let fixture_dir = PathBuf::from(std::env::var("KOHARU_LEVERS_FIXTURE")?).canonicalize()?;
    anyhow::ensure!(
        !root.join("results.json").exists(),
        "refusing to overwrite results"
    );
    let crops_dir = root.join("crops");
    std::fs::create_dir_all(&crops_dir)?;
    let fixture: Value = serde_json::from_slice(&std::fs::read(fixture_dir.join("fixture.json"))?)?;

    let data = std::env::var("KOHARU_OCR017_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("koharu")
        });
    let runtime = RuntimeManager::new(&data, ComputePolicy::PreferGpu)?;
    runtime.prepare().await?;
    koharu_llm::sys::initialize(&runtime)?;
    let backend = Arc::new(LlamaBackend::init()?);
    anyhow::ensure!(backend.supports_gpu_offload(), "GPU runtime unavailable");
    let mut paddle = PaddleOcrVl::load(&runtime, false, backend).await?;
    let mut korean = KoreanOcr::load(&runtime).await?;

    let mut rows: Vec<Value> = match std::fs::read(root.join("progress.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(_) => Vec::new(),
    };
    let done: HashSet<String> = rows
        .iter()
        .filter_map(|r| r["id"].as_str().map(str::to_owned))
        .collect();
    for page in fixture["pages"].as_array().context("pages")? {
        let name = page["page"].as_str().context("page")?;
        let stem = name.trim_end_matches(".jpg");
        let source = image::open(fixture_dir.join(page["source"].as_str().context("source")?))?;
        for block in page["blocks"].as_array().context("blocks")? {
            let n = block["block"].as_u64().context("block")? as usize;
            let id = format!("{stem}-{n:02}");
            if done.contains(&id) {
                continue;
            }
            let t = &block["transform"];
            let region = TextRegion {
                x: t["x"].as_f64().context("x")? as f32,
                y: t["y"].as_f64().context("y")? as f32,
                width: t["width"].as_f64().context("width")? as f32,
                height: t["height"].as_f64().context("height")? as f32,
                rotation_deg: Some(t["rotationDeg"].as_f64().unwrap_or(0.0) as f32),
                ..Default::default()
            };
            let paddle_crop = crop_text_block_deskewed(&source, &region);
            let verifier = verifier_crop(&source, &region);
            paddle_crop.save(crops_dir.join(format!("{id}-block.png")))?;
            let legacy = legacy_split_text_lines(&verifier);
            let current = split_text_lines(&verifier);
            for (k, line) in current.iter().enumerate() {
                line.save(crops_dir.join(format!("{id}-line{k}.png")))?;
            }

            let started = Instant::now();
            let raw = vl_raw(&mut paddle, &paddle_crop)?;
            let vl_ms = started.elapsed().as_millis();
            let vl_text = single_line_ocr_text(&raw);
            let legacy_orig = read_lines(&mut korean, &legacy)?;
            let legacy_inv =
                read_lines(&mut korean, &legacy.iter().map(invert).collect::<Vec<_>>())?;
            let started = Instant::now();
            let new_orig = read_lines(&mut korean, &current)?;
            let pp_ms = started.elapsed().as_millis();
            let new_inv = read_lines(&mut korean, &current.iter().map(invert).collect::<Vec<_>>())?;

            let repaired = repair_hangul(&vl_text, &legacy_orig, REPAIR_MIN_CONFIDENCE);
            let production = if is_degenerate_ocr_text(&repaired) {
                String::new()
            } else {
                repaired
            };
            println!(
                "{id}: legacy lines {} -> new lines {}",
                legacy.len(),
                current.len()
            );
            rows.push(json!({
                "id": id,
                "page": name,
                "block": n,
                "vl": {"raw": raw, "hint": vl_text},
                "pp_legacy": {"original": lines_json(&legacy_orig), "inverted": lines_json(&legacy_inv)},
                "pp_new": {"original": lines_json(&new_orig), "inverted": lines_json(&new_inv)},
                "production": production,
                "timing_ms": {"vl_hint": vl_ms, "pp_new_original": pp_ms},
            }));
            std::fs::write(
                root.join("progress.json"),
                serde_json::to_vec_pretty(&rows)?,
            )?;
        }
    }
    std::fs::write(
        root.join("results.json"),
        serde_json::to_vec_pretty(
            &json!({"completed": true, "repair_min_confidence": REPAIR_MIN_CONFIDENCE, "rows": rows}),
        )?,
    )?;
    println!("wrote {}", root.join("results.json").display());
    Ok(())
}
