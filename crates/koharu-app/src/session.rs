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
use serde::{Deserialize, Serialize, de::DeserializeOwned};

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
/// v4: `TextStyle.color` became `Option` — `None` = auto, so pure
///     black/white are finally expressible as manual picks.
/// v5: `TextStrokeStyle.color` became `Option` too — `None` = contrast
///     against the text colour instead of a hard-coded white.
/// v6: `TextData` gained `rendered_text_color` — the colour the renderer
///     actually painted, so the UI swatch stops guessing.
/// v7: `TextData` gained character-level `style_ranges`.
/// v8: current layout (`TextData` gained the explicit `writing_direction`
///     override).
const SCENE_FORMAT_VERSION: u16 = 8;

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

    /// Apply an Op. Returns the epoch after apply. On error the scene is
    /// unchanged because history applies to a clone and swaps on commit.
    ///
    /// Every deletion entry point (panel, canvas, RPC, MCP, pipeline batches)
    /// funnels through here, so this is where a text node's erase-mask
    /// contribution, its inpainted pixels and the stale rendered composite
    /// are retired with it — see [`crate::text_erase`]. The extra ops join
    /// the caller's op in one batch, so the whole thing is a single atomic,
    /// undoable history entry. If those layers can't be read or written the
    /// deletion fails as a whole; a malformed op is passed through unchanged
    /// and fails in history. Either way the scene, log and undo stacks are
    /// untouched.
    pub fn apply(&self, op: Op) -> Result<u64> {
        let mut history = self.history.lock();
        let mut scene = self.scene.write();
        let op = crate::text_erase::sync_deleted_text_erase(&scene, &self.blobs, op)?;
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
    /// The history guard spans write + truncate, serializing concurrent edits.
    pub fn compact(&self) -> Result<()> {
        let mut history = self.history.lock();
        let snap = {
            let scene = self.scene.read();
            Snapshot {
                epoch: history.epoch(),
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
        history.truncate_log()?;
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
            SCENE_FORMAT_VERSION => decode_postcard_exact(payload, "v8"),
            // The first vertical-writing build accidentally reused v7 for
            // the v8 layout. Exact consumption distinguishes the real v7
            // shape from snapshots written during that collision window.
            7 => match decode_postcard_exact::<compat::SnapshotV7>(payload, "v7") {
                Ok(snap) => Ok(snap.upgrade()),
                Err(v7_err) => decode_postcard_exact::<Snapshot>(payload, "v7 collision layout")
                    .with_context(|| format!("canonical v7 also failed: {v7_err:#}")),
            },
            6 => decode_postcard_exact::<compat::SnapshotV6>(payload, "v6")
                .map(compat::SnapshotV6::upgrade),
            5 => decode_postcard_exact::<compat::SnapshotV5>(payload, "v5")
                .map(compat::SnapshotV5::upgrade),
            4 => decode_postcard_exact::<compat::SnapshotV4>(payload, "v4")
                .map(compat::SnapshotV4::upgrade),
            3 => decode_postcard_exact::<compat::SnapshotV3>(payload, "v3")
                .map(compat::SnapshotV3::upgrade),
            2 => decode_postcard_exact::<compat::SnapshotV2>(payload, "v2")
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

    // A few interim builds wrote then-current layouts without a header. Keep
    // both recoverable, requiring full consumption so a wrong shape cannot
    // silently decode a prefix.
    if let Ok((snap, rest)) = postcard::take_from_bytes::<compat::SnapshotV7>(bytes)
        && rest.is_empty()
    {
        return Ok(snap.upgrade());
    }

    if let Ok((snap, rest)) = postcard::take_from_bytes::<compat::SnapshotV6>(bytes)
        && rest.is_empty()
    {
        return Ok(snap.upgrade());
    }

    let (legacy, rest) = postcard::take_from_bytes::<compat::SnapshotV1>(bytes)
        .context("postcard decode (current, v7, v6, and v1 layouts all failed)")?;
    if !rest.is_empty() {
        anyhow::bail!("trailing bytes after v1 snapshot — file corrupt?");
    }
    Ok(legacy.upgrade())
}

fn decode_postcard_exact<T: DeserializeOwned>(bytes: &[u8], version: &str) -> Result<T> {
    let (value, rest) =
        postcard::take_from_bytes(bytes).with_context(|| format!("postcard decode ({version})"))?;
    anyhow::ensure!(
        rest.is_empty(),
        "postcard decode ({version}) left {} trailing bytes",
        rest.len()
    );
    Ok(value)
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
        ProjectMeta, Scene, TextData, TextDirection, TextStyle, TextStyleRange, Transform,
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

    /// `TextStrokeStyle` before v5 — colour was required and the UI
    /// materialised a hard-coded white whenever any border control was
    /// touched. Shared by every pre-v5 style layout.
    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct TextStrokeStyleV4 {
        pub(super) enabled: bool,
        pub(super) color: [u8; 4],
        pub(super) width_px: Option<f32>,
    }

    /// Pre-v5 stroke colours: pure white was overwhelmingly the materialised
    /// default (and pure black its mirror), not a deliberate pick — and in
    /// the cases where it *was* deliberate (white outline on black text),
    /// auto contrast reproduces the same colour anyway. Convert both to
    /// auto; anything else stays a manual pick.
    fn upgrade_sentinel_stroke(stroke: TextStrokeStyleV4) -> TextStrokeStyle {
        TextStrokeStyle {
            enabled: stroke.enabled,
            color: match stroke.color {
                [255, 255, 255, 255] | [0, 0, 0, 255] => None,
                other => Some(other),
            },
            width_px: stroke.width_px,
        }
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
                // The renderer refills these on the next render.
                rendered_font_size_px: None,
                rendered_text_color: None,
                lock_layout_box: self.lock_layout_box,
                style_ranges: Vec::new(),
                writing_direction: None,
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
        pub(super) stroke: Option<TextStrokeStyleV4>,
        pub(super) text_align: Option<TextAlign>,
    }

    impl TextStyleV2 {
        fn upgrade(self, prediction: Option<&FontPrediction>) -> TextStyle {
            TextStyle {
                font_families: self.font_families,
                font_size: self.font_size,
                color: upgrade_sentinel_color(self.color, prediction),
                effect: self.effect,
                stroke: self.stroke.map(upgrade_sentinel_stroke),
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
                rendered_text_color: None,
                lock_layout_box: self.lock_layout_box,
                style_ranges: Vec::new(),
                writing_direction: None,
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
        pub(super) stroke: Option<TextStrokeStyleV4>,
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
                stroke: self.stroke.map(upgrade_sentinel_stroke),
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
                rendered_text_color: None,
                lock_layout_box: self.lock_layout_box,
                style_ranges: Vec::new(),
                writing_direction: None,
            }
        }
    }

    // -----------------------------------------------------------------------
    // v4 → v5: `TextStrokeStyle.color` became `Option` (white sentinel → auto).
    // -----------------------------------------------------------------------

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct SnapshotV4 {
        pub(super) epoch: u64,
        pub(super) scene: SceneV4,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct SceneV4 {
        pub(super) project: ProjectMeta,
        pub(super) pages: IndexMap<PageId, PageV4>,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct PageV4 {
        pub(super) id: PageId,
        pub(super) name: String,
        pub(super) width: u32,
        pub(super) height: u32,
        pub(super) nodes: IndexMap<NodeId, NodeV4>,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct NodeV4 {
        pub(super) id: NodeId,
        pub(super) transform: Transform,
        pub(super) visible: bool,
        pub(super) kind: NodeKindV4,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) enum NodeKindV4 {
        #[allow(dead_code)]
        Image(ImageData),
        Text(TextDataV4),
        #[allow(dead_code)]
        Mask(MaskData),
    }

    /// `TextData` as of v4 — identical to current except `style`.
    #[derive(Default, Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct TextDataV4 {
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
        pub(super) style: Option<TextStyleV4>,
        pub(super) font_prediction: Option<FontPrediction>,
        pub(super) sprite: Option<BlobRef>,
        pub(super) sprite_transform: Option<Transform>,
        pub(super) rendered_font_size_px: Option<f32>,
        pub(super) lock_layout_box: bool,
    }

    /// `TextStyle` as of v4 — colour already `Option`, stroke colour still a
    /// bare array defaulting to white.
    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct TextStyleV4 {
        pub(super) font_families: Vec<String>,
        pub(super) font_size: Option<f32>,
        pub(super) color: Option<[u8; 4]>,
        pub(super) effect: Option<TextShaderEffect>,
        pub(super) stroke: Option<TextStrokeStyleV4>,
        pub(super) text_align: Option<TextAlign>,
        pub(super) gradient: Option<TextFillGradient>,
    }

    impl TextStyleV4 {
        fn upgrade(self) -> TextStyle {
            TextStyle {
                font_families: self.font_families,
                font_size: self.font_size,
                // v4 text-colour semantics are already the current ones.
                color: self.color,
                effect: self.effect,
                stroke: self.stroke.map(upgrade_sentinel_stroke),
                text_align: self.text_align,
                gradient: self.gradient,
            }
        }
    }

    impl SnapshotV4 {
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

    impl PageV4 {
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
                                    NodeKindV4::Image(d) => NodeKind::Image(d),
                                    NodeKindV4::Mask(d) => NodeKind::Mask(d),
                                    NodeKindV4::Text(d) => NodeKind::Text(d.upgrade()),
                                },
                            },
                        )
                    })
                    .collect(),
            }
        }
    }

    impl TextDataV4 {
        fn upgrade(self) -> TextData {
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
                style: self.style.map(TextStyleV4::upgrade),
                font_prediction: self.font_prediction,
                sprite: self.sprite,
                sprite_transform: self.sprite_transform,
                rendered_font_size_px: self.rendered_font_size_px,
                rendered_text_color: None,
                lock_layout_box: self.lock_layout_box,
                style_ranges: Vec::new(),
                writing_direction: None,
            }
        }
    }

    // -----------------------------------------------------------------------
    // v5 → v6: `TextData` gained `rendered_text_color`.
    // -----------------------------------------------------------------------

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct SnapshotV5 {
        pub(super) epoch: u64,
        pub(super) scene: SceneV5,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct SceneV5 {
        pub(super) project: ProjectMeta,
        pub(super) pages: IndexMap<PageId, PageV5>,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct PageV5 {
        pub(super) id: PageId,
        pub(super) name: String,
        pub(super) width: u32,
        pub(super) height: u32,
        pub(super) nodes: IndexMap<NodeId, NodeV5>,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct NodeV5 {
        pub(super) id: NodeId,
        pub(super) transform: Transform,
        pub(super) visible: bool,
        pub(super) kind: NodeKindV5,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) enum NodeKindV5 {
        #[allow(dead_code)]
        Image(ImageData),
        Text(TextDataV5),
        #[allow(dead_code)]
        Mask(MaskData),
    }

    /// `TextData` as of v5 — the current layout minus `rendered_text_color`.
    /// The style layout is already the current one (unchanged since v5).
    #[derive(Default, Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct TextDataV5 {
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
        pub(super) style: Option<TextStyle>,
        pub(super) font_prediction: Option<FontPrediction>,
        pub(super) sprite: Option<BlobRef>,
        pub(super) sprite_transform: Option<Transform>,
        pub(super) rendered_font_size_px: Option<f32>,
        pub(super) lock_layout_box: bool,
    }

    impl SnapshotV5 {
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

    impl PageV5 {
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
                                    NodeKindV5::Image(d) => NodeKind::Image(d),
                                    NodeKindV5::Mask(d) => NodeKind::Mask(d),
                                    NodeKindV5::Text(d) => NodeKind::Text(d.upgrade()),
                                },
                            },
                        )
                    })
                    .collect(),
            }
        }
    }

    impl TextDataV5 {
        fn upgrade(self) -> TextData {
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
                style: self.style,
                font_prediction: self.font_prediction,
                sprite: self.sprite,
                sprite_transform: self.sprite_transform,
                rendered_font_size_px: self.rendered_font_size_px,
                // The renderer refills this on the next render.
                rendered_text_color: None,
                lock_layout_box: self.lock_layout_box,
                style_ranges: Vec::new(),
                writing_direction: None,
            }
        }
    }

    // -----------------------------------------------------------------------
    // v6 → v7: `TextData` gained character-level `style_ranges`.
    // -----------------------------------------------------------------------

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct SnapshotV6 {
        pub(super) epoch: u64,
        pub(super) scene: SceneV6,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct SceneV6 {
        pub(super) project: ProjectMeta,
        pub(super) pages: IndexMap<PageId, PageV6>,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct PageV6 {
        pub(super) id: PageId,
        pub(super) name: String,
        pub(super) width: u32,
        pub(super) height: u32,
        pub(super) nodes: IndexMap<NodeId, NodeV6>,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct NodeV6 {
        pub(super) id: NodeId,
        pub(super) transform: Transform,
        pub(super) visible: bool,
        pub(super) kind: NodeKindV6,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) enum NodeKindV6 {
        #[allow(dead_code)]
        Image(ImageData),
        Text(TextDataV6),
        #[allow(dead_code)]
        Mask(MaskData),
    }

    /// Exact v6 layout: current `TextData` minus the appended style ranges and
    /// writing-direction override.
    #[derive(Default, Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct TextDataV6 {
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
        pub(super) style: Option<TextStyle>,
        pub(super) font_prediction: Option<FontPrediction>,
        pub(super) sprite: Option<BlobRef>,
        pub(super) sprite_transform: Option<Transform>,
        pub(super) rendered_font_size_px: Option<f32>,
        pub(super) rendered_text_color: Option<[u8; 4]>,
        pub(super) lock_layout_box: bool,
    }

    impl SnapshotV6 {
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

    impl PageV6 {
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
                                    NodeKindV6::Image(d) => NodeKind::Image(d),
                                    NodeKindV6::Mask(d) => NodeKind::Mask(d),
                                    NodeKindV6::Text(d) => NodeKind::Text(d.upgrade()),
                                },
                            },
                        )
                    })
                    .collect(),
            }
        }
    }

    impl TextDataV6 {
        fn upgrade(self) -> TextData {
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
                style: self.style,
                font_prediction: self.font_prediction,
                sprite: self.sprite,
                sprite_transform: self.sprite_transform,
                rendered_font_size_px: self.rendered_font_size_px,
                rendered_text_color: self.rendered_text_color,
                lock_layout_box: self.lock_layout_box,
                style_ranges: Vec::new(),
                writing_direction: None,
            }
        }
    }

    // -----------------------------------------------------------------------
    // v7 → v8: `TextData` gained the explicit `writing_direction` override.
    // -----------------------------------------------------------------------

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct SnapshotV7 {
        pub(super) epoch: u64,
        pub(super) scene: SceneV7,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct SceneV7 {
        pub(super) project: ProjectMeta,
        pub(super) pages: IndexMap<PageId, PageV7>,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct PageV7 {
        pub(super) id: PageId,
        pub(super) name: String,
        pub(super) width: u32,
        pub(super) height: u32,
        pub(super) nodes: IndexMap<NodeId, NodeV7>,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct NodeV7 {
        pub(super) id: NodeId,
        pub(super) transform: Transform,
        pub(super) visible: bool,
        pub(super) kind: NodeKindV7,
    }

    #[derive(Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) enum NodeKindV7 {
        #[allow(dead_code)]
        Image(ImageData),
        Text(TextDataV7),
        #[allow(dead_code)]
        Mask(MaskData),
    }

    /// Exact v7 layout: current `TextData` minus the appended writing-axis
    /// override. This format was shipped by the rich-text build.
    #[derive(Default, Deserialize)]
    #[cfg_attr(test, derive(serde::Serialize))]
    pub(super) struct TextDataV7 {
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
        pub(super) style: Option<TextStyle>,
        pub(super) font_prediction: Option<FontPrediction>,
        pub(super) sprite: Option<BlobRef>,
        pub(super) sprite_transform: Option<Transform>,
        pub(super) rendered_font_size_px: Option<f32>,
        pub(super) rendered_text_color: Option<[u8; 4]>,
        pub(super) lock_layout_box: bool,
        pub(super) style_ranges: Vec<TextStyleRange>,
    }

    impl SnapshotV7 {
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

    impl PageV7 {
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
                                    NodeKindV7::Image(d) => NodeKind::Image(d),
                                    NodeKindV7::Mask(d) => NodeKind::Mask(d),
                                    NodeKindV7::Text(d) => NodeKind::Text(d.upgrade()),
                                },
                            },
                        )
                    })
                    .collect(),
            }
        }
    }

    impl TextDataV7 {
        fn upgrade(self) -> TextData {
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
                style: self.style,
                font_prediction: self.font_prediction,
                sprite: self.sprite,
                sprite_transform: self.sprite_transform,
                rendered_font_size_px: self.rendered_font_size_px,
                rendered_text_color: self.rendered_text_color,
                lock_layout_box: self.lock_layout_box,
                style_ranges: self.style_ranges,
                writing_direction: None,
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
        BlobRef, ImageData, ImageRole, MaskData, MaskRole, Node, NodeId, NodeKind, Op, Page,
        PageId, TextData, TextDirection, TextRangeStyle, TextShaderEffect, TextStyle,
        TextStyleRange, Transform,
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
    fn v4_scene_bin_upgrades_sentinel_stroke_colors_to_auto() {
        // v4 stroke colours were required and the UI materialised pure white
        // as the default; the v5 upgrade converts pure white/black to auto
        // (contrast) and keeps everything else as a manual pick.
        let (_tmp, path) = tmp_dir();
        {
            let session = ProjectSession::create(&path, "v4").unwrap();
            drop(session);
        }

        let node_with_stroke = |color: [u8; 4]| {
            let id = NodeId::new();
            (
                id,
                compat::NodeV4 {
                    id,
                    transform: Transform {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 40.0,
                        rotation_deg: 0.0,
                    },
                    visible: true,
                    kind: compat::NodeKindV4::Text(compat::TextDataV4 {
                        text: Some("안녕".to_string()),
                        style: Some(compat::TextStyleV4 {
                            font_families: vec!["Arial".to_string()],
                            font_size: None,
                            color: Some([255, 255, 255, 255]),
                            effect: None,
                            stroke: Some(compat::TextStrokeStyleV4 {
                                enabled: true,
                                color,
                                width_px: Some(3.0),
                            }),
                            text_align: None,
                            gradient: None,
                        }),
                        ..Default::default()
                    }),
                },
            )
        };

        let (white_id, white_node) = node_with_stroke([255, 255, 255, 255]);
        let (custom_id, custom_node) = node_with_stroke([255, 249, 249, 255]);

        let page_id = PageId::new();
        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(white_id, white_node);
        nodes.insert(custom_id, custom_node);
        let mut pages = indexmap::IndexMap::new();
        pages.insert(
            page_id,
            compat::PageV4 {
                id: page_id,
                name: "p1".to_string(),
                width: 800,
                height: 600,
                nodes,
            },
        );
        let v4 = compat::SnapshotV4 {
            epoch: 5,
            scene: compat::SceneV4 {
                project: koharu_core::ProjectMeta::default(),
                pages,
            },
        };
        let mut bytes = SCENE_MAGIC.to_vec();
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&postcard::to_allocvec(&v4).unwrap());
        std::fs::write(path.join(SCENE_FILE).as_std_path(), bytes).unwrap();

        let session = ProjectSession::open(&path).expect("v4 scene.bin must open");
        let scene = session.scene.read();
        let stroke_of = |id: &NodeId| {
            let node = scene
                .pages
                .get(&page_id)
                .and_then(|p| p.nodes.get(id))
                .expect("node survives upgrade");
            let NodeKind::Text(text) = &node.kind else {
                panic!("expected text node");
            };
            text.style
                .as_ref()
                .and_then(|s| s.stroke.clone())
                .expect("stroke survives upgrade")
        };
        let white = stroke_of(&white_id);
        assert!(white.enabled);
        assert_eq!(white.color, None, "white sentinel becomes auto contrast");
        assert_eq!(white.width_px, Some(3.0), "width survives");
        assert_eq!(
            stroke_of(&custom_id).color,
            Some([255, 249, 249, 255]),
            "eyedropped near-white stays manual"
        );
        // Text colour semantics were already v4-correct — manual white stays.
        let NodeKind::Text(text) = &scene.pages[&page_id].nodes[&white_id].kind else {
            panic!("expected text node");
        };
        assert_eq!(
            text.style.as_ref().unwrap().color,
            Some([255, 255, 255, 255])
        );
    }

    #[test]
    fn v5_scene_bin_upgrades_on_open() {
        // v5 predates `TextData.rendered_text_color`; the upgrade must leave
        // it `None` (the renderer refills it) and keep every trailing field
        // intact — postcard is positional, so the inserted field shifted
        // `lock_layout_box`.
        let (_tmp, path) = tmp_dir();
        {
            let session = ProjectSession::create(&path, "v5").unwrap();
            drop(session);
        }

        let page_id = PageId::new();
        let node_id = NodeId::new();
        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            node_id,
            compat::NodeV5 {
                id: node_id,
                transform: Transform {
                    x: 1.0,
                    y: 2.0,
                    width: 100.0,
                    height: 40.0,
                    rotation_deg: 0.0,
                },
                visible: true,
                kind: compat::NodeKindV5::Text(compat::TextDataV5 {
                    text: Some("안녕".to_string()),
                    translation: Some("Hi".to_string()),
                    style: Some(TextStyle {
                        font_size: Some(21.0),
                        color: Some([10, 20, 30, 255]),
                        ..Default::default()
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
            compat::PageV5 {
                id: page_id,
                name: "p1".to_string(),
                width: 800,
                height: 600,
                nodes,
            },
        );
        let v5 = compat::SnapshotV5 {
            epoch: 11,
            scene: compat::SceneV5 {
                project: koharu_core::ProjectMeta::default(),
                pages,
            },
        };
        let mut bytes = SCENE_MAGIC.to_vec();
        bytes.extend_from_slice(&5u16.to_le_bytes());
        bytes.extend_from_slice(&postcard::to_allocvec(&v5).unwrap());
        std::fs::write(path.join(SCENE_FILE).as_std_path(), bytes).unwrap();

        let session = ProjectSession::open(&path).expect("v5 scene.bin must open");
        let scene = session.scene.read();
        let node = scene
            .pages
            .get(&page_id)
            .and_then(|p| p.nodes.get(&node_id))
            .expect("node survives upgrade");
        let NodeKind::Text(text) = &node.kind else {
            panic!("expected text node");
        };
        assert_eq!(text.translation.as_deref(), Some("Hi"));
        assert_eq!(
            text.style.as_ref().and_then(|s| s.color),
            Some([10, 20, 30, 255]),
            "manual colour survives verbatim"
        );
        assert_eq!(text.rendered_font_size_px, Some(19.0));
        assert!(
            text.rendered_text_color.is_none(),
            "new field defaults to None for upgraded scenes"
        );
        assert!(text.lock_layout_box, "trailing bool must decode intact");
    }

    #[test]
    fn v6_scene_bin_adds_later_text_defaults() {
        let (_tmp, path) = tmp_dir();
        {
            let session = ProjectSession::create(&path, "v6").unwrap();
            drop(session);
        }

        let page_id = PageId::new();
        let node_id = NodeId::new();
        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            node_id,
            compat::NodeV6 {
                id: node_id,
                transform: Transform::default(),
                visible: true,
                kind: compat::NodeKindV6::Text(compat::TextDataV6 {
                    translation: Some("Hello world".to_string()),
                    rendered_text_color: Some([12, 34, 56, 255]),
                    lock_layout_box: true,
                    ..Default::default()
                }),
            },
        );
        let mut pages = indexmap::IndexMap::new();
        pages.insert(
            page_id,
            compat::PageV6 {
                id: page_id,
                name: "p1".to_string(),
                width: 800,
                height: 600,
                nodes,
            },
        );
        let v6 = compat::SnapshotV6 {
            epoch: 12,
            scene: compat::SceneV6 {
                project: koharu_core::ProjectMeta::default(),
                pages,
            },
        };
        let mut bytes = SCENE_MAGIC.to_vec();
        bytes.extend_from_slice(&6u16.to_le_bytes());
        bytes.extend_from_slice(&postcard::to_allocvec(&v6).unwrap());
        std::fs::write(path.join(SCENE_FILE).as_std_path(), bytes).unwrap();

        let session = ProjectSession::open(&path).expect("v6 scene.bin must open");
        let scene = session.scene.read();
        let NodeKind::Text(text) = &scene.pages[&page_id].nodes[&node_id].kind else {
            panic!("expected text node");
        };
        assert_eq!(text.translation.as_deref(), Some("Hello world"));
        assert_eq!(text.rendered_text_color, Some([12, 34, 56, 255]));
        assert!(text.lock_layout_box);
        assert!(text.style_ranges.is_empty());
        assert_eq!(text.writing_direction, None);
    }

    #[test]
    fn v7_scene_bin_preserves_ranges_and_defaults_writing_direction() {
        let (_tmp, path) = tmp_dir();
        {
            let session = ProjectSession::create(&path, "v7").unwrap();
            drop(session);
        }

        let page_id = PageId::new();
        let node_id = NodeId::new();
        let style_range = TextStyleRange {
            start: 0,
            end: 5,
            style: TextRangeStyle {
                bold: Some(true),
                ..Default::default()
            },
        };
        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            node_id,
            compat::NodeV7 {
                id: node_id,
                transform: Transform::default(),
                visible: true,
                kind: compat::NodeKindV7::Text(compat::TextDataV7 {
                    translation: Some("Hello world".to_string()),
                    rendered_text_color: Some([12, 34, 56, 255]),
                    lock_layout_box: true,
                    style_ranges: vec![style_range],
                    ..Default::default()
                }),
            },
        );
        let mut pages = indexmap::IndexMap::new();
        pages.insert(
            page_id,
            compat::PageV7 {
                id: page_id,
                name: "p1".to_string(),
                width: 800,
                height: 600,
                nodes,
            },
        );
        let v7 = compat::SnapshotV7 {
            epoch: 13,
            scene: compat::SceneV7 {
                project: koharu_core::ProjectMeta::default(),
                pages,
            },
        };
        let mut bytes = SCENE_MAGIC.to_vec();
        bytes.extend_from_slice(&7u16.to_le_bytes());
        bytes.extend_from_slice(&postcard::to_allocvec(&v7).unwrap());
        std::fs::write(path.join(SCENE_FILE).as_std_path(), bytes).unwrap();

        let session = ProjectSession::open(&path).expect("v7 scene.bin must open");
        let scene = session.scene.read();
        let NodeKind::Text(text) = &scene.pages[&page_id].nodes[&node_id].kind else {
            panic!("expected text node");
        };
        assert_eq!(text.translation.as_deref(), Some("Hello world"));
        assert_eq!(text.rendered_text_color, Some([12, 34, 56, 255]));
        assert!(text.lock_layout_box);
        assert_eq!(text.style_ranges, vec![style_range]);
        assert_eq!(text.writing_direction, None);
    }

    #[test]
    fn collided_v7_scene_with_writing_direction_is_recovered() {
        let (_tmp, path) = tmp_dir();
        {
            let session = ProjectSession::create(&path, "collided-v7").unwrap();
            drop(session);
        }

        let mut scene = Scene::default();
        let mut page = Page::new("p1", 800, 600);
        let page_id = page.id;
        let node_id = NodeId::new();
        page.nodes.insert(
            node_id,
            Node {
                id: node_id,
                transform: Transform::default(),
                visible: true,
                kind: NodeKind::Text(TextData {
                    translation: Some("VERTICAL".to_string()),
                    writing_direction: Some(TextDirection::Vertical),
                    ..Default::default()
                }),
            },
        );
        scene.pages.insert(page_id, page);

        let snap = Snapshot { epoch: 14, scene };
        let mut bytes = SCENE_MAGIC.to_vec();
        // Reproduce the faulty build: v8 payload under the already-used v7
        // header. The repaired decoder must recover it before writing v8.
        bytes.extend_from_slice(&7u16.to_le_bytes());
        bytes.extend_from_slice(&postcard::to_allocvec(&snap).unwrap());
        std::fs::write(path.join(SCENE_FILE).as_std_path(), bytes).unwrap();

        let session = ProjectSession::open(&path).expect("collided v7 scene.bin must open");
        let scene = session.scene.read();
        let NodeKind::Text(text) = &scene.pages[&page_id].nodes[&node_id].kind else {
            panic!("expected text node");
        };
        assert_eq!(text.translation.as_deref(), Some("VERTICAL"));
        assert_eq!(text.writing_direction, Some(TextDirection::Vertical));
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

    #[test]
    fn headerless_v6_scene_bin_with_mixed_nodes_upgrades() {
        // A few interim v6 builds wrote scene.bin without the "KSCN" header.
        // The headerless fallback chain in `decode_snapshot` must decode such a
        // payload via the v6 layout — not be mis-caught by the current (v8) or
        // v7 attempts — across multiple text nodes and mixed node kinds, and
        // upgrade every text node with the fields v6 predates (`style_ranges`,
        // `writing_direction`) defaulted.
        let (_tmp, path) = tmp_dir();
        {
            let session = ProjectSession::create(&path, "headerless-v6").unwrap();
            drop(session); // only project.toml written; we supply scene.bin below
        }

        let page_id = PageId::new();
        let image_id = NodeId::new();
        let text1_id = NodeId::new();
        let mask_id = NodeId::new();
        let text2_id = NodeId::new();

        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            image_id,
            compat::NodeV6 {
                id: image_id,
                transform: Transform::default(),
                visible: true,
                kind: compat::NodeKindV6::Image(ImageData {
                    role: ImageRole::Source,
                    blob: BlobRef::new("deadbeef"),
                    opacity: 1.0,
                    natural_width: 800,
                    natural_height: 600,
                    name: None,
                }),
            },
        );
        nodes.insert(
            text1_id,
            compat::NodeV6 {
                id: text1_id,
                transform: Transform {
                    x: 1.0,
                    y: 2.0,
                    width: 100.0,
                    height: 40.0,
                    rotation_deg: 0.0,
                },
                visible: true,
                kind: compat::NodeKindV6::Text(compat::TextDataV6 {
                    text: Some("こんにちは".to_string()),
                    translation: Some("Hello".to_string()),
                    rendered_text_color: Some([12, 34, 56, 255]),
                    lock_layout_box: true,
                    ..Default::default()
                }),
            },
        );
        nodes.insert(
            mask_id,
            compat::NodeV6 {
                id: mask_id,
                transform: Transform::default(),
                visible: false,
                kind: compat::NodeKindV6::Mask(MaskData {
                    role: MaskRole::Segment,
                    blob: BlobRef::new("cafef00d"),
                }),
            },
        );
        nodes.insert(
            text2_id,
            compat::NodeV6 {
                id: text2_id,
                transform: Transform::default(),
                visible: true,
                kind: compat::NodeKindV6::Text(compat::TextDataV6 {
                    translation: Some("World".to_string()),
                    ..Default::default()
                }),
            },
        );

        let mut pages = indexmap::IndexMap::new();
        pages.insert(
            page_id,
            compat::PageV6 {
                id: page_id,
                name: "p1".to_string(),
                width: 800,
                height: 600,
                nodes,
            },
        );
        let v6 = compat::SnapshotV6 {
            epoch: 12,
            scene: compat::SceneV6 {
                project: koharu_core::ProjectMeta::default(),
                pages,
            },
        };
        // Headerless: postcard payload only, no "KSCN" magic / version prefix.
        std::fs::write(
            path.join(SCENE_FILE).as_std_path(),
            postcard::to_allocvec(&v6).unwrap(),
        )
        .unwrap();

        let session = ProjectSession::open(&path).expect("headerless v6 scene.bin must open");
        let scene = session.scene.read();
        let page = scene.pages.get(&page_id).expect("page survives upgrade");
        assert_eq!(page.nodes.len(), 4, "all mixed nodes survive");

        let NodeKind::Image(image) = &page.nodes.get(&image_id).unwrap().kind else {
            panic!("expected image node");
        };
        assert_eq!(image.role, ImageRole::Source);
        assert_eq!(image.blob.hash(), "deadbeef");

        let NodeKind::Mask(mask) = &page.nodes.get(&mask_id).unwrap().kind else {
            panic!("expected mask node");
        };
        assert_eq!(mask.role, MaskRole::Segment);
        assert_eq!(mask.blob.hash(), "cafef00d");

        let NodeKind::Text(text1) = &page.nodes.get(&text1_id).unwrap().kind else {
            panic!("expected text node");
        };
        assert_eq!(text1.translation.as_deref(), Some("Hello"));
        assert_eq!(text1.rendered_text_color, Some([12, 34, 56, 255]));
        assert!(text1.lock_layout_box, "trailing bool must decode intact");
        assert!(text1.style_ranges.is_empty(), "v6 predates style_ranges");
        assert_eq!(
            text1.writing_direction, None,
            "v6 predates writing_direction"
        );

        let NodeKind::Text(text2) = &page.nodes.get(&text2_id).unwrap().kind else {
            panic!("expected text node");
        };
        assert_eq!(text2.translation.as_deref(), Some("World"));
        assert!(text2.style_ranges.is_empty());
        assert_eq!(text2.writing_direction, None);
    }

    #[test]
    fn headerless_v7_scene_bin_with_multiple_text_nodes_upgrades() {
        // Interim v7 (rich-text) builds also wrote headerless scene.bin. The
        // fallback chain must decode via the v7 layout across multiple text
        // nodes and mixed kinds, preserving character `style_ranges` and
        // defaulting the v8-only `writing_direction`.
        let (_tmp, path) = tmp_dir();
        {
            let session = ProjectSession::create(&path, "headerless-v7").unwrap();
            drop(session);
        }

        let page_id = PageId::new();
        let image_id = NodeId::new();
        let text1_id = NodeId::new();
        let text2_id = NodeId::new();

        let bold = TextStyleRange {
            start: 0,
            end: 5,
            style: TextRangeStyle {
                bold: Some(true),
                ..Default::default()
            },
        };
        let italic = TextStyleRange {
            start: 6,
            end: 11,
            style: TextRangeStyle {
                italic: Some(true),
                ..Default::default()
            },
        };

        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            image_id,
            compat::NodeV7 {
                id: image_id,
                transform: Transform::default(),
                visible: true,
                kind: compat::NodeKindV7::Image(ImageData {
                    role: ImageRole::Source,
                    blob: BlobRef::new("deadbeef"),
                    opacity: 1.0,
                    natural_width: 800,
                    natural_height: 600,
                    name: None,
                }),
            },
        );
        nodes.insert(
            text1_id,
            compat::NodeV7 {
                id: text1_id,
                transform: Transform::default(),
                visible: true,
                kind: compat::NodeKindV7::Text(compat::TextDataV7 {
                    translation: Some("Hello world".to_string()),
                    rendered_text_color: Some([12, 34, 56, 255]),
                    lock_layout_box: true,
                    style_ranges: vec![bold, italic],
                    ..Default::default()
                }),
            },
        );
        nodes.insert(
            text2_id,
            compat::NodeV7 {
                id: text2_id,
                transform: Transform::default(),
                visible: true,
                kind: compat::NodeKindV7::Text(compat::TextDataV7 {
                    translation: Some("plain".to_string()),
                    ..Default::default()
                }),
            },
        );

        let mut pages = indexmap::IndexMap::new();
        pages.insert(
            page_id,
            compat::PageV7 {
                id: page_id,
                name: "p1".to_string(),
                width: 800,
                height: 600,
                nodes,
            },
        );
        let v7 = compat::SnapshotV7 {
            epoch: 13,
            scene: compat::SceneV7 {
                project: koharu_core::ProjectMeta::default(),
                pages,
            },
        };
        std::fs::write(
            path.join(SCENE_FILE).as_std_path(),
            postcard::to_allocvec(&v7).unwrap(),
        )
        .unwrap();

        let session = ProjectSession::open(&path).expect("headerless v7 scene.bin must open");
        let scene = session.scene.read();
        let page = scene.pages.get(&page_id).expect("page survives upgrade");
        assert_eq!(page.nodes.len(), 3, "all mixed nodes survive");

        let NodeKind::Image(image) = &page.nodes.get(&image_id).unwrap().kind else {
            panic!("expected image node");
        };
        assert_eq!(image.role, ImageRole::Source);

        let NodeKind::Text(text1) = &page.nodes.get(&text1_id).unwrap().kind else {
            panic!("expected text node");
        };
        assert_eq!(text1.translation.as_deref(), Some("Hello world"));
        assert_eq!(text1.rendered_text_color, Some([12, 34, 56, 255]));
        assert!(text1.lock_layout_box);
        assert_eq!(
            text1.style_ranges,
            vec![bold, italic],
            "character style ranges survive the v7 upgrade"
        );
        assert_eq!(
            text1.writing_direction, None,
            "v7 predates writing_direction"
        );

        let NodeKind::Text(text2) = &page.nodes.get(&text2_id).unwrap().kind else {
            panic!("expected text node");
        };
        assert_eq!(text2.translation.as_deref(), Some("plain"));
        assert!(text2.style_ranges.is_empty());
        assert_eq!(text2.writing_direction, None);
    }

    #[test]
    fn compact_reopen_preserves_style_ranges_and_writing_direction() {
        // Current (v8) write path: a compacted scene.bin must round-trip both
        // character `style_ranges` and an explicit vertical `writing_direction`
        // — the fields appended for scene formats v7 and v8.
        let (_tmp, path) = tmp_dir();
        let page_id: PageId;
        let node_id: NodeId;
        {
            let session = ProjectSession::create(&path, "v8-round-trip").unwrap();
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
                    transform: Transform::default(),
                    visible: true,
                    kind: NodeKind::Text(TextData {
                        translation: Some("Hello world".to_string()),
                        style_ranges: vec![TextStyleRange {
                            start: 0,
                            end: 5,
                            style: TextRangeStyle {
                                bold: Some(true),
                                ..Default::default()
                            },
                        }],
                        writing_direction: Some(TextDirection::Vertical),
                        ..Default::default()
                    }),
                },
            );
            drop(scene);
            session.compact().unwrap();
        }

        let session = ProjectSession::open(&path).unwrap();
        let scene = session.scene.read();
        let NodeKind::Text(text) = &scene.pages[&page_id].nodes[&node_id].kind else {
            panic!("expected text node");
        };
        assert_eq!(text.translation.as_deref(), Some("Hello world"));
        assert_eq!(
            text.style_ranges,
            vec![TextStyleRange {
                start: 0,
                end: 5,
                style: TextRangeStyle {
                    bold: Some(true),
                    ..Default::default()
                },
            }],
            "character style ranges survive a v8 compact/reopen"
        );
        assert_eq!(
            text.writing_direction,
            Some(TextDirection::Vertical),
            "explicit writing direction survives a v8 compact/reopen"
        );
    }
}
