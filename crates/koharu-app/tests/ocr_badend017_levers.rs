//! Opt-in experiment: before calling the remaining BadEnd-017 OCR failures a
//! recognition-model limit, test the levers the first investigation assumed
//! away rather than measured. Reads only the retained export and the existing
//! model cache (mit48px weights are fetched once if missing); never opens a
//! user project or starts the app.
//!
//! Per block, with production geometry:
//!   - VL prompt: plain "OCR:" (what the first harness used) vs the production
//!     Korean hint ("The text in the image is Korean. OCR:")
//!   - polarity: original vs plain inversion (no solidification)
//!   - VL granularity: whole bubble vs one call per split line
//!   - a third recognizer: mit48px (manga-trained) on the same split lines
//!   - PP-OCRv5 lines: original vs inverted
//!   - end-to-end production (hinted VL + alignment repair @0.77 + emoji
//!     safeguard), plus the same pipeline with each alternative line source
//!
//! Env: KOHARU_OCR017_ROOT (output dir; must not contain results.json),
//!      KOHARU_OCR017_EXPORT (dir with source-017.jpg + analysis.json),
//!      KOHARU_OCR017_DATA (runtime data root; default %LOCALAPPDATA%/koharu).
//! Run: bun cargo test -p koharu-app --features cuda --test ocr_badend017_levers \
//!        -- --ignored --nocapture

use std::{path::PathBuf, sync::Arc, time::Instant};

use anyhow::{Context, Result, ensure};
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
    mit48px_ocr::Mit48pxOcr,
};
use koharu_runtime::{ComputePolicy, RuntimeManager};
use serde_json::{Value, json};

/// Production per-line trust bar (paddle_ocr.rs KOREAN_REPAIR_MIN_CONFIDENCE).
const REPAIR_MIN_CONFIDENCE: f32 = 0.77;
/// What the UI sends as `sourceLanguage` when the OCR language is Korean.
const HINT: &str = "Korean";

/// Production Korean-verifier crop: 3% margin, exact (no extra OCR margin).
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

/// One VL call, mirroring production: a hinted result that comes back empty
/// is retried once with the plain prompt. Returns the single-line text.
fn vl(paddle: &mut PaddleOcrVl, crop: &DynamicImage, hint: Option<&str>) -> Result<String> {
    let plain = PaddleOcrVlGenerateOptions {
        max_new_tokens: 256,
        ..Default::default()
    };
    let options = PaddleOcrVlGenerateOptions {
        max_new_tokens: 256,
        language: hint.map(str::to_owned),
        ..Default::default()
    };
    let mut text = paddle
        .inference_with_options(crop, PaddleOcrVlTask::Ocr, &options)?
        .text;
    if hint.is_some() && text.trim().is_empty() {
        text = paddle
            .inference_with_options(crop, PaddleOcrVlTask::Ocr, &plain)?
            .text;
    }
    Ok(single_line_ocr_text(&text))
}

fn pp_lines(korean: &mut KoreanOcr, lines: &[DynamicImage]) -> Result<Vec<LineRecognition>> {
    lines
        .iter()
        .map(|line| korean.recognize_line(line))
        .collect()
}

fn mit_lines(mit: &Mit48pxOcr, lines: &[DynamicImage]) -> Result<Vec<LineRecognition>> {
    Ok(mit
        .inference_regions(lines)?
        .into_iter()
        .map(|p| LineRecognition {
            char_confidences: vec![p.confidence; p.text.chars().count()],
            text: p.text,
            confidence: p.confidence,
        })
        .collect())
}

fn lines_json(lines: &[LineRecognition]) -> Value {
    json!(
        lines
            .iter()
            .map(|l| json!({"text": l.text, "confidence": l.confidence}))
            .collect::<Vec<_>>()
    )
}

fn joined(lines: &[LineRecognition]) -> String {
    lines
        .iter()
        .map(|l| l.text.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The production text path: alignment repair, then the emoji safeguard.
fn production(vl_text: &str, lines: &[LineRecognition]) -> String {
    let repaired = repair_hangul(vl_text, lines, REPAIR_MIN_CONFIDENCE);
    if is_degenerate_ocr_text(&repaired) {
        String::new()
    } else {
        repaired
    }
}

#[test]
#[ignore = "requires the local model cache, a GPU, and the retained 017 export"]
fn badend017_ocr_levers() -> Result<()> {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(run())
                .inspect_err(|e| eprintln!("LEVERS FAILED: {e:#}"))
        })?
        .join()
        .expect("levers experiment thread panicked")
}

async fn run() -> Result<()> {
    let root = PathBuf::from(std::env::var("KOHARU_OCR017_ROOT")?)
        .canonicalize()
        .context("prepared experiment directory")?;
    // Multi-page fixture: fixture.json ({pages:[{page, source, blocks:[{block,node_id,transform}]}]})
    // plus the source images it names.
    let fixture_dir = PathBuf::from(std::env::var("KOHARU_LEVERS_FIXTURE")?).canonicalize()?;
    ensure!(
        !root.join("results.json").exists(),
        "refusing to overwrite an existing experiment"
    );
    let crops_dir = root.join("crops");
    std::fs::create_dir_all(&crops_dir)?;
    let fixture: Value = serde_json::from_slice(&std::fs::read(fixture_dir.join("fixture.json"))?)?;
    let pages = fixture["pages"].as_array().context("fixture pages")?;

    let data = std::env::var("KOHARU_OCR017_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("koharu")
        });
    ensure!(
        data.join("models").exists(),
        "no models dir under {}",
        data.display()
    );
    let runtime = RuntimeManager::new(&data, ComputePolicy::PreferGpu)?;
    runtime.prepare().await?;
    koharu_llm::sys::initialize(&runtime)?;
    let backend = Arc::new(LlamaBackend::init()?);
    ensure!(backend.supports_gpu_offload(), "GPU runtime unavailable");
    let mut paddle = PaddleOcrVl::load(&runtime, false, backend).await?;
    eprintln!("[levers] loaded PaddleOCR-VL");
    let mut korean = KoreanOcr::load(&runtime).await?;
    eprintln!("[levers] loaded PP-OCRv5");
    // CPU: on CUDA the fork's mit48px currently fails with a BF16/F32 dtype
    // mismatch (a pre-existing engine bug, unrelated to this experiment).
    // KOHARU_LEVERS_SKIP_MIT=1 leaves it out (its lines are recorded empty).
    let mit = if std::env::var_os("KOHARU_LEVERS_SKIP_MIT").is_some() {
        eprintln!("[levers] skipping mit48px");
        None
    } else {
        let mit = Mit48pxOcr::load(&runtime, true).await?;
        eprintln!("[levers] loaded mit48px");
        Some(mit)
    };

    // Resume: skip blocks a previous (interrupted) run already recorded.
    let mut rows: Vec<Value> = match std::fs::read(root.join("progress.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(_) => Vec::new(),
    };
    let done: std::collections::HashSet<String> = rows
        .iter()
        .filter_map(|r| r["id"].as_str().map(str::to_owned))
        .collect();
    eprintln!("[levers] resuming with {} blocks already done", done.len());
    for page in pages {
        let name = page["page"].as_str().context("page name")?;
        let stem = name.trim_end_matches(".jpg");
        let source = image::open(fixture_dir.join(page["source"].as_str().context("source")?))
            .with_context(|| format!("source for {name}"))?;
        for block in page["blocks"].as_array().context("blocks")? {
            let n = block["block"].as_u64().context("block number")? as usize;
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
            let lines = split_text_lines(&verifier_crop(&source, &region));
            let lines_inv: Vec<DynamicImage> = lines.iter().map(invert).collect();
            // Save exactly what the readers see, so external readers (Hayai)
            // are evaluated on identical inputs.
            paddle_crop.save(crops_dir.join(format!("{id}-block.png")))?;
            for (k, line) in lines.iter().enumerate() {
                line.save(crops_dir.join(format!("{id}-line{k}.png")))?;
            }

            let vl_plain = vl(&mut paddle, &paddle_crop, None)?;
            let started = Instant::now();
            let vl_hint = vl(&mut paddle, &paddle_crop, Some(HINT))?;
            let vl_hint_ms = started.elapsed().as_millis();
            let vl_hint_inv = vl(&mut paddle, &invert(&paddle_crop), Some(HINT))?;
            let mut per_line = Vec::new();
            for line in &lines {
                per_line.push(vl(&mut paddle, line, Some(HINT))?);
            }
            let vl_hint_per_line = per_line
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let started = Instant::now();
            let pp = pp_lines(&mut korean, &lines)?;
            let pp_ms = started.elapsed().as_millis();
            let pp_inv = pp_lines(&mut korean, &lines_inv)?;
            let (mit_orig, mit_inv) = match &mit {
                Some(mit) => (mit_lines(mit, &lines)?, mit_lines(mit, &lines_inv)?),
                None => (Vec::new(), Vec::new()),
            };

            let e2e = json!({
                "reported_plain_vl_pp":   production(&vl_plain, &pp),
                "production_hint_vl_pp":  production(&vl_hint, &pp),
                "hint_vl_pp_inverted":    production(&vl_hint, &pp_inv),
                "hint_vl_mit":            production(&vl_hint, &mit_orig),
                "hint_vl_mit_inverted":   production(&vl_hint, &mit_inv),
                "per_line_vl_pp":         production(&vl_hint_per_line, &pp),
            });
            println!(
                "{id}: lines={} | production={}",
                lines.len(),
                e2e["production_hint_vl_pp"]
            );
            rows.push(json!({
                "id": id,
                "page": name,
                "block": n,
                "node_id": block["node_id"],
                "line_count": lines.len(),
                "vl": {
                    "plain": vl_plain,
                    "hint": vl_hint,
                    "hint_inverted": vl_hint_inv,
                    "hint_per_line": vl_hint_per_line,
                },
                "pp_ocrv5": {"original": lines_json(&pp), "inverted": lines_json(&pp_inv),
                             "original_joined": joined(&pp), "inverted_joined": joined(&pp_inv)},
                "mit48px": {"original": lines_json(&mit_orig), "inverted": lines_json(&mit_inv),
                            "original_joined": joined(&mit_orig), "inverted_joined": joined(&mit_inv)},
                "end_to_end": e2e,
                "timing_ms": {"vl_hint": vl_hint_ms, "pp_ocrv5": pp_ms},
            }));
            std::fs::write(
                root.join("progress.json"),
                serde_json::to_vec_pretty(&rows)?,
            )?;
        }
    }

    let report = json!({
        "completed": true,
        "note": "diagnostic only; references live in the fixture's answer_key.json",
        "hint": HINT,
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

/// Dump a project's scene as JSON from a COPY of its metadata
/// (scene.bin + history.log + project.toml), so a live project is never opened.
#[test]
#[ignore = "reads a copied project dir (KOHARU_PROJECT_COPY) and writes KOHARU_SCENE_OUT"]
fn dump_project_scene_copy() -> Result<()> {
    let dir = std::env::var("KOHARU_PROJECT_COPY")?;
    let out = std::env::var("KOHARU_SCENE_OUT")?;
    let session = koharu_app::session::ProjectSession::open(dir.as_str())?;
    std::fs::write(&out, serde_json::to_vec_pretty(&session.scene_snapshot())?)?;
    Ok(())
}
