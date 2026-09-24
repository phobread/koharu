//! MCP server exposing Koharu operations as tools.
//!
//! Built on rmcp 1.5's `#[tool_router]` + streamable HTTP transport. Mount
//! via [`mount`] onto an existing axum `Router`; sessions and routing are
//! handled by `StreamableHttpService`.
//!
//! **Tools exposed:**
//!   - `koharu.apply` — apply an `Op` to the active scene
//!   - `koharu.undo` / `koharu.redo`
//!   - `koharu.open_project` / `koharu.close_project`
//!   - `koharu.start_pipeline` — kick off a pipeline run
//!
//! More tools can be added by extending the `#[tool_router]` impl.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use camino::Utf8PathBuf;
use koharu_app::App;
use koharu_core::{NodeId, Op, PageId, ReadingOrder};
use rmcp::handler::server::wrapper::{Json as JsonOutput, Parameters};
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::ApiError;
use crate::routes::pipelines::{self, StartPipelineRequest};

/// Server state handed to each tool call. Carries the shared `App`.
#[derive(Clone)]
pub struct KoharuServer {
    state: AppState,
}

impl KoharuServer {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    fn app(&self) -> Result<Arc<App>, rmcp::ErrorData> {
        self.state
            .app()
            .ok_or_else(|| rmcp::ErrorData::internal_error("app is still bootstrapping", None))
    }
}

// ---------------------------------------------------------------------------
// Tool I/O schemas
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ApplyInput {
    /// The `Op` value to apply.
    pub op: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOutput {
    pub epoch: u64,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UndoOutput {
    pub epoch: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct OpenProjectInput {
    pub path: String,
    /// If set, create the project with this name instead of opening an existing one.
    pub create_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OpenProjectOutput {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct StartPipelineInput {
    pub steps: Vec<String>,
    pub pages: Option<Vec<PageId>>,
    pub text_node_ids: Option<Vec<NodeId>>,
    pub target_language: Option<String>,
    pub system_prompt: Option<String>,
    pub default_font: Option<String>,
    pub reading_order: Option<ReadingOrder>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StartPipelineOutput {
    pub job_id: String,
}

// ---------------------------------------------------------------------------
// Tool router
// ---------------------------------------------------------------------------

#[tool_router]
impl KoharuServer {
    #[tool(name = "koharu.apply", description = "Apply an Op to the active scene")]
    async fn apply(
        &self,
        Parameters(input): Parameters<ApplyInput>,
    ) -> Result<JsonOutput<ApplyOutput>, rmcp::ErrorData> {
        let app = self.app()?;
        let op: Op = serde_json::from_value(input.op).map_err(err)?;
        let epoch = app.apply(op).map_err(err)?;
        Ok(JsonOutput(ApplyOutput { epoch }))
    }

    #[tool(name = "koharu.undo", description = "Undo the most recent op")]
    async fn undo(&self) -> Result<JsonOutput<UndoOutput>, rmcp::ErrorData> {
        let app = self.app()?;
        let epoch = app.undo().map_err(err)?;
        Ok(JsonOutput(UndoOutput { epoch }))
    }

    #[tool(name = "koharu.redo", description = "Redo the most recent undo")]
    async fn redo(&self) -> Result<JsonOutput<UndoOutput>, rmcp::ErrorData> {
        let app = self.app()?;
        let epoch = app.redo().map_err(err)?;
        Ok(JsonOutput(UndoOutput { epoch }))
    }

    #[tool(
        name = "koharu.open_project",
        description = "Open or create a Koharu project directory"
    )]
    async fn open_project(
        &self,
        Parameters(input): Parameters<OpenProjectInput>,
    ) -> Result<JsonOutput<OpenProjectOutput>, rmcp::ErrorData> {
        let app = self.app()?;
        let path = Utf8PathBuf::from(input.path);
        let session = app
            .open_project(path, input.create_name)
            .await
            .map_err(err)?;
        Ok(JsonOutput(OpenProjectOutput {
            name: session.scene.read().project.name.clone(),
            path: session.dir.to_string(),
        }))
    }

    #[tool(
        name = "koharu.close_project",
        description = "Close the active project"
    )]
    async fn close_project(&self) -> Result<JsonOutput<serde_json::Value>, rmcp::ErrorData> {
        let app = self.app()?;
        app.close_project().await.map_err(err)?;
        Ok(JsonOutput(serde_json::Value::Null))
    }

    #[tool(
        name = "koharu.start_pipeline",
        description = "Kick off a pipeline run; returns a job id. Track it via \
                       GET /api/v1/operations/{id} or SSE, cancel it via \
                       DELETE /api/v1/operations/{id}"
    )]
    async fn start_pipeline(
        &self,
        Parameters(input): Parameters<StartPipelineInput>,
    ) -> Result<JsonOutput<StartPipelineOutput>, rmcp::ErrorData> {
        // `launch` derefs into `App`, which panics while still bootstrapping.
        self.app()?;
        let req = StartPipelineRequest {
            steps: input.steps,
            pages: input.pages,
            region: None,
            text_node_ids: input.text_node_ids,
            target_language: input.target_language,
            system_prompt: input.system_prompt,
            default_font: input.default_font,
            reading_order: input.reading_order,
        };
        let res = pipelines::launch(&self.state, req).map_err(api_err)?;
        Ok(JsonOutput(StartPipelineOutput {
            job_id: res.operation_id,
        }))
    }
}

fn api_err(e: ApiError) -> rmcp::ErrorData {
    if e.status == StatusCode::BAD_REQUEST.as_u16() {
        rmcp::ErrorData::invalid_request(e.message, None)
    } else {
        rmcp::ErrorData::internal_error(e.message, None)
    }
}

fn err(e: impl std::fmt::Display) -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error(e.to_string(), None)
}

#[tool_handler]
impl ServerHandler for KoharuServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        let mut implementation = rmcp::model::Implementation::default();
        implementation.name = "koharu".into();
        implementation.version = env!("CARGO_PKG_VERSION").into();
        info.server_info = implementation;
        info
    }
}

// ---------------------------------------------------------------------------
// Axum mount
// ---------------------------------------------------------------------------

/// Mount the MCP endpoint at `/mcp` on `router`. Requests are refused while
/// Settings → Privacy → "Allow AI tools" (`config.mcp.enabled`) is off.
pub fn mount(router: axum::Router, state: AppState) -> axum::Router {
    let manager = Arc::new(LocalSessionManager::default());
    let factory = {
        let state = state.clone();
        move || -> Result<KoharuServer, std::io::Error> { Ok(KoharuServer::new(state.clone())) }
    };
    let service =
        StreamableHttpService::new(factory, manager, StreamableHttpServerConfig::default());
    let mcp = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, require_enabled));
    router.merge(mcp)
}

/// Checked per request so turning MCP off applies immediately. Before the
/// app is ready the config isn't loaded yet; tools answer "bootstrapping"
/// then anyway.
async fn require_enabled(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if let Some(app) = state.app()
        && !app.config.load().mcp.enabled
    {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "the MCP server is turned off in Koharu's settings",
        )
        .into_response();
    }
    next.run(request).await
}
