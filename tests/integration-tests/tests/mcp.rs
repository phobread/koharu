//! The MCP endpoint honours Settings → Privacy → "Allow AI tools"
//! (`config.mcp.enabled`), checked per request.

use koharu_integration_tests::TestApp;
use reqwest::StatusCode;
use serde_json::json;

/// Sends an MCP `initialize` request and returns the HTTP status.
async fn initialize(app: &TestApp) -> anyhow::Result<StatusCode> {
    let res = reqwest::Client::new()
        .post(format!("http://{}/mcp", app.addr))
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "koharu-integration-tests", "version": "0" }
            }
        }))
        .send()
        .await?;
    Ok(res.status())
}

#[tokio::test]
async fn mcp_answers_when_enabled() -> anyhow::Result<()> {
    let app = TestApp::spawn().await?;
    assert_eq!(initialize(&app).await?, StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn mcp_is_refused_when_turned_off() -> anyhow::Result<()> {
    let app = TestApp::spawn_with(|config| config.mcp.enabled = false).await?;
    assert_eq!(initialize(&app).await?, StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
async fn turning_mcp_off_applies_without_a_restart() -> anyhow::Result<()> {
    let app = TestApp::spawn().await?;
    assert_eq!(initialize(&app).await?, StatusCode::OK);

    let res = reqwest::Client::new()
        .patch(format!("{}/config", app.base_url))
        .json(&json!({ "mcp": { "enabled": false } }))
        .send()
        .await?;
    assert!(res.status().is_success(), "PATCH /config: {}", res.status());

    assert_eq!(initialize(&app).await?, StatusCode::FORBIDDEN);
    Ok(())
}
