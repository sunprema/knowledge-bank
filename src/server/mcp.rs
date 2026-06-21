//! MCP server on stdio (PRD §7): JSON-RPC 2.0, newline-delimited, tools
//! `kb_search`, `kb_get_paper`, `kb_add_note`, `kb_capture_idea`. Claude
//! Code launches this as a subprocess via `kb mcp`.
//!
//! Discipline: stdout carries ONLY JSON-RPC frames; all diagnostics go to
//! tracing (stderr). Tool failures are reported as `isError: true` tool
//! results (so the model can react), not protocol errors.

use crate::config::{Config, KbPaths};
use crate::ingest::pipeline;
use crate::search::{retrieval, SearchFilters, SearchMode};
use crate::{DocKind, KbError, PaperMetadata, SectionType};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PROTOCOL_VERSION: &str = "2024-11-05";

pub async fn run(paths: KbPaths, config: Config) -> Result<(), KbError> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("unparseable frame: {e}");
                continue;
            }
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        // Notifications (no id) get no response.
        let Some(id) = id else {
            tracing::debug!("notification: {method}");
            continue;
        };

        let response = match method {
            "initialize" => ok_response(
                &id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "arxiv-kb",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                }),
            ),
            "ping" => ok_response(&id, json!({})),
            "tools/list" => ok_response(&id, json!({ "tools": tool_definitions() })),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                match call_tool(&paths, &config, &name, &args).await {
                    Ok(text) => ok_response(
                        &id,
                        json!({ "content": [{ "type": "text", "text": text }] }),
                    ),
                    Err(e) => ok_response(
                        &id,
                        json!({
                            "content": [{ "type": "text", "text": format!("error: {e}") }],
                            "isError": true,
                        }),
                    ),
                }
            }
            other => error_response(&id, -32601, &format!("method not found: {other}")),
        };

        let frame = serde_json::to_string(&response)
            .map_err(|e| KbError::Index(format!("serialize response: {e}")))?;
        stdout
            .write_all(frame.as_bytes())
            .await
            .map_err(|e| KbError::Index(format!("stdout: {e}")))?;
        stdout
            .write_all(b"\n")
            .await
            .map_err(|e| KbError::Index(format!("stdout: {e}")))?;
        stdout
            .flush()
            .await
            .map_err(|e| KbError::Index(format!("stdout: {e}")))?;
    }
    Ok(())
}

fn ok_response(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The tool surface: the three v0.1 tools (schemas per PRD §7, the
/// contract) plus `kb_capture_idea` (standalone idea capture).
fn tool_definitions() -> Value {
    let section_values: Vec<&str> = SectionType::ALL.iter().map(|t| t.as_str()).collect();
    json!([
        {
            "name": "kb_search",
            "description": "Search the user's arxiv-kb for sections of papers matching a query. Returns top-k chunks ranked by semantic similarity, with paper metadata and PDF deep-links. Use mode='narrow' for direct lookups (when the user asks about a specific concept), mode='wide' for synthesis or ideation queries (when the user wants to combine ideas across papers).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "mode": { "type": "string", "enum": ["narrow", "wide"], "default": "narrow" },
                    "k": { "type": "integer", "default": 10, "minimum": 1, "maximum": 100 },
                    "section_types": {
                        "type": "array",
                        "items": { "type": "string", "enum": section_values },
                        "description": "Restrict to these section types. Useful for synthesis — e.g. ['applications', 'future_work'] surfaces what authors propose."
                    },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "paper_ids": { "type": "array", "items": { "type": "string" } },
                    "kind": {
                        "type": "string",
                        "enum": ["paper", "note", "all"],
                        "default": "all",
                        "description": "Restrict to papers or to captured ideas (notes)."
                    },
                    "project": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Restrict ideas to these projects. Query [current_project, 'global'] to get this project's ideas plus cross-project ones."
                    }
                },
                "required": ["query"]
            }
        },
        {
            "name": "kb_capture_idea",
            "description": "Capture a standalone idea in the user's KB, keyed by project, so it can be recalled semantically from any future session. Use when the user states an idea, decision, or insight worth keeping that is NOT about a specific paper (for paper annotations use kb_add_note). Use project='global' for ideas that apply across every project. Re-capturing with the same title (or upsert_key) updates the idea instead of duplicating it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Project this idea is keyed to, or 'global'." },
                    "title": { "type": "string", "description": "Short title; also derives the idea's stable id (slugified)." },
                    "body": { "type": "string", "description": "The idea itself, markdown. Reference related papers/ideas as [[id]]." },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "links": { "type": "array", "items": { "type": "string" }, "description": "Related paper/idea ids." },
                    "upsert_key": { "type": "string", "description": "Explicit id (slug) to create/update, when the title may have changed." }
                },
                "required": ["project", "title", "body"]
            }
        },
        {
            "name": "kb_find_problems",
            "description": "Hunt the user's corpus for unsolved problems worth building a solution for. Returns problem statements (drawn from papers' limitations/future_work sections), each paired with the nearest method/applications work found in OTHER papers. gap_type is 'greenfield' (nothing in the corpus addresses it) or 'synthesis_opportunity' (the solution pieces exist across papers but aren't assembled). Use when the user asks what to build, what problems are unsolved, or to mine product/research opportunities. After judging the candidates, persist the promising ones with kb_create_reflection so the hunt compounds across sessions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "domain": { "type": "string", "description": "Optional topic to focus the hunt (e.g. 'vector quantization'). Omit to scan broadly." },
                    "k": { "type": "integer", "default": 8, "minimum": 1, "maximum": 30, "description": "Number of problem candidates to return." }
                }
            }
        },
        {
            "name": "kb_brief",
            "description": "Get the user's daily KB brief: new arXiv papers surfaced by their standing watches — each scored by how strongly it connects to their existing corpus, with the connecting papers/reflections named so you can see WHY it's relevant — plus one resurfaced past reflection, a few fresh cross-document sparks, and corpus stats. Use at the start of a session to catch the user up, or when they ask 'what's new', 'anything relevant lately', or to triage their reading queue. To ingest one of the surfaced papers, call the HTTP /ingest path or tell the user to add it; this tool is read-only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "default": 12, "minimum": 1, "maximum": 50, "description": "Max new candidate papers to include." }
                }
            }
        },
        {
            "name": "kb_get_paper",
            "description": "Get full metadata, abstract, and user notes for a specific paper. Use after kb_search when Claude needs more context than a chunk snippet provides.",
            "inputSchema": {
                "type": "object",
                "properties": { "paper_id": { "type": "string" } },
                "required": ["paper_id"]
            }
        },
        {
            "name": "kb_add_note",
            "description": "Append a note to a paper's notes.md. Use when the user (during a Claude conversation) shares an insight about a paper that should be captured for future synthesis.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paper_id": { "type": "string" },
                    "note": { "type": "string" }
                },
                "required": ["paper_id", "note"]
            }
        },
        {
            "name": "kb_create_reflection",
            "description": "Save a cross-paper synthesis reflection to the KB. Call this after using kb_search(mode='wide') to gather papers on a theme and synthesising insights across them. The reflection is indexed with section_type='reflection' and retrieved by future kb_search calls — so today's synthesis compounds into tomorrow's. Re-calling with the same title updates the reflection in place.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short title for this reflection (e.g. 'Memory architectures across agent frameworks')"
                    },
                    "body": {
                        "type": "string",
                        "description": "The synthesis text in markdown. Cover: themes across papers, contradictions, combined ideas, and cite source paper ids."
                    },
                    "scope": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "paper_ids this reflection draws from (used to build the scope record)"
                    }
                },
                "required": ["title", "body"]
            }
        }
    ])
}

async fn call_tool(
    paths: &KbPaths,
    config: &Config,
    name: &str,
    args: &Value,
) -> Result<String, KbError> {
    match name {
        "kb_search" => tool_search(paths, config, args).await,
        "kb_brief" => tool_brief(paths, args),
        "kb_find_problems" => tool_find_problems(paths, config, args).await,
        "kb_get_paper" => tool_get_paper(paths, args),
        "kb_add_note" => tool_add_note(paths, config, args).await,
        "kb_capture_idea" => tool_capture_idea(paths, config, args).await,
        "kb_create_reflection" => tool_create_reflection(paths, config, args).await,
        other => Err(KbError::Usage(format!("unknown tool: {other}"))),
    }
}

fn str_arg(args: &Value, key: &str) -> Result<String, KbError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| KbError::Usage(format!("missing required argument '{key}'")))
}

fn str_list(args: &Value, key: &str) -> Option<Vec<String>> {
    let list: Vec<String> = args
        .get(key)?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    if list.is_empty() {
        None
    } else {
        Some(list)
    }
}

/// Run the `kb_search` tool. `pub(crate)` so the in-process agent harness can
/// reuse the exact same arg-parsing and search logic the MCP server exposes.
pub(crate) async fn tool_search(paths: &KbPaths, config: &Config, args: &Value) -> Result<String, KbError> {
    let query = str_arg(args, "query")?;
    let mode = match args.get("mode").and_then(|m| m.as_str()).unwrap_or("narrow") {
        "wide" => SearchMode::Wide,
        _ => SearchMode::Narrow,
    };
    let k = args
        .get("k")
        .and_then(|k| k.as_u64())
        .map(|k| (k as usize).clamp(1, 100));

    let section_types = match str_list(args, "section_types") {
        Some(names) => {
            let mut types = Vec::new();
            for n in &names {
                types.push(SectionType::parse(n).ok_or_else(|| {
                    KbError::Usage(format!(
                        "unknown section type '{n}' (expected one of: {})",
                        SectionType::ALL.map(|t| t.as_str()).join(", ")
                    ))
                })?);
            }
            Some(types)
        }
        None => None,
    };

    let kind = match args.get("kind").and_then(|k| k.as_str()).unwrap_or("all") {
        "all" => None,
        other => Some(DocKind::parse(other).ok_or_else(|| {
            KbError::Usage(format!("unknown kind '{other}' (expected paper, note, or all)"))
        })?),
    };
    // `project` accepts an array or a bare string (agents send both).
    let projects = str_list(args, "project").or_else(|| {
        args.get("project")
            .and_then(|p| p.as_str())
            .map(|p| vec![p.to_string()])
    });

    let filters = SearchFilters {
        section_types,
        paper_ids: str_list(args, "paper_ids"),
        tags: str_list(args, "tags"),
        kind,
        projects,
    };

    let response = retrieval::search(paths, config, &query, mode, k, filters).await?;
    serde_json::to_string_pretty(&response)
        .map_err(|e| KbError::Index(format!("serialize results: {e}")))
}

/// Run the `kb_brief` tool — the daily digest of new papers, a resurfaced
/// reflection, and sparks. Read-only over `meta.db` (advances the resurfacing
/// rotation cursor only).
fn tool_brief(paths: &KbPaths, args: &Value) -> Result<String, KbError> {
    let limit = args
        .get("limit")
        .and_then(|k| k.as_u64())
        .map(|k| (k as usize).clamp(1, 50))
        .unwrap_or(crate::watch::DEFAULT_BRIEF_PAPERS);
    let brief = crate::watch::brief(paths, limit)?;
    serde_json::to_string_pretty(&brief)
        .map_err(|e| KbError::Index(format!("serialize brief: {e}")))
}

async fn tool_find_problems(
    paths: &KbPaths,
    config: &Config,
    args: &Value,
) -> Result<String, KbError> {
    let domain = args
        .get("domain")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|d| !d.is_empty());
    let k = args
        .get("k")
        .and_then(|k| k.as_u64())
        .map(|k| (k as usize).clamp(1, 30))
        .unwrap_or(8);

    let response = retrieval::find_problems(paths, config, domain, k).await?;
    serde_json::to_string_pretty(&response)
        .map_err(|e| KbError::Index(format!("serialize results: {e}")))
}

/// Run the `kb_get_paper` tool. `pub(crate)` for reuse by the agent harness.
pub(crate) fn tool_get_paper(paths: &KbPaths, args: &Value) -> Result<String, KbError> {
    let paper_id = str_arg(args, "paper_id")?;
    let meta_path = paths.metadata_path(&paper_id);
    if !meta_path.exists() {
        return Err(KbError::NotFound(format!("{paper_id} is not in the KB")));
    }
    let meta = PaperMetadata::load(&meta_path)?;
    let notes = std::fs::read_to_string(paths.notes_path(&paper_id)).unwrap_or_default();
    let payload = json!({
        "metadata": meta,
        "notes": notes,
        "pdf_path": paths.pdf_path(&paper_id),
    });
    serde_json::to_string_pretty(&payload)
        .map_err(|e| KbError::Index(format!("serialize paper: {e}")))
}

async fn tool_add_note(paths: &KbPaths, config: &Config, args: &Value) -> Result<String, KbError> {
    let paper_id = str_arg(args, "paper_id")?;
    let note = str_arg(args, "note")?;
    let meta_path = paths.metadata_path(&paper_id);
    if !meta_path.exists() {
        return Err(KbError::NotFound(format!("{paper_id} is not in the KB")));
    }

    let meta = PaperMetadata::load(&meta_path)?;
    pipeline::ensure_notes_template(paths, &paper_id, &meta.title)?;

    // Append-only (PRD §14: last-write-wins is acceptable; we never rewrite
    // the user's existing notes content).
    use std::io::Write;
    let stamp = crate::now_rfc3339();
    let block = format!("\n---\n_{stamp} (added via Claude)_\n\n{note}\n");
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(paths.notes_path(&paper_id))
        .map_err(|e| KbError::Index(format!("open notes.md: {e}")))?;
    f.write_all(block.as_bytes())
        .map_err(|e| KbError::Index(format!("append notes.md: {e}")))?;

    match pipeline::reembed_notes(paths, config, &paper_id).await {
        Ok(()) => Ok(format!("note appended to {paper_id} and re-embedded")),
        Err(e) => Ok(format!(
            "note appended to {paper_id}; re-embedding deferred ({e}) — a running `kb watch` or `kb reindex` will pick it up"
        )),
    }
}

async fn tool_capture_idea(
    paths: &KbPaths,
    config: &Config,
    args: &Value,
) -> Result<String, KbError> {
    let spec = pipeline::IdeaSpec {
        slug: args
            .get("upsert_key")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        project: str_arg(args, "project")?,
        title: str_arg(args, "title")?,
        body: str_arg(args, "body")?,
        tags: str_list(args, "tags").unwrap_or_default(),
        links: str_list(args, "links").unwrap_or_default(),
    };
    let report = pipeline::ingest_idea(paths, config, &spec, &|_msg| {}).await?;
    let payload = json!({
        "id": report.paper_id,
        "title": report.title,
        "chunks": report.chunks,
    });
    serde_json::to_string_pretty(&payload)
        .map_err(|e| KbError::Index(format!("serialize result: {e}")))
}

/// Run the `kb_create_reflection` tool. `pub(crate)` for reuse by the agent
/// harness — the one writer in the in-process tool set.
pub(crate) async fn tool_create_reflection(
    paths: &KbPaths,
    config: &Config,
    args: &Value,
) -> Result<String, KbError> {
    let spec = pipeline::ReflectionSpec {
        slug: None,
        title: str_arg(args, "title")?,
        body: str_arg(args, "body")?,
        scope: str_list(args, "scope").unwrap_or_default(),
        tags: Vec::new(),
    };
    let report = pipeline::ingest_reflection(paths, config, &spec, &|_msg| {}).await?;
    let payload = json!({
        "id": report.paper_id,
        "title": report.title,
        "chunks": report.chunks,
        "message": format!(
            "Reflection saved. Retrieve in future sessions with kb_search(section_types=['reflection'])."
        ),
    });
    serde_json::to_string_pretty(&payload)
        .map_err(|e| KbError::Index(format!("serialize result: {e}")))
}
