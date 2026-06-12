//! MCP server on stdio (PRD §7): JSON-RPC 2.0, newline-delimited, tools
//! `kb_search`, `kb_get_paper`, `kb_add_note`. Claude Code launches this
//! as a subprocess via `kb mcp`.
//!
//! Discipline: stdout carries ONLY JSON-RPC frames; all diagnostics go to
//! tracing (stderr). Tool failures are reported as `isError: true` tool
//! results (so the model can react), not protocol errors.

use crate::config::{Config, KbPaths};
use crate::ingest::pipeline;
use crate::search::{retrieval, SearchFilters, SearchMode};
use crate::{KbError, PaperMetadata, SectionType};
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

/// The three v0.1 tools, schemas per PRD §7 (the contract).
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
                    "paper_ids": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["query"]
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
        "kb_get_paper" => tool_get_paper(paths, args),
        "kb_add_note" => tool_add_note(paths, config, args).await,
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

async fn tool_search(paths: &KbPaths, config: &Config, args: &Value) -> Result<String, KbError> {
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

    let filters = SearchFilters {
        section_types,
        paper_ids: str_list(args, "paper_ids"),
        tags: str_list(args, "tags"),
    };

    let response = retrieval::search(paths, config, &query, mode, k, filters).await?;
    serde_json::to_string_pretty(&response)
        .map_err(|e| KbError::Index(format!("serialize results: {e}")))
}

fn tool_get_paper(paths: &KbPaths, args: &Value) -> Result<String, KbError> {
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
