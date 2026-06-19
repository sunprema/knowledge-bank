//! Tool harness for KB's in-house agents.
//!
//! Lets the roundtable personas and the problem-hunting ResearchAgent **call
//! tools** mid-turn (Anthropic tool-use) instead of being limited to
//! single-shot text completions. This module owns the two pieces the model
//! does *not* get to control:
//!
//! - [`Tool`] — a callable the model may request by name. Each tool advertises
//!   a JSON-Schema [`Tool::schema`] (the `tools[]` entry sent to the API) and
//!   runs in-process via [`Tool::run`].
//! - [`ToolRegistry`] — the **allowlist**. An agent can only ever invoke a tool
//!   that lives in its registry, so the blast radius is fixed at construction
//!   time. For this phase the registry holds only corpus tools (`kb_search`,
//!   `kb_get_paper`, `kb_create_reflection`) — all in-process, no shell, no
//!   filesystem, no network egress.
//!
//! The tool-use *loop* (`run_agent`) and the tool-aware API client
//! (`complete_with_tools`) land in later steps; this module is just the trait
//! and the registry they build on.

use async_trait::async_trait;

use crate::anthropic::{extract_text, AgentMessage, AnthropicChat, ContentBlock};
use crate::KbError;

pub mod kb_tools;

/// A capability an agent may invoke by name during a turn.
///
/// Implementors wrap existing in-process logic (e.g. corpus search). The model
/// never executes anything itself — it emits a `tool_use` request and the
/// harness dispatches it here, feeding [`run`](Tool::run)'s output back as a
/// `tool_result`.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable identifier the model uses to request this tool. Must match the
    /// `"name"` in [`schema`](Tool::schema) and be unique within a registry.
    fn name(&self) -> &str;

    /// The `tools[]` entry advertised to the Messages API: an object with
    /// `name`, `description`, and an `input_schema` (JSON Schema for the
    /// arguments). Returned by-value so the registry can collect every tool's
    /// schema into the request body.
    fn schema(&self) -> serde_json::Value;

    /// Execute the tool with the model-supplied `input` (the `tool_use.input`
    /// object) and return the result text fed back as a `tool_result`.
    ///
    /// Returning `Err` is expected and recoverable: the loop converts it into
    /// an `is_error` `tool_result` so the model can adjust rather than aborting
    /// the whole turn. Reserve `Err` for genuine failures (bad arguments,
    /// backend error); a legitimate empty result is `Ok` with an explanatory
    /// string.
    async fn run(&self, input: serde_json::Value) -> Result<String, KbError>;
}

/// The set of tools an agent is allowed to call — its allowlist.
///
/// Built once per agent run. [`schemas`](ToolRegistry::schemas) feeds the API
/// request; [`dispatch`](ToolRegistry::dispatch) routes a `tool_use` request to
/// the matching tool. A name the registry doesn't know is an error (surfaced to
/// the model as an `is_error` `tool_result`), never a silent no-op.
#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    /// An empty registry — an agent with no tools (equivalent to a plain
    /// completion). Add tools with [`with`](ToolRegistry::with) or
    /// [`register`](ToolRegistry::register).
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Add a tool. Panics if a tool with the same [`name`](Tool::name) is
    /// already registered — a duplicate name is a construction-time bug
    /// (`dispatch` resolves by name, so collisions would be ambiguous), not a
    /// runtime condition to handle.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        assert!(
            !self.tools.iter().any(|t| t.name() == tool.name()),
            "duplicate tool name in registry: {:?}",
            tool.name()
        );
        self.tools.push(tool);
    }

    /// Builder form of [`register`](ToolRegistry::register), for assembling a
    /// registry in one expression: `ToolRegistry::new().with(a).with(b)`.
    #[must_use]
    pub fn with(mut self, tool: Box<dyn Tool>) -> Self {
        self.register(tool);
        self
    }

    /// `true` if no tools are registered — callers can skip the tool-use path
    /// and fall back to a plain completion.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Every tool's [`schema`](Tool::schema), in registration order — the
    /// `tools[]` array sent to the Messages API.
    pub fn schemas(&self) -> Vec<serde_json::Value> {
        self.tools.iter().map(|t| t.schema()).collect()
    }

    /// Run the tool named `name` with `input`.
    ///
    /// An unknown `name` is a recoverable [`KbError::Usage`] (the model asked
    /// for a tool outside its allowlist — likely a hallucinated name); the
    /// caller turns it into an `is_error` `tool_result` so the model can retry
    /// with a valid tool.
    pub async fn dispatch(
        &self,
        name: &str,
        input: serde_json::Value,
    ) -> Result<String, KbError> {
        match self.tools.iter().find(|t| t.name() == name) {
            Some(tool) => tool.run(input).await,
            None => Err(KbError::Usage(format!(
                "unknown tool {name:?}; available tools: {}",
                self.tool_names().join(", ")
            ))),
        }
    }

    /// Names of all registered tools, for error messages and diagnostics.
    fn tool_names(&self) -> Vec<&str> {
        self.tools.iter().map(|t| t.name()).collect()
    }
}

/// Drive a tool-using conversation to a final text answer.
///
/// This is the harness loop: it calls the model with the registry's tool
/// schemas, and while the model answers with `tool_use`, it dispatches each
/// requested tool, feeds the results back as `tool_result`s, and re-calls —
/// until the model stops (`stop_reason != "tool_use"`) or the iteration cap
/// trips.
///
/// - `messages` seeds the conversation (typically a single
///   [`AgentMessage::user_text`]); it is consumed and grown in place as the
///   loop appends assistant and `tool_result` turns.
/// - `max_iters` bounds the number of model calls — a hard stop against a
///   runaway tool loop. On hitting it, the best-effort text so far is returned
///   (with a warning); if there is none, a [`KbError::Network`] is returned so
///   the failure is explicit rather than a silent empty answer.
/// - A tool that returns `Err` is **not** fatal: its error is sent back as an
///   `is_error` `tool_result` so the model can recover or route around it.
///
/// An empty registry makes this equivalent to a single plain completion (no
/// `tools` sent, the model can't request any).
pub async fn run_agent(
    client: &AnthropicChat,
    system: &str,
    mut messages: Vec<AgentMessage>,
    registry: &ToolRegistry,
    max_iters: usize,
) -> Result<String, KbError> {
    let tools = registry.schemas();
    let mut last_text = String::new();

    for _ in 0..max_iters {
        let (blocks, stop_reason) = client.complete_with_tools(system, &messages, &tools).await?;
        let text = extract_text(&blocks);

        if stop_reason != "tool_use" {
            return Ok(text);
        }
        // Remember the model's prose in case the cap trips before it finishes.
        if !text.trim().is_empty() {
            last_text = text;
        }

        // Collect the tool requests before mutating history.
        let calls: Vec<(String, String, serde_json::Value)> = blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.clone(), name.clone(), input.clone()))
                }
                _ => None,
            })
            .collect();

        // Replay the assistant turn so the following `tool_result`s have their
        // matching `tool_use` in history. Drop `Other` blocks we can't echo
        // back faithfully (they'd serialize to a bare `{"type":"other"}`).
        let assistant: Vec<ContentBlock> = blocks
            .into_iter()
            .filter(|b| !matches!(b, ContentBlock::Other))
            .collect();
        messages.push(AgentMessage::assistant(assistant));

        if calls.is_empty() {
            // `tool_use` stop_reason but no tool_use block — nothing to run, so
            // take the text and stop rather than loop forever.
            return Ok(last_text);
        }

        let mut results = Vec::with_capacity(calls.len());
        for (id, name, input) in calls {
            let block = match registry.dispatch(&name, input).await {
                Ok(out) => ContentBlock::tool_result(id, out, false),
                Err(e) => {
                    tracing::warn!(tool = %name, error = %e, "tool run failed; feeding error back to the model");
                    ContentBlock::tool_result(id, format!("tool error: {e}"), true)
                }
            };
            results.push(block);
        }
        messages.push(AgentMessage::user(results));
    }

    tracing::warn!(max_iters, "agent hit the tool-iteration cap without a final answer");
    if last_text.trim().is_empty() {
        return Err(KbError::Network(format!(
            "agent exceeded {max_iters} tool iterations without a final answer"
        )));
    }
    Ok(last_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trivial tool: echoes its `input` back, or fails on demand, so registry
    /// behaviour can be tested without a model or any KB state.
    struct EchoTool {
        name: &'static str,
        fail: bool,
    }

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            self.name
        }

        fn schema(&self) -> serde_json::Value {
            serde_json::json!({
                "name": self.name,
                "description": "echoes its input",
                "input_schema": {
                    "type": "object",
                    "properties": { "msg": { "type": "string" } },
                    "required": ["msg"],
                },
            })
        }

        async fn run(&self, input: serde_json::Value) -> Result<String, KbError> {
            if self.fail {
                return Err(KbError::Network("backend exploded".into()));
            }
            Ok(format!("echo: {}", input["msg"].as_str().unwrap_or("")))
        }
    }

    fn echo(name: &'static str) -> Box<dyn Tool> {
        Box::new(EchoTool { name, fail: false })
    }

    #[test]
    fn new_registry_is_empty() {
        let r = ToolRegistry::new();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        assert!(r.schemas().is_empty());
    }

    #[test]
    fn schemas_collects_every_tool_in_order() {
        let r = ToolRegistry::new().with(echo("a")).with(echo("b"));
        assert_eq!(r.len(), 2);
        let schemas = r.schemas();
        assert_eq!(schemas.len(), 2);
        assert_eq!(schemas[0]["name"], "a");
        assert_eq!(schemas[1]["name"], "b");
        // schemas carry an input_schema so the API can validate arguments
        assert_eq!(schemas[0]["input_schema"]["type"], "object");
    }

    #[tokio::test]
    async fn dispatch_routes_to_the_named_tool() {
        let r = ToolRegistry::new().with(echo("alpha")).with(echo("beta"));
        let out = r
            .dispatch("beta", serde_json::json!({ "msg": "hi" }))
            .await
            .unwrap();
        assert_eq!(out, "echo: hi");
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_is_a_usage_error_listing_options() {
        let r = ToolRegistry::new().with(echo("alpha"));
        let err = r
            .dispatch("nope", serde_json::json!({}))
            .await
            .unwrap_err();
        match err {
            KbError::Usage(msg) => {
                assert!(msg.contains("nope"), "got: {msg}");
                assert!(msg.contains("alpha"), "should list available tools: {msg}");
            }
            other => panic!("expected Usage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_propagates_tool_errors() {
        let mut r = ToolRegistry::new();
        r.register(Box::new(EchoTool { name: "boom", fail: true }));
        assert!(matches!(
            r.dispatch("boom", serde_json::json!({})).await,
            Err(KbError::Network(_))
        ));
    }

    #[test]
    #[should_panic(expected = "duplicate tool name")]
    fn duplicate_tool_name_panics() {
        let _ = ToolRegistry::new().with(echo("dup")).with(echo("dup"));
    }

    // --- run_agent loop -----------------------------------------------------

    use crate::anthropic::AnthropicChat;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    /// Serve a fixed queue of `(status, body)` responses on a throwaway port,
    /// one per request, returning the captured request bodies on join. Mirrors
    /// the mock used in `anthropic.rs` tests but local to the harness so the
    /// loop can be exercised end-to-end without a live API.
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
                captured.push(String::from_utf8_lossy(&req_body).to_string());

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

    fn client(base_url: String) -> AnthropicChat {
        AnthropicChat::new("sk-ant-TEST".to_string(), "claude-test", base_url)
    }

    fn text_body(text: &str) -> String {
        serde_json::json!({
            "role": "assistant",
            "stop_reason": "end_turn",
            "content": [ { "type": "text", "text": text } ],
        })
        .to_string()
    }

    fn tool_use_body(id: &str, name: &str, input: serde_json::Value) -> String {
        serde_json::json!({
            "role": "assistant",
            "stop_reason": "tool_use",
            "content": [
                { "type": "text", "text": "working on it" },
                { "type": "tool_use", "id": id, "name": name, "input": input },
            ],
        })
        .to_string()
    }

    /// Parse the captured request body and return its `messages` array.
    fn messages_of(req_body: &str) -> serde_json::Value {
        let body: serde_json::Value = serde_json::from_str(req_body).unwrap();
        body["messages"].clone()
    }

    #[tokio::test]
    async fn run_agent_returns_text_when_no_tool_use() {
        // Model answers directly; the tool is never called.
        let (base, server) = mock_server(vec![(200, text_body("the answer is 42"))]);
        let registry = ToolRegistry::new().with(echo("echo"));
        let out = run_agent(
            &client(base),
            "be terse",
            vec![AgentMessage::user_text("what is the answer?")],
            &registry,
            4,
        )
        .await
        .unwrap();
        assert_eq!(out, "the answer is 42");
        assert_eq!(server.join().unwrap().len(), 1, "should call the model once");
    }

    #[tokio::test]
    async fn run_agent_runs_one_tool_then_returns_text() {
        // First turn requests the tool; second turn (after the tool_result) answers.
        let (base, server) = mock_server(vec![
            (200, tool_use_body("toolu_1", "echo", serde_json::json!({ "msg": "ping" }))),
            (200, text_body("done: echo: ping")),
        ]);
        let registry = ToolRegistry::new().with(echo("echo"));
        let out = run_agent(
            &client(base),
            "",
            vec![AgentMessage::user_text("use the tool")],
            &registry,
            4,
        )
        .await
        .unwrap();
        assert_eq!(out, "done: echo: ping");

        // The second request must carry: the replayed assistant tool_use turn,
        // then a user turn with the matching tool_result.
        let reqs = server.join().unwrap();
        assert_eq!(reqs.len(), 2);
        let msgs = messages_of(&reqs[1]);
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"][1]["type"], "tool_use");
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(msgs[2]["content"][0]["content"], "echo: ping");
        // a successful tool_result omits is_error
        assert!(msgs[2]["content"][0].get("is_error").is_none());
    }

    #[tokio::test]
    async fn run_agent_feeds_tool_errors_back_as_is_error() {
        // The tool fails; its error must come back as an is_error tool_result so
        // the model can recover, then it answers.
        let (base, server) = mock_server(vec![
            (200, tool_use_body("toolu_9", "boom", serde_json::json!({}))),
            (200, text_body("recovered")),
        ]);
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool { name: "boom", fail: true }));
        let out = run_agent(
            &client(base),
            "",
            vec![AgentMessage::user_text("go")],
            &registry,
            4,
        )
        .await
        .unwrap();
        assert_eq!(out, "recovered");

        let reqs = server.join().unwrap();
        let msgs = messages_of(&reqs[1]);
        let result = &msgs[2]["content"][0];
        assert_eq!(result["type"], "tool_result");
        assert_eq!(result["is_error"], true);
        assert!(
            result["content"].as_str().unwrap().contains("tool error"),
            "got: {result}"
        );
    }

    #[tokio::test]
    async fn run_agent_stops_at_iteration_cap() {
        // Model loops on tool_use forever; the cap must stop it after max_iters
        // model calls and surface the best-effort prose.
        let (base, server) = mock_server(vec![
            (200, tool_use_body("t1", "echo", serde_json::json!({ "msg": "a" }))),
            (200, tool_use_body("t2", "echo", serde_json::json!({ "msg": "b" }))),
        ]);
        let registry = ToolRegistry::new().with(echo("echo"));
        let out = run_agent(
            &client(base),
            "",
            vec![AgentMessage::user_text("loop")],
            &registry,
            2,
        )
        .await
        .unwrap();
        // best-effort text from the last tool_use turn
        assert_eq!(out, "working on it");
        assert_eq!(server.join().unwrap().len(), 2, "must stop at the cap, not exceed it");
    }
}
