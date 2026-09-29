//! Byte-oriented reads.
//!
//! - `GET /scene.bin` — postcard-encoded `Snapshot { epoch, scene }` (native clients).
//! - `GET /scene.json` — JSON-encoded `{ epoch, scene }` (web/UI clients).
//! - `GET /blobs/:hash` — raw blob bytes.

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header::CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use camino::Utf8Path;
use image::{DynamicImage, GenericImageView, imageops::FilterType};
use koharu_core::{BlobRef, ImageRole, NodeKind, PageId, Scene};
use serde::Serialize;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::AppState;
use crate::error::{ApiError, ApiResult};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::default()
        .routes(routes!(get_scene_bin))
        .routes(routes!(get_scene_json))
        .routes(routes!(get_blob))
        .routes(routes!(get_page_thumbnail))
        .routes(routes!(get_project_thumbnail))
}

/// JSON-shaped scene snapshot for the UI (no postcard decoder in JS).
#[derive(Serialize, utoipa::ToSchema)]
pub struct SceneSnapshot {
    pub epoch: u64,
    pub scene: Scene,
}

#[utoipa::path(
    get,
    path = "/scene.json",
    responses((status = 200, body = SceneSnapshot))
)]
async fn get_scene_json(State(app): State<AppState>) -> ApiResult<Json<SceneSnapshot>> {
    let session = app
        .current_session()
        .ok_or_else(|| ApiError::bad_request("no project open"))?;
    let (epoch, scene) = session.snapshot_with_epoch();
    Ok(Json(SceneSnapshot { epoch, scene }))
}

#[derive(Serialize)]
struct WireSnapshot<'a> {
    epoch: u64,
    scene: &'a koharu_core::Scene,
}

#[utoipa::path(
    get,
    path = "/scene.bin",
    responses((status = 200, content_type = "application/octet-stream"))
)]
async fn get_scene_bin(State(app): State<AppState>) -> ApiResult<Response> {
    let session = app
        .current_session()
        .ok_or_else(|| ApiError::bad_request("no project open"))?;
    let (epoch, scene) = session.snapshot_with_epoch();
    let bytes = postcard::to_allocvec(&WireSnapshot {
        epoch,
        scene: &scene,
    })
    .map_err(|e| ApiError::internal(anyhow::Error::new(e)))?;
    let mut resp = Response::new(Body::from(bytes));
    resp.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    resp.headers_mut().insert(
        "x-koharu-epoch",
        HeaderValue::from_str(&epoch.to_string())
            .map_err(|e| ApiError::internal(anyhow::Error::new(e)))?,
    );
    Ok(resp)
}

#[utoipa::path(
    get,
    path = "/blobs/{hash}",
    params(("hash" = String, Path, description = "Blake3 hash of the blob")),
    responses((status = 200, content_type = "application/octet-stream"))
)]
async fn get_blob(State(app): State<AppState>, Path(hash): Path<String>) -> ApiResult<Response> {
    let session = app
        .current_session()
        .ok_or_else(|| ApiError::bad_request("no project open"))?;
    let blob_ref = BlobRef::new(hash);
    let bytes = session
        .blobs
        .get_bytes(&blob_ref)
        .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, "blob not found"))?;
    let mut resp = Response::new(Body::from(bytes));
    resp.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    Ok(resp.into_response())
}

/// Thumbnail of a page's source image. Cached on disk under
/// `.khrproj/cache/thumbs/<page_id>.webp`; generated on first request.
const THUMB_MAX_DIM: u32 = 320;

#[utoipa::path(
    get,
    path = "/pages/{id}/thumbnail",
    params(("id" = PageId, Path, description = "Page id")),
    responses((status = 200, content_type = "image/webp"))
)]
async fn get_page_thumbnail(
    State(app): State<AppState>,
    Path(id): Path<PageId>,
) -> ApiResult<Response> {
    let session = app
        .current_session()
        .ok_or_else(|| ApiError::bad_request("no project open"))?;

    let source_ref = {
        let scene = session.scene.read();
        let page = scene
            .page(id)
            .ok_or_else(|| ApiError::not_found(format!("page {id}")))?;
        page.nodes
            .values()
            .find_map(|n| match &n.kind {
                NodeKind::Image(img) if img.role == ImageRole::Source => Some(img.blob.clone()),
                _ => None,
            })
            .ok_or_else(|| ApiError::not_found("page has no source image"))?
    };
    let bytes = page_thumbnail(&session.dir, id, &source_ref, &session.blobs)?;
    Ok(webp_response(bytes))
}

/// Cover for the project list: the thumbnail of the project's first page,
/// read without opening the project.
#[utoipa::path(
    get,
    path = "/projects/{id}/thumbnail",
    params(("id" = String, Path, description = "Project id")),
    responses((status = 200, content_type = "image/webp"))
)]
async fn get_project_thumbnail(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let config = (**app.config.load()).clone();
    let dir = koharu_app::projects::project_path(&config, &id)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    // The open project may have edits not yet in its scene.bin.
    let open = app
        .current_session()
        .filter(|s| koharu_app::projects::same_project_dir(&s.dir, &dir));
    let bytes = tokio::task::spawn_blocking(move || -> ApiResult<Vec<u8>> {
        let not_found = || ApiError::not_found(format!("project {id} has no pages"));
        if let Some(session) = open {
            let (page, source) = {
                let scene = session.scene.read();
                let page = scene.pages.values().next().ok_or_else(not_found)?;
                let source = page
                    .nodes
                    .values()
                    .find_map(|n| match &n.kind {
                        NodeKind::Image(img) if img.role == ImageRole::Source => {
                            Some(img.blob.clone())
                        }
                        _ => None,
                    })
                    .ok_or_else(not_found)?;
                (page.id, source)
            };
            return page_thumbnail(&session.dir, page, &source, &session.blobs);
        }
        let blobs_dir = dir.join("blobs");
        if !blobs_dir.is_dir() {
            return Err(not_found());
        }
        let (page, source) = koharu_app::session::first_page_source(&dir)
            .map_err(ApiError::internal)?
            .ok_or_else(not_found)?;
        let blobs =
            koharu_app::BlobStore::open(blobs_dir.as_std_path()).map_err(ApiError::internal)?;
        page_thumbnail(&dir, page, &source, &blobs)
    })
    .await
    .map_err(|e| ApiError::internal(anyhow::Error::new(e)))??;
    Ok(webp_response(bytes))
}

/// A page's source image scaled to fit `THUMB_MAX_DIM`, cached as
/// `<project>/cache/thumbs/<page_id>.webp` (Settings > Clear cache removes
/// these). Cached only when the project already has its `cache` folder:
/// creating it would change the project folder's modified time, which
/// orders the project list.
fn page_thumbnail(
    dir: &Utf8Path,
    page: PageId,
    source: &BlobRef,
    blobs: &koharu_app::BlobStore,
) -> ApiResult<Vec<u8>> {
    let cache_dir = dir.join("cache");
    let thumbs_dir = cache_dir.join("thumbs");
    let cache_path = thumbs_dir.join(format!("{page}.webp"));
    if let Ok(bytes) = std::fs::read(cache_path.as_std_path()) {
        return Ok(bytes);
    }

    let source: DynamicImage = blobs.load_image(source).map_err(ApiError::internal)?;
    let (w, h) = source.dimensions();
    let scale = THUMB_MAX_DIM as f32 / w.max(h) as f32;
    let resized = if scale < 1.0 {
        let nw = (w as f32 * scale).round().max(1.0) as u32;
        let nh = (h as f32 * scale).round().max(1.0) as u32;
        source.resize(nw, nh, FilterType::Triangle)
    } else {
        source
    };
    let mut buf = std::io::Cursor::new(Vec::new());
    resized
        .write_to(&mut buf, image::ImageFormat::WebP)
        .map_err(|e| ApiError::internal(anyhow::Error::new(e)))?;
    let bytes = buf.into_inner();
    if cache_dir.is_dir() {
        let _ = std::fs::create_dir_all(thumbs_dir.as_std_path());
        let _ = std::fs::write(cache_path.as_std_path(), &bytes);
    }
    Ok(bytes)
}

fn webp_response(bytes: Vec<u8>) -> Response {
    let mut resp = Response::new(Body::from(bytes));
    resp.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("image/webp"));
    resp.into_response()
}
