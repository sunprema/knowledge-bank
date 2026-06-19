//! The corpus tools KB's own agents may call.
//!
//! Three [`Tool`]s that wrap the *same* in-process logic the MCP server exposes
//! to Claude Code (`crate::server::mcp::tool_*`), so an agent reasoning over the
//! corpus gets the identical search / lookup / reflection behaviour — no second
//! implementation of the arg parsing to drift out of sync. The only differences
//! from the MCP definitions are mechanical: Anthropic's tools schema uses
//! `input_schema` (snake_case) and a top-level `name`/`description`, whereas MCP
//! uses `inputSchema`.
//!
//! Surface: `kb_search` and `kb_get_paper` are read-only; `kb_create_reflection`
//! is the one writer (it persists a synthesis back into the KB, so a hunt
//! compounds across runs). No filesystem or network tools live here.
//!
//! Build the standard set with [`kb_registry`].

use async_trait::async_trait;

use super::{Tool, ToolRegistry};
use crate::config::{Config, KbPaths};
use crate::server::mcp;
use crate::{KbError, SectionType};

/// A [`ToolRegistry`] holding all three corpus tools, ready to hand to
/// [`run_agent`](super::run_agent). Each tool owns a clone of `paths`/`config`
/// (both cheap to clone) so the registry is self-contained for the run.
///
/// Includes the writer (`kb_create_reflection`); use
/// [`kb_registry_readonly`] for contexts where an agent should be able to read
/// the corpus but not mutate it (e.g. a live roundtable turn).
pub fn kb_registry(paths: &KbPaths, config: &Config) -> ToolRegistry {
    kb_registry_readonly(paths, config).with(Box::new(KbCreateReflectionTool::new(paths, config)))
}

/// The read-only corpus tools (`kb_search`, `kb_get_paper`) — no writer.
///
/// Used where active corpus lookup is wanted but persisting back is not, so a
/// tool-using agent can't have surprising write side effects (a roundtable
/// persona shouldn't drop reflections into the KB mid-debate).
pub fn kb_registry_readonly(paths: &KbPaths, config: &Config) -> ToolRegistry {
    ToolRegistry::new()
        .with(Box::new(KbSearchTool::new(paths, config)))
        .with(Box::new(KbGetPaperTool::new(paths)))
}

/// Semantic search over the corpus — the agent's primary way to pull grounding.
pub struct KbSearchTool {
    paths: KbPaths,
    config: Config,
}

impl KbSearchTool {
    pub fn new(paths: &KbPaths, config: &Config) -> Self {
        Self { paths: paths.clone(), config: config.clone() }
    }
}

#[async_trait]
impl Tool for KbSearchTool {
    fn name(&self) -> &str {
        "kb_search"
    }

    fn schema(&self) -> serde_json::Value {
        let section_values: Vec<&str> = SectionType::ALL.iter().map(|t| t.as_str()).collect();
        serde_json::json!({
            "name": "kb_search",
            "description": "Search the knowledge bank for sections of papers (and captured ideas) matching a query. Returns top-k chunks ranked by semantic similarity, with paper metadata. Use mode='narrow' for direct lookups, mode='wide' for synthesis or ideation across papers.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "mode": { "type": "string", "enum": ["narrow", "wide"], "default": "narrow" },
                    "k": { "type": "integer", "default": 10, "minimum": 1, "maximum": 100 },
                    "section_types": {
                        "type": "array",
                        "items": { "type": "string", "enum": section_values },
                        "description": "Restrict to these section types, e.g. ['applications','future_work']."
                    },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "paper_ids": { "type": "array", "items": { "type": "string" } },
                    "kind": {
                        "type": "string",
                        "enum": ["paper", "note", "all"],
                        "default": "all"
                    }
                },
                "required": ["query"]
            }
        })
    }

    async fn run(&self, input: serde_json::Value) -> Result<String, KbError> {
        mcp::tool_search(&self.paths, &self.config, &input).await
    }
}

/// Full metadata, abstract, and user notes for one paper — deeper context than
/// a search chunk.
pub struct KbGetPaperTool {
    paths: KbPaths,
}

impl KbGetPaperTool {
    pub fn new(paths: &KbPaths) -> Self {
        Self { paths: paths.clone() }
    }
}

#[async_trait]
impl Tool for KbGetPaperTool {
    fn name(&self) -> &str {
        "kb_get_paper"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "name": "kb_get_paper",
            "description": "Get full metadata, abstract, and user notes for a specific paper. Use after kb_search when more context than a chunk snippet is needed.",
            "input_schema": {
                "type": "object",
                "properties": { "paper_id": { "type": "string" } },
                "required": ["paper_id"]
            }
        })
    }

    async fn run(&self, input: serde_json::Value) -> Result<String, KbError> {
        mcp::tool_get_paper(&self.paths, &input)
    }
}

/// Persist a cross-paper synthesis back into the KB (indexed as a reflection),
/// so today's synthesis is retrievable in future runs. The only writer.
pub struct KbCreateReflectionTool {
    paths: KbPaths,
    config: Config,
}

impl KbCreateReflectionTool {
    pub fn new(paths: &KbPaths, config: &Config) -> Self {
        Self { paths: paths.clone(), config: config.clone() }
    }
}

#[async_trait]
impl Tool for KbCreateReflectionTool {
    fn name(&self) -> &str {
        "kb_create_reflection"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "name": "kb_create_reflection",
            "description": "Save a cross-paper synthesis reflection to the KB. Indexed with section_type='reflection' and retrieved by future kb_search calls, so today's synthesis compounds. Re-calling with the same title updates it in place.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short title for the reflection." },
                    "body": { "type": "string", "description": "Synthesis in markdown; cite source paper ids." },
                    "scope": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "paper_ids this reflection draws from."
                    }
                },
                "required": ["title", "body"]
            }
        })
    }

    async fn run(&self, input: serde_json::Value) -> Result<String, KbError> {
        // `ingest_reflection` holds a non-`Send` `MetaDb` (rusqlite) across its
        // awaits, so its future isn't `Send`. The harness needs `Send` futures
        // (the roundtable runs under `tokio::spawn`), so drive the reflection on
        // a dedicated current-thread runtime via `spawn_blocking`: the non-`Send`
        // future lives entirely on that one thread and never crosses a thread
        // boundary, leaving this tool's outer future `Send`. Reflections are
        // infrequent (an agent occasionally persists a synthesis), so the
        // one-off runtime is not a hot path.
        let paths = self.paths.clone();
        let config = self.config.clone();
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| KbError::Index(format!("reflection runtime: {e}")))?;
            rt.block_on(mcp::tool_create_reflection(&paths, &config, &input))
        })
        .await
        .map_err(|e| KbError::Index(format!("reflection task panicked: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> KbPaths {
        KbPaths { root: std::path::PathBuf::from("/tmp/kb-test") }
    }

    #[test]
    fn kb_registry_holds_the_three_corpus_tools() {
        let reg = kb_registry(&paths(), &Config::default());
        assert_eq!(reg.len(), 3);
        let names: Vec<String> = reg
            .schemas()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, ["kb_search", "kb_get_paper", "kb_create_reflection"]);
    }

    #[test]
    fn schemas_use_anthropic_shape_with_required_fields() {
        // Anthropic tools use `input_schema` (not MCP's `inputSchema`) and a
        // top-level name/description.
        let search = KbSearchTool::new(&paths(), &Config::default()).schema();
        assert_eq!(search["name"], "kb_search");
        assert!(search.get("input_schema").is_some());
        assert!(search.get("inputSchema").is_none());
        assert_eq!(search["input_schema"]["required"][0], "query");

        let reflect = KbCreateReflectionTool::new(&paths(), &Config::default()).schema();
        let required = reflect["input_schema"]["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "title"));
        assert!(required.iter().any(|v| v == "body"));
    }

    #[test]
    fn tool_names_match_their_schema_names() {
        // run_agent dispatches by name(), so it must equal the schema "name".
        let s = KbSearchTool::new(&paths(), &Config::default());
        assert_eq!(s.name(), s.schema()["name"].as_str().unwrap());
        let g = KbGetPaperTool::new(&paths());
        assert_eq!(g.name(), g.schema()["name"].as_str().unwrap());
        let r = KbCreateReflectionTool::new(&paths(), &Config::default());
        assert_eq!(r.name(), r.schema()["name"].as_str().unwrap());
    }
}
