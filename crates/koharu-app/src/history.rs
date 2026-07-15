//! Linear undo/redo history + append-only durable op log.
//!
//! Two concerns, deliberately separated:
//!   1. **Durability log** — `history.log`: each applied op fsynced before ack
//!      so a crash loses at most the op currently being written. The file starts
//!      with `"KHLG"` + a u16 LE format version; headerless logs are legacy v0.
//!      Future versions freeze old `LogFrame` layouts at the decode seam and
//!      upgrade them before replay.
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

/// Headerless files are legacy v0. `KHLG` as a u32 LE is about 1.1 GB, far
/// past the u32 frame-length guard, so a length prefix can never be mistaken
/// for the magic and the formats are unambiguous.
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
    epoch: u64,
    undo_stack: VecDeque<Op>,
    redo_stack: Vec<Op>,
    limit: usize,
}

impl History {
    /// Open the log at `path`, creating it if missing. Caller is expected to
    /// have already replayed any existing frames (see `Self::replay`).
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
        Ok(Self {
            log_path,
            log: BufWriter::new(file),
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
        op.apply(scene).context("apply op to scene")?;
        self.epoch += 1;
        self.write_frame(&op)?;
        self.push_undo(op);
        self.redo_stack.clear();
        Ok(self.epoch)
    }

    /// Undo the most recent op. Applies its inverse, records the inverse in
    /// the log, and moves the original onto the redo stack. Returns the new
    /// epoch + the inverse op that was just applied (so the RPC layer can
    /// broadcast it for clients to patch their mirrors without refetching).
    pub fn undo(&mut self, scene: &mut Scene) -> Result<Option<(u64, Op)>> {
        let Some(original) = self.undo_stack.pop_back() else {
            return Ok(None);
        };
        let mut inverse = original.inverse();
        inverse.apply(scene).context("apply inverse op")?;
        self.epoch += 1;
        self.write_frame(&inverse)?;
        let inverse_out = inverse.clone();
        self.redo_stack.push(original);
        Ok(Some((self.epoch, inverse_out)))
    }

    /// Re-apply the most recent undo. Symmetric with `undo`. Returns the new
    /// epoch + the op that was just re-applied.
    pub fn redo(&mut self, scene: &mut Scene) -> Result<Option<(u64, Op)>> {
        let Some(mut op) = self.redo_stack.pop() else {
            return Ok(None);
        };
        op.apply(scene).context("re-apply op")?;
        self.epoch += 1;
        self.write_frame(&op)?;
        let applied = op.clone();
        self.push_undo(op);
        Ok(Some((self.epoch, applied)))
    }

    /// Truncate the log after a snapshot has been committed.
    /// Caller must have already fsynced the snapshot file.
    pub fn truncate_log(&mut self) -> Result<()> {
        self.log.flush()?;
        self.log.get_ref().sync_all()?;
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
        Ok(())
    }

    // --- internals ---------------------------------------------------------

    fn write_frame(&mut self, op: &Op) -> Result<()> {
        let frame = LogFrame {
            epoch: self.epoch,
            op: op.clone(),
        };
        let bytes = postcard::to_allocvec(&frame).context("encode log frame")?;
        let len = u32::try_from(bytes.len()).context("log frame too large")?;
        self.log.write_all(&len.to_le_bytes())?;
        self.log.write_all(&bytes)?;
        self.log.flush()?;
        self.log.get_ref().sync_data()?;
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
                break;
            }
            Err(err) => {
                return Err(anyhow::Error::new(err).context("read log frame length"));
            }
        }
        let len = u32::from_le_bytes(len_buf) as usize;
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
                break;
            }
        };
        if frame.epoch > epoch {
            let mut op = frame.op;
            op.apply(scene).context("replay op")?;
            epoch = frame.epoch;
        }
    }
    // Seek to end so subsequent appends go after the last valid frame.
    let _ = reader.seek(SeekFrom::End(0));
    Ok(epoch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use koharu_core::Page;
    use tempfile::tempdir;

    fn add_page(name: &str) -> Op {
        Op::AddPage {
            page: Page::new(name, 800, 600),
            at: 0,
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
}
