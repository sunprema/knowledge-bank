//! Anthropic Messages API client for the multi-agent roundtable.
//!
//! There is no official Anthropic Rust SDK, so this is a thin raw-HTTP client
//! over `POST /v1/messages` (the documented surface), mirroring the retry
//! discipline and key-hygiene of [`crate::chat::OpenAiChat`] so the roundtable
//! can run agents on Claude models alongside OpenAI ones.
//!
//! SECURITY: `ANTHROPIC_API_KEY` must never appear in logs, error messages, or
//! debug output — the Debug impl omits it and 401/403 responses are reported
//! without echoing the key.
//!
//! Sampling note: Claude Opus 4.8 / 4.7 (and Fable 5) REJECT `temperature` /
//! `top_p` with a 400, unlike OpenAI's chat API. So this client never sends
//! sampling params — depth is a model/effort concern, not a temperature one.

use crate::chat::ChatMessage;
use crate::KbError;
use serde::Deserialize;

/// Backoff schedule for 429/5xx (matches the OpenAI client): 1s, 2s, 4s.
const DEFAULT_BACKOFF_MS: [u64; 3] = [1000, 2000, 4000];

/// Anthropic API version header value (the documented, stable version).
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Per-turn output cap. Roundtable turns are a few paragraphs; this keeps
/// latency and cost bounded while leaving room for the synthesis.
const MAX_TOKENS: u32 = 2048;

pub struct AnthropicChat {
    client: reqwest::Client,
    api_key: String,
    model: String,
    /// e.g. "https://api.anthropic.com/v1" — overridable for tests.
    base_url: String,
    backoff_ms: [u64; 3],
}

impl std::fmt::Debug for AnthropicChat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicChat")
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive() // api_key intentionally omitted
    }
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    text: Option<String>,
}

impl AnthropicChat {
    /// Construct from `ANTHROPIC_API_KEY`. Missing key ⇒ `Config` error telling
    /// the user to export it (without echoing anything).
    pub fn from_env(model: &str) -> Result<Self, KbError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| {
                KbError::Config(
                    "ANTHROPIC_API_KEY is not set; export it first, e.g. \
                     `export ANTHROPIC_API_KEY=sk-ant-...` (the key is never logged)"
                        .to_string(),
                )
            })?;
        Ok(Self::new(api_key, model, "https://api.anthropic.com/v1".to_string()))
    }

    pub fn new(api_key: String, model: &str, base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model: model.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            backoff_ms: DEFAULT_BACKOFF_MS,
        }
    }

    #[cfg(test)]
    fn with_backoff_ms(api_key: String, model: &str, base_url: String, backoff_ms: [u64; 3]) -> Self {
        let mut c = Self::new(api_key, model, base_url);
        c.backoff_ms = backoff_ms;
        c
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// One `POST {base_url}/messages`. The Anthropic Messages API takes `system`
    /// as a top-level string (not a message role), so we lift any `system`
    /// turns out of `messages` and concatenate them. Returns the assistant text.
    ///
    /// Retry policy mirrors the OpenAI client: 429/5xx ⇒ exponential backoff
    /// (1s, 2s, 4s), max 3 retries, then `Network`. 401/403 ⇒ `Config`.
    pub async fn complete(&self, messages: &[ChatMessage]) -> Result<String, KbError> {
        let system: String = messages
            .iter()
            .filter(|m| m.role == "system")
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let turns: Vec<serde_json::Value> = messages
            .iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
            .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
            .collect();

        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "messages": turns,
        });
        if !system.is_empty() {
            body["system"] = serde_json::Value::String(system);
        }

        let url = format!("{}/messages", self.base_url);
        let max_retries = self.backoff_ms.len();
        let mut last_status = None;
        for attempt in 0..=max_retries {
            let resp = self
                .client
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .json(&body)
                .send()
                .await?;

            let status = resp.status();
            if status.is_success() {
                return self.parse_response(resp).await;
            }
            if status.as_u16() == 401 || status.as_u16() == 403 {
                // NEVER include the key.
                return Err(KbError::Config(format!(
                    "invalid ANTHROPIC_API_KEY (Messages API returned HTTP {}); \
                     check the key and re-export it",
                    status.as_u16()
                )));
            }
            if status.as_u16() == 429 || status.is_server_error() {
                last_status = Some(status.as_u16());
                if attempt < max_retries {
                    let delay = self.backoff_ms[attempt];
                    tracing::warn!(
                        status = status.as_u16(),
                        attempt = attempt + 1,
                        delay_ms = delay,
                        "Anthropic API transient failure, retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    continue;
                }
                break;
            }
            return Err(KbError::Network(format!(
                "Anthropic API returned HTTP {}",
                status.as_u16()
            )));
        }

        Err(KbError::Network(format!(
            "Anthropic API failed after {max_retries} retries (last HTTP status: {})",
            last_status.map_or_else(|| "unknown".to_string(), |s| s.to_string())
        )))
    }

    async fn parse_response(&self, resp: reqwest::Response) -> Result<String, KbError> {
        let parsed: MessagesResponse = resp.json().await.map_err(|e| {
            KbError::Network(format!("malformed Anthropic response: {}", e.without_url()))
        })?;
        // Concatenate text blocks (a refusal or tool turn may carry none).
        let text: String = parsed
            .content
            .into_iter()
            .filter(|b| b.block_type == "text")
            .filter_map(|b| b.text)
            .collect::<Vec<_>>()
            .join("");
        if text.trim().is_empty() {
            return Err(KbError::Network("Anthropic API returned no text content".into()));
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const KEY: &str = "sk-ant-TEST-SECRET-DO-NOT-LEAK";

    fn mock_server(
        responses: Vec<(u16, String)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let mut captured = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = Vec::new();
                let mut byte = [0u8; 1];
                while !buf.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    buf.push(byte[0]);
                }
                let headers = String::from_utf8_lossy(&buf).to_string();
                let content_length: usize = headers
                    .lines()
                    .find_map(|l| {
                        let (name, value) = l.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().ok())?
                    })
                    .unwrap_or(0);
                let mut req_body = vec![0u8; content_length];
                stream.read_exact(&mut req_body).unwrap();
                captured.push(format!("{headers}{}", String::from_utf8_lossy(&req_body)));

                let resp = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(resp.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
            captured
        });
        (format!("http://127.0.0.1:{port}/v1"), handle)
    }

    fn ok_body(text: &str) -> String {
        serde_json::json!({
            "content": [ { "type": "text", "text": text } ],
            "role": "assistant"
        })
        .to_string()
    }

    fn chat(base_url: String) -> AnthropicChat {
        AnthropicChat::with_backoff_ms(KEY.to_string(), "claude-test", base_url, [1, 1, 1])
    }

    #[tokio::test]
    async fn happy_path_lifts_system_and_sends_headers() {
        let (base, server) = mock_server(vec![(200, ok_body("hi from claude"))]);
        let c = chat(base);
        let msgs = vec![ChatMessage::system("be the technologist"), ChatMessage::user("brainstorm")];
        let got = c.complete(&msgs).await.unwrap();
        assert_eq!(got, "hi from claude");

        let reqs = server.join().unwrap();
        let req = &reqs[0];
        assert!(req.starts_with("POST /v1/messages HTTP/1.1\r\n"), "got: {req}");
        assert!(req.to_lowercase().contains("x-api-key:"));
        assert!(req.contains(KEY));
        assert!(req.to_lowercase().contains("anthropic-version: 2023-06-01"));
        let json_start = req.find("\r\n\r\n").unwrap() + 4;
        let body: serde_json::Value = serde_json::from_str(&req[json_start..]).unwrap();
        assert_eq!(body["model"], "claude-test");
        // system lifted out of messages, into top-level field
        assert_eq!(body["system"], "be the technologist");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "brainstorm");
        // never send sampling params (they 400 on Opus 4.8)
        assert!(body.get("temperature").is_none());
    }

    #[tokio::test]
    async fn retries_on_529_then_succeeds() {
        let (base, server) = mock_server(vec![
            (529, r#"{"type":"error","error":{"type":"overloaded_error"}}"#.to_string()),
            (200, ok_body("ok")),
        ]);
        let c = chat(base);
        let got = c.complete(&[ChatMessage::user("x")]).await.unwrap();
        assert_eq!(got, "ok");
        assert_eq!(server.join().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn auth_failure_is_config_error_without_the_key() {
        let (base, server) = mock_server(vec![(401, r#"{"error":{"message":"bad key"}}"#.to_string())]);
        let c = chat(base);
        let err = c.complete(&[ChatMessage::user("x")]).await.unwrap_err();
        match err {
            KbError::Config(msg) => {
                assert!(msg.contains("ANTHROPIC_API_KEY"), "got: {msg}");
                assert!(!msg.contains(KEY), "key leaked: {msg}");
                assert!(!msg.contains("sk-ant"), "key fragment leaked: {msg}");
            }
            other => panic!("expected Config, got {other:?}"),
        }
        server.join().unwrap();
    }

    #[tokio::test]
    async fn empty_text_is_a_network_error() {
        let (base, server) = mock_server(vec![(200, r#"{"content":[]}"#.to_string())]);
        let c = chat(base);
        assert!(matches!(
            c.complete(&[ChatMessage::user("x")]).await,
            Err(KbError::Network(_))
        ));
        server.join().unwrap();
    }

    #[test]
    fn debug_never_exposes_the_key() {
        let c = AnthropicChat::new(KEY.to_string(), "m", "http://localhost/v1".into());
        let dbg = format!("{c:?}");
        assert!(!dbg.contains(KEY), "Debug leaked the key: {dbg}");
        assert!(dbg.contains('m'));
    }
}
