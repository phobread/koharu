//! Font routes.
//!
//! - `GET /fonts` — combined system + Google Fonts catalog.
//! - `POST /fonts/upload` — import a font file from the user's disk.
//! - `GET /google-fonts` — the Google Fonts catalog as a standalone list.
//! - `POST /google-fonts/{family}/fetch` — download and cache a family.
//! - `GET /google-fonts/{family}/{file}` — serve the cached TTF/WOFF file.

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header::CONTENT_TYPE};
use axum::response::{Json, Response};
use koharu_core::{FontFaceInfo, GoogleFontCatalog};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::AppState;
use crate::error::{ApiError, ApiResult};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::default()
        .routes(routes!(list_fonts))
        .routes(routes!(upload_font))
        .routes(routes!(get_google_fonts_catalog))
        .routes(routes!(fetch_google_font))
        .routes(routes!(get_google_font_file))
}

#[utoipa::path(get, path = "/fonts", responses((status = 200, body = Vec<FontFaceInfo>)))]
async fn list_fonts(State(app): State<AppState>) -> ApiResult<Json<Vec<FontFaceInfo>>> {
    let fonts = app.renderer.available_fonts().map_err(ApiError::internal)?;
    Ok(Json(fonts))
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
struct UploadFontParams {
    /// Original filename, used to name the cached copy on disk.
    filename: String,
}

/// Import a font from the raw file bytes in the request body. The font is
/// validated, registered for immediate use, and cached so it persists across
/// restarts. Returns the added face(s).
#[utoipa::path(
    post,
    path = "/fonts/upload",
    params(UploadFontParams),
    request_body(content_type = "application/octet-stream"),
    responses(
        (status = 200, body = Vec<FontFaceInfo>),
        (status = 400, description = "Empty body or unreadable font file"),
    )
)]
async fn upload_font(
    State(app): State<AppState>,
    Query(params): Query<UploadFontParams>,
    body: Bytes,
) -> ApiResult<Json<Vec<FontFaceInfo>>> {
    if body.is_empty() {
        return Err(ApiError::bad_request("empty body"));
    }
    let added = app
        .renderer
        .import_custom_font(&params.filename, body.to_vec())
        .map_err(|e| ApiError::bad_request(format!("{e:#}")))?;
    Ok(Json(added))
}

#[utoipa::path(
    get,
    path = "/google-fonts",
    responses((status = 200, body = GoogleFontCatalog))
)]
async fn get_google_fonts_catalog(
    State(app): State<AppState>,
) -> ApiResult<Json<GoogleFontCatalog>> {
    Ok(Json(app.renderer.google_fonts.catalog().clone()))
}

#[utoipa::path(
    post,
    path = "/google-fonts/{family}/fetch",
    params(("family" = String, Path, description = "Google Fonts family name")),
    responses((status = 204))
)]
async fn fetch_google_font(
    State(app): State<AppState>,
    Path(family_query): Path<String>,
) -> ApiResult<StatusCode> {
    let http = app.runtime.http_client();
    let (family, weight, style) = koharu_app::google_fonts::parse_variant_query(&family_query);

    app.renderer
        .google_fonts
        .fetch_variant(family, weight, style, &http)
        .await
        .map_err(ApiError::internal)?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/google-fonts/{family}/{file}",
    params(
        ("family" = String, Path, description = "Google Fonts family name"),
        ("file" = String, Path, description = "Font filename"),
    ),
    responses((status = 200, content_type = "font/ttf"))
)]
async fn get_google_font_file(
    State(app): State<AppState>,
    Path((family_query, _file)): Path<(String, String)>,
) -> ApiResult<Response> {
    let (family, weight, style) = koharu_app::google_fonts::parse_variant_query(&family_query);
    let bytes = app
        .renderer
        .google_fonts
        .read_cached_variant(family, weight, style)
        .map_err(ApiError::internal)?;

    let bytes =
        bytes.ok_or_else(|| ApiError::not_found(format!("font {family_query} not cached")))?;
    let mut resp = Response::new(Body::from(bytes));
    resp.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("font/ttf"));
    Ok(resp)
}
