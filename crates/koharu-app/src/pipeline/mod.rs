//! Pipeline: runs an ordered set of engines across one or more pages and
//! wraps each engine's output in one `Op::Batch` before applying via the
//! session's history.
//!
//! **Engines don't mutate the scene.** They return `Vec<Op>`; this driver
//! applies them transactionally (per-engine) against the active session.

pub mod artifacts;
pub mod engine;
mod engines;
mod gpu_gate;
mod missing;
mod plan;

pub use artifacts::Artifact;
pub use engine::{
    BoxFuture, Engine, EngineCtx, EngineInfo, EngineLoadFn, PipelineRunOptions, Registry,
    build_order,
};
pub use engines::support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::Result;
use koharu_core::{Op, PageId, PipelineStep};
use koharu_runtime::RuntimeManager;
use tracing::Instrument;

/// Observer for pipeline progress. `step_id` is the engine id of the step
/// about to run (or just finished); step_index / page_index are 0-based.
pub type ProgressSink = Arc<dyn Fn(ProgressTick) + Send + Sync>;

/// Observer for non-fatal step failures. Called once per failed step; the
/// pipeline then skips that page's steps that need the failed step's output
/// and carries on with the rest.
pub type WarningSink = Arc<dyn Fn(WarningTick) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct ProgressTick {
    /// Coarse UI-facing step tag derived from the engine's primary
    /// produced artifact. `None` for the final 100% tick where no engine
    /// is running.
    pub step: Option<PipelineStep>,
    /// Engine id (e.g. `"paddle-ocr-vl-1.6"`) for diagnostics + logs.
    pub step_id: String,
    pub step_index: usize,
    pub total_steps: usize,
    pub page_index: usize,
    pub total_pages: usize,
    pub overall_percent: u8,
}

#[derive(Debug, Clone)]
pub struct WarningTick {
    pub step_id: String,
    pub page_index: usize,
    pub total_pages: usize,
    pub message: String,
}

/// Returned by [`run`]. `warning_count == 0` means the run finished cleanly.
#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub warning_count: usize,
}

/// Map an engine's produced artifact to its UI step category. Stays
/// co-located with the engine metadata so adding a new engine can't
/// silently bypass the toolbar spinner — only the registered artifact
/// matters, not the engine's string id.
fn step_for(info: &EngineInfo) -> Option<PipelineStep> {
    info.produces.iter().find_map(|a| match a {
        Artifact::TextBoxes
        | Artifact::SegmentMask
        | Artifact::FontPredictions
        | Artifact::BubbleMask => Some(PipelineStep::Detect),
        Artifact::OcrText => Some(PipelineStep::Ocr),
        Artifact::Translations => Some(PipelineStep::LlmGenerate),
        Artifact::Inpainted => Some(PipelineStep::Inpaint),
        Artifact::FinalRender => Some(PipelineStep::Render),
        // Non-UI-facing artifacts (inputs, intermediate sprites) — no
        // toolbar step tag.
        _ => None,
    })
}

use crate::llm;
use crate::renderer;
use crate::session::ProjectSession;

// ---------------------------------------------------------------------------
// Spec + scope
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PipelineSpec {
    pub scope: Scope,
    pub steps: Vec<String>,
    pub options: PipelineRunOptions,
    /// Run each step only where its output is missing, and the text steps
    /// only on the boxes that lack their field (see [`missing`]). Callers
    /// should also narrow the scope with [`pages_with_missing_work`].
    pub only_missing: bool,
}

/// The pages of `pages` on which an only-missing run of `steps` has
/// anything to do, in the given order.
pub fn pages_with_missing_work(
    scene: &koharu_core::Scene,
    steps: &[String],
    pages: Vec<PageId>,
) -> Result<Vec<PageId>> {
    let produces: Vec<&[Artifact]> = steps
        .iter()
        .map(|id| Registry::find(id).map(|info| info.produces))
        .collect::<Result<_>>()?;
    Ok(pages
        .into_iter()
        .filter(|id| {
            scene
                .pages
                .get(id)
                .is_some_and(|page| missing::page_needs_work(&produces, page))
        })
        .collect())
}

#[derive(Debug, Clone)]
pub enum Scope {
    WholeProject,
    Pages(Vec<PageId>),
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

/// Execute `spec` against `session`. Each engine step becomes one `Op::Batch`
/// applied via the session's history (one undo step per step per page).
///
/// A failed step on a given page is non-fatal: one [`WarningTick`] is emitted
/// via `warnings`, the page's later steps that need (directly or not) what it
/// would have produced are skipped, and independent ones still run, so a
/// failed translation doesn't cost the page its inpainting. The function
/// returns the total number of per-step warnings that fired, letting callers
/// flag the run as `CompletedWithErrors`.
///
/// Pages run one after another (see [`chunk_size`] for the opt-in
/// alternative).
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(level = "info", skip_all)]
pub async fn run(
    session: Arc<ProjectSession>,
    registry: Arc<Registry>,
    runtime: Arc<RuntimeManager>,
    cpu: bool,
    llm: Arc<llm::Model>,
    renderer: Arc<renderer::Renderer>,
    spec: PipelineSpec,
    cancel: Arc<AtomicBool>,
    progress: Option<ProgressSink>,
    warnings: Option<WarningSink>,
) -> Result<RunOutcome> {
    let infos: Vec<&EngineInfo> = spec
        .steps
        .iter()
        .map(|id| Registry::find(id))
        .collect::<Result<_>>()?;
    let order = build_order(&infos)?;

    let pages = match &spec.scope {
        Scope::WholeProject => session
            .scene
            .read()
            .pages
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        Scope::Pages(ids) => ids.clone(),
    };

    let total_pages = pages.len().max(1);
    let total_steps = order.len().max(1);
    let total_units = (total_pages * total_steps) as u64;
    let mut warning_count: usize = 0;
    let steps: Vec<(&EngineInfo, plan::StepIo)> = order
        .iter()
        .map(|&i| {
            let io = plan::StepIo {
                needs: infos[i].needs,
                produces: infos[i].produces,
            };
            (infos[i], io)
        })
        .collect();
    // Per page: what failed steps never produced, and whether the user has
    // deleted the page since the run started.
    let mut missing = vec![plan::MissingArtifacts::default(); pages.len()];
    let mut deleted = vec![false; pages.len()];
    // Only-missing runs: whether this run has changed the page yet.
    let mut changed = vec![false; pages.len()];
    let chunk = chunk_size();
    if chunk > 1 {
        tracing::info!(
            chunk,
            pages = pages.len(),
            "running pages in chunks, stage by stage"
        );
    }

    // `completed` counts units already handled, whether they ran, failed or
    // were skipped, so progress always reaches 100%.
    let units = plan::schedule(pages.len(), steps.len(), chunk);
    for (completed, (page_index, seq)) in (0_u64..).zip(units) {
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::Cancelled.into());
        }
        let (info, io) = steps[seq];
        let page_id = &pages[page_index];
        let percent = ((completed * 100) / total_units).min(100) as u8;

        if deleted[page_index] || !session.scene.read().pages.contains_key(page_id) {
            deleted[page_index] = true;
            continue;
        }
        // Skip only the steps that need what a failed step never produced:
        // a failed translation still leaves the page to be inpainted.
        if missing[page_index].blocks(io) {
            missing[page_index].record(io);
            continue;
        }
        let work = if spec.only_missing {
            match session.scene.read().pages.get(page_id) {
                Some(page) => missing::missing_work(io.produces, page, changed[page_index]),
                None => missing::Work::Skip,
            }
        } else {
            missing::Work::Page
        };
        if work.is_skip() {
            continue;
        }
        // Text steps limited to the boxes that lack their field.
        let node_options;
        let options = match &work {
            missing::Work::Nodes(ids) => {
                node_options = PipelineRunOptions {
                    text_node_ids: Some(ids.clone()),
                    ..spec.options.clone()
                };
                &node_options
            }
            _ => &spec.options,
        };

        if let Some(sink) = progress.as_ref() {
            sink(ProgressTick {
                step: step_for(info),
                step_id: info.id.to_string(),
                step_index: seq,
                total_steps,
                page_index,
                total_pages,
                overall_percent: percent,
            });
            // Let the tick go out before the step. Engines compute without
            // yielding, and the task that streams this event to clients was
            // woken on this worker thread, where it would otherwise wait for
            // the whole run (measured: no progress reached the UI for 100 s).
            tokio::task::yield_now().await;
        }

        // Declared before `engine`, so the turn outlives the engine handle.
        let _gpu_turn = registry.gpu_turn(info.id, cpu, &cancel).await?;
        let engine = match registry.get(info.id, &runtime, cpu).await {
            Ok(e) => e,
            Err(err) => {
                // Engine *load* failure: same recovery as a run failure.
                report_step_failure(
                    info.id,
                    page_id,
                    seq,
                    page_index,
                    total_pages,
                    &err,
                    &mut warning_count,
                    warnings.as_ref(),
                );
                missing[page_index].record(io);
                continue;
            }
        };
        let scene_snap = session.scene_snapshot();
        let ctx = EngineCtx {
            scene: &scene_snap,
            page: *page_id,
            blobs: &session.blobs,
            runtime: &runtime,
            cancel: &cancel,
            options,
            llm: &llm,
            renderer: &renderer,
        };
        let step_started = Instant::now();
        let step_result = async { engine.run(ctx).await }
            .instrument(tracing::info_span!("step", engine = info.id, page = %page_id))
            .await;
        tracing::info!(
            engine = info.id,
            page = %page_id,
            elapsed_ms = step_started.elapsed().as_millis(),
            "pipeline step finished"
        );
        let mut ops = match step_result {
            Ok(ops) => ops,
            Err(err) => {
                report_step_failure(
                    info.id,
                    page_id,
                    seq,
                    page_index,
                    total_pages,
                    &err,
                    &mut warning_count,
                    warnings.as_ref(),
                );
                missing[page_index].record(io);
                continue;
            }
        };
        if let missing::Work::Nodes(ids) = &work {
            // Engines that ignore `text_node_ids` still may not touch the
            // boxes that already have their field.
            ops.retain(|op| matches!(op, Op::UpdateNode { id, .. } if ids.contains(id)));
        }
        if ops.is_empty() {
            continue;
        }
        let batch = Op::Batch {
            ops,
            label: format!("{}: page {}", info.id, page_id),
        };
        if let Err(err) = session.apply(batch) {
            report_step_failure(
                info.id,
                page_id,
                seq,
                page_index,
                total_pages,
                &err,
                &mut warning_count,
                warnings.as_ref(),
            );
            missing[page_index].record(io);
        } else {
            changed[page_index] = true;
        }
    }

    if let Some(sink) = progress.as_ref() {
        sink(ProgressTick {
            step: None,
            step_id: String::new(),
            step_index: total_steps.saturating_sub(1),
            total_steps,
            page_index: total_pages.saturating_sub(1),
            total_pages,
            overall_percent: 100,
        });
    }
    Ok(RunOutcome { warning_count })
}

/// Pages per chunk: 1 (page by page) unless `KOHARU_PIPELINE_CHUNK` says
/// otherwise. Chunks go stage by stage, so Flux.2 Klein and the
/// detection/OCR engines, which evict each other, load once per chunk instead
/// of once per page. Measured on 9 pages of 926 (2026-09-28), chunks of 8 cut
/// loads 54 -> 12 but saved only ~3% (393-419 s vs 408-423 s, one run 546 s),
/// and the first page was ready at 62 s instead of 29 s.
fn chunk_size() -> usize {
    std::env::var("KOHARU_PIPELINE_CHUNK")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map_or(1, |chunk| chunk.max(1))
}

#[allow(clippy::too_many_arguments)]
fn report_step_failure(
    engine_id: &str,
    page_id: &PageId,
    step_index: usize,
    page_index: usize,
    total_pages: usize,
    err: &anyhow::Error,
    warning_count: &mut usize,
    sink: Option<&WarningSink>,
) {
    tracing::warn!(
        engine = engine_id,
        page = %page_id,
        step_index,
        "pipeline step failed: {err:#}"
    );
    *warning_count += 1;
    if let Some(sink) = sink {
        sink(WarningTick {
            step_id: engine_id.to_string(),
            page_index,
            total_pages,
            message: format!("{err:#}"),
        });
    }
}

// ---------------------------------------------------------------------------
// Engine catalog building (API surface)
// ---------------------------------------------------------------------------

use koharu_core::{EngineCatalog, EngineCatalogEntry};

/// Build the engine catalog DTO for the API.
pub fn catalog() -> EngineCatalog {
    let entry = |info: &&EngineInfo| EngineCatalogEntry {
        id: info.id.to_string(),
        name: info.name.to_string(),
        produces: info.produces.iter().map(|a| format!("{a:?}")).collect(),
    };
    EngineCatalog {
        detectors: Registry::providers(Artifact::TextBoxes)
            .iter()
            .map(entry)
            .collect(),
        font_detectors: Registry::providers(Artifact::FontPredictions)
            .iter()
            .map(entry)
            .collect(),
        segmenters: Registry::providers(Artifact::SegmentMask)
            .iter()
            .map(entry)
            .collect(),
        bubble_segmenters: Registry::providers(Artifact::BubbleMask)
            .iter()
            .map(entry)
            .collect(),
        ocr: Registry::providers(Artifact::OcrText)
            .iter()
            .map(entry)
            .collect(),
        translators: Registry::providers(Artifact::Translations)
            .iter()
            .map(entry)
            .collect(),
        inpainters: Registry::providers(Artifact::Inpainted)
            .iter()
            .map(entry)
            .collect(),
        renderers: Registry::providers(Artifact::FinalRender)
            .iter()
            .map(entry)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_includes_anime_text_detector() {
        let catalog = catalog();

        assert!(catalog.detectors.iter().any(|engine| {
            engine.id == "anime-text"
                && engine.name == "Anime Text YOLO (N)"
                && engine.produces.iter().map(String::as_str).eq(["TextBoxes"])
        }));
    }
}
