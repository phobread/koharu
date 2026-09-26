//! Opt-in real-model recognition experiment for BadEnd page 017 (hollow
//! outlined Hangul on black balloons). Reads only the retained export
//! fixtures; never opens a user project or starts the desktop/server.
//!
//! Purpose: get the raw per-model outputs the FINDINGS/Astra/Claude review all
//! agreed we need before touching recognition — for each of the 7 blocks it
//! logs raw PaddleOCR-VL text, each PP-OCRv5 line + confidence, the repair-gate
//! decision *and its reason*, and the final text, against the saved
//! (reference-only) string. It then runs the experimental arm: the same crops
//! passed through candidate outline-normalizations (stroke solidification,
//! same-polarity and inverted) so we can measure whether preprocessing helps
//! VL and/or PP-OCRv5 — and it dumps the height-48 normalized preview the CTC
//! model actually sees (per Astra: check whether the resize erases the
//! contour).
//!
//! This is a diagnostic. It changes no production behavior and asserts nothing
//! about accuracy; it produces evidence for a later decision.
//!
//! Reuses the existing local model cache read-only (no downloads, no copies):
//! `PaddleOcrVl::load` and `KoreanOcr::load` resolve from the runtime data root,
//! which defaults to the app's own `%LOCALAPPDATA%/koharu`. Only the small
//! output (crops + JSON) is written, to the isolated output dir. Env:
//!   KOHARU_OCR017_ROOT     output dir; must not already contain results.json.
//!   KOHARU_OCR017_EXPORT   the export dir (contains source-017.jpg + analysis.json).
//!   KOHARU_OCR017_DATA     runtime data root; default `%LOCALAPPDATA%/koharu`.
//!   KOHARU_OCR017_CLOSE_RADIUS  optional morphological-close radius (px), default 2.
//!
//! Run (from an env with the CUDA build set up):
//!   KOHARU_OCR017_ROOT=... KOHARU_OCR017_EXPORT=... \
//!     bun cargo test -p koharu-app --features cuda --test ocr_badend017_recognition \
//!     -- --ignored --nocapture

use std::{path::PathBuf, sync::Arc, time::Instant};

use anyhow::{Context, Result, ensure};
use image::{DynamicImage, GenericImageView, ImageBuffer, Luma, RgbImage, imageops::FilterType};
use koharu_app::pipeline::support::{is_degenerate_ocr_text, single_line_ocr_text};
use koharu_llm::{
    paddleocr_vl::{PaddleOcrVl, PaddleOcrVlGenerateOptions, PaddleOcrVlTask},
    safe::llama_backend::LlamaBackend,
};
use koharu_ml::{
    TextRegion,
    comic_text_detector::{crop_text_block_deskewed, crop_text_block_exact},
    korean_ocr::{KoreanOcr, LineRecognition, contains_lexical_hangul, repair_hangul},
};
use koharu_runtime::{ComputePolicy, RuntimeManager};
use serde_json::{Value, json};

const REPAIR_MIN_CONFIDENCE: f32 = 0.77; // per-line bar (production)
const OLD_MIN_CONFIDENCE: f32 = 0.90; // previous all-or-nothing gate, for before/after

fn is_lexical_hangul(c: char) -> bool {
    ('\u{ac00}'..='\u{d7a3}').contains(&c) || ('\u{3131}'..='\u{314e}').contains(&c)
}

/// Mirror production's Korean verifier crop (paddle_ocr.rs::korean_verification_crop
/// after the double-margin fix): 3% margin, then the exact no-extra-margin crop.
fn verifier_crop(image: &DynamicImage, region: &TextRegion) -> DynamicImage {
    let mut tight = region.clone();
    let pad = (tight.width.min(tight.height) * 0.03).max(2.0);
    tight.x -= pad;
    tight.y -= pad;
    tight.width += pad * 2.0;
    tight.height += pad * 2.0;
    crop_text_block_exact(image, &tight)
}

/// Solidify hollow outlined glyphs: take the bright-ink mask on a dark
/// background, morphologically CLOSE it (dilate then erode by `radius`) to
/// bridge the thin black stroke-interior into a solid stroke, then re-render.
/// `invert=false` keeps the source polarity (solid white ink on black);
/// `invert=true` emits conventional black ink on white. Large letter counters
/// survive when `radius` is smaller than half their width. Diagnostic only —
/// no gating; the caller decides which blocks to apply it to.
fn solidify(image: &DynamicImage, radius: u32, invert: bool) -> DynamicImage {
    let gray = image.to_luma8();
    let (w, h) = gray.dimensions();
    if w == 0 || h == 0 {
        return image.clone();
    }
    let mean = gray.pixels().map(|p| u64::from(p[0])).sum::<u64>() as f64 / f64::from(w * h);
    let dark_bg = mean < 128.0;
    let idx = |x: u32, y: u32| (y * w + x) as usize;

    let mut mask = vec![false; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let luma = gray.get_pixel(x, y)[0];
            mask[idx(x, y)] = if dark_bg { luma >= 180 } else { luma <= 75 };
        }
    }

    let morph = |src: &[bool], dilate: bool| -> Vec<bool> {
        let mut out = vec![false; src.len()];
        let r = radius as i64;
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let mut hit = false;
                'k: for dy in -r..=r {
                    for dx in -r..=r {
                        let (nx, ny) = (x + dx, y + dy);
                        let inside = nx >= 0 && ny >= 0 && nx < w as i64 && ny < h as i64;
                        // Erosion treats out-of-bounds as background so glyphs
                        // touching the crop edge aren't wrongly thinned inward.
                        let val = if inside {
                            src[idx(nx as u32, ny as u32)]
                        } else {
                            false
                        };
                        if dilate == val {
                            // dilate: any neighbor set -> set; erode: any neighbor
                            // unset -> unset.
                            hit = true;
                            break 'k;
                        }
                    }
                }
                out[idx(x as u32, y as u32)] = if dilate { hit } else { !hit };
            }
        }
        out
    };
    let closed = morph(&morph(&mask, true), false);

    // Render solid: ink color vs paper color.
    let (ink, paper) = if invert {
        (Luma([0u8]), Luma([255u8]))
    } else {
        (Luma([255u8]), Luma([0u8]))
    };
    let mut out = ImageBuffer::from_pixel(w, h, paper);
    for y in 0..h {
        for x in 0..w {
            if closed[idx(x, y)] {
                out.put_pixel(x, y, ink);
            }
        }
    }
    DynamicImage::ImageLuma8(out)
}

/// Replicate KoreanOcr::preprocess's geometry so we can save the exact image
/// the CTC model effectively sees (height 48, proportional width, Triangle).
fn normalized_preview(image: &DynamicImage) -> RgbImage {
    const H: u32 = 48;
    let (sw, sh) = image.dimensions();
    let scaled = ((H as f64 * sw as f64 / sh.max(1) as f64).ceil() as u32).max(1);
    let target = scaled.clamp(320, 3200);
    let resized_w = scaled.min(target);
    image::imageops::resize(&image.to_rgb8(), resized_w, H, FilterType::Triangle)
}

/// The previous all-or-nothing gate (reject the whole block unless every line
/// clears `min_conf`), kept only to produce the "before" column.
fn repair_all_or_nothing(vl_single: &str, lines: &[LineRecognition], min_conf: f32) -> String {
    if lines.is_empty() || lines.iter().any(|l| l.confidence < min_conf) {
        return vl_single.to_owned();
    }
    let replacements: Vec<char> = lines
        .iter()
        .flat_map(|l| l.text.chars())
        .filter(|c| is_lexical_hangul(*c))
        .collect();
    let mut out: Vec<char> = vl_single.chars().collect();
    let positions: Vec<usize> = out
        .iter()
        .enumerate()
        .filter_map(|(i, c)| is_lexical_hangul(*c).then_some(i))
        .collect();
    if replacements.len() != positions.len() {
        return vl_single.to_owned();
    }
    for (p, r) in positions.into_iter().zip(replacements) {
        out[p] = r;
    }
    out.into_iter().collect()
}

/// Report why the count check would (not) fire, for the log.
fn count_note(vl_single: &str, lines: &[LineRecognition]) -> String {
    let verifier_hangul = lines
        .iter()
        .flat_map(|l| l.text.chars())
        .filter(|c| is_lexical_hangul(*c))
        .count();
    let vl_hangul = vl_single.chars().filter(|c| is_lexical_hangul(*c)).count();
    if verifier_hangul == vl_hangul {
        format!("counts match ({vl_hangul})")
    } else {
        format!("count mismatch (verifier {verifier_hangul} vs vl {vl_hangul}) -> no repair")
    }
}

fn run_paddle(paddle: &mut PaddleOcrVl, crop: &DynamicImage) -> Result<Value> {
    let options = PaddleOcrVlGenerateOptions {
        max_new_tokens: 256,
        ..Default::default()
    };
    let started = Instant::now();
    let out = paddle.inference_with_options(crop, PaddleOcrVlTask::Ocr, &options)?;
    Ok(json!({
        "seconds": started.elapsed().as_secs_f64(),
        "raw": out.text,
        "single_line": single_line_ocr_text(&out.text),
        "has_hangul": contains_lexical_hangul(&out.text),
    }))
}

fn run_verifier(korean: &mut KoreanOcr, crop: &DynamicImage, vl_single: &str) -> Result<Value> {
    let started = Instant::now();
    let lines = korean.recognize_block(crop)?;
    let seconds = started.elapsed().as_secs_f64();
    // before: previous all-or-nothing gate at 0.90; after: alignment repair at
    // the production threshold, then the engine's degenerate-output safeguard.
    let before = repair_all_or_nothing(vl_single, &lines, OLD_MIN_CONFIDENCE);
    let repaired = repair_hangul(vl_single, &lines, REPAIR_MIN_CONFIDENCE);
    let after = if is_degenerate_ocr_text(&repaired) {
        String::new()
    } else {
        repaired.clone()
    };
    Ok(json!({
        "seconds": seconds,
        "line_count": lines.len(),
        "lines": lines.iter().map(|l| json!({"text": l.text, "confidence": l.confidence})).collect::<Vec<_>>(),
        "count_note": count_note(vl_single, &lines),
        "before_all_or_nothing_0_90": before,
        "after_aligned_then_safeguard": after,
        "blanked_degenerate": is_degenerate_ocr_text(&repaired),
        "changed": before != after,
    }))
}

#[test]
#[ignore = "requires prepared isolated runtime/models and the retained 017 export"]
fn badend017_recognition_ab() -> Result<()> {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(run())
        })?
        .join()
        .expect("recognition experiment thread panicked")
}

async fn run() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("koharu_llm::paddleocr_vl=debug")
        .with_ansi(false)
        .try_init();

    let root = PathBuf::from(std::env::var("KOHARU_OCR017_ROOT")?)
        .canonicalize()
        .context("prepared experiment directory")?;
    let export = PathBuf::from(std::env::var("KOHARU_OCR017_EXPORT")?).canonicalize()?;
    let radius: u32 = std::env::var("KOHARU_OCR017_CLOSE_RADIUS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    ensure!(
        !root.join("results.json").exists(),
        "refusing to overwrite an existing experiment"
    );
    std::fs::create_dir_all(root.join("crops"))?;

    let source = image::open(export.join("source-017.jpg")).context("source-017.jpg")?;
    let analysis: Value = serde_json::from_slice(&std::fs::read(export.join("analysis.json"))?)?;
    let blocks = analysis["rows"].as_array().context("analysis.json rows")?;
    ensure!(blocks.len() == 7, "expected 7 blocks, got {}", blocks.len());

    let data = std::env::var("KOHARU_OCR017_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("koharu")
        });
    ensure!(
        data.join("models").exists(),
        "runtime data root `{}` has no models dir; set KOHARU_OCR017_DATA",
        data.display()
    );
    let runtime = RuntimeManager::new(&data, ComputePolicy::PreferGpu)?;
    // Idempotent against an already-populated root (same call the app makes on
    // every launch); no downloads because the cache is present.
    runtime.prepare().await?;
    koharu_llm::sys::initialize(&runtime)?;
    let backend = Arc::new(LlamaBackend::init()?);
    ensure!(backend.supports_gpu_offload(), "GPU runtime unavailable");
    let mut paddle = PaddleOcrVl::load(&runtime, false, backend).await?;
    let mut korean = KoreanOcr::load(&runtime).await?;

    // Named preprocessing variants applied to each crop before recognition.
    let variants: [(&str, Box<dyn Fn(&DynamicImage) -> DynamicImage>); 3] = [
        ("original", Box::new(|c: &DynamicImage| c.clone())),
        (
            "solid_same_polarity",
            Box::new(move |c: &DynamicImage| solidify(c, radius, false)),
        ),
        (
            "solid_inverted",
            Box::new(move |c: &DynamicImage| solidify(c, radius, true)),
        ),
    ];

    let mut rows = Vec::new();
    for block in blocks {
        let n = block["block"].as_u64().context("block number")? as usize;
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

        // Self-check: our reconstructed paddle crop must match the size the
        // export recorded for production (proves the region/margin math).
        if let Some(bounds) = block["paddle_crop_bounds"].as_array() {
            let b: Vec<i64> = bounds.iter().filter_map(|v| v.as_i64()).collect();
            if b.len() == 4 {
                let (ew, eh) = ((b[2] - b[0]) as u32, (b[3] - b[1]) as u32);
                ensure!(
                    paddle_crop.dimensions() == (ew, eh),
                    "block {n}: paddle crop {:?} != recorded {:?}",
                    paddle_crop.dimensions(),
                    (ew, eh)
                );
            }
        }

        let mut variant_results = Vec::new();
        for (name, transform) in &variants {
            let vl_input = transform(&paddle_crop);
            let ver_input = transform(&verifier);
            vl_input.save(root.join(format!("crops/{n:02}-{name}-vl.png")))?;
            ver_input.save(root.join(format!("crops/{n:02}-{name}-verifier.png")))?;
            normalized_preview(&ver_input)
                .save(root.join(format!("crops/{n:02}-{name}-verifier-h48.png")))?;

            let paddle_out = run_paddle(&mut paddle, &vl_input)?;
            let vl_single = paddle_out["single_line"].as_str().unwrap_or("").to_owned();
            let verifier_out = run_verifier(&mut korean, &ver_input, &vl_single)?;
            println!(
                "block {n} [{name}]: VL={:?} | lines={} | before={} after={} changed={}",
                vl_single,
                verifier_out["line_count"],
                verifier_out["before_all_or_nothing_0_90"],
                verifier_out["after_aligned_then_safeguard"],
                verifier_out["changed"],
            );
            variant_results.push(json!({
                "variant": name,
                "paddle_vl": paddle_out,
                "verifier": verifier_out,
            }));
        }

        rows.push(json!({
            "block": n,
            "node_id": block["node_id"],
            "transform": t,
            "saved_reference_not_ground_truth": block["saved_text_not_fresh_inference"],
            "visual_reference_provisional": block["visual_reference_provisional"],
            "visual_lines": block["visual_lines"],
            "paddle_crop_size": [paddle_crop.width(), paddle_crop.height()],
            "verifier_crop_size": [verifier.width(), verifier.height()],
            "variants": variant_results,
        }));
        std::fs::write(
            root.join("progress.json"),
            serde_json::to_vec_pretty(&rows)?,
        )?;
    }

    let report = json!({
        "completed": true,
        "note": "diagnostic only; saved text is a reference, not ground truth; no accuracy claim",
        "close_radius": radius,
        "repair_min_confidence": REPAIR_MIN_CONFIDENCE,
        "rows": rows,
    });
    std::fs::write(
        root.join("results.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("wrote {}", root.join("results.json").display());
    Ok(())
}
