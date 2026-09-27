//! Opt-in helper: build (or re-run steps on) a SEPARATE evaluation project the
//! owner can open in Koharu, without touching any existing project.
//!
//! Mode `create`: makes `KOHARU_EVAL_PROJECT` (must not exist), imports every
//! `source-*.jpg` from `KOHARU_EVAL_PAGES` as pages named `NNN.jpg` in
//! filename order, then runs `KOHARU_EVAL_STEPS` over all pages.
//! Mode `open`: re-opens that project and runs `KOHARU_EVAL_STEPS` again
//! (e.g. OCR after the detection pass).
//! Both modes write the final scene (text nodes, transforms, OCR text) to
//! `KOHARU_EVAL_SCENE_OUT` as JSON.
//!
//! Models are read from `KOHARU_OCR017_DATA` (default %LOCALAPPDATA%/koharu);
//! the app config lives in a throwaway temp dir, so the owner's settings are
//! never read or written.
//! Run: bun cargo test -p koharu-app --features cuda --test ocr_eval_project \
//!        -- --ignored --nocapture

use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::{Context, Result, anyhow, ensure};
use camino::Utf8PathBuf;
use image::GenericImageView;
use koharu_app::{App, AppConfig, PipelineRunOptions, pipeline};
use koharu_core::{ImageData, ImageRole, Node, NodeId, NodeKind, Op, Page, PageId, Transform};
use koharu_runtime::{ComputePolicy, RuntimeManager};

#[test]
#[ignore = "builds a project from local images with the real models; owner-requested"]
fn build_eval_project() -> Result<()> {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(run())
        })?
        .join()
        .map_err(|_| anyhow!("eval project thread panicked"))?
}

async fn run() -> Result<()> {
    let mode = std::env::var("KOHARU_EVAL_MODE")?;
    let project: Utf8PathBuf = std::env::var("KOHARU_EVAL_PROJECT")?.parse()?;
    let steps: Vec<String> = std::env::var("KOHARU_EVAL_STEPS")?
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let scene_out = PathBuf::from(std::env::var("KOHARU_EVAL_SCENE_OUT")?);
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

    let config_root = tempfile::tempdir()?;
    let mut cfg = AppConfig::default();
    cfg.data.path = config_root.path().to_string_lossy().parse()?;
    let runtime = RuntimeManager::new(&data, ComputePolicy::PreferGpu)?;
    runtime.prepare().await?;
    let app = Arc::new(App::new(cfg, Arc::new(runtime), false, "eval")?);

    let pages: Vec<PageId> = match mode.as_str() {
        "create" => {
            ensure!(
                !project.exists(),
                "{project} already exists; refusing to overwrite"
            );
            let pages_dir = PathBuf::from(std::env::var("KOHARU_EVAL_PAGES")?);
            app.open_project(project.clone(), Some("BadEnd OCR test".to_owned()))
                .await?;
            let mut sources: Vec<PathBuf> = std::fs::read_dir(&pages_dir)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("source-") && n.ends_with(".jpg"))
                })
                .collect();
            sources.sort();
            let mut ids = Vec::new();
            for (index, path) in sources.iter().enumerate() {
                ids.push(import_page(&app, path, index)?);
            }
            eprintln!("[eval] imported {} pages", ids.len());
            ids
        }
        "open" => {
            app.open_project(project.clone(), None).await?;
            let session = app.current_session().context("no session")?;
            let scene = session.scene.read();
            let mut pages: Vec<_> = scene
                .pages
                .values()
                .map(|p| (p.name.clone(), p.id))
                .collect();
            pages.sort();
            pages.into_iter().map(|(_, id)| id).collect()
        }
        other => anyhow::bail!("KOHARU_EVAL_MODE must be create|open, got {other}"),
    };

    if !steps.is_empty() {
        eprintln!(
            "[eval] running {} on {} pages",
            steps.join(" -> "),
            pages.len()
        );
        let warnings: pipeline::WarningSink = Arc::new(|tick: pipeline::WarningTick| {
            eprintln!(
                "[eval] warn: {} failed on page {}: {}",
                tick.step_id,
                tick.page_index + 1,
                tick.message
            );
        });
        let session = app.current_session().context("no session")?;
        let outcome = pipeline::run(
            session,
            app.registry.clone(),
            app.runtime.clone(),
            app.cpu_only(),
            app.llm.clone(),
            app.renderer.clone(),
            pipeline::PipelineSpec {
                scope: pipeline::Scope::Pages(pages.clone()),
                steps,
                options: PipelineRunOptions {
                    source_language: Some("Korean".to_owned()),
                    ..Default::default()
                },
            },
            Arc::new(AtomicBool::new(false)),
            None,
            Some(warnings),
        )
        .await?;
        ensure!(
            outcome.warning_count == 0,
            "{} step failures",
            outcome.warning_count
        );
    }

    {
        let session = app.current_session().context("no session")?;
        let scene = session.scene.read();
        std::fs::write(&scene_out, serde_json::to_vec_pretty(&*scene)?)?;
    }
    app.close_project().await?;
    eprintln!("[eval] done; scene -> {}", scene_out.display());
    Ok(())
}

/// Mirrors the pipeline CLI's `import_page`, appending in order.
fn import_page(app: &App, input: &std::path::Path, at: usize) -> Result<PageId> {
    let bytes = std::fs::read(input).with_context(|| format!("read {}", input.display()))?;
    let decoded = image::load_from_memory(&bytes)?;
    let (w, h) = decoded.dimensions();
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("source-"))
        .context("source-NNN.jpg name")?;
    let name = format!("{stem}.jpg");
    let session = app.current_session().context("no session open")?;
    let blob = session.blobs.put_bytes(&bytes)?;
    let mut page = Page::new(&name, w, h);
    let page_id = page.id;
    let node_id = NodeId::new();
    page.nodes.insert(
        node_id,
        Node {
            id: node_id,
            transform: Transform::default(),
            visible: true,
            kind: NodeKind::Image(ImageData {
                role: ImageRole::Source,
                blob,
                opacity: 1.0,
                natural_width: w,
                natural_height: h,
                name: Some(name),
            }),
        },
    );
    app.apply(Op::AddPage { page, at })?;
    Ok(page_id)
}
