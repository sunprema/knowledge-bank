//! Ingest orchestration (PRD §4 data flow + addendum §5 write sequence).
//! Implemented by the integrator after the ingest and storage slices land.

use crate::config::{Config, KbPaths};
use crate::KbError;

/// Outcome summary for logging and CLI display.
#[derive(Debug)]
pub struct IngestReport {
    pub paper_id: String,
    pub title: String,
    pub chunks: usize,
    pub cache_hits: usize,
    pub source_format: crate::SourceFormat,
    pub elapsed_secs: f64,
}

/// Full `kb add` flow for one arXiv id. Steps (PRD §4): metadata →
/// e-print → pandoc (or PDF fallback) → PDF + TOC → classify → notes
/// template → embed (cache-aware) → two-store commit (addendum §5 order:
/// meta.db BEGIN → inserts → index add in memory → index.tv atomic write →
/// COMMIT). Any failure before the commit leaves both stores untouched.
///
/// `progress` receives human-readable step labels for the CLI progress
/// display; pass a no-op for server/watcher contexts.
pub async fn ingest_paper(
    paths: &KbPaths,
    config: &Config,
    input: &str,
    progress: &dyn Fn(&str),
) -> Result<IngestReport, KbError> {
    let _ = (paths, config, input, progress);
    todo!("implemented in the integration slice")
}

/// Re-embed just the notes chunk(s) of one paper (watcher fast path).
pub async fn reembed_notes(
    paths: &KbPaths,
    config: &Config,
    paper_id: &str,
) -> Result<(), KbError> {
    let _ = (paths, config, paper_id);
    todo!("implemented in the integration slice")
}

/// Rebuild both derived stores from canonical files (addendum §8).
pub async fn reindex_all(
    paths: &KbPaths,
    config: &Config,
    progress: &dyn Fn(&str),
) -> Result<(usize, usize), KbError> {
    let _ = (paths, config, progress);
    todo!("implemented in the integration slice")
}
