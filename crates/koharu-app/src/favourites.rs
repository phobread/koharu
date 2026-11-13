//! Favourites: a star on a project and on pages inside it.
//!
//! Kept in a small side file, `.khrproj/favourites.json`, so the saved scene
//! and its format stay untouched:
//!
//! ```json
//! { "version": 1, "project": true, "pages": ["<page uuid>", "..."] }
//! ```
//!
//! - A missing or unreadable file means "no favourites"; it never stops a
//!   project from listing or opening.
//! - Pages are named by their stable id. Ids of pages that no longer exist
//!   stay in the file (deleting a page can be undone) and are left out when
//!   the stars are read for a scene, see [`Favourites::pages_in`].
//! - Stars are not edits: no history entry, no undo.
//! - The file is inside the project folder, so a `.khr` export carries it.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{Context, Result};
use atomicwrites::{AtomicFile, OverwriteBehavior};
use camino::Utf8Path;
use koharu_core::PageId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const FAVOURITES_FILE: &str = "favourites.json";
const FILE_VERSION: u32 = 1;

/// One read-change-write at a time, so two quick clicks can't lose a star.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// What is starred in one project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Favourites {
    /// The project itself, for the project list.
    pub project: bool,
    /// Starred pages, by id. May name pages that were deleted since.
    pub pages: BTreeSet<PageId>,
}

impl Favourites {
    /// The starred pages among `known`, in the order given (page order).
    /// Ids the scene doesn't have are ignored.
    pub fn pages_in(&self, known: impl IntoIterator<Item = PageId>) -> Vec<PageId> {
        known
            .into_iter()
            .filter(|id| self.pages.contains(id))
            .collect()
    }

    fn is_empty(&self) -> bool {
        !self.project && self.pages.is_empty()
    }
}

#[derive(Serialize, Deserialize)]
struct OnDisk {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    project: bool,
    #[serde(default)]
    pages: Vec<String>,
}

fn parse(bytes: &[u8]) -> Favourites {
    let Ok(file) = serde_json::from_slice::<OnDisk>(bytes) else {
        return Favourites::default();
    };
    Favourites {
        project: file.project,
        pages: file
            .pages
            .iter()
            .filter_map(|id| Uuid::parse_str(id).ok().map(PageId))
            .collect(),
    }
}

/// The project's favourites. Never fails: no file, a file that can't be
/// read or one that isn't valid all mean "nothing starred".
pub fn load(project_dir: &Utf8Path) -> Favourites {
    fs::read(project_dir.join(FAVOURITES_FILE).as_std_path())
        .map(|bytes| parse(&bytes))
        .unwrap_or_default()
}

/// Star or unstar the project itself.
pub fn set_project(project_dir: &Utf8Path, starred: bool) -> Result<Favourites> {
    update(project_dir, |favourites| favourites.project = starred)
}

/// Star or unstar one page.
pub fn set_page(project_dir: &Utf8Path, page: PageId, starred: bool) -> Result<Favourites> {
    update(project_dir, |favourites| {
        if starred {
            favourites.pages.insert(page);
        } else {
            favourites.pages.remove(&page);
        }
    })
}

fn update(project_dir: &Utf8Path, change: impl FnOnce(&mut Favourites)) -> Result<Favourites> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = project_dir.join(FAVOURITES_FILE);
    // Unlike `load`, a file that is there but can't be read stops the write:
    // carrying on would replace the stars it holds with just this one. (A
    // file that reads but isn't valid holds none, and is replaced.)
    let mut favourites = match fs::read(path.as_std_path()) {
        Ok(bytes) => parse(&bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Favourites::default(),
        Err(e) => return Err(anyhow::Error::new(e).context(format!("read {path}"))),
    };
    let before = favourites.clone();
    change(&mut favourites);
    if favourites == before {
        return Ok(favourites);
    }

    // The project list sorts by the folder's modified time and shows it as
    // the last edit; a star is not an edit, so the time is put back.
    let folder_time = fs::metadata(project_dir.as_std_path())
        .and_then(|meta| meta.modified())
        .ok();
    let written = write(&path, &favourites);
    if let Some(time) = folder_time {
        let _ = set_dir_modified(project_dir, time);
    }
    written?;
    Ok(favourites)
}

fn write(path: &Utf8Path, favourites: &Favourites) -> Result<()> {
    if favourites.is_empty() {
        // Nothing starred: no file, as before the first star.
        return match fs::remove_file(path.as_std_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(anyhow::Error::new(e).context(format!("remove {path}"))),
        };
    }
    let file = OnDisk {
        version: FILE_VERSION,
        project: favourites.project,
        pages: favourites.pages.iter().map(|id| id.0.to_string()).collect(),
    };
    let mut bytes = serde_json::to_vec_pretty(&file).context("encode favourites")?;
    bytes.push(b'\n');
    // Temp file, then rename: a crash leaves the old file or the new one.
    AtomicFile::new(path.as_std_path(), OverwriteBehavior::AllowOverwrite)
        .write(|f| f.write_all(&bytes))
        .with_context(|| format!("write {path}"))?;
    Ok(())
}

fn set_dir_modified(dir: &Utf8Path, time: SystemTime) -> io::Result<()> {
    #[cfg(windows)]
    let handle = {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
        // Needed to open a directory at all.
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        fs::OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(dir.as_std_path())?
    };
    #[cfg(not(windows))]
    let handle = fs::File::open(dir.as_std_path())?;
    handle.set_modified(time)
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;

    use super::*;

    fn project() -> (tempfile::TempDir, Utf8PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().join("p.khrproj")).unwrap();
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("project.toml"), b"name = \"p\"\n").unwrap();
        fs::write(dir.join("scene.bin"), b"scene").unwrap();
        (temp, dir)
    }

    fn names(dir: &Utf8Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn stars_round_trip_through_the_side_file() {
        let (_temp, dir) = project();
        let (a, b) = (PageId::new(), PageId::new());

        set_project(&dir, true).unwrap();
        set_page(&dir, a, true).unwrap();
        let after = set_page(&dir, b, true).unwrap();
        assert!(after.project);
        assert_eq!(after.pages, BTreeSet::from([a, b]));
        assert_eq!(load(&dir), after);

        // The file is what the module comment says it is.
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.join(FAVOURITES_FILE)).unwrap()).unwrap();
        assert_eq!(json["version"], 1);
        assert_eq!(json["project"], true);
        assert_eq!(json["pages"].as_array().unwrap().len(), 2);

        let after = set_page(&dir, a, false).unwrap();
        assert_eq!(after.pages, BTreeSet::from([b]));
        assert_eq!(load(&dir), after);
        // Setting what is already set changes nothing.
        assert_eq!(set_page(&dir, b, true).unwrap(), after);
    }

    #[test]
    fn missing_file_means_nothing_starred() {
        let (_temp, dir) = project();
        assert_eq!(load(&dir), Favourites::default());
        // Unstarring with no file doesn't create one.
        set_project(&dir, false).unwrap();
        set_page(&dir, PageId::new(), false).unwrap();
        assert_eq!(names(&dir), ["project.toml", "scene.bin"]);
        // Nor does a folder that isn't there fail the read.
        assert_eq!(load(&dir.join("gone")), Favourites::default());
    }

    #[test]
    fn corrupt_file_means_nothing_starred_and_is_replaced_by_the_next_star() {
        let (_temp, dir) = project();
        let path = dir.join(FAVOURITES_FILE);
        for junk in [
            &b"{ not json"[..],
            b"",
            b"[1, 2]",
            b"{\"pages\": 5}",
            b"\xff\xfe\x00",
        ] {
            fs::write(&path, junk).unwrap();
            assert_eq!(load(&dir), Favourites::default(), "{junk:?}");
        }

        let page = PageId::new();
        let after = set_page(&dir, page, true).unwrap();
        assert_eq!(after.pages, BTreeSet::from([page]));
        assert_eq!(load(&dir), after);
    }

    #[test]
    fn unknown_and_malformed_page_ids_are_ignored() {
        let (_temp, dir) = project();
        let (kept, deleted, plain) = (PageId::new(), PageId::new(), PageId::new());
        fs::write(
            dir.join(FAVOURITES_FILE),
            format!(
                "{{\"project\": false, \"pages\": [\"{}\", \"not-an-id\", \"{}\"], \"later\": 1}}",
                deleted.0, kept.0
            ),
        )
        .unwrap();

        let favourites = load(&dir);
        assert_eq!(favourites.pages, BTreeSet::from([kept, deleted]));
        // Only pages the scene still has come back, in page order.
        assert_eq!(favourites.pages_in([plain, kept]), [kept]);
        assert!(Favourites::default().pages_in([plain, kept]).is_empty());
    }

    #[test]
    fn writing_leaves_only_the_side_file_and_removes_it_when_nothing_is_starred() {
        let (_temp, dir) = project();
        let page = PageId::new();
        let scene_before = fs::read(dir.join("scene.bin")).unwrap();

        set_page(&dir, page, true).unwrap();
        set_project(&dir, true).unwrap();
        // No temp file or temp folder is left beside it.
        assert_eq!(
            names(&dir),
            ["favourites.json", "project.toml", "scene.bin"]
        );
        assert_eq!(fs::read(dir.join("scene.bin")).unwrap(), scene_before);

        set_project(&dir, false).unwrap();
        set_page(&dir, page, false).unwrap();
        assert_eq!(names(&dir), ["project.toml", "scene.bin"]);
    }

    #[test]
    fn starring_keeps_the_folder_modified_time() {
        let (_temp, dir) = project();
        let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        set_dir_modified(&dir, old).unwrap();
        let modified = || fs::metadata(&dir).unwrap().modified().unwrap();
        assert_eq!(modified(), old);

        set_project(&dir, true).unwrap();
        assert_eq!(modified(), old);
        set_page(&dir, PageId::new(), true).unwrap();
        assert_eq!(modified(), old);
        set_project(&dir, false).unwrap();
        assert_eq!(modified(), old);
    }

    #[test]
    fn stars_travel_in_a_khr_export() {
        let (temp, dir) = project();
        let page = PageId::new();
        set_project(&dir, true).unwrap();
        let starred = set_page(&dir, page, true).unwrap();

        let root = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let bytes = crate::archive::export_khr_bytes(&dir).unwrap();
        let restored = root.join("restored.khrproj");
        crate::archive::import_khr_bytes(&bytes, &restored).unwrap();
        assert_eq!(load(&restored), starred);
    }
}
