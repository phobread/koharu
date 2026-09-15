use std::sync::Arc;

use reqwest_middleware::ClientWithMiddleware;
use serde::Serialize;

use super::ensure_provider_success;

pub enum ChatCompletionsAuth {
    None,
    Bearer(String),
}

pub struct ChatCompletionsRequest {
    pub provider: &'static str,
    pub endpoint: String,
    pub auth: ChatCompletionsAuth,
    pub model: String,
    pub system_prompt: String,
    pub user_prompt: String,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
}

#[derive(Serialize)]
struct ChatMessage {
    role: &'static str,
    content: String,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<serde_json::Value>,
}

pub async fn send_chat_completion(
    http_client: Arc<ClientWithMiddleware>,
    request: ChatCompletionsRequest,
) -> anyhow::Result<String> {
    send_completion(http_client, request, None).await
}

/// OpenRouter-only routing requirements accompany a structured response format.
pub(super) async fn send_openrouter_structured_completion(
    http_client: Arc<ClientWithMiddleware>,
    request: ChatCompletionsRequest,
    response_format: serde_json::Value,
) -> anyhow::Result<String> {
    send_completion(http_client, request, Some(response_format)).await
}

async fn send_completion(
    http_client: Arc<ClientWithMiddleware>,
    request: ChatCompletionsRequest,
    response_format: Option<serde_json::Value>,
) -> anyhow::Result<String> {
    let structured = response_format.is_some();
    let body = ChatRequest {
        model: &request.model,
        messages: vec![
            ChatMessage {
                role: "system",
                content: request.system_prompt,
            },
            ChatMessage {
                role: "user",
                content: request.user_prompt,
            },
        ],
        temperature: request.temperature,
        max_tokens: request.max_tokens,
        response_format,
        provider: structured.then(|| serde_json::json!({"require_parameters": true})),
    };

    let mut http_request = http_client.post(&request.endpoint);
    if let ChatCompletionsAuth::Bearer(api_key) = request.auth {
        http_request = http_request.bearer_auth(api_key);
    }

    let response = http_request
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&body)?)
        .send()
        .await?;

    let resp: serde_json::Value = ensure_provider_success(request.provider, response)
        .await?
        .json()
        .await?;

    completion_content(request.provider, &resp, structured)
}

fn completion_content(
    provider: &str,
    resp: &serde_json::Value,
    structured: bool,
) -> anyhow::Result<String> {
    if structured {
        let choice = &resp["choices"][0];
        anyhow::ensure!(
            choice["finish_reason"] == "stop",
            "OpenRouter structured translation did not finish normally ({}); no translations were applied",
            choice["finish_reason"]
        );
        anyhow::ensure!(
            choice["message"]["refusal"].is_null(),
            "OpenRouter refused the translation; no translations were applied"
        );
    }

    resp["choices"][0]["message"]["content"]
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow::anyhow!("{provider} returned no content"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn structured_request_sends_schema_routing_and_original_generation_settings() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            time::Duration,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/chat/completions", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            let body = loop {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if bytes.len() >= end + 4 + length {
                        break serde_json::from_slice::<serde_json::Value>(
                            &bytes[end + 4..end + 4 + length],
                        )
                        .unwrap();
                    }
                }
            };
            let response = json!({"choices": [{"finish_reason": "stop", "message": {"content": "{\"translations\":{\"1\":\"Hello\"}}"}}]}).to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
            body
        });
        let client = Arc::new(
            reqwest_middleware::ClientBuilder::new(
                reqwest::Client::builder()
                    .no_proxy()
                    .timeout(Duration::from_secs(10))
                    .build()
                    .unwrap(),
            )
            .build(),
        );
        let schema = super::super::structured_translation::response_format(1);
        let result = send_openrouter_structured_completion(
            client,
            ChatCompletionsRequest {
                provider: "test",
                endpoint,
                auth: ChatCompletionsAuth::None,
                model: "chosen-model".into(),
                system_prompt: "custom guidance".into(),
                user_prompt: "source text".into(),
                temperature: Some(0.42),
                max_tokens: Some(256),
            },
            schema.clone(),
        )
        .await
        .unwrap();
        assert_eq!(result, r#"{"translations":{"1":"Hello"}}"#);
        let sent = server.join().unwrap();
        assert_eq!(sent["response_format"], schema);
        assert_eq!(sent["provider"], json!({"require_parameters": true}));
        assert_eq!(sent["temperature"], 0.42);
        assert_eq!(sent["max_tokens"], 256);
        assert_eq!(sent["model"], "chosen-model");
        assert_eq!(sent["messages"][0]["content"], "custom guidance");
    }

    #[test]
    fn structured_completion_requires_a_complete_non_refusal_response() {
        let mut response =
            json!({"choices": [{"finish_reason": "stop", "message": {"content": "{}"}}]});
        assert_eq!(completion_content("test", &response, true).unwrap(), "{}");
        for reason in [
            json!("length"),
            json!("content_filter"),
            json!("tool_calls"),
            json!(null),
        ] {
            response["choices"][0]["finish_reason"] = reason;
            assert!(completion_content("test", &response, true).is_err());
            assert!(completion_content("test", &response, false).is_ok());
        }
        response["choices"][0]["finish_reason"] = json!("stop");
        response["choices"][0]["message"]["refusal"] = json!("refused");
        assert!(completion_content("test", &response, true).is_err());
        assert!(
            completion_content("test", &json!({"error": {"message": "failed"}}), true).is_err()
        );
    }

    #[test]
    fn legacy_requests_omit_structured_fields() {
        let request = ChatRequest {
            model: "model",
            messages: vec![],
            temperature: None,
            max_tokens: None,
            response_format: None,
            provider: None,
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({"model": "model", "messages": []})
        );
    }
}
