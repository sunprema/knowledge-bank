//! Retrieval: embed query → turbovec search (allowlist for filters) →
//! meta.db hydration → paper-level grouping (PRD §5).

use crate::config::{Config, KbPaths};
use crate::{KbError, SectionType};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// k=10 default, min_score floor 0.72 — direct lookups.
    Narrow,
    /// k=40 default, no floor — synthesis (Claude clusters the results).
    Wide,
}

#[derive(Debug, Clone, Default)]
pub struct SearchFilters {
    pub section_types: Option<Vec<SectionType>>,
    pub paper_ids: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
}

impl SearchFilters {
    pub fn is_empty(&self) -> bool {
        self.section_types.is_none() && self.paper_ids.is_none() && self.tags.is_none()
    }
}

/// One matching chunk (PRD §5 result shape).
#[derive(Debug, Clone, Serialize)]
pub struct ChunkHit {
    pub chunk_id: String,
    pub section_type: String,
    pub score: f32,
    pub snippet: String,
    pub page: Option<u32>,
    pub deep_link: String,
}

/// Paper metadata subset embedded in results.
#[derive(Debug, Clone, Serialize)]
pub struct PaperInfo {
    pub title: String,
    pub authors: Vec<String>,
    #[serde(rename = "abstract")]
    pub abstract_text: String,
    pub categories: Vec<String>,
    pub published_at: String,
}

/// Paper-level deduplication group (PRD §5): each paper appears once with
/// all matching chunks under it, ordered by best_score desc.
#[derive(Debug, Clone, Serialize)]
pub struct PaperGroup {
    pub paper_id: String,
    pub best_score: f32,
    pub matched_sections: Vec<String>,
    pub chunks: Vec<ChunkHit>,
    pub paper: PaperInfo,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub mode: String,
    pub papers: Vec<PaperGroup>,
    pub total_chunks: usize,
}

/// End-to-end search. Runs the addendum §7 startup consistency check
/// (query mode: refuse and point at `kb reindex` on failure) and the
/// resolved config-fingerprint policy (vector mismatch ⇒ refuse;
/// chunking mismatch ⇒ stderr warning, proceed).
pub async fn search(
    paths: &KbPaths,
    config: &Config,
    query: &str,
    mode: SearchMode,
    k: Option<usize>,
    filters: SearchFilters,
) -> Result<SearchResponse, KbError> {
    let _ = (paths, config, query, mode, k, filters);
    todo!("implemented in the integration slice")
}
