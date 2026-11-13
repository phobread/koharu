//! Favourites: a star on a project and on its pages, kept in the project's
//! `favourites.json` side file (`koharu_app::favourites`). Not scene edits:
//! no history entry, no undo.
//!
//! - `GET /projects` lists the starred project ids (see `projects.rs`)
//! - `PUT /projects/{id}/favourite` — star / unstar a managed project
//! - `GET /projects/current/favourites` — the open project's stars
//! - `PUT /pages/{id}/favourite` — star / unstar a page of the open project

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use koharu_app::favourites::{self, Favourites};
use koharu_app::projects as project_dirs;
use koharu_core::{PageId, Scene};
use serde::{Deserialize, Serialize};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::AppState;
use crate::error::{ApiError, ApiResult};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::default()
        .routes(routes!(put_project_favourite))
        .routes(routes!(get_current_favourites))
        .routes(routes!(put_page_favourite))
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetFavouriteRequest {
    /// `true` to star, `false` to remove the star.
    pub favourite: bool,
}

/// The open project's stars.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFavourites {
    /// Whether the project itself is starred.
    pub project: bool,
    /// Starred pages that still exist, in page order.
    pub pages: Vec<PageId>,
}

fn for_scene(favourites: &Favourites, scene: &Scene) -> ProjectFavourites {
    ProjectFavourites {
        project: favourites.project,
        pages: favourites.pages_in(scene.pages.keys().copied()),
    }
}

/// Star or unstar a project in the list. It doesn't have to be open.
#[utoipa::path(
    put,
    path = "/projects/{id}/favourite",
    params(("id" = String, Path, description = "Project id")),
    request_body = SetFavouriteRequest,
    responses(
        (status = 204, description = "Saved"),
        (status = 400, description = "Invalid project id"),
        (status = 404, description = "Project not found")
    )
)]
async fn put_project_favourite(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SetFavouriteRequest>,
) -> ApiResult<StatusCode> {
    let config = (**app.config.load()).clone();
    let path = project_dirs::project_path(&config, &id)
        .map_err(|e| ApiError::bad_request(format!("{e:#}")))?;
    if !path.is_dir() {
        return Err(ApiError::not_found(format!("project {id}")));
    }
    favourites::set_project(&path, req.favourite).map_err(ApiError::internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/projects/current/favourites",
    responses((status = 200, body = ProjectFavourites))
)]
async fn get_current_favourites(State(app): State<AppState>) -> ApiResult<Json<ProjectFavourites>> {
    let session = app
        .current_session()
        .ok_or_else(|| ApiError::bad_request("no project open"))?;
    let stars = favourites::load(&session.dir);
    Ok(Json(for_scene(&stars, &session.scene.read())))
}

/// Star or unstar a page of the open project. Answers with the project's
/// stars after the change.
#[utoipa::path(
    put,
    path = "/pages/{id}/favourite",
    params(("id" = PageId, Path, description = "Page id")),
    request_body = SetFavouriteRequest,
    responses(
        (status = 200, body = ProjectFavourites),
        (status = 404, description = "Page not found")
    )
)]
async fn put_page_favourite(
    State(app): State<AppState>,
    Path(page_id): Path<PageId>,
    Json(req): Json<SetFavouriteRequest>,
) -> ApiResult<Json<ProjectFavourites>> {
    let session = app
        .current_session()
        .ok_or_else(|| ApiError::bad_request("no project open"))?;
    if !session.scene.read().pages.contains_key(&page_id) {
        return Err(ApiError::not_found(format!("page {page_id}")));
    }
    let stars =
        favourites::set_page(&session.dir, page_id, req.favourite).map_err(ApiError::internal)?;
    Ok(Json(for_scene(&stars, &session.scene.read())))
}

#[cfg(test)]
mod tests {
    use koharu_core::Page;

    use super::*;

    #[test]
    fn stars_are_reported_for_pages_the_scene_still_has_in_page_order() {
        let mut scene = Scene::default();
        let pages: Vec<Page> = (0..3)
            .map(|i| Page::new(format!("{i}.png"), 10, 10))
            .collect();
        let ids: Vec<PageId> = pages.iter().map(|page| page.id).collect();
        for page in pages {
            scene.pages.insert(page.id, page);
        }

        let deleted = PageId::new();
        let stars = Favourites {
            project: true,
            pages: [ids[2], deleted, ids[0]].into_iter().collect(),
        };
        assert_eq!(
            for_scene(&stars, &scene),
            ProjectFavourites {
                project: true,
                pages: vec![ids[0], ids[2]],
            }
        );
        assert_eq!(
            for_scene(&Favourites::default(), &scene),
            ProjectFavourites {
                project: false,
                pages: vec![],
            }
        );
    }

    #[test]
    fn answers_use_the_documented_field_names() {
        let page = PageId::new();
        let json = serde_json::to_value(ProjectFavourites {
            project: true,
            pages: vec![page],
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "project": true, "pages": [page.0.to_string()] })
        );
        let req: SetFavouriteRequest = serde_json::from_str("{\"favourite\":true}").unwrap();
        assert!(req.favourite);
    }
}
