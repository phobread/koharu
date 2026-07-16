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
use koharu_core::{Op, Scene};
use serde::{Deserialize, Serialize};

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
const HISTORY_LOG_VERSION: u16 = 1;

// ---------------------------------------------------------------------------
// Log frames
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct LogFrame {
    epoch: u64,
    op: Op,
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
                HISTORY_LOG_VERSION => Some(version),
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
        let decoded = match log_version {
            None | Some(HISTORY_LOG_VERSION) => postcard::from_bytes::<LogFrame>(&buf),
            // When v2 changes the layout, decode v1 via compat::LogFrameV1 here.
            Some(_) => unreachable!("unsupported history log version was rejected above"),
        };
        let frame = match decoded {
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
        if frame.epoch > epoch {
            let mut op = frame.op;
            op.apply(scene).context("replay op")?;
            epoch = frame.epoch;
        }
    }
    if discarded_tail && std::fs::metadata(log_path)?.len() > valid_len {
        tracing::warn!(
            path = %log_path.display(),
            valid_len,
            "truncating invalid trailing bytes from history log"
        );
        drop(reader);
        let file = OpenOptions::new()
            .write(true)
            .open(log_path)
            .with_context(|| format!("open history log {} for tail repair", log_path.display()))?;
        file.set_len(valid_len)
            .context("truncate invalid history log tail")?;
        file.sync_all().context("sync repaired history log")?;
    }
    Ok(epoch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use koharu_core::{BlobRef, ImageData, ImageRole, Node, NodeId, NodeKind, Page, Transform};
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
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&[1, 2, 3]);
        std::fs::write(&path, bytes).unwrap();

        let err = replay(&path, 0, &mut Scene::default()).unwrap_err();
        assert!(format!("{err:#}").contains("unsupported history.log format version 2"));
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

        let base = Scene::default();
        let mut expected = base.clone();
        for frame in &frames {
            let mut op = frame.op.clone();
            op.apply(&mut expected).unwrap();
        }
        let mut replayed = base;
        assert_eq!(replay(&path, 0, &mut replayed).unwrap(), 2);
        assert_same_scene(&replayed, &expected);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), valid_len);
    }
}
