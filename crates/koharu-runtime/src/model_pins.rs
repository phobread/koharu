//! Immutable revisions for bundled image-processing assets. Local translation
//! LLMs and user-selected model paths are deliberately outside this catalog.
mod catalog;

use anyhow::{Context, Result};

#[derive(Debug)]
pub(crate) struct ModelPin {
    pub repo: &'static str,
    pub revision: &'static str,
    pub files: &'static [ModelFile],
}

#[derive(Debug)]
pub(crate) struct ModelFile {
    pub filename: &'static str,
    /// Hugging Face ETag: LFS SHA256 or the Git blob SHA1 for a regular file.
    pub oid: &'static str,
    pub size: u64,
}

pub(crate) fn get(repo: &str, filename: &str) -> Result<(&'static ModelPin, &'static ModelFile)> {
    let pin = catalog::PINS
        .iter()
        .find(|pin| pin.repo == repo)
        .with_context(|| format!("no bundled model revision is pinned for `{repo}`"))?;
    let file = pin
        .files
        .iter()
        .find(|file| file.filename == filename)
        .with_context(|| {
            format!(
                "model file `{repo}@{}/{filename}` is not in the pinned catalog",
                pin.revision
            )
        })?;
    Ok((pin, file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn pins_are_complete_immutable_and_unique() {
        let mut repos = HashSet::new();
        for pin in catalog::PINS {
            assert!(repos.insert(pin.repo));
            assert_eq!(pin.revision.len(), 40);
            assert!(pin.revision.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert!(!pin.files.is_empty());
            let mut files = HashSet::new();
            for file in pin.files {
                assert!(files.insert(file.filename));
                assert!(matches!(file.oid.len(), 40 | 64));
                assert!(file.oid.bytes().all(|byte| byte.is_ascii_hexdigit()));
                assert!(file.size > 0);
                assert!(!file.filename.split('/').any(|part| part == ".."));
                get(pin.repo, file.filename).unwrap();
            }
        }
        assert!(get("custom/my-model", "model.gguf").is_err());
        assert!(
            get("mayocream/lama-manga", "missing.safetensors")
                .unwrap_err()
                .to_string()
                .contains("f91c85b26913b3e83f9877867b4c336da3675238")
        );
    }
}
