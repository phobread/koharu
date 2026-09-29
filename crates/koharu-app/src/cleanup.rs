//! Free up space: delete blobs that no closed project can reach any more.
//!
//! Blobs are content-addressed and never deleted when a layer is replaced, so
//! every re-inpaint or re-render leaves the previous full-page image behind.
//! Undo history ends when a project closes (the log is compacted into
//! `scene.bin` and the undo stack lives in memory), so in a closed project a
//! blob that no metadata file mentions is unreachable.
//!
//! Safety rules:
//! - Only closed projects: the project's `.lock` is held from the scan to
//!   the last deletion, so it cannot be opened meanwhile; a project that is
//!   open (here or in another instance) or never opened is skipped.
//! - "Mentioned" is a byte scan: every 64-hex-digit run in every file of the
//!   project outside `blobs/`, `cache/` and `.lock` (scene, whole undo log incl. any
//!   torn tail, project.toml, ...). Unknown content only keeps more.
//! - The project is skipped unless its scene decodes and every blob the
//!   decoded scene refers to was found by the scan, the undo log has a known
//!   format, and nothing in it is a link or junction.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::path::Path;

use anyhow::{Context, Result};
use camino::Utf8Path;
use fs4::FileExt;

use crate::projects::PROJECT_EXT;

/// What cleaning (or a dry run of it) did to one project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectCleanup {
    pub id: String,
    /// Unused blobs removed (or removable, in a dry run).
    pub blobs: u64,
    pub bytes: u64,
    /// Removals that failed; those files stay.
    pub failed: u64,
    /// Why the project was left alone.
    pub skipped: Option<SkipReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// Open here or elsewhere (its undo history may still need old blobs).
    Open,
    /// Unreadable, unknown format, or not a plain folder.
    Unreadable(String),
}

/// Clean every closed project under `projects_dir`, skipping `open` (the
/// project open in this app). `delete: false` only measures.
pub fn clean_projects(
    projects_dir: &Utf8Path,
    open: Option<&Utf8Path>,
    delete: bool,
) -> Result<Vec<ProjectCleanup>> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(projects_dir.as_std_path()) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(anyhow::Error::new(e).context(format!("read {projects_dir}"))),
    };
    let suffix = format!(".{PROJECT_EXT}");
    let mut dirs: Vec<_> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let id = name.strip_suffix(&suffix)?.to_string();
            Some((id, projects_dir.join(&name)))
        })
        .collect();
    dirs.sort();
    for (id, dir) in dirs {
        if open.is_some_and(|open| crate::projects::same_project_dir(open, &dir)) {
            out.push(ProjectCleanup {
                id,
                skipped: Some(SkipReason::Open),
                ..Default::default()
            });
            continue;
        }
        out.push(clean_project(&id, &dir, delete));
    }
    Ok(out)
}

/// Clean one project folder. Never fails: problems become a skip.
pub fn clean_project(id: &str, dir: &Utf8Path, delete: bool) -> ProjectCleanup {
    let mut report = ProjectCleanup {
        id: id.to_string(),
        ..Default::default()
    };
    match clean_locked(dir, delete, &mut report) {
        Ok(()) => {}
        Err(Skip::Open) => report.skipped = Some(SkipReason::Open),
        Err(Skip::Unreadable(why)) => {
            report.blobs = 0;
            report.bytes = 0;
            report.skipped = Some(SkipReason::Unreadable(why));
        }
    }
    report
}

enum Skip {
    Open,
    Unreadable(String),
}

impl From<anyhow::Error> for Skip {
    fn from(e: anyhow::Error) -> Self {
        Skip::Unreadable(format!("{e:#}"))
    }
}

fn unreadable(why: impl Into<String>) -> Skip {
    Skip::Unreadable(why.into())
}

fn clean_locked(dir: &Utf8Path, delete: bool, report: &mut ProjectCleanup) -> Result<(), Skip> {
    if !plain_dir(dir.as_std_path())? {
        return Err(unreadable("not a plain folder"));
    }
    // Never create the lock file: a folder without one was never opened (or
    // is still being imported).
    let lock_path = dir.join(".lock");
    let lock = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path.as_std_path())
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(unreadable("never opened"));
        }
        Err(_) => return Err(Skip::Open),
    };
    if FileExt::try_lock(&lock).is_err() {
        return Err(Skip::Open);
    }
    let result = scan_and_delete(dir, delete, report);
    drop(lock);
    result
}

fn scan_and_delete(dir: &Utf8Path, delete: bool, report: &mut ProjectCleanup) -> Result<(), Skip> {
    let referenced = referenced_hashes(dir)?;

    let blobs_dir = dir.join("blobs");
    if !plain_dir(blobs_dir.as_std_path())? {
        return Err(unreadable("blobs is not a plain folder"));
    }
    let mut unused = Vec::new();
    for shard in fs::read_dir(blobs_dir.as_std_path()).context("read blobs")? {
        let shard = shard.context("read blobs")?;
        let prefix = shard.file_name().to_str().unwrap_or_default().to_string();
        let meta = fs::symlink_metadata(shard.path()).context("read blobs")?;
        if is_link(&meta) {
            return Err(unreadable("link inside blobs"));
        }
        if !meta.is_dir() || prefix.len() != 2 || !is_lower_hex(&prefix) {
            continue;
        }
        for file in fs::read_dir(shard.path()).context("read blobs")? {
            let file = file.context("read blobs")?;
            let meta = fs::symlink_metadata(file.path()).context("read blobs")?;
            if is_link(&meta) {
                return Err(unreadable("link inside blobs"));
            }
            let rest = file.file_name().to_str().unwrap_or_default().to_string();
            // Only finished blobs; anything else (e.g. a write's temp file)
            // is left alone.
            if !meta.is_file() || rest.len() != 62 || !is_lower_hex(&rest) {
                continue;
            }
            if !referenced.contains(&format!("{prefix}{rest}")) {
                unused.push((file.path(), meta.len()));
            }
        }
    }

    for (path, len) in unused {
        if !delete {
            report.blobs += 1;
            report.bytes += len;
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => {
                report.blobs += 1;
                report.bytes += len;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => report.failed += 1,
        }
    }
    Ok(())
}

/// Every blob hash any file outside `blobs/` and `cache/` mentions, after
/// checking the project's metadata is understood.
fn referenced_hashes(dir: &Utf8Path) -> Result<HashSet<String>, Skip> {
    let scene_bytes = fs::read(dir.join("scene.bin").as_std_path())
        .map_err(|e| unreadable(format!("scene.bin: {e}")))?;
    let scene = crate::session::decode_scene(&scene_bytes)?;
    let log = dir.join("history.log");
    match fs::read(log.as_std_path()) {
        Ok(bytes) if !crate::history::log_format_known(&bytes) => {
            return Err(unreadable("history.log written by a newer build"));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(unreadable(format!("history.log: {e}"))),
    }

    let mut referenced = HashSet::new();
    collect_roots(dir.as_std_path(), true, &mut referenced)?;

    // Cross-check the byte scan against the decoded scene, in another
    // encoding: a blob the scene uses must have been found.
    let json = serde_json::to_vec(&scene).context("encode scene")?;
    let mut in_scene = HashSet::new();
    hex_windows(&json, &mut in_scene);
    if !in_scene.is_subset(&referenced) {
        return Err(unreadable("scene blob references not found by the scan"));
    }
    Ok(referenced)
}

fn collect_roots(dir: &Path, top: bool, referenced: &mut HashSet<String>) -> Result<(), Skip> {
    for entry in fs::read_dir(dir).context("read project folder")? {
        let entry = entry.context("read project folder")?;
        let name = entry.file_name();
        // `.lock` is held (and byte-range locked) by this clean-up; it is
        // empty and refers to nothing.
        if top && (name == "blobs" || name == "cache" || name == ".lock") {
            continue;
        }
        let meta = fs::symlink_metadata(entry.path()).context("read project folder")?;
        if is_link(&meta) {
            return Err(unreadable("link inside the project"));
        }
        if meta.is_dir() {
            collect_roots(&entry.path(), false, referenced)?;
        } else if meta.is_file() {
            let bytes = fs::read(entry.path())
                .with_context(|| format!("read {}", entry.path().display()))?;
            hex_windows(&bytes, referenced);
        }
    }
    Ok(())
}

/// Add every 64-character window of lowercase hex digits in `bytes`: the
/// hashes a blob name could match, wherever a run of hex digits starts.
fn hex_windows(bytes: &[u8], out: &mut HashSet<String>) {
    let mut run_start = None;
    for i in 0..=bytes.len() {
        let hex = i < bytes.len() && matches!(bytes[i], b'0'..=b'9' | b'a'..=b'f');
        match (hex, run_start) {
            (true, None) => run_start = Some(i),
            (false, Some(start)) => {
                let run = &bytes[start..i];
                for window in run.windows(64) {
                    // All bytes are ASCII hex digits.
                    out.insert(String::from_utf8_lossy(window).into_owned());
                }
                run_start = None;
            }
            _ => {}
        }
    }
}

fn is_lower_hex(s: &str) -> bool {
    s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn plain_dir(path: &Path) -> Result<bool, Skip> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(meta.is_dir() && !is_link(&meta)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(unreadable(format!("{}: {e}", path.display()))),
    }
}

fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Junctions and other reparse points must not be traversed either.
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use koharu_core::{ImageData, ImageRole, Node, NodeId, NodeKind, Op, Page, Transform};
    use tempfile::tempdir;

    use super::*;
    use crate::blobs::BlobStore;
    use crate::session::ProjectSession;

    fn image_page(blob: koharu_core::BlobRef) -> Page {
        let mut page = Page::new("001.jpg", 4, 4);
        let id = NodeId::new();
        page.nodes.insert(
            id,
            Node {
                id,
                transform: Transform::default(),
                visible: true,
                kind: NodeKind::Image(ImageData {
                    role: ImageRole::Source,
                    blob,
                    opacity: 1.0,
                    natural_width: 4,
                    natural_height: 4,
                    name: None,
                }),
            },
        );
        page
    }

    fn blob_path(dir: &Utf8Path, hash: &str) -> Utf8PathBuf {
        dir.join("blobs").join(&hash[..2]).join(&hash[2..])
    }

    /// A closed project whose scene uses `kept`; `junk` blobs are unused.
    fn closed_project(dir: &Utf8Path) -> (String, Vec<String>) {
        let session = ProjectSession::create(dir, "p").unwrap();
        let kept = session.blobs.put_bytes(b"kept page").unwrap();
        let junk = (0..3)
            .map(|i| {
                session
                    .blobs
                    .put_bytes(format!("old render {i}").as_bytes())
                    .unwrap()
                    .0
            })
            .collect();
        session
            .apply(Op::AddPage {
                page: image_page(kept.clone()),
                at: 0,
            })
            .unwrap();
        session.compact().unwrap();
        (kept.0, junk)
    }

    fn project_dir() -> (tempfile::TempDir, Utf8PathBuf) {
        let tmp = tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(tmp.path().join("p.khrproj")).unwrap();
        (tmp, dir)
    }

    #[test]
    fn measures_then_removes_only_blobs_nothing_refers_to() {
        let (_tmp, dir) = project_dir();
        let (kept, junk) = closed_project(&dir);
        let junk_bytes: u64 = junk
            .iter()
            .map(|h| fs::metadata(blob_path(&dir, h)).unwrap().len())
            .sum();

        let dry = clean_project("p", &dir, false);
        assert_eq!((dry.blobs, dry.bytes, dry.skipped), (3, junk_bytes, None));
        assert!(junk.iter().all(|h| blob_path(&dir, h).exists()));

        let done = clean_project("p", &dir, true);
        assert_eq!((done.blobs, done.bytes, done.failed), (3, junk_bytes, 0));
        assert!(junk.iter().all(|h| !blob_path(&dir, h).exists()));
        assert!(blob_path(&dir, &kept).exists());

        // Still opens with its page and image, and nothing is left to do.
        let session = ProjectSession::open(&dir).unwrap();
        let image = session
            .scene
            .read()
            .pages
            .values()
            .next()
            .unwrap()
            .nodes
            .len();
        assert_eq!(image, 1);
        let bytes = session
            .blobs
            .get_bytes(&koharu_core::BlobRef::new(kept))
            .unwrap();
        assert_eq!(bytes, b"kept page");
        drop(session);
        assert_eq!(clean_project("p", &dir, true).blobs, 0);
    }

    #[test]
    fn keeps_blobs_the_undo_log_still_mentions() {
        let (_tmp, dir) = project_dir();
        let (_, junk) = closed_project(&dir);
        {
            // Applied after the last compaction: only history.log has it.
            let session = ProjectSession::open(&dir).unwrap();
            let blob = koharu_core::BlobRef::new(junk[0].clone());
            session
                .apply(Op::AddPage {
                    page: image_page(blob),
                    at: 1,
                })
                .unwrap();
        }
        let done = clean_project("p", &dir, true);
        assert_eq!(done.blobs, 2);
        assert!(blob_path(&dir, &junk[0]).exists());
    }

    #[test]
    fn skips_open_projects_and_unknown_formats() {
        let (_tmp, dir) = project_dir();
        let (_, junk) = closed_project(&dir);

        let session = ProjectSession::open(&dir).unwrap();
        assert_eq!(
            clean_project("p", &dir, true).skipped,
            Some(SkipReason::Open)
        );
        drop(session);
        assert!(junk.iter().all(|h| blob_path(&dir, h).exists()));

        // A newer build's undo log: leave the project alone.
        let log = dir.join("history.log");
        let original = fs::read(&log).unwrap();
        let mut newer = b"KHLG".to_vec();
        newer.extend_from_slice(&99u16.to_le_bytes());
        fs::write(&log, &newer).unwrap();
        assert!(matches!(
            clean_project("p", &dir, true).skipped,
            Some(SkipReason::Unreadable(_))
        ));
        fs::write(&log, &original).unwrap();

        // A scene that doesn't decode: same.
        let scene = dir.join("scene.bin");
        let good = fs::read(&scene).unwrap();
        fs::write(&scene, b"KSCN\x63\x00garbage").unwrap();
        assert!(matches!(
            clean_project("p", &dir, true).skipped,
            Some(SkipReason::Unreadable(_))
        ));
        fs::write(&scene, &good).unwrap();
        assert!(junk.iter().all(|h| blob_path(&dir, h).exists()));

        // Never opened (no lock file): skipped, and no lock file is made.
        fs::remove_file(dir.join(".lock")).unwrap();
        assert!(matches!(
            clean_project("p", &dir, true).skipped,
            Some(SkipReason::Unreadable(_))
        ));
        assert!(!dir.join(".lock").exists());
    }

    #[test]
    fn leaves_files_that_are_not_finished_blobs() {
        let (_tmp, dir) = project_dir();
        closed_project(&dir);
        let shard = dir.join("blobs").join("ab");
        fs::create_dir_all(&shard).unwrap();
        let temp = shard.join(".tmpXYZ");
        fs::write(&temp, b"half written").unwrap();
        let odd = dir.join("blobs").join("notes.txt");
        fs::write(&odd, b"keep").unwrap();

        assert_eq!(clean_project("p", &dir, true).blobs, 3);
        assert!(temp.exists() && odd.exists());
    }

    #[test]
    fn clean_projects_skips_the_open_one_by_path() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        closed_project(&root.join("a.khrproj"));
        closed_project(&root.join("b.khrproj"));
        let _store = BlobStore::open(root.join("ignored").as_std_path()).unwrap();

        let open = root.join("b.khrproj");
        let out = clean_projects(&root, Some(open.as_path()), true).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].id.as_str(), out[0].blobs), ("a", 3));
        assert_eq!(
            (out[1].id.as_str(), &out[1].skipped),
            ("b", &Some(SkipReason::Open))
        );
    }

    #[test]
    fn finds_a_hash_inside_a_longer_run_of_hex_digits() {
        let hash = "a".repeat(63) + "b";
        let mut found = HashSet::new();
        hex_windows(format!("x0{hash}f@").as_bytes(), &mut found);
        assert!(found.contains(&hash));
    }
}
