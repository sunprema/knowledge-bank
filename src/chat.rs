//! OpenAI chat-completions client for chat-over-corpus (web app `POST /chat`).
//!
//! The KB's embedding pipeline is already OpenAI-keyed (`OPENAI_API_KEY`,
//! `text-embedding-3-small`), so chat reuses the same single key and the same
//! retry discipline as [`crate::embed::OpenAiEmbedder`] rather than pulling in
//! a second provider/credential.
//!
//! SECURITY: `OPENAI_API_KEY` must never appear in logs, error messages, or
//! debug output — the Debug impl omits it and 401/403 responses are reported
//! without echoing the key (cf. the embedder).

use crate::KbError;
use serde::{Deserialize, Serialize};

/// Backoff schedule for 429/5xx responses (mirrors the embedder): 1s, 2s, 4s —
/// one entry per retry, so max 3 retries (4 attempts total).
const DEFAULT_BACKOFF_MS: [u64; 3] = [1000, 2000, 4000];

/// One turn in a chat conversation. Roles follow the OpenAI convention
/// (`system` | `user` | `assistant`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system".into(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".into(), content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: "assistant".into(), content: content.into() }
    }

    /// Keep only the roles the chat API accepts, so a client-supplied history
    /// can't inject arbitrary roles into the request.
    pub fn is_valid_role(&self) -> bool {
        matches!(self.role.as_str(), "system" | "user" | "assistant")
    }
}

pub struct OpenAiChat {
    client: reqwest::Client,
    api_key: String,
    model: String,
    /// e.g. "https://api.openai.com/v1" — overridable for tests.
    base_url: String,
    /// Shrunk in tests so retry paths don't sleep for real.
    backoff_ms: [u64; 3],
    /// Optional `max_tokens` cap. `None` ⇒ the field is omitted (model default).
    /// Raised by longer-form paths like the Clean Read rewrite.
    max_tokens: Option<u32>,
}

impl std::fmt::Debug for OpenAiChat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiChat")
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive() // api_key intentionally omitted
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: Option<String>,
}

impl OpenAiChat {
    /// Construct from `OPENAI_API_KEY`. Missing key ⇒ `Config` error telling
    /// the user to export it (without echoing anything).
    pub fn from_env(model: &str) -> Result<Self, KbError> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| {
                KbError::Config(
                    "OPENAI_API_KEY is not set; export it first, e.g. \
                     `export OPENAI_API_KEY=sk-...` (the key is never logged)"
                        .to_string(),
                )
            })?;
        Ok(Self::new(api_key, model, "https://api.openai.com/v1".to_string()))
    }

    /// Construct with an explicit key and base URL (mock server in tests).
    pub fn new(api_key: String, model: &str, base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model: model.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            backoff_ms: DEFAULT_BACKOFF_MS,
            max_tokens: None,
        }
    }

    /// Set the output token cap (`max_tokens`) for this client. Used by
    /// longer-form paths like the Clean Read rewrite; otherwise the field is
    /// omitted and the model's default applies.
    #[must_use]
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = Some(n);
        self
    }

    /// Test-only constructor with a custom backoff so retry tests don't sleep.
    #[cfg(test)]
    fn with_backoff_ms(api_key: String, model: &str, base_url: String, backoff_ms: [u64; 3]) -> Self {
        let mut c = Self::new(api_key, model, base_url);
        c.backoff_ms = backoff_ms;
        c
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// One `POST {base_url}/chat/completions` (body: `{ model, messages,
    /// temperature }`), returning the assistant's reply text.
    ///
    /// Retry policy (mirrors the embedder): on 429/5xx, exponential backoff
    /// (1s, 2s, 4s), max 3 retries, then `Network`. 401/403 ⇒ `Config`
    /// ("invalid OPENAI_API_KEY" — never include the key).
    pub async fn complete(
        &self,
        messages: &[ChatMessage],
        temperature: f32,
    ) -> Result<String, KbError> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "temperature": temperature,
        });
        if let Some(mt) = self.max_tokens {
            body["max_tokens"] = mt.into();
        }

        let max_retries = self.backoff_ms.len();
        let mut last_status = None;
        for attempt in 0..=max_retries {
            let resp = self
                .client
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await?;

            let status = resp.status();
            if status.is_success() {
                return self.parse_response(resp).await;
            }
            if status.as_u16() == 401 || status.as_u16() == 403 {
                // NEVER include the key (or any content that could echo it).
                return Err(KbError::Config(format!(
                    "invalid OPENAI_API_KEY (chat API returned HTTP {}); \
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
                        "chat API transient failure, retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    continue;
                }
                break;
            }
            return Err(KbError::Network(format!(
                "chat API returned HTTP {}",
                status.as_u16()
            )));
        }

        Err(KbError::Network(format!(
            "chat API failed after {max_retries} retries (last HTTP status: {})",
            last_status.map_or_else(|| "unknown".to_string(), |s| s.to_string())
        )))
    }

    /// Streaming variant of [`complete`](Self::complete): sets `stream: true`
    /// and parses the SSE token deltas, invoking `on_delta` with each text
    /// fragment as it arrives. Returns the full concatenated answer.
    ///
    /// Retry only covers the initial connect (before any token streams); once
    /// the 2xx body starts flowing a mid-stream error surfaces as `Network`
    /// (no replay, since partial tokens were already delivered). Same
    /// key-hygiene discipline as [`complete`](Self::complete).
    pub async fn complete_stream<F: FnMut(&str)>(
        &self,
        messages: &[ChatMessage],
        temperature: f32,
        mut on_delta: F,
    ) -> Result<String, KbError> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "temperature": temperature,
            "stream": true,
        });
        if let Some(mt) = self.max_tokens {
            body["max_tokens"] = mt.into();
        }

        let max_retries = self.backoff_ms.len();
        let mut resp = None;
        for attempt in 0..=max_retries {
            let r = self
                .client
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await?;
            let status = r.status();
            if status.is_success() {
                resp = Some(r);
                break;
            }
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(KbError::Config(format!(
                    "invalid OPENAI_API_KEY (chat API returned HTTP {}); \
                     check the key and re-export it",
                    status.as_u16()
                )));
            }
            if (status.as_u16() == 429 || status.is_server_error()) && attempt < max_retries {
                let delay = self.backoff_ms[attempt];
                tracing::warn!(status = status.as_u16(), attempt = attempt + 1, delay_ms = delay,
                    "chat stream API transient failure, retrying");
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                continue;
            }
            return Err(KbError::Network(format!(
                "chat stream API returned HTTP {}",
                status.as_u16()
            )));
        }
        let resp = resp.ok_or_else(|| {
            KbError::Network(format!("chat stream API failed after {max_retries} retries"))
        })?;

        // Parse the SSE byte stream: `data: {json}` lines, terminated by
        // `data: [DONE]`. Each json carries `choices[0].delta.content`.
        let mut resp = resp;
        let mut buf = String::new();
        let mut answer = String::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| {
            KbError::Network(format!("chat stream interrupted: {}", e.without_url()))
        })? {
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(nl) = buf.find('\n') {
                let line = buf[..nl].trim().to_string();
                buf.drain(..=nl);
                let Some(data) = line.strip_prefix("data:") else { continue };
                let data = data.trim();
                if data == "[DONE]" {
                    return Ok(answer);
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(delta) = v
                        .get("choices")
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("delta"))
                        .and_then(|d| d.get("content"))
                        .and_then(|t| t.as_str())
                    {
                        if !delta.is_empty() {
                            answer.push_str(delta);
                            on_delta(delta);
                        }
                    }
                }
            }
        }
        Ok(answer)
    }

    async fn parse_response(&self, resp: reqwest::Response) -> Result<String, KbError> {
        let parsed: ChatResponse = resp.json().await.map_err(|e| {
            KbError::Network(format!("malformed chat API response: {}", e.without_url()))
        })?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| KbError::Network("chat API returned no completion".into()))?;
        Ok(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const KEY: &str = "sk-TEST-SECRET-DO-NOT-LEAK";

    /// Tiny canned-response HTTP server (same shape as the embedder's tests).
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

    fn ok_body(content: &str) -> String {
        serde_json::json!({
            "choices": [ { "message": { "role": "assistant", "content": content } } ]
        })
        .to_string()
    }

    fn chat(base_url: String) -> OpenAiChat {
        OpenAiChat::with_backoff_ms(KEY.to_string(), "test-model", base_url, [1, 1, 1])
    }

    #[tokio::test]
    async fn happy_path_returns_content_and_sends_messages() {
        let (base, server) = mock_server(vec![(200, ok_body("hello from the model"))]);
        let c = chat(base);
        let msgs = vec![ChatMessage::system("be brief"), ChatMessage::user("hi")];
        let got = c.complete(&msgs, 0.2).await.unwrap();
        assert_eq!(got, "hello from the model");

        let reqs = server.join().unwrap();
        let req = &reqs[0];
        assert!(req.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"), "got: {req}");
        assert!(req.contains(&format!("Bearer {KEY}")));
        let json_start = req.find("\r\n\r\n").unwrap() + 4;
        let body: serde_json::Value = serde_json::from_str(&req[json_start..]).unwrap();
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "hi");
    }

    #[tokio::test]
    async fn retries_on_429_then_succeeds() {
        let (base, server) = mock_server(vec![
            (429, r#"{"error":{"message":"rate limited"}}"#.to_string()),
            (200, ok_body("ok")),
        ]);
        let c = chat(base);
        let got = c.complete(&[ChatMessage::user("x")], 0.0).await.unwrap();
        assert_eq!(got, "ok");
        assert_eq!(server.join().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn auth_failure_is_config_error_without_the_key() {
        let (base, server) = mock_server(vec![(401, r#"{"error":{"message":"bad key"}}"#.to_string())]);
        let c = chat(base);
        let err = c.complete(&[ChatMessage::user("x")], 0.0).await.unwrap_err();
        match err {
            KbError::Config(msg) => {
                assert!(msg.contains("OPENAI_API_KEY"), "got: {msg}");
                assert!(!msg.contains(KEY), "key leaked: {msg}");
                assert!(!msg.contains("sk-"), "key fragment leaked: {msg}");
            }
            other => panic!("expected Config, got {other:?}"),
        }
        server.join().unwrap();
    }

    #[tokio::test]
    async fn empty_choices_is_a_network_error() {
        let (base, server) = mock_server(vec![(200, r#"{"choices":[]}"#.to_string())]);
        let c = chat(base);
        assert!(matches!(
            c.complete(&[ChatMessage::user("x")], 0.0).await,
            Err(KbError::Network(_))
        ));
        server.join().unwrap();
    }

    #[test]
    fn debug_never_exposes_the_key() {
        let c = OpenAiChat::new(KEY.to_string(), "m", "http://localhost/v1".into());
        let dbg = format!("{c:?}");
        assert!(!dbg.contains(KEY), "Debug leaked the key: {dbg}");
        assert!(dbg.contains('m'));
    }

    #[test]
    fn role_validation() {
        assert!(ChatMessage::user("a").is_valid_role());
        assert!(ChatMessage::assistant("a").is_valid_role());
        assert!(ChatMessage::system("a").is_valid_role());
        assert!(!ChatMessage { role: "tool".into(), content: "a".into() }.is_valid_role());
    }
}
