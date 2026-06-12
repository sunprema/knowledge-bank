//! notify-based folder watcher (PRD §9): re-embed notes.md / sections.md
//! changes, ingest new paper folders, remove deleted ones. 2s debounce.
//! Writes kb.pid; logs lifecycle events to .arxiv-kb/kb.log.

use crate::config::{Config, KbPaths};
use crate::KbError;

/// Foreground watcher (v0.1; --daemon is v0.2). Runs until SIGINT/SIGTERM,
/// then flushes in-flight work and removes the pid file.
pub async fn run(paths: KbPaths, config: Config) -> Result<(), KbError> {
    let _ = (paths, config);
    todo!("implemented in the integration slice")
}
