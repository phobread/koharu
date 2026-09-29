//! Project lifecycle routes. Every project lives under the managed
//! `{data.path}/projects/` directory; clients never supply filesystem
//! paths. A project's `id` is the `.khrproj/` directory basename.
//!
//! - `GET    /projects` — list managed projects
//! - `POST   /projects` — create a new project (`{name}`), server allocates path
//! - `POST   /projects/import` — extract a `.khr` archive into a fresh dir + open
//! - `PUT    /projects/current` — open a managed project by `id`
//! - `DELETE /projects/current` — close current session
//! - `POST   /projects/current/export` — export current; returns bytes

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use koharu_app::projects as project_dirs;
use koharu_core::{ImageRole, PageId, ProjectSummary};
use serde::{Deserialize, Serialize};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::AppState;
use crate::error::{ApiError, ApiResult};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::default()
        .routes(routes!(list_projects))
        .routes(routes!(create_project))
        .routes(routes!(import_project))
        .routes(routes!(put_current_project))
        .routes(routes!(delete_current_project))
        .routes(routes!(delete_project))
        .routes(routes!(export_current_project))
}

// ---------------------------------------------------------------------------
// GET /projects
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListProjectsResponse {
    pub projects: Vec<ProjectSummary>,
}

#[utoipa::path(
    get,
    path = "/projects",
    responses((status = 200, body = ListProjectsResponse))
)]
async fn list_projects(State(app): State<AppState>) -> ApiResult<Json<ListProjectsResponse>> {
    let config = (**app.config.load()).clone();
    let projects = project_dirs::list_projects(&config).map_err(ApiError::internal)?;
    Ok(Json(ListProjectsResponse { projects }))
}

// ---------------------------------------------------------------------------
// POST /projects — create a new project from a display name
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectRequest {
    pub name: String,
}

#[utoipa::path(
    post,
    path = "/projects",
    request_body = CreateProjectRequest,
    responses((status = 200, body = ProjectSummary))
)]
async fn create_project(
    State(app): State<AppState>,
    Json(req): Json<CreateProjectRequest>,
) -> ApiResult<Json<ProjectSummary>> {
    let trimmed = req.name.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request("name must not be empty"));
    }
    let config = (**app.config.load()).clone();
    let path = project_dirs::allocate_named(&config, trimmed).map_err(ApiError::internal)?;
    // `allocate_named` atomically created the directory so concurrent
    // callers can't collide. Session::create wants an empty-or-missing dir
    // and writes the scaffold — remove so it can populate.
    std::fs::remove_dir(path.as_std_path())
        .map_err(|e| ApiError::internal(anyhow::Error::new(e)))?;
    let session = app
        .open_project(path, Some(trimmed.to_string()))
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(koharu_app::app::project_summary(&session)))
}

// ---------------------------------------------------------------------------
// PUT /projects/current — open a managed project by id
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OpenProjectRequest {
    /// `.khrproj/` directory basename (no extension). Must exist under the
    /// managed projects directory.
    pub id: String,
}

#[utoipa::path(
    put,
    path = "/projects/current",
    request_body = OpenProjectRequest,
    responses((status = 200, body = ProjectSummary))
)]
async fn put_current_project(
    State(app): State<AppState>,
    Json(req): Json<OpenProjectRequest>,
) -> ApiResult<Json<ProjectSummary>> {
    let config = (**app.config.load()).clone();
    let path = project_dirs::project_path(&config, &req.id)
        .map_err(|e| ApiError::bad_request(format!("{e:#}")))?;
    if !path.exists() {
        return Err(ApiError::not_found(format!("project {}", req.id)));
    }
    let session = app
        .open_project(path, None)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(koharu_app::app::project_summary(&session)))
}

#[utoipa::path(delete, path = "/projects/current", responses((status = 204)))]
async fn delete_current_project(State(app): State<AppState>) -> ApiResult<axum::http::StatusCode> {
    app.close_project().await.map_err(ApiError::internal)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// DELETE /projects/{id} — delete a managed project recursively
// ---------------------------------------------------------------------------

#[utoipa::path(
    delete,
    path = "/projects/{id}",
    params(
        ("id" = String, Path, description = "Project ID to delete")
    ),
    responses(
        (status = 204, description = "Project successfully deleted"),
        (status = 400, description = "Invalid project ID"),
        (status = 404, description = "Project not found"),
        (status = 500, description = "Internal filesystem error")
    )
)]
async fn delete_project(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<axum::http::StatusCode> {
    let config = (**app.config.load()).clone();
    let path = project_dirs::project_path(&config, &id)
        .map_err(|e| ApiError::bad_request(format!("{e:#}")))?;

    if !path.exists() {
        return Err(ApiError::not_found(format!("project {}", id)));
    }

    // If the active session is the project we are deleting, close it first to release lock files
    if app
        .current_session()
        .is_some_and(|session| project_dirs::same_project_dir(&session.dir, &path))
    {
        app.close_project().await.map_err(ApiError::internal)?;
    }

    // Recursively delete the project directory from disk
    tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(path.as_std_path()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    })
    .await
    .map_err(|e| ApiError::internal(anyhow::Error::new(e)))?
    .map_err(|e| ApiError::internal(anyhow::Error::new(e)))?;

    Ok(axum::http::StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// POST /projects/import — extract an archive into a fresh allocated dir
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/projects/import",
    request_body(content_type = "application/zip"),
    responses((status = 200, body = ProjectSummary))
)]
async fn import_project(
    State(app): State<AppState>,
    body: Bytes,
) -> ApiResult<Json<ProjectSummary>> {
    if body.is_empty() {
        return Err(ApiError::bad_request("empty archive body"));
    }
    let config = (**app.config.load()).clone();
    let dest =
        project_dirs::allocate_imported(&config, Some("imported")).map_err(ApiError::internal)?;
    // Atomic-created dir must be removed so `import_khr_bytes` can do its
    // own exists-check + populate.
    std::fs::remove_dir(dest.as_std_path())
        .map_err(|e| ApiError::internal(anyhow::Error::new(e)))?;

    let dest_c = dest.clone();
    tokio::task::spawn_blocking(move || koharu_app::archive::import_khr_bytes(&body, &dest_c))
        .await
        .map_err(|e| ApiError::internal(anyhow::Error::new(e)))?
        .map_err(ApiError::internal)?;

    let session = match app.open_project(dest.clone(), None).await {
        Ok(session) => session,
        Err(err) => {
            // Otherwise the list keeps a project that can't be opened.
            let _ = std::fs::remove_dir_all(dest.as_std_path());
            return Err(ApiError::internal(err));
        }
    };
    Ok(Json(koharu_app::app::project_summary(&session)))
}

// ---------------------------------------------------------------------------
// Export — returns bytes (zip when the format produces >1 file)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExportProjectRequest {
    pub format: ExportFormat,
    /// Optional subset of pages; defaults to every page.
    #[serde(default)]
    pub pages: Option<Vec<PageId>>,
    /// Optional global font override (from UI preferences).
    #[serde(default)]
    pub default_font: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// Whole project as a `.khr` archive (always a single zip).
    Khr,
    /// One `.psd` per page.
    Psd,
    /// One `.png` per page (the Rendered layer).
    Rendered,
    /// One `.png` per page (the Inpainted layer).
    Inpainted,
    /// One `.png` for every page: the Rendered layer, else the Inpainted
    /// layer, else the source image.
    Best,
}

#[utoipa::path(
    post,
    path = "/projects/current/export",
    request_body = ExportProjectRequest,
    responses((
        status = 200,
        content_type = "application/octet-stream",
        description = "Export bytes. Content-Type is `application/zip` when the format produces multiple files."
    ))
)]
async fn export_current_project(
    State(app): State<AppState>,
    Json(req): Json<ExportProjectRequest>,
) -> ApiResult<Response> {
    let session = app
        .current_session()
        .ok_or_else(|| ApiError::bad_request("no project open"))?;

    let s_for_compact = session.clone();
    tokio::task::spawn_blocking(move || s_for_compact.compact())
        .await
        .map_err(|e| ApiError::internal(anyhow::Error::new(e)))?
        .map_err(ApiError::internal)?;

    let project_name = session.scene.read().project.name.clone();

    match req.format {
        ExportFormat::Khr => {
            let src = session.dir.clone();
            let bytes =
                tokio::task::spawn_blocking(move || koharu_app::archive::export_khr_bytes(&src))
                    .await
                    .map_err(|e| ApiError::internal(anyhow::Error::new(e)))?
                    .map_err(ApiError::internal)?;
            Ok(bytes_response(
                bytes,
                &sanitize(&project_name, "project"),
                "khr",
                "application/octet-stream",
            ))
        }
        ExportFormat::Psd => {
            let page_ids = resolve_page_ids(&session, req.pages.as_deref())?;
            if page_ids.is_empty() {
                return Err(ApiError::bad_request("no pages in selection"));
            }
            let session_c = session.clone();
            let page_ids_c = page_ids.clone();
            let renderer_c = app.renderer.clone();
            let default_font_c = req.default_font.clone();
            let files = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                let mut out = Vec::with_capacity(page_ids_c.len());
                for (stem, id) in &page_ids_c {
                    let bytes = crate::psd_export::psd_bytes_for_page(
                        &session_c,
                        &renderer_c,
                        default_font_c.clone(),
                        *id,
                    )?;
                    out.push((format!("{stem}.psd"), bytes));
                }
                Ok(out)
            })
            .await
            .map_err(|e| ApiError::internal(anyhow::Error::new(e)))?
            .map_err(ApiError::internal)?;
            Ok(files_to_response(files, &project_name, "psd")?)
        }
        ExportFormat::Rendered => {
            export_image_role(
                &session,
                req.pages.as_deref(),
                &[ImageRole::Rendered],
                &project_name,
            )
            .await
        }
        ExportFormat::Inpainted => {
            export_image_role(
                &session,
                req.pages.as_deref(),
                &[ImageRole::Inpainted],
                &project_name,
            )
            .await
        }
        ExportFormat::Best => {
            export_image_role(
                &session,
                req.pages.as_deref(),
                &[ImageRole::Rendered, ImageRole::Inpainted, ImageRole::Source],
                &project_name,
            )
            .await
        }
    }
}

/// One `.png` per page from the first of `roles` the page has; pages with
/// none of them are left out.
async fn export_image_role(
    session: &std::sync::Arc<koharu_app::ProjectSession>,
    pages: Option<&[PageId]>,
    roles: &'static [ImageRole],
    project_name: &str,
) -> ApiResult<Response> {
    let page_ids = resolve_page_ids(session, pages)?;
    if page_ids.is_empty() {
        return Err(ApiError::bad_request("no pages in selection"));
    }
    let session_c = session.clone();
    let page_ids_c = page_ids.clone();
    let files = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let mut out: Vec<(String, Vec<u8>)> = Vec::new();
        for (stem, id) in &page_ids_c {
            if let Some(bytes) = crate::psd_export::png_bytes_for_page(&session_c, *id, roles)? {
                out.push((format!("{stem}.png"), bytes));
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| ApiError::internal(anyhow::Error::new(e)))?
    .map_err(ApiError::internal)?;

    if files.is_empty() {
        return Err(ApiError::bad_request(
            "no pages have the requested layer populated",
        ));
    }
    files_to_response(files, project_name, "png")
}

/// The requested pages (every page when `None`) with the file name stem
/// their exports get: the page's original file name, see [`export_stems`].
fn resolve_page_ids(
    session: &koharu_app::ProjectSession,
    requested: Option<&[PageId]>,
) -> ApiResult<Vec<(String, PageId)>> {
    let scene = session.scene.read();
    let ids: Vec<PageId> = match requested {
        None => scene.pages.keys().copied().collect(),
        Some(ids) => {
            for id in ids {
                if !scene.pages.contains_key(id) {
                    return Err(ApiError::not_found(format!("page {id}")));
                }
            }
            ids.to_vec()
        }
    };
    // Stems are made unique over the whole project, so a page keeps the same
    // name whether it's exported alone or with the others.
    let stems = export_stems(scene.pages.values().map(|p| p.name.as_str()));
    Ok(ids
        .into_iter()
        .map(|id| {
            let index = scene.pages.get_index_of(&id).unwrap_or_default();
            (stems[index].clone(), id)
        })
        .collect())
}

/// One export file stem per page, in order: the original file name without
/// its extension, made safe for Windows. Pages without a usable name get
/// `page-NNN`; repeats get ` (2)`, ` (3)`… (compared case-insensitively).
fn export_stems<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut used = std::collections::HashSet::new();
    names
        .enumerate()
        .map(|(i, name)| {
            // Plain string split: `Path` on Windows reads `a:b` as a drive.
            let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
            let stem = match file.rsplit_once('.') {
                Some((stem, _)) if !stem.is_empty() => stem,
                _ => file,
            };
            let mut base: String = stem
                .chars()
                .map(|c| {
                    if c.is_control() || "\\/:*?\"<>|".contains(c) {
                        '_'
                    } else {
                        c
                    }
                })
                .collect();
            base = base.trim().trim_end_matches('.').to_string();
            if base.is_empty() {
                base = format!("page-{:03}", i + 1);
            }
            const RESERVED: [&str; 22] = [
                "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
                "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
                "LPT9",
            ];
            if RESERVED.contains(&base.to_ascii_uppercase().as_str()) {
                base.push('_');
            }
            let mut stem = base.clone();
            let mut n = 2;
            while !used.insert(stem.to_lowercase()) {
                stem = format!("{base} ({n})");
                n += 1;
            }
            stem
        })
        .collect()
}

fn files_to_response(
    mut files: Vec<(String, Vec<u8>)>,
    project_name: &str,
    ext: &str,
) -> ApiResult<Response> {
    if files.len() == 1 {
        let (fname, bytes) = files.remove(0);
        let content_type = match ext {
            "psd" => "image/vnd.adobe.photoshop",
            "png" => "image/png",
            "khr" => "application/octet-stream",
            _ => "application/octet-stream",
        };
        return Ok(bytes_response_with_filename(bytes, &fname, content_type));
    }
    let zip_bytes = koharu_app::archive::zip_files_to_bytes(&files).map_err(ApiError::internal)?;
    let base = sanitize(project_name, "export");
    let filename = format!("{base}-{ext}.zip");
    Ok(bytes_response_with_filename(
        zip_bytes,
        &filename,
        "application/zip",
    ))
}

fn bytes_response(bytes: Vec<u8>, base: &str, ext: &str, content_type: &str) -> Response {
    let filename = format!("{base}.{ext}");
    bytes_response_with_filename(bytes, &filename, content_type)
}

fn bytes_response_with_filename(bytes: Vec<u8>, filename: &str, content_type: &str) -> Response {
    // Plain `filename` must be ASCII for the header to be valid; the real
    // (possibly Korean/Japanese) name travels percent-encoded in `filename*`.
    let ascii: String = filename
        .chars()
        .map(|c| {
            if (c.is_ascii_graphic() && c != '"') || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = filename
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let cd = format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}");
    let mut resp = Response::new(Body::from(bytes));
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    if let Ok(v) = HeaderValue::from_str(&cd) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    resp.into_response()
}

fn sanitize(name: &str, fallback: &str) -> String {
    let s: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if s.is_empty() {
        fallback.to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_stems_keep_original_names() {
        let names = [
            "003.jpg",
            "004.png",
            "003.png",
            "COM1.jpg",
            "a:b?.webp",
            "",
            "뱃엔드 12.jpg",
        ];
        assert_eq!(
            export_stems(names.into_iter()),
            [
                "003",
                "004",
                "003 (2)",
                "COM1_",
                "a_b_",
                "page-006",
                "뱃엔드 12"
            ]
        );
    }

    #[test]
    fn filename_header_carries_unicode_names() {
        let resp = bytes_response_with_filename(vec![], "뱃 1.png", "image/png");
        let cd = resp.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap();
        assert_eq!(
            cd,
            "attachment; filename=\"_ 1.png\"; filename*=UTF-8''%EB%B1%83%201.png"
        );
    }
}
