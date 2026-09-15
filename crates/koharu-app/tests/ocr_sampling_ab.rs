//! Opt-in real-page OCR experiment. Reads retained scene/image fixtures only;
//! never opens a user project or starts the desktop/server.
use std::{path::PathBuf, sync::Arc, time::Instant};

use anyhow::{Context, Result, ensure};
use image::DynamicImage;
use koharu_app::pipeline::support::{single_line_ocr_text, text_node_to_region, text_nodes};
use koharu_core::Scene;
use koharu_llm::{
    paddleocr_vl::{PaddleOcrVl, PaddleOcrVlGenerateOptions, PaddleOcrVlTask},
    safe::llama_backend::LlamaBackend,
};
use koharu_ml::{
    TextRegion,
    comic_text_detector::crop_text_block_deskewed,
    korean_ocr::{KoreanOcr, contains_lexical_hangul, repair_hangul},
};
use koharu_runtime::{ComputePolicy, RuntimeManager};
use serde_json::{Value, json};

// Mirrors production's tighter PP-OCR crop, distinct from the Paddle VL crop.
fn verification_crop(image: &DynamicImage, region: &TextRegion) -> DynamicImage {
    let mut tight = region.clone();
    let pad = (tight.width.min(tight.height) * 0.03).max(2.0);
    tight.x -= pad;
    tight.y -= pad;
    tight.width += pad * 2.0;
    tight.height += pad * 2.0;
    tight.detector = None;
    tight.line_polygons = None;
    crop_text_block_deskewed(image, &tight)
}

#[test]
#[ignore = "requires prepared isolated runtime/models and retained M fixtures"]
fn compare_m_sampling() -> Result<()> {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(run())
        })?
        .join()
        .expect("OCR experiment thread panicked")
}

async fn run() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("koharu_llm::paddleocr_vl=debug")
        .with_ansi(false)
        .try_init();
    let root = PathBuf::from(std::env::var("KOHARU_OCR_AB_ROOT")?)
        .canonicalize()
        .context("prepared experiment directory")?;
    let fixture = PathBuf::from(std::env::var("KOHARU_OCR_AB_FIXTURE")?).canonicalize()?;
    ensure!(
        !root.join("results.json").exists(),
        "refusing to overwrite an existing experiment"
    );
    std::fs::create_dir_all(root.join("crops"))?;
    let envelope: Value = serde_json::from_slice(&std::fs::read(fixture.join("before.json"))?)?;
    let scene: Scene = serde_json::from_value(envelope["scene"].clone())?;
    let pages = ["001.jpg", "002.jpg", "003.jpg"]
        .map(|name| {
            scene
                .pages
                .values()
                .find(|page| page.name == name)
                .with_context(|| format!("missing {name}"))
        })
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        pages
            .iter()
            .map(|page| text_nodes(&scene, page.id).len())
            .collect::<Vec<_>>()
            == [9, 15, 11]
    );

    let runtime = RuntimeManager::new(root.join("data"), ComputePolicy::PreferGpu)?;
    // Assets must be staged first. Binding initialization uses only staged DLLs.
    runtime.prepare().await?;
    koharu_llm::sys::initialize(&runtime)?;
    let backend = Arc::new(LlamaBackend::init()?);
    ensure!(backend.supports_gpu_offload(), "GPU runtime unavailable");
    let load_started = Instant::now();
    let mut paddle =
        PaddleOcrVl::load_from_dir(&runtime, root.join("paddle-model"), false, backend)?;
    let paddle_load_seconds = load_started.elapsed().as_secs_f64();
    let verifier_started = Instant::now();
    let mut korean = KoreanOcr::load(&runtime).await?;
    let verifier_load_seconds = verifier_started.elapsed().as_secs_f64();
    let mut rows = Vec::new();
    let mut page_hybrid = Vec::new();
    let mut warmup_seconds = 0.0;
    for (page_index, page) in pages.iter().enumerate() {
        let label = format!("M-{:03}", page_index + 1);
        let source = image::load_from_memory(&std::fs::read(
            fixture.join("images").join(format!("{label}-source.webp")),
        )?)?;
        let mut has_hangul = [false; 2];
        for (index, (node_id, transform, saved)) in text_nodes(&scene, page.id).iter().enumerate() {
            let region = text_node_to_region(transform, saved);
            let crop = crop_text_block_deskewed(&source, &region);
            let crop_file = format!("crops/{label}-{:02}.png", index + 1);
            crop.save(root.join(&crop_file))?;
            if rows.is_empty() {
                let started = Instant::now();
                paddle.inference_with_options(
                    &crop,
                    PaddleOcrVlTask::Ocr,
                    &PaddleOcrVlGenerateOptions::default(),
                )?;
                warmup_seconds = started.elapsed().as_secs_f64();
            }
            let mut variants = [Value::Null, Value::Null];
            let order = if rows.len() % 2 == 0 { [0, 1] } else { [1, 0] };
            for variant in order {
                let options = PaddleOcrVlGenerateOptions {
                    max_new_tokens: 256,
                    repetition_penalty: if variant == 0 { 1.0 } else { 1.2 },
                    repetition_last_n: 512,
                    ..Default::default()
                };
                let started = Instant::now();
                let output =
                    paddle.inference_with_options(&crop, PaddleOcrVlTask::Ocr, &options)?;
                let seconds = started.elapsed().as_secs_f64();
                has_hangul[variant] |= contains_lexical_hangul(&output.text);
                variants[variant] = json!({"seconds": seconds, "output": output});
            }
            let started = Instant::now();
            let lines = korean.recognize_block(&verification_crop(&source, &region))?;
            let verifier_seconds = started.elapsed().as_secs_f64();
            for variant in &mut variants {
                let single_line = single_line_ocr_text(variant["output"]["text"].as_str().unwrap());
                variant["single_line"] = json!(single_line);
                variant["repaired"] = json!(repair_hangul(&single_line, &lines, 0.90));
            }
            println!(
                "{label} block {}/{}: greedy {:.2}s; penalty {:.2}s; changed {}",
                index + 1,
                text_nodes(&scene, page.id).len(),
                variants[0]["seconds"].as_f64().unwrap(),
                variants[1]["seconds"].as_f64().unwrap(),
                variants[0]["repaired"] != variants[1]["repaired"]
            );
            rows.push(json!({"page": label, "block": index + 1, "node_id": node_id,
                "crop": crop_file, "crop_size": [crop.width(), crop.height()],
                "saved_reference_not_ground_truth": saved.text,
                "order": order, "greedy": variants[0], "penalty": variants[1],
                "verifier_seconds": verifier_seconds,
                "verifier_lines": lines.iter().map(|line| json!({"text": line.text, "confidence": line.confidence})).collect::<Vec<_>>()
            }));
            std::fs::write(
                root.join("progress.json"),
                serde_json::to_vec_pretty(&rows)?,
            )?;
        }
        // Repair is active in production only if a block on the page has Hangul.
        // Verify that the above repaired comparison represents that path in both runs.
        ensure!(
            has_hangul == [true, true],
            "hybrid activation differs from the experiment on {label}"
        );
        page_hybrid.push(json!({"page": label, "hybrid_active": has_hangul}));
    }
    let report = json!({"completed": true, "max_new_tokens": 256, "language": null,
        "repeat_penalties": [1.0, 1.2], "repeat_last_n": 512,
        "prompt_text_history_seeded": true, "string_repeat_guard_unchanged": true,
        "paddle_load_seconds": paddle_load_seconds, "verifier_load_seconds": verifier_load_seconds,
        "warmup_seconds": warmup_seconds, "page_hybrid": page_hybrid, "rows": rows});
    std::fs::write(
        root.join("results.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(())
}
