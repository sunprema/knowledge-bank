# Agent Tool Harness — Design & As-Built

**Status: implemented** (steps 1–5 below). Give KB's in-house agents the ability
to **call tools** during a turn, instead of being limited to single-shot
text-in/text-out completions. The model decides *what* to call; our Rust code
owns the loop, the allowlist, and the tool bodies.

The roundtable personas are the integration point: a persona with the **tools**
flag set (and running a Claude model) drives its own corpus lookups mid-turn.
`find_problems` (the "ResearchAgent") was **not** wired — it turns out to be a
deterministic vector-search pipeline with no LLM call, so it has no turn to give
tools to (see *Out of scope*).

## Scope (locked)

- **Tools exposed:** `kb_search`, `kb_get_paper`, `kb_create_reflection` — the
  same corpus surface already exposed over MCP in `src/server/mcp.rs`, but
  callable in-process so our own agents get it (not only Claude Code). **No
  filesystem and no web** in this phase — keeps the harness read-mostly and
  removes path-sandboxing / SSRF concerns entirely.
- **Shape:** a reusable harness layer, then wire the roundtable onto it via a
  per-persona opt-in. Plain (non-tool) turns keep their current path.

## Why it doesn't work today

Every agent funnels through `complete()` (`src/agents/roundtable.rs:561`) →
`AnthropicChat::complete()`. That client is deliberately single-shot and
text-only:

- It never sends a `tools` field in the request body (`src/anthropic.rs:118`).
- `parse_response` filters the response to `text` blocks and discards the rest
  (`src/anthropic.rs:184`) — so a `tool_use` block would be thrown away.
- `ChatMessage.content` is a plain `String` (`src/chat.rs:24`), with no place
  to carry `tool_use` / `tool_result` blocks.

So the model can't request a tool, and there's no loop to run one if it did.

## How Anthropic tool-use works (the loop we must own)

1. Send the request with a `tools: [...]` array (JSON Schema per tool).
2. Model may reply with `stop_reason: "tool_use"` and one or more `tool_use`
   blocks, each carrying `id`, `name`, `input`.
3. **We** execute each tool locally and send a `user` message containing
   `tool_result` blocks (each matching its `tool_use_id`).
4. Repeat until `stop_reason` is `end_turn` (or our iteration cap trips).

"The harness" = our code that owns this loop and the tool implementations.

## Design

### New: `src/agents/harness/mod.rs`

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn schema(&self) -> serde_json::Value;   // { name, description, input_schema }
    async fn run(&self, input: serde_json::Value) -> Result<String, KbError>;
}

pub struct ToolRegistry { tools: Vec<Box<dyn Tool>> }
impl ToolRegistry {
    pub fn schemas(&self) -> Vec<serde_json::Value>;        // -> request `tools`
    pub async fn dispatch(&self, name: &str, input: serde_json::Value)
        -> Result<String, KbError>;                          // by name; unknown => error string
}

/// Owns the tool-use loop. Returns the final assistant text.
pub async fn run_agent(
    client: &AnthropicChat,
    system: &str,
    messages: Vec<AgentMessage>,
    registry: &ToolRegistry,
    max_iters: usize,           // hard cap; trips => return best-effort text + warn
) -> Result<String, KbError>;
```

- **Iteration cap** prevents a runaway tool loop.
- A tool body returning `Err` is converted to a `tool_result` with
  `is_error: true` and fed back, so the model can recover rather than aborting
  the whole turn.

### New: `src/agents/harness/kb_tools.rs`

Three `Tool` impls, each holding a clone of the `paths` / `config` the MCP
handlers take, calling **straight into the existing handlers** (which were made
`pub(crate)` — single source of truth for the arg parsing, no second copy to
drift):

| Tool                  | Wraps (`server::mcp::…`)   | Access     |
|-----------------------|----------------------------|------------|
| `kb_search`           | `tool_search`              | read-only  |
| `kb_get_paper`        | `tool_get_paper`           | read-only  |
| `kb_create_reflection`| `tool_create_reflection`   | **writer** |

Two constructors:

- `kb_registry(paths, config)` — all three tools (includes the writer).
- `kb_registry_readonly(paths, config)` — `kb_search` + `kb_get_paper` only.
  Used by the roundtable so a persona can read the corpus but can't drop
  reflections into the KB mid-debate.

Schemas are written in Anthropic shape (`input_schema`, top-level
`name`/`description`) — distinct from MCP's `inputSchema`.

#### The `Send` snag (reflection)

`kb_create_reflection` → `pipeline::ingest_reflection` holds a non-`Send`
rusqlite `MetaDb` across an `.await`, so its future isn't `Send`. The MCP server
never hit this (single-threaded stdio loop), but the harness needs `Send`
futures — the roundtable runs under `tokio::spawn` (`http.rs:559`). Rather than
relax the `Tool` trait to `?Send` (which would make `run_agent`'s future
non-`Send` and break that spawn), the reflection tool drives the non-`Send`
future on a dedicated current-thread runtime via `spawn_blocking`: it lives
entirely on one thread and never crosses a boundary, leaving the outer future
`Send`. Reflections are infrequent, so the one-off runtime isn't a hot path.
`kb_search` / `kb_get_paper` are already `Send` (the roundtable already calls
`retrieval::search` inside that spawn).

### `src/anthropic.rs` — add `complete_with_tools`

```rust
pub async fn complete_with_tools(
    &self,
    system: &str,
    messages: &[AgentMessage],
    tools: &[serde_json::Value],
) -> Result<(Vec<ContentBlock>, String /* stop_reason */), KbError>;
```

- Adds `"tools"` to the body.
- Parses **all** block types (`text` **and** `tool_use` with `id`/`name`/
  `input`) and reads `stop_reason` — i.e. stops discarding non-text blocks.
- Reuses the existing retry / key-hygiene discipline; existing `complete()`
  stays as-is for plain turns.

### The one real design fork: message type

`ChatMessage.content` is a `String` shared with the OpenAI client
(`src/chat.rs:24`). Tool turns need structured content blocks.

**Decision:** add a **parallel `AgentMessage { role, content: Vec<ContentBlock> }`**
used only on the tool path. Leave `ChatMessage` and the OpenAI client
untouched.

- *Why:* most turns (closeout, scoring, moderator) are plain text and don't
  want tools; a structured-content refactor of `ChatMessage` would churn code
  that doesn't benefit and complicate the OpenAI client. The tool path is
  Anthropic-only for now anyway.
- *Cost:* a small `From<ChatMessage>`-style bridge to seed an `AgentMessage`
  conversation from existing system/user turns.

### Wiring: per-persona `tools` flag

Tool access is a **per-persona opt-in**, not a dedicated agent:

- `PersonaSpec` gains `tools: bool` (`#[serde(default)]` ⇒ false, so older
  clients are unaffected). `complete()` (`roundtable.rs`) gains a sibling
  `complete_tooled` that lifts `system` out, bridges the `ChatMessage` prompt
  into `AgentMessage`s, and runs `run_agent` with `kb_registry_readonly`, capped
  at `MAX_TOOL_ITERS` (6).
- `run_turn` routes on `persona.tools && model.starts_with("claude")` — tool-use
  is Anthropic-only, so OpenAI personas (and tools-off personas) keep the plain
  `complete()` path untouched. Tools **augment** the existing pre-fetched
  grounding rather than replacing it.
- The default panel ships the Technologist (Aria, Claude) with `tools: true` as
  a live demo; everyone else off.

**macOS app** (`Features/Roundtable/`): `Persona` mirrors `tools` (property,
init, tolerant decode ⇒ false, wire payload `"tools"`), and `PersonaEditorCard`
shows a **Tools** checkbox, disabled for non-Claude models with a tooltip
explaining the Anthropic-only constraint.

## Security / safety

- **Allowlist by construction:** agents can only call tools in the registry —
  KB corpus tools, all in-process. No shell, no fs, no network egress.
- **Read-only in the roundtable:** personas get `kb_registry_readonly` (search +
  get_paper), so a debate turn has no write side effects. `kb_create_reflection`
  (the only writer) exists in the full `kb_registry` for future write-capable
  agents, and still goes through the existing reflection path (no new write
  surface).
- Iteration cap + per-tool error-to-`tool_result` keep a turn bounded and
  recoverable.
- Key hygiene unchanged: `ANTHROPIC_API_KEY` never logged (`src/anthropic.rs`).

## Tests (as built — 23 new, full lib suite green at 214)

- **`anthropic.rs`** (mock-server, extends the existing harness): `tool_use`
  response parsing + request-body assertions (system, `tools` array, structured
  content, no sampling params); empty-tools/empty-system omission + `stop_reason`
  default; shared 401⇒`Config` path with no key leak; `is_error` omission +
  round-trip; unknown block ⇒ `Other`.
- **`harness` registry:** empty registry, schema collection/order, dispatch
  routing, unknown-tool ⇒ `Usage` listing options, tool-error propagation,
  duplicate-name panic.
- **`run_agent` loop:** direct answer (tool never called), one-tool round-trip
  (asserts replayed assistant `tool_use` + matching `tool_result`), tool-failure
  ⇒ `is_error` fed back then recovery, iteration cap stops at `max_iters`.
- **`kb_tools`:** registry holds the three named tools in order, Anthropic schema
  shape with correct `required`, `name()` matches schema name. (Tool `run()`
  isn't unit-tested — it needs a live KB + embeddings; the handlers are already
  covered via the MCP path.)
- **`roundtable`:** `tools` defaults off, parses when present, default panel
  ships Aria with it on.

## Build order (all done)

1. ✅ `Tool` trait + `ToolRegistry` (+ unit tests).
2. ✅ `AgentMessage` / `ContentBlock` types + `complete_with_tools` (+
   mock-server tests).
3. ✅ `run_agent` loop (+ cap / error-path tests).
4. ✅ `kb_tools.rs` — three KB tool impls (+ the `Send` isolation above).
5. ✅ Wire the roundtable via the per-persona `tools` flag (+ macOS checkbox).
6. ✅ `mcp.rs` handlers made `pub(crate)` and reused by `kb_tools` (no dup).

## Out of scope (future phases)

- **`find_problems` (ResearchAgent):** not an LLM agent — a deterministic
  vector-search pipeline with no model turn. Giving it tools means adding a *new*
  LLM reasoning layer on top, a larger change deferred to its own phase.
- **Write-capable agents:** a roundtable persona (or a dedicated research agent)
  using the full `kb_registry` to persist reflections.
- Filesystem tools (`grep`, `read_file`, sandboxed `write_file`).
- Web fetch / search.
- Streaming tool-use; parallel tool calls beyond sequential dispatch.
- Live tool-call events (`KbQuery`/`Citation`) emitted from inside `run_agent`
  for the macOS "thinking → searching → speaking" animation — today only the
  pre-fetch grounding emits those.
- Exposing the harness to OpenAI-backed personas (would generalize the message
  type beyond Anthropic).
