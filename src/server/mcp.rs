//! MCP server on stdio (PRD §7): JSON-RPC 2.0, tools `kb_search`,
//! `kb_get_paper`, `kb_add_note`. Claude Code launches this as a
//! subprocess via `kb mcp`.

use crate::config::{Config, KbPaths};
use crate::KbError;

/// Run until stdin closes. Protocol notes: respond to `initialize`,
/// `tools/list`, `tools/call`; ignore notifications; never write anything
/// but JSON-RPC frames to stdout (diagnostics go to stderr/tracing).
pub async fn run(paths: KbPaths, config: Config) -> Result<(), KbError> {
    let _ = (paths, config);
    todo!("implemented in the integration slice")
}
