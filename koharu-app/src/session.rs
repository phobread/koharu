//! A loaded project. One `ProjectSession` = one `.khrproj/` directory.
//!
//! Holds:
//!   - an exclusive `.lock` via `fs4` (refuses second opener)
//!   - the in-memory `Scene` behind a `parking_lot::RwLock` (never held across `.await`)
//!   - the `History` behind a `Mutex` (linear, all writes serialized)
//!   - the `BlobStore` (content-addressed images)
//!
//! On-disk layout:
//!   `.khrproj/project.toml`    — TOML-encoded `ProjectMeta`
//!   `.khrproj/scene.bin`       — `"KSCN"` + u16 LE format version + postcard
//!                                `Snapshot { epoch, scene }` (headerless files
//!                                predate versioning and are upgraded on load)
//!   `.khrproj/history.log`     — append-only `LogFrame { epoch, op }`
//!   `.khrproj/blobs/ab/cdef…`  — content-addressed blobs
//!   `.khrproj/.lock`           — fs4 exclusive lock (session lifetime)

use std::fs::File;
use std::io::Write;
use std::sync::Arc;

use anyhow::{Context, Result};
use atomicwrites::{AtomicFile, OverwriteBehavior};
use camino::{Utf8Path, Utf8PathBuf};
use chrono::Utc;
use fs4::FileExt;
use koharu_core::{Scene, op::Op};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use crate::blobs::BlobStore;
use crate::history::{self, History};

const SCENE_FILE: &str = "scene.bin";
const LOG_FILE: &str = "history.log";
const LOCK_FILE: &str = ".lock";
const BLOBS_DIR: &str = "blobs";
const CACHE_DIR: &str = "cache";
const PROJECT_TOML: &str = "project.toml";

/// `scene.bin` header: magic + format version. Postcard is positional (not
/// self-describing), so *any* change to a persisted struct silently breaks
/// old files — the version lets us decode them with the layout they were
/// written in and upgrade. Files without the magic predate versioning.
const SCENE_MAGIC: [u8; 4] = *b"KSCN";
/// v1 (implicit, headerless): layout before `TextData.rendered_font_size_px`.
/// v2: first to carry the header; layout before `TextStyle.gradient`.
/// v3: `TextStyle` gained `gradient`; colour still a bare `[u8; 4]` with
///     sentinel semantics (pure black / predicted colour = auto).
/// v4: current layout (`TextStyle.color` became `Option` — `None` = auto,
///     so pure black/white are finally expressible as manual picks).
const SCENE_FORMAT_VERSION: u16 = 4;

/// Snapshot written to `scene.bin`.
#[derive(Serialize, Deserialize)]
struct Snapshot {
    epoch: u64,
    scene: Scene,
}

/// A loaded project.
pub struct ProjectSession {
    pub dir: Utf8PathBuf,
    pub scene: RwLock<Scene>,
    pub history: Mutex<History>,
    pub blobs: Arc<BlobStore>,
    /// Held for the lifetime of the session.
    _lock: File,
}

impl ProjectSession {
    /// Open an existing `.khrproj/` directory.
    pub fn open(dir: impl AsRef<Utf8Path>) -> Result<Arc<Self>> {
        let dir = dir.as_ref().to_path_buf();
        if !dir.is_dir() {
            anyhow::bail!("not a project directory: {dir}");
        }
        Self::open_inner(dir, false)
    }

    /// Create a fresh `.khrproj/` at `dir`, failing if it already exists.
    pub fn create(dir: impl AsRef<Utf8Path>, name: impl Into<String>) -> Result<Arc<Self>> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(dir.as_std_path())
            .with_context(|| format!("create project dir {dir}"))?;
        // Project should be empty.
        let is_empty = std::fs::read_dir(dir.as_std_path())?.next().is_none();
        if !is_empty {
            anyhow::bail!("project directory not empty: {dir}");
        }
        // Seed the TOML with the name so open_inner can load it.
        let meta = ProjectTomlFile {
            name: name.into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        std::fs::write(
            dir.join(PROJECT_TOML).as_std_path(),
            toml::to_string_pretty(&meta)?,
        )?;
        Self::open_inner(dir, true)
    }

    fn open_inner(dir: Utf8PathBuf, creating: bool) -> Result<Arc<Self>> {
        std::fs::create_dir_all(dir.join(BLOBS_DIR).as_std_path())?;
        std::fs::create_dir_all(dir.join(CACHE_DIR).as_std_path())?;

        // Exclusive lock — one opener at a time.
        let lock_path = dir.join(LOCK_FILE);
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path.as_std_path())
            .with_context(|| format!("open lock file {}", lock_path))?;
        FileExt::try_lock(&lock).context("project is already open elsewhere")?;

        let blobs = Arc::new(BlobStore::open(dir.join(BLOBS_DIR).as_std_path())?);

        // Load or synthesize the scene + epoch.
        let (mut scene, mut epoch) = load_snapshot(&dir, creating)?;
        // Replay any log frames past the snapshot epoch.
        let log_path = dir.join(LOG_FILE);
        epoch = history::replay(log_path.as_std_path(), epoch, &mut scene)
            .with_context(|| format!("replay log {}", log_path))?;

        let history_obj = History::open(log_path.as_std_path(), epoch)?;

        Ok(Arc::new(Self {
            dir,
            scene: RwLock::new(scene),
            history: Mutex::new(history_obj),
            blobs,
            _lock: lock,
        }))
    }

    // --- scene mutation ----------------------------------------------------

    /// Apply an Op. Returns the epoch after apply.
    pub fn apply(&self, op: Op) -> Result<u64> {
        let mut history = self.history.lock();
        let mut scene = self.scene.write();
        history.apply(&mut scene, op)
    }

    pub fn undo(&self) -> Result<Option<(u64, Op)>> {
        let mut history = self.history.lock();
        let mut scene = self.scene.write();
        history.undo(&mut scene)
    }

    pub fn redo(&self) -> Result<Option<(u64, Op)>> {
        let mut history = self.history.lock();
        let mut scene = self.scene.write();
        history.redo(&mut scene)
    }

    pub fn epoch(&self) -> u64 {
        self.history.lock().epoch()
    }

    /// Cheap clone of the scene for read-only consumers (pipeline engines).
    pub fn scene_snapshot(&self) -> Scene {
        self.scene.read().clone()
    }

    // --- compaction --------------------------------------------------------

    /// Write a new snapshot (scene.bin) and truncate the log. Safe to call
    /// at any time; crash mid-compaction leaves the old snapshot + full log.
    pub fn compact(&self) -> Result<()> {
        let snap = {
            let scene = self.scene.read();
            let epoch = self.history.lock().epoch();
            Snapshot {
                epoch,
                scene: scene.clone(),
            }
        };
        let payload = postcard::to_allocvec(&snap).context("encode snapshot")?;
        let mut bytes = Vec::with_capacity(SCENE_MAGIC.len() + 2 + payload.len());
        bytes.extend_from_slice(&SCENE_MAGIC);
        bytes.extend_from_slice(&SCENE_FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&payload);
        AtomicFile::new(
            self.dir.join(SCENE_FILE).as_std_path(),
            OverwriteBehavior::AllowOverwrite,
        )
        .write(|f| f.write_all(&bytes))
        .context("write scene.bin atomically")?;
        // Log truncation only after snapshot is durably on disk.
        self.history.lock().truncate_log()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Snapshot loading / TOML metadata
// ---------------------------------------------------------------------------

fn load_snapshot(dir: &Utf8Path, creating: bool) -> Result<(Scene, u64)> {
    let scene_path = dir.join(SCENE_FILE);
    if scene_path.exists() {
        let bytes = std::fs::read(scene_path.as_std_path())
            .with_context(|| format!("read {}", scene_path))?;
        let snap = decode_snapshot(&bytes).with_context(|| format!("decode {}", scene_path))?;
        return Ok((snap.scene, snap.epoch));
    }

    // No snapshot — build one from `project.toml` (or defaults for creation).
    let toml_path = dir.join(PROJECT_TOML);
    let meta = if toml_path.exists() {
        let text = std::fs::read_to_string(toml_path.as_std_path())?;
        toml::from_str::<ProjectTomlFile>(&text).with_context(|| format!("parse {}", toml_path))?
    } else if creating {
        ProjectTomlFile {
            name: String::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    } else {
        anyhow::bail!("missing project.toml at {}", toml_path);
    };

    let mut scene = Scene::default();
    scene.project.name = meta.name;
    scene.project.created_at = meta.created_at;
    scene.project.updated_at = meta.updated_at;
    Ok((scene, 0))
}

/// Decode `scene.bin` in whichever format it was written.
///
/// - `"KSCN"` + version header → decode with that version's layout.
/// - Headerless (pre-versioning): try the current layout first (a few interim
///   builds wrote it headerless), then fall back to the v1 layout and upgrade.
///   `take_from_bytes` + full-consumption check keeps a wrong-layout decode
///   from "succeeding" on garbage.
fn decode_snapshot(bytes: &[u8]) -> Result<Snapshot> {
    if let Some(rest) = bytes.strip_prefix(&SCENE_MAGIC) {
        if rest.len() < 2 {
            anyhow::bail!("truncated scene.bin header");
        }
        let (ver, payload) = rest.split_at(2);
        let version = u16::from_le_bytes([ver[0], ver[1]]);
        return match version {
            SCENE_FORMAT_VERSION => postcard::from_bytes(payload).context("postcard decode (v4)"),
            3 => postcard::from_bytes::<compat::SnapshotV3>(payload)
                .context("postcard decode (v3)")
                .map(compat::SnapshotV3::upgrade),
            2 => postcard::from_bytes::<compat::SnapshotV2>(payload)
                .context("postcard decode (v2)")
                .map(compat::SnapshotV2::upgrade),
            _ => anyhow::bail!(
                "unsupported scene.bin format version {version} (written by a newer build?)"
            ),
        };
    }

    if let Ok((snap, rest)) = postcard::take_from_bytes::<Snapshot>(bytes)
        && rest.is_empty()
    {
        return Ok(snap);
    }

    let (legacy, rest) = postcard::take_from_bytes::<compat::SnapshotV1>(bytes)
        .context("postcard decode (current and v1 layouts both failed)")?;
    if !rest.is_empty() {
        anyhow::bail!("trailing bytes after v1 snapshot — file corrupt?");
    }
    Ok(legacy.upgrade())
}

/// Legacy (pre-versioning) on-disk layouts, decoded field-for-field as they
/// were written and upgraded to the current types. Only the structs that
/// changed shape need a frozen copy here; everything else is reused. Postcard
/// cares about field/variant *order* only, so keep it identical to the
/// original definitions.
mod compat {
    use indexmap::IndexMap;
    use koharu_core::{
        BlobRef, FontPrediction, ImageData, MaskData, Node, NodeId, NodeKind, Page, PageId,
        ProjectMeta, Scene, TextData, TextDirection, TextStyle, Transform,
    };
    use serde::Deserialize;

    use super::Snapshot;

    /// v1–v3 had no way to say "automatic colour": the UI stored pure black
    /// as a reset placeholder and old pipelines froze the model's predicted
    /// colour into the style, and the renderer sniffed both back out as
    /// "auto". v4 makes auto a real `None`, so convert those sentinels here —
    /// anything else was a genuine manual pick and stays verbatim.
    fn upgrade_sentinel_color(
        color: [u8; 4],
        prediction: Option<&FontPrediction>,
    ) -> Option<[u8; 4]> {
        if color[3] != 255 {
            return Some(color);
        }
        if color == [0, 0, 0, 255] {
            return None;
        }
        if let Some(pred) = prediction
            && pred.text_color == [color[0], color[1], color[2]]
        {
            return None;
        }
        Some(color)
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct SnapshotV1 {
        pub(super) epoch: u64,
        pub(super) scene: SceneV1,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct SceneV1 {
        pub(super) project: ProjectMeta,
        pub(super) pages: IndexMap<PageId, PageV1>,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct PageV1 {
        pub(super) id: PageId,
        pub(super) name: String,
        pub(super) width: u32,
        pub(super) height: u32,
        pub(super) nodes: IndexMap<NodeId, NodeV1>,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct NodeV1 {
        pub(super) id: NodeId,
        pub(super) transform: Transform,
        pub(super) visible: bool,
        pub(super) kind: NodeKindV1,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) enum NodeKindV1 {
        #[allow(dead_code)]
        Image(ImageData),
        Text(TextDataV1),
        #[allow(dead_code)]
        Mask(MaskData),
    }

    /// `TextData` before `rendered_font_size_px` was inserted.
    #[derive(Default, Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct TextDataV1 {
        pub(super) confidence: f32,
        pub(super) source_lang: Option<String>,
        pub(super) source_direction: Option<TextDirection>,
        pub(super) rendered_direction: Option<TextDirection>,
        pub(super) line_polygons: Option<Vec<[[f32; 2]; 4]>>,
        pub(super) rotation_deg: Option<f32>,
        pub(super) detected_font_size_px: Option<f32>,
        pub(super) detector: Option<String>,
        pub(super) text: Option<String>,
        pub(super) translation: Option<String>,
        // v1 files carried the pre-gradient TextStyle layout (same as v2's).
        pub(super) style: Option<TextStyleV2>,
        pub(super) font_prediction: Option<FontPrediction>,
        pub(super) sprite: Option<BlobRef>,
        pub(super) sprite_transform: Option<Transform>,
        pub(super) lock_layout_box: bool,
    }

    impl SnapshotV1 {
        pub(super) fn upgrade(self) -> Snapshot {
            Snapshot {
                epoch: self.epoch,
                scene: Scene {
                    project: self.scene.project,
                    pages: self
                        .scene
                        .pages
                        .into_iter()
                        .map(|(id, p)| (id, p.upgrade()))
                        .collect(),
                },
            }
        }
    }

    impl PageV1 {
        fn upgrade(self) -> Page {
            Page {
                id: self.id,
                name: self.name,
                width: self.width,
                height: self.height,
                nodes: self
                    .nodes
                    .into_iter()
                    .map(|(id, n)| {
                        (
                            id,
                            Node {
                                id: n.id,
                                transform: n.transform,
                                visible: n.visible,
                                kind: match n.kind {
                                    NodeKindV1::Image(d) => NodeKind::Image(d),
                                    NodeKindV1::Mask(d) => NodeKind::Mask(d),
                                    NodeKindV1::Text(d) => NodeKind::Text(d.upgrade()),
                                },
                            },
                        )
                    })
                    .collect(),
            }
        }
    }

    impl TextDataV1 {
        fn upgrade(self) -> TextData {
            let style = self.style.map(|s| s.upgrade(self.font_prediction.as_ref()));
            TextData {
                confidence: self.confidence,
                source_lang: self.source_lang,
                source_direction: self.source_direction,
                rendered_direction: self.rendered_direction,
                line_polygons: self.line_polygons,
                rotation_deg: self.rotation_deg,
                detected_font_size_px: self.detected_font_size_px,
                detector: self.detector,
                text: self.text,
                translation: self.translation,
                style,
                font_prediction: self.font_prediction,
                sprite: self.sprite,
                sprite_transform: self.sprite_transform,
                // The renderer refills this on the next render.
                rendered_font_size_px: None,
                lock_layout_box: self.lock_layout_box,
            }
        }
    }

    // -----------------------------------------------------------------------
    // v2 → current: `TextStyle` gained `gradient` (v3), colour became Option (v4).
    // -----------------------------------------------------------------------

    use koharu_core::{TextAlign, TextFillGradient, TextShaderEffect, TextStrokeStyle};

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct SnapshotV2 {
        pub(super) epoch: u64,
        pub(super) scene: SceneV2,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct SceneV2 {
        pub(super) project: ProjectMeta,
        pub(super) pages: IndexMap<PageId, PageV2>,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct PageV2 {
        pub(super) id: PageId,
        pub(super) name: String,
        pub(super) width: u32,
        pub(super) height: u32,
        pub(super) nodes: IndexMap<NodeId, NodeV2>,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct NodeV2 {
        pub(super) id: NodeId,
        pub(super) transform: Transform,
        pub(super) visible: bool,
        pub(super) kind: NodeKindV2,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) enum NodeKindV2 {
        #[allow(dead_code)]
        Image(ImageData),
        Text(TextDataV2),
        #[allow(dead_code)]
        Mask(MaskData),
    }

    /// `TextData` as of v2 — identical to current except `style`.
    #[derive(Default, Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct TextDataV2 {
        pub(super) confidence: f32,
        pub(super) source_lang: Option<String>,
        pub(super) source_direction: Option<TextDirection>,
        pub(super) rendered_direction: Option<TextDirection>,
        pub(super) line_polygons: Option<Vec<[[f32; 2]; 4]>>,
        pub(super) rotation_deg: Option<f32>,
        pub(super) detected_font_size_px: Option<f32>,
        pub(super) detector: Option<String>,
        pub(super) text: Option<String>,
        pub(super) translation: Option<String>,
        pub(super) style: Option<TextStyleV2>,
        pub(super) font_prediction: Option<FontPrediction>,
        pub(super) sprite: Option<BlobRef>,
        pub(super) sprite_transform: Option<Transform>,
        pub(super) rendered_font_size_px: Option<f32>,
        pub(super) lock_layout_box: bool,
    }

    /// `TextStyle` before `gradient` was appended (v1 and v2 files).
    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct TextStyleV2 {
        pub(super) font_families: Vec<String>,
        pub(super) font_size: Option<f32>,
        pub(super) color: [u8; 4],
        pub(super) effect: Option<TextShaderEffect>,
        pub(super) stroke: Option<TextStrokeStyle>,
        pub(super) text_align: Option<TextAlign>,
    }

    impl TextStyleV2 {
        fn upgrade(self, prediction: Option<&FontPrediction>) -> TextStyle {
            TextStyle {
                font_families: self.font_families,
                font_size: self.font_size,
                color: upgrade_sentinel_color(self.color, prediction),
                effect: self.effect,
                stroke: self.stroke,
                text_align: self.text_align,
                gradient: None,
            }
        }
    }

    impl SnapshotV2 {
        pub(super) fn upgrade(self) -> Snapshot {
            Snapshot {
                epoch: self.epoch,
                scene: Scene {
                    project: self.scene.project,
                    pages: self
                        .scene
                        .pages
                        .into_iter()
                        .map(|(id, p)| (id, p.upgrade()))
                        .collect(),
                },
            }
        }
    }

    impl PageV2 {
        fn upgrade(self) -> Page {
            Page {
                id: self.id,
                name: self.name,
                width: self.width,
                height: self.height,
                nodes: self
                    .nodes
                    .into_iter()
                    .map(|(id, n)| {
                        (
                            id,
                            Node {
                                id: n.id,
                                transform: n.transform,
                                visible: n.visible,
                                kind: match n.kind {
                                    NodeKindV2::Image(d) => NodeKind::Image(d),
                                    NodeKindV2::Mask(d) => NodeKind::Mask(d),
                                    NodeKindV2::Text(d) => NodeKind::Text(d.upgrade()),
                                },
                            },
                        )
                    })
                    .collect(),
            }
        }
    }

    impl TextDataV2 {
        fn upgrade(self) -> TextData {
            let style = self.style.map(|s| s.upgrade(self.font_prediction.as_ref()));
            TextData {
                confidence: self.confidence,
                source_lang: self.source_lang,
                source_direction: self.source_direction,
                rendered_direction: self.rendered_direction,
                line_polygons: self.line_polygons,
                rotation_deg: self.rotation_deg,
                detected_font_size_px: self.detected_font_size_px,
                detector: self.detector,
                text: self.text,
                translation: self.translation,
                style,
                font_prediction: self.font_prediction,
                sprite: self.sprite,
                sprite_transform: self.sprite_transform,
                rendered_font_size_px: self.rendered_font_size_px,
                lock_layout_box: self.lock_layout_box,
            }
        }
    }

    // -----------------------------------------------------------------------
    // v3 → v4: `TextStyle.color` became `Option` (sentinels → real auto).
    // -----------------------------------------------------------------------

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct SnapshotV3 {
        pub(super) epoch: u64,
        pub(super) scene: SceneV3,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct SceneV3 {
        pub(super) project: ProjectMeta,
        pub(super) pages: IndexMap<PageId, PageV3>,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct PageV3 {
        pub(super) id: PageId,
        pub(super) name: String,
        pub(super) width: u32,
        pub(super) height: u32,
        pub(super) nodes: IndexMap<NodeId, NodeV3>,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct NodeV3 {
        pub(super) id: NodeId,
        pub(super) transform: Transform,
        pub(super) visible: bool,
        pub(super) kind: NodeKindV3,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) enum NodeKindV3 {
        #[allow(dead_code)]
        Image(ImageData),
        Text(TextDataV3),
        #[allow(dead_code)]
        Mask(MaskData),
    }

    /// `TextData` as of v3 — identical to current except `style`.
    #[derive(Default, Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct TextDataV3 {
        pub(super) confidence: f32,
        pub(super) source_lang: Option<String>,
        pub(super) source_direction: Option<TextDirection>,
        pub(super) rendered_direction: Option<TextDirection>,
        pub(super) line_polygons: Option<Vec<[[f32; 2]; 4]>>,
        pub(super) rotation_deg: Option<f32>,
        pub(super) detected_font_size_px: Option<f32>,
        pub(super) detector: Option<String>,
        pub(super) text: Option<String>,
        pub(super) translation: Option<String>,
        pub(super) style: Option<TextStyleV3>,
        pub(super) font_prediction: Option<FontPrediction>,
        pub(super) sprite: Option<BlobRef>,
        pub(super) sprite_transform: Option<Transform>,
        pub(super) rendered_font_size_px: Option<f32>,
        pub(super) lock_layout_box: bool,
    }

    /// `TextStyle` as of v3 — `gradient` present, colour still a bare array
    /// with sentinel semantics.
    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct TextStyleV3 {
        pub(super) font_families: Vec<String>,
        pub(super) font_size: Option<f32>,
        pub(super) color: [u8; 4],
        pub(super) effect: Option<TextShaderEffect>,
        pub(super) stroke: Option<TextStrokeStyle>,
        pub(super) text_align: Option<TextAlign>,
        pub(super) gradient: Option<TextFillGradient>,
    }

    impl TextStyleV3 {
        fn upgrade(self, prediction: Option<&FontPrediction>) -> TextStyle {
            TextStyle {
                font_families: self.font_families,
                font_size: self.font_size,
                color: upgrade_sentinel_color(self.color, prediction),
                effect: self.effect,
                stroke: self.stroke,
                text_align: self.text_align,
                gradient: self.gradient,
            }
        }
    }

    impl SnapshotV3 {
        pub(super) fn upgrade(self) -> Snapshot {
            Snapshot {
                epoch: self.epoch,
                scene: Scene {
                    project: self.scene.project,
                    pages: self
                        .scene
                        .pages
                        .into_iter()
                        .map(|(id, p)| (id, p.upgrade()))
                        .collect(),
                },
            }
        }
    }

    impl PageV3 {
        fn upgrade(self) -> Page {
            Page {
                id: self.id,
                name: self.name,
                width: self.width,
                height: self.height,
                nodes: self
                    .nodes
                    .into_iter()
                    .map(|(id, n)| {
                        (
                            id,
                            Node {
                                id: n.id,
                                transform: n.transform,
                                visible: n.visible,
                                kind: match n.kind {
                                    NodeKindV3::Image(d) => NodeKind::Image(d),
                                    NodeKindV3::Mask(d) => NodeKind::Mask(d),
                                    NodeKindV3::Text(d) => NodeKind::Text(d.upgrade()),
                                },
                            },
                        )
                    })
                    .collect(),
            }
        }
    }

    impl TextDataV3 {
        fn upgrade(self) -> TextData {
            let style = self.style.map(|s| s.upgrade(self.font_prediction.as_ref()));
            TextData {
                confidence: self.confidence,
                source_lang: self.source_lang,
                source_direction: self.source_direction,
                rendered_direction: self.rendered_direction,
                line_polygons: self.line_polygons,
                rotation_deg: self.rotation_deg,
                detected_font_size_px: self.detected_font_size_px,
                detector: self.detector,
                text: self.text,
                translation: self.translation,
                style,
                font_prediction: self.font_prediction,
                sprite: self.sprite,
                sprite_transform: self.sprite_transform,
                rendered_font_size_px: self.rendered_font_size_px,
                lock_layout_box: self.lock_layout_box,
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ProjectTomlFile {
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use koharu_core::{
        Node, NodeId, NodeKind, Op, Page, PageId, TextData, TextShaderEffect, TextStyle, Transform,
    };
    use tempfile::tempdir;

    fn tmp_dir() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        (dir, path.join("proj.khrproj"))
    }

    #[test]
    fn create_apply_close_reopen_preserves_scene() {
        let (_tmp, path) = tmp_dir();
        let page_id: PageId;
        {
            let session = ProjectSession::create(&path, "test").unwrap();
            let page = Page::new("p1", 800, 600);
            page_id = page.id;
            session
                .apply(Op::AddPage { page, at: 0 })
                .expect("apply AddPage");
            session.compact().unwrap();
            // Session drops, lock released.
        }
        let session = ProjectSession::open(&path).unwrap();
        assert_eq!(session.scene.read().pages.len(), 1);
        assert!(session.scene.read().pages.contains_key(&page_id));
    }

    #[test]
    fn compact_writes_versioned_scene_bin() {
        let (_tmp, path) = tmp_dir();
        {
            let session = ProjectSession::create(&path, "versioned").unwrap();
            session
                .apply(Op::AddPage {
                    page: Page::new("p1", 800, 600),
                    at: 0,
                })
                .unwrap();
            session.compact().unwrap();
        }
        let bytes = std::fs::read(path.join(SCENE_FILE).as_std_path()).unwrap();
        assert_eq!(&bytes[..4], &SCENE_MAGIC);
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            SCENE_FORMAT_VERSION
        );
    }

    #[test]
    fn headerless_v1_scene_bin_upgrades_on_open() {
        // Regression: projects written before `TextData.rendered_font_size_px`
        // existed (headerless postcard, v1 layout) must still open — postcard
        // is positional, so the new field shifted every byte after it and old
        // files failed with "found a bool that wasn't 0 or 1".
        let (_tmp, path) = tmp_dir();
        {
            let session = ProjectSession::create(&path, "legacy").unwrap();
            drop(session); // only project.toml written; we supply scene.bin below
        }

        let page_id = PageId::new();
        let node_id = NodeId::new();
        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            node_id,
            compat::NodeV1 {
                id: node_id,
                transform: Transform {
                    x: 1.0,
                    y: 2.0,
                    width: 100.0,
                    height: 40.0,
                    rotation_deg: 0.0,
                },
                visible: true,
                kind: compat::NodeKindV1::Text(compat::TextDataV1 {
                    text: Some("こんにちは".to_string()),
                    translation: Some("Hello".to_string()),
                    lock_layout_box: true,
                    ..Default::default()
                }),
            },
        );
        let mut pages = indexmap::IndexMap::new();
        pages.insert(
            page_id,
            compat::PageV1 {
                id: page_id,
                name: "p1".to_string(),
                width: 800,
                height: 600,
                nodes,
            },
        );
        let legacy = compat::SnapshotV1 {
            epoch: 7,
            scene: compat::SceneV1 {
                project: koharu_core::ProjectMeta::default(),
                pages,
            },
        };
        let bytes = postcard::to_allocvec(&legacy).unwrap();
        std::fs::write(path.join(SCENE_FILE).as_std_path(), bytes).unwrap();

        let session = ProjectSession::open(&path).expect("legacy scene.bin must open");
        let scene = session.scene.read();
        let page = scene.pages.get(&page_id).expect("page survives upgrade");
        let node = page.nodes.get(&node_id).expect("node survives upgrade");
        let NodeKind::Text(text) = &node.kind else {
            panic!("expected text node");
        };
        assert_eq!(text.translation.as_deref(), Some("Hello"));
        assert!(text.lock_layout_box, "trailing bool must decode intact");
        assert!(
            text.rendered_font_size_px.is_none(),
            "new field defaults to None for upgraded scenes"
        );
    }

    #[test]
    fn v2_scene_bin_upgrades_on_open() {
        // Regression: projects written before `TextStyle.gradient` existed
        // (v2 header) must still open with styles intact.
        let (_tmp, path) = tmp_dir();
        {
            let session = ProjectSession::create(&path, "v2").unwrap();
            drop(session);
        }

        let page_id = PageId::new();
        let node_id = NodeId::new();
        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            node_id,
            compat::NodeV2 {
                id: node_id,
                transform: Transform {
                    x: 1.0,
                    y: 2.0,
                    width: 100.0,
                    height: 40.0,
                    rotation_deg: 12.5,
                },
                visible: true,
                kind: compat::NodeKindV2::Text(compat::TextDataV2 {
                    text: Some("안녕".to_string()),
                    translation: Some("Hi".to_string()),
                    style: Some(compat::TextStyleV2 {
                        font_families: vec!["Arial".to_string()],
                        font_size: Some(21.0),
                        color: [10, 20, 30, 255],
                        effect: None,
                        stroke: None,
                        text_align: None,
                    }),
                    rendered_font_size_px: Some(19.0),
                    lock_layout_box: true,
                    ..Default::default()
                }),
            },
        );
        let mut pages = indexmap::IndexMap::new();
        pages.insert(
            page_id,
            compat::PageV2 {
                id: page_id,
                name: "p1".to_string(),
                width: 800,
                height: 600,
                nodes,
            },
        );
        let v2 = compat::SnapshotV2 {
            epoch: 9,
            scene: compat::SceneV2 {
                project: koharu_core::ProjectMeta::default(),
                pages,
            },
        };
        let mut bytes = SCENE_MAGIC.to_vec();
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&postcard::to_allocvec(&v2).unwrap());
        std::fs::write(path.join(SCENE_FILE).as_std_path(), bytes).unwrap();

        let session = ProjectSession::open(&path).expect("v2 scene.bin must open");
        let scene = session.scene.read();
        let node = scene
            .pages
            .get(&page_id)
            .and_then(|p| p.nodes.get(&node_id))
            .expect("node survives upgrade");
        let NodeKind::Text(text) = &node.kind else {
            panic!("expected text node");
        };
        let style = text.style.as_ref().expect("style survives upgrade");
        assert_eq!(style.font_size, Some(21.0));
        assert_eq!(
            style.color,
            Some([10, 20, 30, 255]),
            "manual colour survives as an explicit pick"
        );
        assert!(style.gradient.is_none(), "new field defaults to None");
        assert_eq!(text.rendered_font_size_px, Some(19.0));
        assert!(text.lock_layout_box, "trailing bool must decode intact");
    }

    #[test]
    fn v3_scene_bin_upgrades_sentinel_colors_to_auto() {
        // v3 stored "auto" colour as sentinels: pure black, or the model's
        // predicted colour frozen into the style. The v4 upgrade must turn
        // both into `None` and keep genuine manual picks verbatim.
        let (_tmp, path) = tmp_dir();
        {
            let session = ProjectSession::create(&path, "v3").unwrap();
            drop(session);
        }

        let style_v3 = |color: [u8; 4]| compat::TextStyleV3 {
            font_families: vec!["Arial".to_string()],
            font_size: Some(21.0),
            color,
            effect: None,
            stroke: None,
            text_align: None,
            gradient: None,
        };
        let text_node = |style: compat::TextStyleV3,
                         prediction: Option<koharu_core::FontPrediction>| {
            let id = NodeId::new();
            (
                id,
                compat::NodeV3 {
                    id,
                    transform: Transform {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 40.0,
                        rotation_deg: 0.0,
                    },
                    visible: true,
                    kind: compat::NodeKindV3::Text(compat::TextDataV3 {
                        text: Some("안녕".to_string()),
                        style: Some(style),
                        font_prediction: prediction,
                        ..Default::default()
                    }),
                },
            )
        };

        let predicted = koharu_core::FontPrediction {
            text_color: [200, 40, 90],
            ..Default::default()
        };
        let (black_id, black_node) = text_node(style_v3([0, 0, 0, 255]), None);
        let (stale_id, stale_node) =
            text_node(style_v3([200, 40, 90, 255]), Some(predicted.clone()));
        let (manual_id, manual_node) = text_node(style_v3([255, 255, 255, 255]), Some(predicted));

        let page_id = PageId::new();
        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(black_id, black_node);
        nodes.insert(stale_id, stale_node);
        nodes.insert(manual_id, manual_node);
        let mut pages = indexmap::IndexMap::new();
        pages.insert(
            page_id,
            compat::PageV3 {
                id: page_id,
                name: "p1".to_string(),
                width: 800,
                height: 600,
                nodes,
            },
        );
        let v3 = compat::SnapshotV3 {
            epoch: 4,
            scene: compat::SceneV3 {
                project: koharu_core::ProjectMeta::default(),
                pages,
            },
        };
        let mut bytes = SCENE_MAGIC.to_vec();
        bytes.extend_from_slice(&3u16.to_le_bytes());
        bytes.extend_from_slice(&postcard::to_allocvec(&v3).unwrap());
        std::fs::write(path.join(SCENE_FILE).as_std_path(), bytes).unwrap();

        let session = ProjectSession::open(&path).expect("v3 scene.bin must open");
        let scene = session.scene.read();
        let color_of = |id: &NodeId| {
            let node = scene
                .pages
                .get(&page_id)
                .and_then(|p| p.nodes.get(id))
                .expect("node survives upgrade");
            let NodeKind::Text(text) = &node.kind else {
                panic!("expected text node");
            };
            text.style.as_ref().expect("style survives").color
        };
        assert_eq!(color_of(&black_id), None, "black sentinel becomes auto");
        assert_eq!(
            color_of(&stale_id),
            None,
            "prediction-equal colour becomes auto"
        );
        assert_eq!(
            color_of(&manual_id),
            Some([255, 255, 255, 255]),
            "genuine manual pick (even pure white) stays"
        );
    }

    #[test]
    fn future_scene_bin_version_is_rejected_cleanly() {
        let (_tmp, path) = tmp_dir();
        {
            let session = ProjectSession::create(&path, "future").unwrap();
            drop(session);
        }
        let mut bytes = SCENE_MAGIC.to_vec();
        bytes.extend_from_slice(&99u16.to_le_bytes());
        bytes.extend_from_slice(&[1, 2, 3]);
        std::fs::write(path.join(SCENE_FILE).as_std_path(), bytes).unwrap();

        let err = match ProjectSession::open(&path) {
            Ok(_) => panic!("unknown version must not decode"),
            Err(err) => err,
        };
        assert!(format!("{err:#}").contains("unsupported scene.bin format version"));
    }

    #[test]
    fn reopen_preserves_text_style_effects_in_scene_bin() {
        let (_tmp, path) = tmp_dir();
        let page_id: PageId;
        let node_id: NodeId;
        {
            let session = ProjectSession::create(&path, "styled").unwrap();
            let page = Page::new("p1", 800, 600);
            page_id = page.id;
            session
                .apply(Op::AddPage { page, at: 0 })
                .expect("apply AddPage");

            node_id = NodeId::new();
            let mut scene = session.scene.write();
            let page = scene.pages.get_mut(&page_id).expect("page");
            page.nodes.insert(
                node_id,
                Node {
                    id: node_id,
                    transform: Transform {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 40.0,
                        rotation_deg: 0.0,
                    },
                    visible: true,
                    kind: NodeKind::Text(TextData {
                        style: Some(TextStyle {
                            font_families: vec!["Arial".to_string()],
                            font_size: Some(20.0),
                            // v4: explicit pure black round-trips as manual.
                            color: Some([0, 0, 0, 255]),
                            effect: Some(TextShaderEffect {
                                italic: true,
                                bold: true,
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                },
            );
            drop(scene);
            session.compact().unwrap();
        }

        let session = ProjectSession::open(&path).unwrap();
        let scene = session.scene.read();
        let page = scene.pages.get(&page_id).expect("page");
        let node = page.nodes.get(&node_id).expect("node");
        let NodeKind::Text(text) = &node.kind else {
            panic!("expected text node");
        };
        let effect = text
            .style
            .as_ref()
            .and_then(|style| style.effect)
            .expect("effect");
        assert!(effect.italic);
        assert!(effect.bold);
    }

    #[test]
    fn exclusive_lock_prevents_second_open() {
        let (_tmp, path) = tmp_dir();
        let a = ProjectSession::create(&path, "test").unwrap();
        let err = ProjectSession::open(&path)
            .err()
            .expect("second open must fail");
        assert!(err.to_string().contains("already open"));
        drop(a);
    }
}
