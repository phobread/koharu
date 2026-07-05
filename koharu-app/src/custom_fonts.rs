//! User-imported fonts.
//!
//! Font files the user uploads are cached under `<app_data>/fonts/custom` so
//! they survive restarts. The faces themselves are registered in the
//! renderer's `FontBook` (see [`crate::renderer`]); this store owns the
//! directory and remembers which PostScript names came from custom files so
//! the API can label them [`koharu_core::FontSource::Custom`].

use std::sync::Mutex;

use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};

const FONT_EXTS: &[&str] = &[".ttf", ".otf", ".ttc", ".otc", ".woff", ".woff2"];

/// A font face imported from the user's disk.
#[derive(Clone, Debug)]
pub struct CustomFontFace {
    pub post_script_name: String,
    pub family_name: String,
}

/// Manages user-imported font files under `<app_data>/fonts/custom`.
pub struct CustomFontStore {
    dir: Utf8PathBuf,
    faces: Mutex<Vec<CustomFontFace>>,
}

impl CustomFontStore {
    pub fn new(app_data_root: &Utf8Path) -> Result<Self> {
        let dir = app_data_root.join("fonts").join("custom");
        std::fs::create_dir_all(dir.as_std_path()).context("failed to create custom fonts dir")?;
        Ok(Self {
            dir,
            faces: Mutex::new(Vec::new()),
        })
    }

    pub fn dir(&self) -> &Utf8Path {
        &self.dir
    }

    /// Font files currently on disk, sorted for a deterministic load order.
    pub fn files(&self) -> Vec<Utf8PathBuf> {
        let mut files: Vec<Utf8PathBuf> = std::fs::read_dir(self.dir.as_std_path())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
            .filter(|p| p.is_file() && has_font_ext(p.as_str()))
            .collect();
        files.sort();
        files
    }

    /// Record a face as custom. Idempotent by PostScript name.
    pub fn record(&self, face: CustomFontFace) {
        let mut faces = self.faces.lock().unwrap();
        if !faces
            .iter()
            .any(|f| f.post_script_name == face.post_script_name)
        {
            faces.push(face);
        }
    }

    /// Whether a PostScript name belongs to an imported font.
    pub fn is_custom(&self, post_script_name: &str) -> bool {
        self.faces
            .lock()
            .unwrap()
            .iter()
            .any(|f| f.post_script_name == post_script_name)
    }

    /// Persist uploaded bytes to disk under a sanitized filename.
    pub fn store_bytes(&self, filename: &str, bytes: &[u8]) -> Result<Utf8PathBuf> {
        let name = sanitize_filename(filename);
        let path = self.dir.join(&name);
        std::fs::write(path.as_std_path(), bytes)
            .with_context(|| format!("failed to write custom font {name}"))?;
        Ok(path)
    }
}

/// True if `name` ends with a recognized font-file extension.
fn has_font_ext(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    FONT_EXTS.iter().any(|ext| lower.ends_with(ext))
}

/// Strip any path components and replace characters that don't belong in a
/// filename, keeping a recognized font extension (defaulting to `.ttf` — the
/// loader parses by content, so the extension only drives the startup scan).
fn sanitize_filename(filename: &str) -> String {
    let base = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
    if cleaned.is_empty() {
        "font.ttf".to_string()
    } else if has_font_ext(&cleaned) {
        cleaned
    } else {
        format!("{cleaned}.ttf")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_paths_and_keeps_font_extensions() {
        assert_eq!(sanitize_filename("Comic Sans.otf"), "Comic Sans.otf");
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd.ttf");
        assert_eq!(sanitize_filename("C:\\Windows\\evil.ttf"), "evil.ttf");
        assert_eq!(sanitize_filename("we!rd/na*me.woff2"), "na_me.woff2");
        assert_eq!(sanitize_filename(""), "font.ttf");
        assert_eq!(sanitize_filename("..."), "font.ttf");
        // A double extension keeps only the recognized-as-font trailing check.
        assert_eq!(sanitize_filename("font.tar.gz"), "font.tar.gz.ttf");
    }

    #[test]
    fn font_ext_detection_is_case_insensitive() {
        assert!(has_font_ext("Foo.TTF"));
        assert!(has_font_ext("bar.WOFF2"));
        assert!(!has_font_ext("baz.png"));
        assert!(!has_font_ext("noext"));
    }

    #[test]
    fn records_are_deduped_by_post_script_name() {
        let dir = std::env::temp_dir();
        let store =
            CustomFontStore::new(Utf8Path::from_path(&dir).expect("utf8 temp dir")).unwrap();
        store.record(CustomFontFace {
            post_script_name: "Foo-Regular".into(),
            family_name: "Foo".into(),
        });
        store.record(CustomFontFace {
            post_script_name: "Foo-Regular".into(),
            family_name: "Foo (dup)".into(),
        });
        assert!(store.is_custom("Foo-Regular"));
        assert!(!store.is_custom("Bar-Regular"));
        assert_eq!(store.faces.lock().unwrap().len(), 1);
    }
}
