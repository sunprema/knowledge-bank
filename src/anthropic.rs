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
use serde::{Deserialize, Serialize};

/// One content block in a Messages-API turn.
///
/// The plain [`complete`](AnthropicChat::complete) path only ever sees
/// [`Text`](ContentBlock::Text), but the tool-use path needs the full set: a
/// model turn may carry [`ToolUse`](ContentBlock::ToolUse) requests, and we
/// reply with [`ToolResult`](ContentBlock::ToolResult) blocks. Tagged on
/// `"type"` to match the wire format exactly, so the same enum both
/// deserializes responses and serializes requests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Assistant prose (or a user text turn).
    Text { text: String },
    /// A model request to run a tool: `name` with JSON `input`, tagged by `id`
    /// so the matching [`ToolResult`](ContentBlock::ToolResult) can refer back.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Our answer to a [`ToolUse`](ContentBlock::ToolUse), echoing its `id` in
    /// `tool_use_id`. `is_error` marks a failed tool run so the model can
    /// recover; it is omitted from the request when false.
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "is_false")]
        is_error: bool,
    },
    /// Any block type we don't model (e.g. extended-thinking blocks). Lets a
    /// response deserialize cleanly instead of erroring on an unknown `type`;
    /// these are ignored by [`extract_text`] and the tool loop.
    #[serde(other)]
    Other,
}

impl ContentBlock {
    /// A text block.
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text { text: s.into() }
    }

    /// A `tool_result` answering the `tool_use` with id `tool_use_id`.
    pub fn tool_result(tool_use_id: impl Into<String>, content: impl Into<String>, is_error: bool) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error,
        }
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde `skip_serializing_if` requires `&bool`
fn is_false(b: &bool) -> bool {
    !*b
}

/// A single conversation turn on the tool-use path: a `user` or `assistant`
/// role with one or more [`ContentBlock`]s.
///
/// Distinct from [`ChatMessage`](crate::chat::ChatMessage) (whose content is a
/// plain `String`): tool turns carry structured blocks, and only this path
/// needs them, so the plain-text clients stay untouched. `system` is *not* a
/// turn here — it is passed separately to
/// [`complete_with_tools`](AnthropicChat::complete_with_tools), matching the
/// Messages API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMessage {
    pub role: String,
    pub content: Vec<ContentBlock>,
}

impl AgentMessage {
    /// A `user` turn with the given blocks.
    pub fn user(content: Vec<ContentBlock>) -> Self {
        Self { role: "user".into(), content }
    }

    /// A `user` turn carrying a single text block — the usual way to seed a
    /// conversation (e.g. from an existing prompt string).
    pub fn user_text(text: impl Into<String>) -> Self {
        Self::user(vec![ContentBlock::text(text)])
    }

    /// An `assistant` turn with the given blocks — typically the model's own
    /// prior response (text + any `tool_use`) replayed back into the loop.
    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Self { role: "assistant".into(), content }
    }
}

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
    /// Why the model stopped: `end_turn`, `tool_use`, `max_tokens`, … Absent on
    /// some error shapes, so optional.
    #[serde(default)]
    stop_reason: Option<String>,
}

/// Concatenate the text of every [`ContentBlock::Text`] in `blocks`, ignoring
/// tool and unknown blocks. Used by the plain `complete` path and for surfacing
/// a tool turn's accompanying prose.
pub(crate) fn extract_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
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

    /// Plain text completion. The Anthropic Messages API takes `system` as a
    /// top-level string (not a message role), so we lift any `system` turns out
    /// of `messages` and concatenate them. Returns the assistant text.
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

        let parsed = self.send(&body).await?;
        let text = extract_text(&parsed.content);
        if text.trim().is_empty() {
            return Err(KbError::Network("Anthropic API returned no text content".into()));
        }
        Ok(text)
    }

    /// Tool-aware completion: the request carries a `tools` array, so the model
    /// may answer with `tool_use` blocks. Returns the full block list **and**
    /// the `stop_reason`, so the caller's loop can decide whether to run tools
    /// and re-call, or stop. Unlike [`complete`](Self::complete) this does not
    /// reduce to text — the caller owns the tool-use loop.
    ///
    /// `system` is the top-level system prompt (empty string ⇒ omitted).
    /// `tools` is the schema array (e.g. from `ToolRegistry::schemas`); an empty
    /// slice ⇒ the field is omitted (a plain turn). Same retry/key-hygiene
    /// discipline as [`complete`](Self::complete).
    pub async fn complete_with_tools(
        &self,
        system: &str,
        messages: &[AgentMessage],
        tools: &[serde_json::Value],
    ) -> Result<(Vec<ContentBlock>, String), KbError> {
        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "messages": messages,
        });
        if !system.is_empty() {
            body["system"] = serde_json::Value::String(system.to_string());
        }
        if !tools.is_empty() {
            body["tools"] = serde_json::Value::Array(tools.to_vec());
        }

        let parsed = self.send(&body).await?;
        // `stop_reason` is normally present on success; default to "end_turn"
        // so a missing field can't be mistaken for an open tool loop.
        let stop_reason = parsed.stop_reason.unwrap_or_else(|| "end_turn".to_string());
        Ok((parsed.content, stop_reason))
    }

    /// `POST {base_url}/messages` with `body`, applying the shared retry and
    /// key-hygiene policy, and parse the success response. Both [`complete`] and
    /// [`complete_with_tools`] go through here so retry/auth behaviour stays
    /// identical.
    ///
    /// 429/5xx ⇒ exponential backoff (max `backoff_ms.len()` retries) then
    /// `Network`; 401/403 ⇒ `Config` (never echoing the key); other non-2xx ⇒
    /// `Network`.
    ///
    /// [`complete`]: Self::complete
    /// [`complete_with_tools`]: Self::complete_with_tools
    async fn send(&self, body: &serde_json::Value) -> Result<MessagesResponse, KbError> {
        let url = format!("{}/messages", self.base_url);
        let max_retries = self.backoff_ms.len();
        let mut last_status = None;
        for attempt in 0..=max_retries {
            let resp = self
                .client
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .json(body)
                .send()
                .await?;

            let status = resp.status();
            if status.is_success() {
                return resp.json().await.map_err(|e| {
                    KbError::Network(format!("malformed Anthropic response: {}", e.without_url()))
                });
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

    /// A `stop_reason: "tool_use"` response: a line of prose plus one tool_use
    /// request — the shape that drives the agentic loop.
    fn tool_use_body(id: &str, name: &str, input: serde_json::Value) -> String {
        serde_json::json!({
            "role": "assistant",
            "stop_reason": "tool_use",
            "content": [
                { "type": "text", "text": "let me look that up" },
                { "type": "tool_use", "id": id, "name": name, "input": input },
            ],
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

    #[tokio::test]
    async fn complete_with_tools_sends_tools_and_returns_tool_use() {
        let (base, server) = mock_server(vec![(
            200,
            tool_use_body("toolu_1", "kb_search", serde_json::json!({ "query": "rag" })),
        )]);
        let c = chat(base);
        let tools = vec![serde_json::json!({
            "name": "kb_search",
            "description": "search the corpus",
            "input_schema": { "type": "object", "properties": { "query": { "type": "string" } } },
        })];
        let msgs = vec![AgentMessage::user_text("find papers on rag")];
        let (blocks, stop_reason) = c.complete_with_tools("be terse", &msgs, &tools).await.unwrap();

        assert_eq!(stop_reason, "tool_use");
        // both the prose and the tool_use block come back, in order
        assert_eq!(blocks[0], ContentBlock::text("let me look that up"));
        match &blocks[1] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_1");
                assert_eq!(name, "kb_search");
                assert_eq!(input["query"], "rag");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }

        // the request carried system, the tools array, and structured content
        let reqs = server.join().unwrap();
        let json_start = reqs[0].find("\r\n\r\n").unwrap() + 4;
        let body: serde_json::Value = serde_json::from_str(&reqs[0][json_start..]).unwrap();
        assert_eq!(body["system"], "be terse");
        assert_eq!(body["tools"][0]["name"], "kb_search");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"][0]["type"], "text");
        assert_eq!(body["messages"][0]["content"][0]["text"], "find papers on rag");
        // never send sampling params (they 400 on Opus 4.8)
        assert!(body.get("temperature").is_none());
    }

    #[tokio::test]
    async fn complete_with_tools_omits_tools_when_empty_and_defaults_stop_reason() {
        // No tools, and a response without stop_reason ⇒ defaults to end_turn.
        let (base, server) = mock_server(vec![(200, ok_body("plain answer"))]);
        let c = chat(base);
        let msgs = vec![AgentMessage::user_text("hi")];
        let (blocks, stop_reason) = c.complete_with_tools("", &msgs, &[]).await.unwrap();
        assert_eq!(stop_reason, "end_turn");
        assert_eq!(extract_text(&blocks), "plain answer");

        let reqs = server.join().unwrap();
        let json_start = reqs[0].find("\r\n\r\n").unwrap() + 4;
        let body: serde_json::Value = serde_json::from_str(&reqs[0][json_start..]).unwrap();
        assert!(body.get("tools").is_none(), "empty tools must be omitted");
        assert!(body.get("system").is_none(), "empty system must be omitted");
    }

    #[tokio::test]
    async fn complete_with_tools_retries_and_reuses_auth_path() {
        // Shares the retry/auth path with `complete`: 401 ⇒ Config without the key.
        let (base, server) =
            mock_server(vec![(401, r#"{"error":{"message":"bad key"}}"#.to_string())]);
        let c = chat(base);
        let err = c
            .complete_with_tools("", &[AgentMessage::user_text("x")], &[])
            .await
            .unwrap_err();
        match err {
            KbError::Config(msg) => assert!(!msg.contains("sk-ant"), "key leaked: {msg}"),
            other => panic!("expected Config, got {other:?}"),
        }
        server.join().unwrap();
    }

    #[test]
    fn content_block_serde_roundtrips_and_omits_false_is_error() {
        // tool_result with is_error=false must omit the field on the wire.
        let ok = ContentBlock::tool_result("toolu_1", "42 results", false);
        let v = serde_json::to_value(&ok).unwrap();
        assert_eq!(v["type"], "tool_result");
        assert_eq!(v["tool_use_id"], "toolu_1");
        assert!(v.get("is_error").is_none(), "false is_error must be omitted");

        // is_error=true is sent, and the block round-trips.
        let err = ContentBlock::tool_result("toolu_2", "boom", true);
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["is_error"], true);
        let back: ContentBlock = serde_json::from_value(v).unwrap();
        assert_eq!(back, err);
    }

    #[test]
    fn unknown_block_type_deserializes_as_other() {
        // A future/unmodeled block type (e.g. extended thinking) must not error.
        let v = serde_json::json!({ "type": "thinking", "thinking": "hmm" });
        let block: ContentBlock = serde_json::from_value(v).unwrap();
        assert_eq!(block, ContentBlock::Other);
        // and it contributes no text
        assert_eq!(extract_text(&[block]), "");
    }

    #[test]
    fn debug_never_exposes_the_key() {
        let c = AnthropicChat::new(KEY.to_string(), "m", "http://localhost/v1".into());
        let dbg = format!("{c:?}");
        assert!(!dbg.contains(KEY), "Debug leaked the key: {dbg}");
        assert!(dbg.contains('m'));
    }
}
