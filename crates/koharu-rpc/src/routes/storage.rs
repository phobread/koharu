//! Remove only regenerable thumbnails, never saved project content or history.

use std::fs;
use std::path::Path;

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    AppState,
    error::{ApiError, ApiResult},
};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::default().routes(routes!(clear_project_cache))
}

#[derive(Debug, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClearProjectCacheResponse {
    pub bytes_freed: u64,
    pub files_removed: u64,
    pub files_skipped: u64,
}

#[utoipa::path(
    post,
    path = "/storage/project-cache/clear",
    responses((status = 200, body = ClearProjectCacheResponse))
)]
async fn clear_project_cache(
    State(app): State<AppState>,
) -> ApiResult<Json<ClearProjectCacheResponse>> {
    // Use the running app's root, not a pending restart's config.data.path.
    let root = app.runtime().root().to_path_buf();
    let result = tokio::task::spawn_blocking(move || clear_thumbnails(&root))
        .await
        .map_err(|e| ApiError::internal(anyhow::Error::new(e)))?
        .map_err(ApiError::internal)?;
    Ok(Json(result))
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

fn plain_directory(path: &Path) -> std::io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(meta.is_dir() && !is_link(&meta)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn clear_thumbnails(root: &Path) -> anyhow::Result<ClearProjectCacheResponse> {
    let mut result = ClearProjectCacheResponse::default();
    let projects = root.join("projects");
    if !plain_directory(&projects)? {
        return Ok(result);
    }
    for project in fs::read_dir(&projects)? {
        let project = project?.path();
        if project.extension().and_then(|v| v.to_str()) != Some("khrproj") {
            continue;
        }
        let cache = project.join("cache");
        let thumbs = cache.join("thumbs");
        if !plain_directory(&project)? || !plain_directory(&cache)? || !plain_directory(&thumbs)? {
            continue;
        }
        for entry in fs::read_dir(&thumbs)? {
            let path = entry?.path();
            if path.extension().and_then(|v| v.to_str()) != Some("webp")
                || !path
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
            {
                continue;
            }
            let meta = match fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    result.files_skipped += 1;
                    continue;
                }
            };
            if !meta.is_file() || is_link(&meta) {
                result.files_skipped += 1;
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => {
                    result.files_removed += 1;
                    result.bytes_freed += meta.len();
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => result.files_skipped += 1,
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clears_only_generated_thumbnails_and_is_repeatable() {
        let root = std::env::temp_dir().join(format!("koharu-cache-test-{}", uuid::Uuid::new_v4()));
        let project = root.join("projects/example.khrproj");
        let thumbs = project.join("cache/thumbs");
        fs::create_dir_all(&thumbs).unwrap();
        fs::create_dir_all(project.join("blobs")).unwrap();
        let generated = thumbs.join(format!("{}.webp", uuid::Uuid::new_v4()));
        fs::write(&generated, b"thumbnail").unwrap();
        let kept = [
            project.join("scene.bin"),
            project.join("history.log"),
            project.join("project.toml"),
            project.join("blobs/source.webp"),
            project.join("cache/unknown"),
            thumbs.join("manual.webp"),
        ];
        for path in &kept {
            fs::write(path, b"keep").unwrap();
        }
        let result = clear_thumbnails(&root).unwrap();
        assert_eq!(result.files_removed, 1);
        assert_eq!(result.bytes_freed, 9);
        assert_eq!(result.files_skipped, 0);
        for path in &kept {
            assert_eq!(fs::read(path).unwrap(), b"keep");
        }
        assert_eq!(clear_thumbnails(&root).unwrap().files_removed, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn does_not_follow_project_junction() {
        let root =
            std::env::temp_dir().join(format!("koharu-cache-link-test-{}", uuid::Uuid::new_v4()));
        let external = root.join("outside");
        fs::create_dir_all(root.join("projects")).unwrap();
        fs::create_dir_all(external.join("cache/thumbs")).unwrap();
        let thumb = external
            .join("cache/thumbs")
            .join(format!("{}.webp", uuid::Uuid::new_v4()));
        fs::write(&thumb, b"keep").unwrap();
        let link = root.join("projects/linked.khrproj");
        #[cfg(windows)]
        {
            let status = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", "$null = New-Item -ItemType Junction -Path $env:KOHARU_TEST_LINK -Target $env:KOHARU_TEST_TARGET"])
                .env("KOHARU_TEST_LINK", &link).env("KOHARU_TEST_TARGET", &external).status().unwrap();
            assert!(status.success());
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&external, &link).unwrap();
        assert_eq!(clear_thumbnails(&root).unwrap().files_removed, 0);
        assert_eq!(fs::read(&thumb).unwrap(), b"keep");
        #[cfg(windows)]
        fs::remove_dir(&link).unwrap();
        #[cfg(unix)]
        fs::remove_file(&link).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
