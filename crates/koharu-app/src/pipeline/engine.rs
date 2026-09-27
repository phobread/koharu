//! Engine trait + inventory-based registry + DAG resolver.
//!
//! An engine is a pluggable model that transforms one page. It declares the
//! artifacts it needs and produces; the DAG resolver derives execution order.
//!
//! **Engines emit ops, not mutations.** `run()` returns `Vec<Op>`; the driver
//! wraps them in `Op::Batch` and hands to `ProjectSession::apply`.
//!
//! ## Adding an engine
//!
//! 1. Define a struct holding your model.
//! 2. Implement `Engine` for it (returning `Vec<Op>`).
//! 3. Register via `inventory::submit! { EngineInfo { … } }` with a static
//!    async `load` function.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use anyhow::{Result, bail};
use async_trait::async_trait;
use koharu_core::{
    NodeId, Op, PageId, ReadingOrder, Region, Scene, TextAlign, TextShaderEffect, TextStrokeStyle,
};
use koharu_runtime::RuntimeManager;
use parking_lot::RwLock;
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;
use tokio::sync::SemaphorePermit;
use tracing::Instrument;

use super::gpu_gate::GpuGate;
use crate::blobs::BlobStore;
use crate::llm;
use crate::pipeline::artifacts::Artifact;
use crate::renderer;

// ---------------------------------------------------------------------------
// EngineCtx — everything an engine needs to produce ops
// ---------------------------------------------------------------------------

pub struct EngineCtx<'a> {
    /// A cheap clone of the target page (read-only).
    pub scene: &'a Scene,
    pub page: PageId,
    pub blobs: &'a BlobStore,
    pub runtime: &'a RuntimeManager,
    pub cancel: &'a AtomicBool,
    pub options: &'a PipelineRunOptions,
    pub llm: &'a llm::Model,
    pub renderer: &'a renderer::Renderer,
}

/// Options threaded through a pipeline run.
#[derive(Debug, Clone, Default)]
pub struct PipelineRunOptions {
    pub target_language: Option<String>,
    /// Language of the SOURCE text, as an OCR hint (e.g. "Korean") — steers
    /// PaddleOCR-VL away from misreading stylized fonts as the wrong CJK
    /// script. `None` = model auto-detect (training-time prompt).
    pub source_language: Option<String>,
    pub system_prompt: Option<String>,
    pub default_font: Option<String>,
    /// Optional text-node scope for engines that can operate on individual
    /// text blocks. Engines that render full-page artifacts ignore it.
    pub text_node_ids: Option<Vec<NodeId>>,
    /// Optional bounding-box hint. Inpainter engines (lama/aot) honor it:
    /// composite onto the existing `Image { Inpainted }` (fallback Source)
    /// and process just that one block. Other engines ignore it.
    pub region: Option<Region>,
    /// Whether a regional inpaint should first restore its bounding rectangle
    /// from the source image. Mask erasure/un-inpaint needs this; repair-brush
    /// additions must keep the existing cleaned background or the rectangle
    /// itself becomes visible. `None` preserves the legacy restore behavior.
    pub restore_source_region: Option<bool>,
    /// Flux.2 Klein tuning. `None` leaves the engine's built-in default in effect.
    pub flux2_strength: Option<f64>,
    pub flux2_steps: Option<u32>,
    /// Fill plain single-colour bubbles flat instead of running Flux2.
    pub flux2_flat_fill: Option<bool>,
    pub reading_order: Option<ReadingOrder>,
    /// Global render defaults (renderer engine only). Applied when a text node
    /// has no explicit per-node override; otherwise the renderer auto-fits the
    /// font and derives stroke/alignment as before.
    pub default_font_size: Option<f32>,
    pub box_padding: Option<f32>,
    pub shader_effect: Option<TextShaderEffect>,
    pub shader_stroke: Option<TextStrokeStyle>,
    pub text_align: Option<TextAlign>,
}

// ---------------------------------------------------------------------------
// Engine trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait Engine: Send + Sync + 'static {
    /// Run the engine on one page. Return the ops to apply.
    /// Empty `Vec` = nothing changed (still a success).
    async fn run(&self, ctx: EngineCtx<'_>) -> Result<Vec<Op>>;
}

// ---------------------------------------------------------------------------
// EngineInfo — static descriptor + factory (registered via inventory)
// ---------------------------------------------------------------------------

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type EngineLoadFn =
    for<'a> fn(&'a RuntimeManager, bool) -> BoxFuture<'a, Result<Box<dyn Engine>>>;

pub struct EngineInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub needs: &'static [Artifact],
    pub produces: &'static [Artifact],
    pub load: EngineLoadFn,
}

inventory::collect!(EngineInfo);

// ---------------------------------------------------------------------------
// Registry — lazy load + cache engine instances
// ---------------------------------------------------------------------------

/// Engines too large to share the GPU. Flux.2 Klein alone nearly fills a
/// 6 GB card; kept resident beside the detection/OCR models, the driver falls
/// back to system memory and every step slows 3-75x (measured 2026-09-27).
/// Loading one of these unloads every other GPU engine, and loading any other
/// GPU engine unloads these.
const EXCLUSIVE_ENGINES: &[&str] = &["flux2-klein"];

/// CPU-only engines: they never trigger or suffer eviction.
const CPU_ENGINES: &[&str] = &["koharu-renderer", "llm"];

/// Cached engines to unload before `requested` runs, per the residency rules
/// above.
fn engines_to_evict<'a>(
    loaded: impl IntoIterator<Item = &'a str>,
    requested: &str,
) -> Vec<&'a str> {
    if CPU_ENGINES.contains(&requested) {
        return Vec::new();
    }
    let requested_exclusive = EXCLUSIVE_ENGINES.contains(&requested);
    loaded
        .into_iter()
        .filter(|&other| other != requested && !CPU_ENGINES.contains(&other))
        .filter(|other| requested_exclusive || EXCLUSIVE_ENGINES.contains(other))
        .collect()
}

pub struct Registry {
    engines: RwLock<HashMap<&'static str, Arc<dyn Engine>>>,
    gpu: GpuGate,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            engines: RwLock::new(HashMap::new()),
            gpu: GpuGate::default(),
        }
    }
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Wait for the GPU before loading and running engine `id`: GPU engines
    /// take turns across pipeline jobs and repair-brush strokes (see
    /// [`GpuGate`]), while CPU engines and CPU-only runs never wait. Fails
    /// with [`crate::Cancelled`] if `cancel` is set while waiting. Keep the
    /// turn until the engine handle from [`Self::get`] has been dropped.
    pub async fn gpu_turn(
        &self,
        id: &str,
        cpu: bool,
        cancel: &AtomicBool,
    ) -> Result<Option<SemaphorePermit<'_>>> {
        if cpu || CPU_ENGINES.contains(&id) {
            return Ok(None);
        }
        match self.gpu.turn(cancel).await {
            Some(turn) => Ok(Some(turn)),
            None => Err(crate::Cancelled.into()),
        }
    }

    /// Get or load an engine instance by id.
    pub async fn get(
        &self,
        id: &str,
        runtime: &RuntimeManager,
        cpu: bool,
    ) -> Result<Arc<dyn Engine>> {
        if !cpu {
            self.make_room_for(id);
        }
        if let Some(engine) = self.engines.read().get(id).cloned() {
            return Ok(engine);
        }
        let info = Self::find(id)?;
        let started = Instant::now();
        let loaded = async { (info.load)(runtime, cpu).await }
            .instrument(tracing::info_span!("engine_load", engine = id))
            .await?;
        tracing::info!(
            engine = id,
            elapsed_ms = started.elapsed().as_millis(),
            "engine loaded"
        );
        let engine: Arc<dyn Engine> = Arc::from(loaded);
        self.engines.write().insert(info.id, engine.clone());
        Ok(engine)
    }

    /// Unload the engines that must not share the GPU with `id`, then return
    /// their freed memory to the driver. An engine still in use elsewhere (a
    /// concurrent job) is only freed once that job drops it.
    fn make_room_for(&self, id: &str) {
        let evicted: Vec<(&'static str, Arc<dyn Engine>)> = {
            let mut engines = self.engines.write();
            engines_to_evict(engines.keys().copied(), id)
                .into_iter()
                .filter_map(|victim| engines.remove_entry(victim))
                .collect()
        };
        if evicted.is_empty() {
            return;
        }
        let names: Vec<&str> = evicted.iter().map(|(name, _)| *name).collect();
        let started = Instant::now();
        drop(evicted);
        if let Err(err) = koharu_ml::release_gpu_memory() {
            tracing::warn!("failed to release GPU memory after unloading engines: {err:#}");
        }
        tracing::info!(
            engine = id,
            unloaded = ?names,
            elapsed_ms = started.elapsed().as_millis(),
            "unloaded engines to free GPU memory"
        );
    }

    /// Drop all cached engines (frees GPU memory).
    pub fn clear(&self) {
        self.engines.write().clear();
    }

    /// Find engine descriptor by id.
    pub fn find(id: &str) -> Result<&'static EngineInfo> {
        Self::catalog()
            .into_iter()
            .find(|e| e.id == id)
            .ok_or_else(|| anyhow::anyhow!("unknown engine: {id}"))
    }

    /// All registered engine descriptors.
    pub fn catalog() -> Vec<&'static EngineInfo> {
        inventory::iter::<EngineInfo>.into_iter().collect()
    }

    /// Engines that produce a given artifact.
    pub fn providers(artifact: Artifact) -> Vec<&'static EngineInfo> {
        Self::catalog()
            .into_iter()
            .filter(|e| e.produces.contains(&artifact))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// DAG — derive execution order from artifact dependencies
// ---------------------------------------------------------------------------

/// Build a topological execution order from a set of engine infos.
pub fn build_order(infos: &[&EngineInfo]) -> Result<Vec<usize>> {
    let mut g = DiGraph::<usize, ()>::new();
    let mut id_to_node: HashMap<&str, _> = HashMap::new();

    for (i, info) in infos.iter().enumerate() {
        let n = g.add_node(i);
        if id_to_node.insert(info.id, n).is_some() {
            bail!("duplicate engine: {}", info.id);
        }
    }

    let mut producers: HashMap<Artifact, usize> = HashMap::new();
    for (i, info) in infos.iter().enumerate() {
        for &artifact in info.produces {
            producers.insert(artifact, i);
        }
    }

    for info in infos.iter() {
        let to = id_to_node[info.id];
        for &artifact in info.needs {
            if let Some(&producer) = producers.get(&artifact) {
                g.add_edge(id_to_node[infos[producer].id], to, ());
            }
        }
    }

    let order = toposort(&g, None)
        .map_err(|c| anyhow::anyhow!("cycle at '{}'", infos[g[c.node_id()]].id))?;
    Ok(order.into_iter().map(|n| g[n]).collect())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::{Registry, engines_to_evict};

    const DETECT_OCR: [&str; 5] = [
        "comic-text-bubble-detector",
        "comic-text-detector-seg",
        "speech-bubble-segmentation",
        "yuzumarker-font-detection",
        "paddle-ocr-vl-1.6",
    ];

    fn sorted(mut ids: Vec<&str>) -> Vec<&str> {
        ids.sort_unstable();
        ids
    }

    #[test]
    fn exclusive_engine_unloads_every_other_gpu_engine() {
        let loaded = DETECT_OCR
            .iter()
            .copied()
            .chain(["lama-manga", "koharu-renderer", "llm"]);
        let mut expected = DETECT_OCR.to_vec();
        expected.push("lama-manga");
        assert_eq!(
            sorted(engines_to_evict(loaded, "flux2-klein")),
            sorted(expected)
        );
    }

    #[test]
    fn gpu_engine_unloads_only_exclusive_engines() {
        let loaded = ["flux2-klein", "comic-text-detector-seg", "koharu-renderer"];
        assert_eq!(
            engines_to_evict(loaded, "paddle-ocr-vl-1.6"),
            vec!["flux2-klein"]
        );
    }

    #[test]
    fn cpu_engines_neither_trigger_nor_suffer_eviction() {
        let loaded = ["flux2-klein", "paddle-ocr-vl-1.6", "llm"];
        assert!(engines_to_evict(loaded, "koharu-renderer").is_empty());
        assert!(engines_to_evict(loaded, "llm").is_empty());
        assert!(engines_to_evict(["koharu-renderer", "llm"], "flux2-klein").is_empty());
    }

    #[test]
    fn requested_engine_is_never_evicted() {
        assert!(engines_to_evict(["flux2-klein"], "flux2-klein").is_empty());
        assert!(engines_to_evict(["lama-manga"], "lama-manga").is_empty());
    }

    #[tokio::test]
    async fn gpu_engines_take_turns_and_cpu_engines_never_wait() {
        let registry = Registry::new();
        let never = AtomicBool::new(false);
        let held = registry.gpu_turn("flux2-klein", false, &never).await;
        assert!(held.unwrap().is_some());

        assert!(
            registry
                .gpu_turn("llm", false, &never)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            registry
                .gpu_turn("koharu-renderer", false, &never)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            registry
                .gpu_turn("lama-manga", true, &never)
                .await
                .unwrap()
                .is_none()
        );

        let cancelled = AtomicBool::new(true);
        let err = registry
            .gpu_turn("lama-manga", false, &cancelled)
            .await
            .unwrap_err();
        assert!(crate::is_cancelled(&err));
    }
}
