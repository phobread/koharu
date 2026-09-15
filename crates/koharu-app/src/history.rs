//! Linear undo/redo history + append-only durable op log.
//!
//! Two concerns, deliberately separated:
//!   1. **Durability log** — `history.log`: each applied op is first applied to
//!      a scene clone, then its frame is fsynced before the in-memory scene is
//!      committed. A failed apply changes nothing, and replay truncates torn
//!      trailing frames before future appends. The file starts with `"KHLG"` +
//!      a u16 LE format version; headerless logs are legacy v0. Future versions
//!      freeze old `LogFrame` layouts at the decode seam and upgrade them before
//!      replay.
//!   2. **Undo/redo stacks** — in-memory only; Cmd+Z within a session.
//!
//! Undo/redo are themselves logged ops: when the user undoes, we apply the
//! inverse and append it to the log as a normal op. Replay on open always
//! produces the post-undo state. No special entry type.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use atomicwrites::{AtomicFile, OverwriteBehavior};
use koharu_core::{Op, Scene};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Default cap for the in-memory undo stack. The log on disk is not capped —
/// it's compacted on snapshot.
const DEFAULT_UNDO_LIMIT: usize = 500;

/// Caps replay allocations and frame writes, and stays below `KHLG` interpreted
/// as a u32 LE so a length prefix is unambiguous with the versioned-log magic.
const MAX_FRAME_LEN: u32 = 512 * 1024 * 1024;

/// Headerless files are legacy v0. `KHLG` as a u32 LE is about 1.1 GB, above
/// `MAX_FRAME_LEN`, so a guarded length prefix can never be mistaken for the
/// magic and the formats are unambiguous.
const HISTORY_LOG_MAGIC: [u8; 4] = *b"KHLG";
const HISTORY_LOG_VERSION: u16 = 3;

// ---------------------------------------------------------------------------
// Log frames
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct LogFrame {
    epoch: u64,
    op: Op,
}

/// Frozen history-log layouts. v1 predates `TextData::style_ranges`; v2 has
/// style ranges but predates `TextData::writing_direction`. Postcard is
/// positional, so every old operation capable of carrying a text node/patch
/// decodes through its exact historical shape before being upgraded.
mod compat {
    use indexmap::IndexMap;
    use koharu_core::{
        BlobRef, FontPrediction, ImageData, ImageDataPatch, MaskData, MaskDataPatch, Node, NodeId,
        NodeKind, Op, Page, PageId, PagePatch, ProjectMetaPatch, TextData, TextDataPatch,
        TextDirection, TextStyle, TextStyleRange, Transform,
    };
    use serde::Deserialize;

    use super::LogFrame;

    #[derive(Deserialize)]
    pub(super) struct LogFrameV1 {
        epoch: u64,
        op: OpV1,
    }

    #[derive(Deserialize)]
    enum OpV1 {
        UpdateProjectMeta {
            patch: ProjectMetaPatch,
            prev: ProjectMetaPatch,
        },
        AddPage {
            page: PageV1,
            at: usize,
        },
        RemovePage {
            id: PageId,
            prev_page: PageV1,
            prev_index: usize,
        },
        UpdatePage {
            id: PageId,
            patch: PagePatch,
            prev: PagePatch,
        },
        ReorderPages {
            order: Vec<PageId>,
            prev_order: Vec<PageId>,
        },
        AddNode {
            page: PageId,
            node: NodeV1,
            at: usize,
        },
        RemoveNode {
            page: PageId,
            id: NodeId,
            prev_node: NodeV1,
            prev_index: usize,
        },
        UpdateNode {
            page: PageId,
            id: NodeId,
            patch: NodePatchV1,
            prev: NodePatchV1,
        },
        ReorderNodes {
            page: PageId,
            order: Vec<NodeId>,
            prev_order: Vec<NodeId>,
        },
        Batch {
            ops: Vec<OpV1>,
            label: String,
        },
    }

    #[derive(Deserialize)]
    struct PageV1 {
        id: PageId,
        name: String,
        width: u32,
        height: u32,
        nodes: IndexMap<NodeId, NodeV1>,
    }

    #[derive(Deserialize)]
    struct NodeV1 {
        id: NodeId,
        transform: Transform,
        visible: bool,
        kind: NodeKindV1,
    }

    #[derive(Deserialize)]
    enum NodeKindV1 {
        Image(ImageData),
        Text(TextDataV1),
        Mask(MaskData),
    }

    #[derive(Default, Deserialize)]
    struct TextDataV1 {
        confidence: f32,
        source_lang: Option<String>,
        source_direction: Option<TextDirection>,
        rendered_direction: Option<TextDirection>,
        line_polygons: Option<Vec<[[f32; 2]; 4]>>,
        rotation_deg: Option<f32>,
        detected_font_size_px: Option<f32>,
        detector: Option<String>,
        text: Option<String>,
        translation: Option<String>,
        style: Option<TextStyle>,
        font_prediction: Option<FontPrediction>,
        sprite: Option<BlobRef>,
        sprite_transform: Option<Transform>,
        rendered_font_size_px: Option<f32>,
        rendered_text_color: Option<[u8; 4]>,
        lock_layout_box: bool,
    }

    #[derive(Default, Deserialize)]
    struct NodePatchV1 {
        transform: Option<Transform>,
        visible: Option<bool>,
        data: Option<NodeDataPatchV1>,
    }

    #[derive(Deserialize)]
    enum NodeDataPatchV1 {
        Text(TextDataPatchV1),
        Image(ImageDataPatch),
        Mask(MaskDataPatch),
    }

    #[derive(Default, Deserialize)]
    struct TextDataPatchV1 {
        confidence: Option<f32>,
        source_lang: Option<Option<String>>,
        source_direction: Option<Option<TextDirection>>,
        rendered_direction: Option<Option<TextDirection>>,
        line_polygons: Option<Option<Vec<[[f32; 2]; 4]>>>,
        rotation_deg: Option<Option<f32>>,
        detected_font_size_px: Option<Option<f32>>,
        detector: Option<Option<String>>,
        text: Option<Option<String>>,
        translation: Option<Option<String>>,
        style: Option<Option<TextStyle>>,
        font_prediction: Option<Option<FontPrediction>>,
        sprite: Option<Option<BlobRef>>,
        sprite_transform: Option<Option<Transform>>,
        rendered_font_size_px: Option<Option<f32>>,
        rendered_text_color: Option<Option<[u8; 4]>>,
        lock_layout_box: Option<bool>,
    }

    impl LogFrameV1 {
        pub(super) fn upgrade(self) -> LogFrame {
            LogFrame {
                epoch: self.epoch,
                op: self.op.upgrade(),
            }
        }
    }

    impl OpV1 {
        fn upgrade(self) -> Op {
            match self {
                Self::UpdateProjectMeta { patch, prev } => Op::UpdateProjectMeta { patch, prev },
                Self::AddPage { page, at } => Op::AddPage {
                    page: page.upgrade(),
                    at,
                },
                Self::RemovePage {
                    id,
                    prev_page,
                    prev_index,
                } => Op::RemovePage {
                    id,
                    prev_page: prev_page.upgrade(),
                    prev_index,
                },
                Self::UpdatePage { id, patch, prev } => Op::UpdatePage { id, patch, prev },
                Self::ReorderPages { order, prev_order } => Op::ReorderPages { order, prev_order },
                Self::AddNode { page, node, at } => Op::AddNode {
                    page,
                    node: node.upgrade(),
                    at,
                },
                Self::RemoveNode {
                    page,
                    id,
                    prev_node,
                    prev_index,
                } => Op::RemoveNode {
                    page,
                    id,
                    prev_node: prev_node.upgrade(),
                    prev_index,
                },
                Self::UpdateNode {
                    page,
                    id,
                    patch,
                    prev,
                } => Op::UpdateNode {
                    page,
                    id,
                    patch: patch.upgrade(),
                    prev: prev.upgrade(),
                },
                Self::ReorderNodes {
                    page,
                    order,
                    prev_order,
                } => Op::ReorderNodes {
                    page,
                    order,
                    prev_order,
                },
                Self::Batch { ops, label } => Op::Batch {
                    ops: ops.into_iter().map(Self::upgrade).collect(),
                    label,
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
                    .map(|(id, node)| (id, node.upgrade()))
                    .collect(),
            }
        }
    }

    impl NodeV1 {
        fn upgrade(self) -> Node {
            Node {
                id: self.id,
                transform: self.transform,
                visible: self.visible,
                kind: match self.kind {
                    NodeKindV1::Image(data) => NodeKind::Image(data),
                    NodeKindV1::Text(data) => NodeKind::Text(data.upgrade()),
                    NodeKindV1::Mask(data) => NodeKind::Mask(data),
                },
            }
        }
    }

    impl TextDataV1 {
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

    impl NodePatchV1 {
        fn upgrade(self) -> koharu_core::NodePatch {
            koharu_core::NodePatch {
                transform: self.transform,
                visible: self.visible,
                data: self.data.map(NodeDataPatchV1::upgrade),
            }
        }
    }

    impl NodeDataPatchV1 {
        fn upgrade(self) -> koharu_core::NodeDataPatch {
            match self {
                Self::Text(data) => koharu_core::NodeDataPatch::Text(data.upgrade()),
                Self::Image(data) => koharu_core::NodeDataPatch::Image(data),
                Self::Mask(data) => koharu_core::NodeDataPatch::Mask(data),
            }
        }
    }

    impl TextDataPatchV1 {
        fn upgrade(self) -> TextDataPatch {
            TextDataPatch {
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
                style_ranges: None,
                writing_direction: None,
            }
        }
    }

    /// Exact history-log v2 layout: rich-text ranges are present, but the
    /// writing-direction fields have not yet been appended.
    pub(super) mod v2 {
        use super::*;

        #[derive(Deserialize)]
        pub(in crate::history) struct LogFrameV2 {
            epoch: u64,
            op: OpV2,
        }

        #[derive(Deserialize)]
        enum OpV2 {
            UpdateProjectMeta {
                patch: ProjectMetaPatch,
                prev: ProjectMetaPatch,
            },
            AddPage {
                page: PageV2,
                at: usize,
            },
            RemovePage {
                id: PageId,
                prev_page: PageV2,
                prev_index: usize,
            },
            UpdatePage {
                id: PageId,
                patch: PagePatch,
                prev: PagePatch,
            },
            ReorderPages {
                order: Vec<PageId>,
                prev_order: Vec<PageId>,
            },
            AddNode {
                page: PageId,
                node: NodeV2,
                at: usize,
            },
            RemoveNode {
                page: PageId,
                id: NodeId,
                prev_node: NodeV2,
                prev_index: usize,
            },
            UpdateNode {
                page: PageId,
                id: NodeId,
                patch: NodePatchV2,
                prev: NodePatchV2,
            },
            ReorderNodes {
                page: PageId,
                order: Vec<NodeId>,
                prev_order: Vec<NodeId>,
            },
            Batch {
                ops: Vec<OpV2>,
                label: String,
            },
        }

        #[derive(Deserialize)]
        struct PageV2 {
            id: PageId,
            name: String,
            width: u32,
            height: u32,
            nodes: IndexMap<NodeId, NodeV2>,
        }

        #[derive(Deserialize)]
        struct NodeV2 {
            id: NodeId,
            transform: Transform,
            visible: bool,
            kind: NodeKindV2,
        }

        #[derive(Deserialize)]
        enum NodeKindV2 {
            Image(ImageData),
            Text(TextDataV2),
            Mask(MaskData),
        }

        #[derive(Default, Deserialize)]
        struct TextDataV2 {
            confidence: f32,
            source_lang: Option<String>,
            source_direction: Option<TextDirection>,
            rendered_direction: Option<TextDirection>,
            line_polygons: Option<Vec<[[f32; 2]; 4]>>,
            rotation_deg: Option<f32>,
            detected_font_size_px: Option<f32>,
            detector: Option<String>,
            text: Option<String>,
            translation: Option<String>,
            style: Option<TextStyle>,
            font_prediction: Option<FontPrediction>,
            sprite: Option<BlobRef>,
            sprite_transform: Option<Transform>,
            rendered_font_size_px: Option<f32>,
            rendered_text_color: Option<[u8; 4]>,
            lock_layout_box: bool,
            style_ranges: Vec<TextStyleRange>,
        }

        #[derive(Default, Deserialize)]
        struct NodePatchV2 {
            transform: Option<Transform>,
            visible: Option<bool>,
            data: Option<NodeDataPatchV2>,
        }

        #[derive(Deserialize)]
        enum NodeDataPatchV2 {
            Text(TextDataPatchV2),
            Image(ImageDataPatch),
            Mask(MaskDataPatch),
        }

        #[derive(Default, Deserialize)]
        struct TextDataPatchV2 {
            confidence: Option<f32>,
            source_lang: Option<Option<String>>,
            source_direction: Option<Option<TextDirection>>,
            rendered_direction: Option<Option<TextDirection>>,
            line_polygons: Option<Option<Vec<[[f32; 2]; 4]>>>,
            rotation_deg: Option<Option<f32>>,
            detected_font_size_px: Option<Option<f32>>,
            detector: Option<Option<String>>,
            text: Option<Option<String>>,
            translation: Option<Option<String>>,
            style: Option<Option<TextStyle>>,
            font_prediction: Option<Option<FontPrediction>>,
            sprite: Option<Option<BlobRef>>,
            sprite_transform: Option<Option<Transform>>,
            rendered_font_size_px: Option<Option<f32>>,
            rendered_text_color: Option<Option<[u8; 4]>>,
            lock_layout_box: Option<bool>,
            style_ranges: Option<Vec<TextStyleRange>>,
        }

        impl LogFrameV2 {
            pub(in crate::history) fn upgrade(self) -> LogFrame {
                LogFrame {
                    epoch: self.epoch,
                    op: self.op.upgrade(),
                }
            }
        }

        impl OpV2 {
            fn upgrade(self) -> Op {
                match self {
                    Self::UpdateProjectMeta { patch, prev } => {
                        Op::UpdateProjectMeta { patch, prev }
                    }
                    Self::AddPage { page, at } => Op::AddPage {
                        page: page.upgrade(),
                        at,
                    },
                    Self::RemovePage {
                        id,
                        prev_page,
                        prev_index,
                    } => Op::RemovePage {
                        id,
                        prev_page: prev_page.upgrade(),
                        prev_index,
                    },
                    Self::UpdatePage { id, patch, prev } => Op::UpdatePage { id, patch, prev },
                    Self::ReorderPages { order, prev_order } => {
                        Op::ReorderPages { order, prev_order }
                    }
                    Self::AddNode { page, node, at } => Op::AddNode {
                        page,
                        node: node.upgrade(),
                        at,
                    },
                    Self::RemoveNode {
                        page,
                        id,
                        prev_node,
                        prev_index,
                    } => Op::RemoveNode {
                        page,
                        id,
                        prev_node: prev_node.upgrade(),
                        prev_index,
                    },
                    Self::UpdateNode {
                        page,
                        id,
                        patch,
                        prev,
                    } => Op::UpdateNode {
                        page,
                        id,
                        patch: patch.upgrade(),
                        prev: prev.upgrade(),
                    },
                    Self::ReorderNodes {
                        page,
                        order,
                        prev_order,
                    } => Op::ReorderNodes {
                        page,
                        order,
                        prev_order,
                    },
                    Self::Batch { ops, label } => Op::Batch {
                        ops: ops.into_iter().map(Self::upgrade).collect(),
                        label,
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
                        .map(|(id, node)| (id, node.upgrade()))
                        .collect(),
                }
            }
        }

        impl NodeV2 {
            fn upgrade(self) -> Node {
                Node {
                    id: self.id,
                    transform: self.transform,
                    visible: self.visible,
                    kind: match self.kind {
                        NodeKindV2::Image(data) => NodeKind::Image(data),
                        NodeKindV2::Text(data) => NodeKind::Text(data.upgrade()),
                        NodeKindV2::Mask(data) => NodeKind::Mask(data),
                    },
                }
            }
        }

        impl TextDataV2 {
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

        impl NodePatchV2 {
            fn upgrade(self) -> koharu_core::NodePatch {
                koharu_core::NodePatch {
                    transform: self.transform,
                    visible: self.visible,
                    data: self.data.map(NodeDataPatchV2::upgrade),
                }
            }
        }

        impl NodeDataPatchV2 {
            fn upgrade(self) -> koharu_core::NodeDataPatch {
                match self {
                    Self::Text(data) => koharu_core::NodeDataPatch::Text(data.upgrade()),
                    Self::Image(data) => koharu_core::NodeDataPatch::Image(data),
                    Self::Mask(data) => koharu_core::NodeDataPatch::Mask(data),
                }
            }
        }

        impl TextDataPatchV2 {
            fn upgrade(self) -> TextDataPatch {
                TextDataPatch {
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
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

pub struct History {
    log_path: PathBuf,
    log: BufWriter<File>,
    committed_len: u64,
    poisoned: bool,
    epoch: u64,
    undo_stack: VecDeque<Op>,
    redo_stack: Vec<Op>,
    limit: usize,
}

impl History {
    /// Open the log at `path`, creating it if missing. Callers are expected to
    /// run `replay` first so torn tails are truncated; this method appends
    /// blindly to any existing bytes.
    pub fn open(path: impl Into<PathBuf>, epoch: u64) -> Result<Self> {
        let log_path = path.into();
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(&log_path)
            .with_context(|| format!("open history log {}", log_path.display()))?;
        if file.metadata()?.len() == 0 {
            file.write_all(&HISTORY_LOG_MAGIC)?;
            file.write_all(&HISTORY_LOG_VERSION.to_le_bytes())?;
            file.flush()?;
            file.sync_all()?;
        }
        let committed_len = file.metadata()?.len();
        Ok(Self {
            log_path,
            log: BufWriter::new(file),
            committed_len,
            poisoned: false,
            epoch,
            undo_stack: VecDeque::new(),
            redo_stack: Vec::new(),
            limit: DEFAULT_UNDO_LIMIT,
        })
    }

    /// Override the in-memory undo-stack cap.
    pub fn with_undo_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Apply an op to the scene, fsync a frame to disk, push to the undo stack.
    pub fn apply(&mut self, scene: &mut Scene, mut op: Op) -> Result<u64> {
        if self.poisoned {
            anyhow::bail!(
                "history log write previously failed and could not be rolled back; reopen the project to recover"
            );
        }
        let mut work = scene.clone();
        op.apply(&mut work).context("apply op to scene")?;
        let next = self.epoch + 1;
        self.write_frame(next, &op)?;
        self.epoch = next;
        *scene = work;
        self.push_undo(op);
        self.redo_stack.clear();
        Ok(next)
    }

    /// Undo the most recent op. Applies its inverse, records the inverse in
    /// the log, and moves the original onto the redo stack. Returns the new
    /// epoch + the inverse op that was just applied (so the RPC layer can
    /// broadcast it for clients to patch their mirrors without refetching).
    pub fn undo(&mut self, scene: &mut Scene) -> Result<Option<(u64, Op)>> {
        let Some(original) = self.undo_stack.back() else {
            return Ok(None);
        };
        let mut inverse = original.inverse();
        let mut work = scene.clone();
        inverse.apply(&mut work).context("apply inverse op")?;
        let next = self.epoch + 1;
        self.write_frame(next, &inverse)?;
        self.epoch = next;
        *scene = work;
        let original = self
            .undo_stack
            .pop_back()
            .expect("undo stack was peeked immediately before commit");
        self.redo_stack.push(original);
        Ok(Some((next, inverse)))
    }

    /// Re-apply the most recent undo. Symmetric with `undo`. Returns the new
    /// epoch + the op that was just re-applied.
    pub fn redo(&mut self, scene: &mut Scene) -> Result<Option<(u64, Op)>> {
        let Some(original) = self.redo_stack.last() else {
            return Ok(None);
        };
        let mut op = original.clone();
        let mut work = scene.clone();
        op.apply(&mut work).context("re-apply op")?;
        let next = self.epoch + 1;
        self.write_frame(next, &op)?;
        self.epoch = next;
        *scene = work;
        self.redo_stack.pop();
        self.push_undo(op.clone());
        Ok(Some((next, op)))
    }

    /// Truncate the log after a snapshot has been committed.
    /// Caller must have already fsynced the snapshot file.
    pub fn truncate_log(&mut self) -> Result<()> {
        let replacement = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&self.log_path)?;
        let old = std::mem::replace(&mut self.log, BufWriter::new(replacement));
        let (old_file, _residue) = old.into_parts();
        drop(old_file);
        // Reopen to truncate; BufWriter's underlying file handle is append-only.
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .truncate(true)
            .open(&self.log_path)
            .with_context(|| format!("truncate history log {}", self.log_path.display()))?;
        file.write_all(&HISTORY_LOG_MAGIC)?;
        file.write_all(&HISTORY_LOG_VERSION.to_le_bytes())?;
        file.flush()?;
        file.sync_all()?;
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&self.log_path)?;
        self.log = BufWriter::new(file);
        self.committed_len = (HISTORY_LOG_MAGIC.len() + 2) as u64;
        self.poisoned = false;
        Ok(())
    }

    // --- internals ---------------------------------------------------------

    fn write_frame(&mut self, epoch: u64, op: &Op) -> Result<()> {
        if self.poisoned {
            anyhow::bail!(
                "history log write previously failed and could not be rolled back; reopen the project to recover"
            );
        }
        let frame = LogFrame {
            epoch,
            op: op.clone(),
        };
        let bytes = postcard::to_allocvec(&frame).context("encode log frame")?;
        let len = u32::try_from(bytes.len()).context("log frame too large")?;
        anyhow::ensure!(
            len <= MAX_FRAME_LEN,
            "history log frame length {len} exceeds maximum {MAX_FRAME_LEN}"
        );
        let write_result = (|| -> std::io::Result<()> {
            self.log.write_all(&len.to_le_bytes())?;
            self.log.write_all(&bytes)?;
            self.log.flush()?;
            self.log.get_ref().sync_data()?;
            Ok(())
        })();
        match write_result {
            Ok(()) => {
                self.committed_len += 4 + bytes.len() as u64;
                Ok(())
            }
            Err(write_error) => match self.recover_log_tail() {
                Ok(()) => Err(anyhow::Error::new(write_error).context("write history log frame")),
                Err(recovery_error) => {
                    self.poisoned = true;
                    anyhow::bail!(
                        "history log frame write failed: {write_error}; rollback also failed: {recovery_error:#}; project must be reopened to recover"
                    )
                }
            },
        }
    }

    fn recover_log_tail(&mut self) -> Result<()> {
        let replacement = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&self.log_path)
            .with_context(|| format!("reopen history log {}", self.log_path.display()))?;
        let old = std::mem::replace(&mut self.log, BufWriter::new(replacement));
        let (old_file, _residue) = old.into_parts();
        drop(old_file);

        let file = OpenOptions::new()
            .write(true)
            .open(&self.log_path)
            .with_context(|| {
                format!("open history log {} for recovery", self.log_path.display())
            })?;
        file.set_len(self.committed_len)
            .context("truncate failed history log frame")?;
        file.sync_all().context("sync recovered history log")?;
        Ok(())
    }

    fn push_undo(&mut self, op: Op) {
        self.undo_stack.push_back(op);
        while self.undo_stack.len() > self.limit {
            self.undo_stack.pop_front();
        }
    }
}

// ---------------------------------------------------------------------------
// Replay — called once on project open, before a `History` is constructed.
// ---------------------------------------------------------------------------

/// Replay each frame in `log_path` with epoch greater than `start_epoch`
/// against `scene`. Returns the final epoch seen.
pub fn replay(log_path: &Path, start_epoch: u64, scene: &mut Scene) -> Result<u64> {
    if !log_path.exists() {
        return Ok(start_epoch);
    }
    let file =
        File::open(log_path).with_context(|| format!("open history log {}", log_path.display()))?;
    let mut reader = BufReader::new(file);
    let mut magic = [0u8; 4];
    let log_version = match reader.read_exact(&mut magic) {
        Ok(()) if magic == HISTORY_LOG_MAGIC => {
            let mut version = [0u8; 2];
            match reader.read_exact(&mut version) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    anyhow::bail!("truncated history.log header");
                }
                Err(err) => {
                    return Err(anyhow::Error::new(err).context("read history log version"));
                }
            }
            let version = u16::from_le_bytes(version);
            match version {
                1 | 2 | HISTORY_LOG_VERSION => Some(version),
                _ => anyhow::bail!(
                    "unsupported history.log format version {version} (written by a newer build?)"
                ),
            }
        }
        Ok(()) => {
            reader.seek(SeekFrom::Start(0))?;
            None
        }
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
            reader.seek(SeekFrom::Start(0))?;
            None
        }
        Err(err) => return Err(anyhow::Error::new(err).context("read history log header")),
    };
    let mut valid_len = if log_version.is_some() {
        (HISTORY_LOG_MAGIC.len() + 2) as u64
    } else {
        0
    };
    let mut discarded_tail = false;
    let mut epoch = start_epoch;
    let migrate_legacy = log_version != Some(HISTORY_LOG_VERSION);
    let mut migrated_frames = Vec::new();
    loop {
        let mut len_buf = [0u8; 4];
        match reader.read(&mut len_buf[..1]) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) => {
                return Err(anyhow::Error::new(err).context("read log frame length"));
            }
        }
        match reader.read_exact(&mut len_buf[1..]) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                tracing::warn!(
                    path = %log_path.display(),
                    "truncated trailing frame length in history log; discarding"
                );
                discarded_tail = true;
                break;
            }
            Err(err) => {
                return Err(anyhow::Error::new(err).context("read log frame length"));
            }
        }
        let len = u32::from_le_bytes(len_buf);
        if len > MAX_FRAME_LEN {
            tracing::warn!(
                path = %log_path.display(),
                bogus_len = len,
                max_len = MAX_FRAME_LEN,
                "oversized trailing frame in history log; discarding"
            );
            discarded_tail = true;
            break;
        }
        let len = len as usize;
        let mut buf = vec![0u8; len];
        match reader.read_exact(&mut buf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                // Truncated frame (likely crash mid-write) — stop cleanly.
                tracing::warn!(
                    path = %log_path.display(),
                    expected_len = len,
                    "truncated trailing frame in history log; discarding"
                );
                discarded_tail = true;
                break;
            }
            Err(e) => return Err(anyhow::Error::new(e).context("read log frame body")),
        }
        let frame: Result<LogFrame> = match log_version {
            Some(HISTORY_LOG_VERSION) => decode_frame_exact::<LogFrame>(&buf),
            Some(2) => match decode_frame_exact::<compat::v2::LogFrameV2>(&buf) {
                Ok(frame) => Ok(frame.upgrade()),
                Err(v2_err) => decode_frame_exact::<LogFrame>(&buf)
                    .with_context(|| format!("canonical history v2 also failed: {v2_err:#}")),
            },
            None | Some(1) => {
                decode_frame_exact::<compat::LogFrameV1>(&buf).map(compat::LogFrameV1::upgrade)
            }
            Some(_) => unreachable!("unsupported history log version was rejected above"),
        };
        let frame = match frame {
            Ok(frame) => frame,
            Err(err) => {
                tracing::warn!(
                    path = %log_path.display(),
                    error = %err,
                    "undecodable frame in history log; stopping replay"
                );
                discarded_tail = true;
                break;
            }
        };
        valid_len += 4 + len as u64;
        if migrate_legacy {
            migrated_frames.push(LogFrame {
                epoch: frame.epoch,
                op: frame.op.clone(),
            });
        }
        if frame.epoch > epoch {
            let mut op = frame.op;
            op.apply(scene).context("replay op")?;
            epoch = frame.epoch;
        }
    }
    drop(reader);
    if discarded_tail && std::fs::metadata(log_path)?.len() > valid_len {
        tracing::warn!(
            path = %log_path.display(),
            valid_len,
            "truncating invalid trailing bytes from history log"
        );
        let file = OpenOptions::new()
            .write(true)
            .open(log_path)
            .with_context(|| format!("open history log {} for tail repair", log_path.display()))?;
        file.set_len(valid_len)
            .context("truncate invalid history log tail")?;
        file.sync_all().context("sync repaired history log")?;
    }
    if migrate_legacy {
        migrate_history_log(log_path, &migrated_frames)?;
    }
    Ok(epoch)
}

fn decode_frame_exact<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let (value, rest) = postcard::take_from_bytes(bytes).context("decode history frame")?;
    anyhow::ensure!(
        rest.is_empty(),
        "history frame left {} trailing bytes",
        rest.len()
    );
    Ok(value)
}

/// Rewrite a fully decoded legacy log under the current header/layout before
/// `History::open` appends another frame. The atomic replacement means a
/// crash cannot leave a mixed-version log.
fn migrate_history_log(log_path: &Path, frames: &[LogFrame]) -> Result<()> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&HISTORY_LOG_MAGIC);
    bytes.extend_from_slice(&HISTORY_LOG_VERSION.to_le_bytes());
    for frame in frames {
        let body = postcard::to_allocvec(frame).context("encode migrated history frame")?;
        let len = u32::try_from(body.len()).context("migrated history frame too large")?;
        anyhow::ensure!(
            len <= MAX_FRAME_LEN,
            "migrated history frame length {len} exceeds maximum {MAX_FRAME_LEN}"
        );
        bytes.extend_from_slice(&len.to_le_bytes());
        bytes.extend_from_slice(&body);
    }
    AtomicFile::new(log_path, OverwriteBehavior::AllowOverwrite)
        .write(|file| {
            file.write_all(&bytes)?;
            file.sync_all()
        })
        .context("migrate history log to current format")
}

#[cfg(test)]
mod tests {
    use super::*;
    use koharu_core::{
        BlobRef, FontPrediction, ImageData, ImageRole, Node, NodeId, NodeKind, Page, PageId,
        TextData, TextDirection, TextRangeStyle, TextStyle, TextStyleRange, Transform,
    };
    use tempfile::tempdir;

    fn add_page(name: &str) -> Op {
        Op::AddPage {
            page: Page::new(name, 800, 600),
            at: 0,
        }
    }

    fn source_node(blob: &str) -> Node {
        Node {
            id: NodeId::new(),
            transform: Transform::default(),
            visible: true,
            kind: NodeKind::Image(ImageData {
                role: ImageRole::Source,
                blob: BlobRef::new(blob),
                opacity: 1.0,
                natural_width: 10,
                natural_height: 10,
                name: None,
            }),
        }
    }

    fn write_headerless_frames(path: &Path, frames: &[LogFrame]) {
        let mut file = File::create(path).unwrap();
        for frame in frames {
            let bytes = postcard::to_allocvec(frame).unwrap();
            let len = u32::try_from(bytes.len()).unwrap();
            file.write_all(&len.to_le_bytes()).unwrap();
            file.write_all(&bytes).unwrap();
        }
        file.flush().unwrap();
    }

    fn assert_same_scene(actual: &Scene, expected: &Scene) {
        assert_eq!(
            postcard::to_allocvec(actual).unwrap(),
            postcard::to_allocvec(expected).unwrap()
        );
    }

    #[test]
    fn fresh_log_round_trips_with_versioned_header() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        // Seed both sides from one base: Scene::default() stamps project meta
        // with Utc::now(), so two independent defaults never compare equal.
        let base = Scene::default();
        let mut scene = base.clone();
        let mut history = History::open(&path, 0).unwrap();
        history.apply(&mut scene, add_page("p1")).unwrap();
        history.apply(&mut scene, add_page("p2")).unwrap();
        drop(history);

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], &HISTORY_LOG_MAGIC);
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            HISTORY_LOG_VERSION
        );

        let mut replayed = base.clone();
        assert_eq!(replay(&path, 0, &mut replayed).unwrap(), 2);
        assert_same_scene(&replayed, &scene);
    }

    #[test]
    fn headerless_legacy_log_replays() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        let mut op1 = add_page("legacy-1");
        let mut op2 = add_page("legacy-2");
        write_headerless_frames(
            &path,
            &[
                LogFrame {
                    epoch: 1,
                    op: op1.clone(),
                },
                LogFrame {
                    epoch: 2,
                    op: op2.clone(),
                },
            ],
        );

        // Shared base: Scene::default() stamps project meta with Utc::now().
        let base = Scene::default();
        let mut expected = base.clone();
        op1.apply(&mut expected).unwrap();
        op2.apply(&mut expected).unwrap();
        let mut replayed = base.clone();
        assert_eq!(replay(&path, 0, &mut replayed).unwrap(), 2);
        assert_same_scene(&replayed, &expected);
        let migrated = std::fs::read(&path).unwrap();
        assert_eq!(&migrated[..4], &HISTORY_LOG_MAGIC);
        assert_eq!(
            u16::from_le_bytes([migrated[4], migrated[5]]),
            HISTORY_LOG_VERSION
        );
    }

    #[test]
    fn truncate_log_rewrites_header_and_subsequent_ops_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        let mut scene = Scene::default();
        let mut history = History::open(&path, 0).unwrap();
        history.apply(&mut scene, add_page("snapshot")).unwrap();
        let mut snapshot = scene.clone();
        history.truncate_log().unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], &HISTORY_LOG_MAGIC);
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]),
            HISTORY_LOG_VERSION
        );

        history
            .apply(&mut scene, add_page("after-snapshot"))
            .unwrap();
        drop(history);
        assert_eq!(replay(&path, 1, &mut snapshot).unwrap(), 2);
        assert_same_scene(&snapshot, &scene);
    }

    #[test]
    fn newer_history_log_version_is_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        let mut bytes = HISTORY_LOG_MAGIC.to_vec();
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&[1, 2, 3]);
        std::fs::write(&path, bytes).unwrap();

        let err = replay(&path, 0, &mut Scene::default()).unwrap_err();
        assert!(format!("{err:#}").contains("unsupported history.log format version 4"));
    }

    #[test]
    fn header_only_log_replays_to_start_epoch() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        let mut bytes = HISTORY_LOG_MAGIC.to_vec();
        bytes.extend_from_slice(&HISTORY_LOG_VERSION.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();

        let mut scene = Scene::default();
        assert_eq!(replay(&path, 7, &mut scene).unwrap(), 7);
        assert!(scene.pages.is_empty());
    }

    #[test]
    fn failed_batch_leaves_scene_epoch_log_and_stacks_untouched() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        let base = Scene::default();
        let mut scene = base.clone();
        let mut history = History::open(&path, 0).unwrap();

        let page = Page::new("p1", 800, 600);
        let page_id = page.id;
        history
            .apply(&mut scene, Op::AddPage { page, at: 0 })
            .unwrap();
        let scene_before = postcard::to_allocvec(&scene).unwrap();
        let epoch_before = history.epoch();
        let log_len_before = std::fs::metadata(&path).unwrap().len();
        let undo_len_before = history.undo_stack.len();

        let result = history.apply(
            &mut scene,
            Op::Batch {
                ops: vec![
                    Op::AddNode {
                        page: page_id,
                        node: source_node("first"),
                        at: 0,
                    },
                    Op::AddNode {
                        page: page_id,
                        node: source_node("second"),
                        at: 1,
                    },
                ],
                label: "partially valid".into(),
            },
        );
        assert!(result.is_err());
        assert_eq!(postcard::to_allocvec(&scene).unwrap(), scene_before);
        assert_eq!(history.epoch(), epoch_before);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), log_len_before);
        assert_eq!(history.undo_stack.len(), undo_len_before);
        assert!(history.redo_stack.is_empty());

        let next = history.apply(&mut scene, add_page("p2")).unwrap();
        assert_eq!(next, epoch_before + 1);
        drop(history);
        let mut replayed = base;
        assert_eq!(replay(&path, 0, &mut replayed).unwrap(), next);
        assert_same_scene(&replayed, &scene);
    }

    #[test]
    fn undo_failure_restores_undo_stack() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        let mut scene = Scene::default();
        let mut history = History::open(&path, 0).unwrap();
        history.apply(&mut scene, add_page("p1")).unwrap();
        let epoch_before = history.epoch();
        scene.pages.clear();

        assert!(history.undo(&mut scene).is_err());
        assert_eq!(history.undo_stack.len(), 1);
        assert!(history.redo_stack.is_empty());
        assert_eq!(history.epoch(), epoch_before);
    }

    #[test]
    fn redo_failure_restores_redo_stack() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        let mut scene = Scene::default();
        let mut history = History::open(&path, 0).unwrap();
        let page = Page::new("p1", 800, 600);
        let page_id = page.id;
        history
            .apply(
                &mut scene,
                Op::AddPage {
                    page: page.clone(),
                    at: 0,
                },
            )
            .unwrap();
        history.undo(&mut scene).unwrap();
        scene.pages.insert(page_id, page);
        let epoch_before = history.epoch();

        assert!(history.redo(&mut scene).is_err());
        assert_eq!(history.redo_stack.len(), 1);
        assert_eq!(history.epoch(), epoch_before);
    }

    #[test]
    fn oversized_torn_tail_is_truncated_and_appends_survive() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        let base = Scene::default();
        let mut expected = base.clone();
        let mut history = History::open(&path, 0).unwrap();
        history.apply(&mut expected, add_page("p1")).unwrap();
        history.apply(&mut expected, add_page("p2")).unwrap();
        drop(history);

        let valid_len = std::fs::metadata(&path).unwrap().len();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&u32::MAX.to_le_bytes()).unwrap();
        file.write_all(&[1, 2, 3]).unwrap();
        file.flush().unwrap();

        let mut replayed = base.clone();
        assert_eq!(replay(&path, 0, &mut replayed).unwrap(), 2);
        assert_same_scene(&replayed, &expected);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), valid_len);

        let mut history = History::open(&path, 2).unwrap();
        history.apply(&mut replayed, add_page("p3")).unwrap();
        drop(history);
        let mut replayed_again = base;
        assert_eq!(replay(&path, 0, &mut replayed_again).unwrap(), 3);
        assert_same_scene(&replayed_again, &replayed);
    }

    #[test]
    fn torn_tail_on_headerless_legacy_log_truncates() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        let frames = [
            LogFrame {
                epoch: 1,
                op: add_page("legacy-1"),
            },
            LogFrame {
                epoch: 2,
                op: add_page("legacy-2"),
            },
        ];
        write_headerless_frames(&path, &frames);
        let valid_len = std::fs::metadata(&path).unwrap().len();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&200u32.to_le_bytes()).unwrap();
        file.write_all(&[1, 2, 3]).unwrap();
        file.flush().unwrap();
        drop(file);

        let base = Scene::default();
        let mut expected = base.clone();
        for frame in &frames {
            let mut op = frame.op.clone();
            op.apply(&mut expected).unwrap();
        }
        let mut replayed = base;
        assert_eq!(replay(&path, 0, &mut replayed).unwrap(), 2);
        assert_same_scene(&replayed, &expected);
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            valid_len + (HISTORY_LOG_MAGIC.len() + 2) as u64
        );
    }

    #[test]
    fn failed_log_write_discards_buffer_and_repairs_tail_before_retry() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        let base = Scene::default();
        let mut scene = base.clone();
        let mut history = History::open(&path, 0).unwrap();
        history
            .apply(
                &mut scene,
                Op::AddPage {
                    page: Page::new("committed", 100, 100),
                    at: 0,
                },
            )
            .unwrap();
        let committed = std::fs::read(&path).unwrap();
        let before = postcard::to_allocvec(&scene).unwrap();
        // Model a partially written tail, then force flush to fail using a real
        // read-only file handle. No production-only fault injection is needed.
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[8, 0])
            .unwrap();
        history.log = BufWriter::new(File::open(&path).unwrap());
        let op = Op::AddPage {
            page: Page::new("retry", 100, 100),
            at: 1,
        };
        assert!(history.apply(&mut scene, op.clone()).is_err());
        assert_eq!(postcard::to_allocvec(&scene).unwrap(), before);
        assert_eq!(history.epoch(), 1);
        assert_eq!(history.committed_len, committed.len() as u64);
        assert_eq!(history.undo_stack.len(), 1);
        assert!(history.redo_stack.is_empty());
        assert!(!history.poisoned);
        assert_eq!(std::fs::read(&path).unwrap(), committed);
        assert_eq!(history.apply(&mut scene, op).unwrap(), 2);
        drop(history);
        let mut replayed = base;
        assert_eq!(replay(&path, 0, &mut replayed).unwrap(), 2);
        assert_same_scene(&replayed, &scene);
    }

    #[test]
    fn failed_log_rollback_blocks_writes_until_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        let base = Scene::default();
        let mut scene = base.clone();
        let mut history = History::open(&path, 0).unwrap();
        history
            .apply(
                &mut scene,
                Op::AddPage {
                    page: Page::new("committed", 100, 100),
                    at: 0,
                },
            )
            .unwrap();
        let committed = std::fs::read(&path).unwrap();
        let before = postcard::to_allocvec(&scene).unwrap();
        history.log = BufWriter::new(File::open(&path).unwrap());
        history.log_path = dir.path().join("missing/history.log");
        let op = Op::AddPage {
            page: Page::new("retry", 100, 100),
            at: 1,
        };
        assert!(
            history
                .apply(&mut scene, op.clone())
                .unwrap_err()
                .to_string()
                .contains("rollback also failed")
        );
        assert!(history.poisoned);
        history.log_path = path.clone();
        assert!(
            history
                .apply(&mut scene, op.clone())
                .unwrap_err()
                .to_string()
                .contains("previously failed")
        );
        assert_eq!(postcard::to_allocvec(&scene).unwrap(), before);
        assert_eq!(history.epoch(), 1);
        assert_eq!(history.committed_len, committed.len() as u64);
        assert_eq!(history.undo_stack.len(), 1);
        assert!(history.redo_stack.is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), committed);
        drop(history);
        let mut replayed = base;
        assert_eq!(replay(&path, 0, &mut replayed).unwrap(), 1);
        assert_same_scene(&replayed, &scene);
        let mut reopened = History::open(&path, 1).unwrap();
        assert_eq!(reopened.apply(&mut replayed, op).unwrap(), 2);
    }

    #[test]
    fn v1_text_patch_replays_and_migrates_to_current() {
        #[derive(Serialize)]
        struct FrameV1 {
            epoch: u64,
            op: OpV1,
        }

        // Only the selected variant's payload is serialized; the seven unit
        // placeholders preserve UpdateNode's historical enum index.
        #[allow(dead_code)]
        #[derive(Serialize)]
        enum OpV1 {
            UpdateProjectMeta,
            AddPage,
            RemovePage,
            UpdatePage,
            ReorderPages,
            AddNode,
            RemoveNode,
            UpdateNode {
                page: PageId,
                id: NodeId,
                patch: NodePatchV1,
                prev: NodePatchV1,
            },
            ReorderNodes,
            Batch,
        }

        #[derive(Default, Serialize)]
        struct NodePatchV1 {
            transform: Option<Transform>,
            visible: Option<bool>,
            data: Option<NodeDataPatchV1>,
        }

        #[allow(dead_code)]
        #[derive(Serialize)]
        enum NodeDataPatchV1 {
            Text(TextDataPatchV1),
            Image,
            Mask,
        }

        #[derive(Default, Serialize)]
        struct TextDataPatchV1 {
            confidence: Option<f32>,
            source_lang: Option<Option<String>>,
            source_direction: Option<Option<TextDirection>>,
            rendered_direction: Option<Option<TextDirection>>,
            line_polygons: Option<Option<Vec<[[f32; 2]; 4]>>>,
            rotation_deg: Option<Option<f32>>,
            detected_font_size_px: Option<Option<f32>>,
            detector: Option<Option<String>>,
            text: Option<Option<String>>,
            translation: Option<Option<String>>,
            style: Option<Option<TextStyle>>,
            font_prediction: Option<Option<FontPrediction>>,
            sprite: Option<Option<BlobRef>>,
            sprite_transform: Option<Option<Transform>>,
            rendered_font_size_px: Option<Option<f32>>,
            rendered_text_color: Option<Option<[u8; 4]>>,
            lock_layout_box: Option<bool>,
        }

        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        let mut scene = Scene::default();
        let mut page = Page::new("p", 100, 100);
        let page_id = page.id;
        let node_id = NodeId::new();
        page.nodes.insert(
            node_id,
            Node {
                id: node_id,
                transform: Transform::default(),
                visible: true,
                kind: NodeKind::Text(TextData::default()),
            },
        );
        scene.pages.insert(page_id, page);

        let frame = FrameV1 {
            epoch: 1,
            op: OpV1::UpdateNode {
                page: page_id,
                id: node_id,
                patch: NodePatchV1 {
                    data: Some(NodeDataPatchV1::Text(TextDataPatchV1 {
                        translation: Some(Some("formatted later".to_string())),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                prev: NodePatchV1::default(),
            },
        };
        let body = postcard::to_allocvec(&frame).unwrap();
        let mut bytes = HISTORY_LOG_MAGIC.to_vec();
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&body);
        std::fs::write(&path, bytes).unwrap();

        assert_eq!(replay(&path, 0, &mut scene).unwrap(), 1);
        let NodeKind::Text(text) = &scene.pages[&page_id].nodes[&node_id].kind else {
            panic!("expected text node");
        };
        assert_eq!(text.translation.as_deref(), Some("formatted later"));
        assert!(text.style_ranges.is_empty());
        assert_eq!(text.writing_direction, None);

        let migrated = std::fs::read(&path).unwrap();
        assert_eq!(&migrated[..4], &HISTORY_LOG_MAGIC);
        assert_eq!(
            u16::from_le_bytes([migrated[4], migrated[5]]),
            HISTORY_LOG_VERSION
        );
    }

    #[test]
    fn v2_rich_text_patch_replays_and_defaults_writing_direction() {
        #[derive(Serialize)]
        struct FrameV2 {
            epoch: u64,
            op: OpV2,
        }

        #[allow(dead_code)]
        #[derive(Serialize)]
        enum OpV2 {
            UpdateProjectMeta,
            AddPage,
            RemovePage,
            UpdatePage,
            ReorderPages,
            AddNode,
            RemoveNode,
            UpdateNode {
                page: PageId,
                id: NodeId,
                patch: NodePatchV2,
                prev: NodePatchV2,
            },
            ReorderNodes,
            Batch,
        }

        #[derive(Default, Serialize)]
        struct NodePatchV2 {
            transform: Option<Transform>,
            visible: Option<bool>,
            data: Option<NodeDataPatchV2>,
        }

        #[allow(dead_code)]
        #[derive(Serialize)]
        enum NodeDataPatchV2 {
            Text(TextDataPatchV2),
            Image,
            Mask,
        }

        #[derive(Default, Serialize)]
        struct TextDataPatchV2 {
            confidence: Option<f32>,
            source_lang: Option<Option<String>>,
            source_direction: Option<Option<TextDirection>>,
            rendered_direction: Option<Option<TextDirection>>,
            line_polygons: Option<Option<Vec<[[f32; 2]; 4]>>>,
            rotation_deg: Option<Option<f32>>,
            detected_font_size_px: Option<Option<f32>>,
            detector: Option<Option<String>>,
            text: Option<Option<String>>,
            translation: Option<Option<String>>,
            style: Option<Option<TextStyle>>,
            font_prediction: Option<Option<FontPrediction>>,
            sprite: Option<Option<BlobRef>>,
            sprite_transform: Option<Option<Transform>>,
            rendered_font_size_px: Option<Option<f32>>,
            rendered_text_color: Option<Option<[u8; 4]>>,
            lock_layout_box: Option<bool>,
            style_ranges: Option<Vec<TextStyleRange>>,
        }

        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        let mut scene = Scene::default();
        let mut page = Page::new("p", 100, 100);
        let page_id = page.id;
        let node_id = NodeId::new();
        page.nodes.insert(
            node_id,
            Node {
                id: node_id,
                transform: Transform::default(),
                visible: true,
                kind: NodeKind::Text(TextData::default()),
            },
        );
        scene.pages.insert(page_id, page);

        let style_range = TextStyleRange {
            start: 0,
            end: 9,
            style: TextRangeStyle {
                italic: Some(true),
                ..Default::default()
            },
        };
        let frame = FrameV2 {
            epoch: 1,
            op: OpV2::UpdateNode {
                page: page_id,
                id: node_id,
                patch: NodePatchV2 {
                    data: Some(NodeDataPatchV2::Text(TextDataPatchV2 {
                        translation: Some(Some("formatted later".to_string())),
                        style_ranges: Some(vec![style_range]),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                prev: NodePatchV2::default(),
            },
        };
        let body = postcard::to_allocvec(&frame).unwrap();
        let mut bytes = HISTORY_LOG_MAGIC.to_vec();
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&body);
        std::fs::write(&path, bytes).unwrap();

        assert_eq!(replay(&path, 0, &mut scene).unwrap(), 1);
        let NodeKind::Text(text) = &scene.pages[&page_id].nodes[&node_id].kind else {
            panic!("expected text node");
        };
        assert_eq!(text.translation.as_deref(), Some("formatted later"));
        assert_eq!(text.style_ranges, vec![style_range]);
        assert_eq!(text.writing_direction, None);

        let migrated = std::fs::read(&path).unwrap();
        assert_eq!(&migrated[..4], &HISTORY_LOG_MAGIC);
        assert_eq!(
            u16::from_le_bytes([migrated[4], migrated[5]]),
            HISTORY_LOG_VERSION
        );
    }

    #[test]
    fn collided_v2_history_preserves_writing_direction() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        let mut scene = Scene::default();
        let mut page = Page::new("p", 100, 100);
        let page_id = page.id;
        let node_id = NodeId::new();
        page.nodes.insert(
            node_id,
            Node {
                id: node_id,
                transform: Transform::default(),
                visible: true,
                kind: NodeKind::Text(TextData::default()),
            },
        );
        scene.pages.insert(page_id, page);

        let frame = LogFrame {
            epoch: 1,
            op: Op::UpdateNode {
                page: page_id,
                id: node_id,
                patch: koharu_core::NodePatch {
                    data: Some(koharu_core::NodeDataPatch::Text(
                        koharu_core::TextDataPatch {
                            writing_direction: Some(Some(TextDirection::Vertical)),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                },
                prev: Default::default(),
            },
        };
        let body = postcard::to_allocvec(&frame).unwrap();
        let mut bytes = HISTORY_LOG_MAGIC.to_vec();
        // Reproduce the faulty build: v3 frame payload under a v2 header.
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&body);
        std::fs::write(&path, bytes).unwrap();

        assert_eq!(replay(&path, 0, &mut scene).unwrap(), 1);
        let NodeKind::Text(text) = &scene.pages[&page_id].nodes[&node_id].kind else {
            panic!("expected text node");
        };
        assert_eq!(text.writing_direction, Some(TextDirection::Vertical));

        let migrated = std::fs::read(&path).unwrap();
        assert_eq!(&migrated[..4], &HISTORY_LOG_MAGIC);
        assert_eq!(
            u16::from_le_bytes([migrated[4], migrated[5]]),
            HISTORY_LOG_VERSION
        );
    }
}
