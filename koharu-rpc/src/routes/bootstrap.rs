//! Startup status. Served before the app is ready, so the UI can show why
//! startup failed (e.g. a runtime download error) and offer a retry instead
//! of the process exiting.
//!
//! - `GET  /bootstrap`       — `starting` / `ready` / `failed` (+ error)
//! - `POST /bootstrap/retry` — re-run a failed startup

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::AppState;
use crate::bootstrap::BootstrapStatus;
use crate::error::{ApiError, ApiResult};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::default()
        .routes(routes!(get_bootstrap))
        .routes(routes!(retry_bootstrap))
}

#[utoipa::path(get, path = "/bootstrap", responses((status = 200, body = BootstrapStatus)))]
async fn get_bootstrap(State(app): State<AppState>) -> Json<BootstrapStatus> {
    Json(app.status())
}

#[utoipa::path(
    post,
    path = "/bootstrap/retry",
    responses(
        (status = 202, body = BootstrapStatus),
        (status = 409, body = ApiError, description = "Startup hasn't failed"),
    )
)]
async fn retry_bootstrap(
    State(app): State<AppState>,
) -> ApiResult<(StatusCode, Json<BootstrapStatus>)> {
    if !app.retry() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "startup hasn't failed; nothing to retry",
        ));
    }
    Ok((StatusCode::ACCEPTED, Json(app.status())))
}
