//! Retrieval: embed query → turbovec search (allowlist for filters) →
//! meta.db hydration → paper-level grouping (PRD §5).

use crate::config::{Config, KbPaths};
use crate::embed::OpenAiEmbedder;
use crate::index::{consistency_check, MetaDb, VectorIndex};
use crate::{deep_link, KbError, PaperMetadata, SectionType};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// k=10 default, min_score floor 0.72 — direct lookups.
    Narrow,
    /// k=40 default, no floor — synthesis (Claude clusters the results).
    Wide,
}

impl SearchMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SearchMode::Narrow => "narrow",
            SearchMode::Wide => "wide",
        }
    }
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

/// Per-process query-embedding cache (PRD §5): same query twice in one
/// process (CLI run, MCP server lifetime) = one API call. Keyed by
/// model + query; never persisted.
fn query_cache() -> &'static Mutex<HashMap<String, Vec<f32>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<f32>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn embed_query(config: &Config, query: &str) -> Result<Vec<f32>, KbError> {
    let key = format!("{}\u{1}{query}", config.embedding.model);
    if let Some(v) = query_cache().lock().unwrap().get(&key) {
        return Ok(v.clone());
    }
    let embedder =
        OpenAiEmbedder::from_env(&config.embedding.model, config.embedding.dimensions)?;
    let mut vecs = embedder.embed_batch(&[query]).await?;
    let vec = vecs
        .pop()
        .ok_or_else(|| KbError::Network("embedding API returned no vector".into()))?;
    query_cache().lock().unwrap().insert(key, vec.clone());
    Ok(vec)
}

/// Open both stores in query mode: startup consistency check (addendum §7 —
/// refuse and point at `kb reindex` on failure) plus the resolved
/// config-fingerprint policy (vector mismatch ⇒ refuse; chunking mismatch ⇒
/// stderr warning, proceed).
pub fn open_stores_for_query(
    paths: &KbPaths,
    config: &Config,
) -> Result<(MetaDb, VectorIndex), KbError> {
    let db = MetaDb::open(&paths.meta_db_path())?;
    let index = VectorIndex::open_or_create(
        &paths.index_path(),
        config.embedding.dimensions,
        config.turbovec.bit_width,
    )?;

    let report = consistency_check(&db, &index, false)?;
    if !report.ok {
        return Err(KbError::Index(format!(
            "index out of sync ({} chunks in meta.db, {} vectors in index) — run `kb reindex` to rebuild",
            report.db_chunks, report.index_vectors
        )));
    }

    if let Some(stored) = db.meta_get("vector_fingerprint")? {
        if stored != config.vector_fingerprint() {
            return Err(KbError::Index(format!(
                "embedding/index config changed ({} → {}) — existing vectors are unusable; run `kb reindex`",
                stored,
                config.vector_fingerprint()
            )));
        }
    }
    if let Some(stored) = db.meta_get("chunking_fingerprint")? {
        if stored != config.chunking_fingerprint() {
            eprintln!(
                "warning: chunking config changed ({} → {}); results reflect the old chunking until you run `kb reindex`",
                stored,
                config.chunking_fingerprint()
            );
        }
    }

    Ok((db, index))
}

/// End-to-end search (PRD §5 data flow).
pub async fn search(
    paths: &KbPaths,
    config: &Config,
    query: &str,
    mode: SearchMode,
    k: Option<usize>,
    filters: SearchFilters,
) -> Result<SearchResponse, KbError> {
    let (db, index) = open_stores_for_query(paths, config)?;

    let empty = |mode: SearchMode| SearchResponse {
        query: query.to_string(),
        mode: mode.as_str().to_string(),
        papers: Vec::new(),
        total_chunks: 0,
    };

    if index.is_empty() {
        return Ok(empty(mode));
    }

    // Filters → allowlist of vector ids (turbovec honors it in the SIMD
    // kernel — selective filters are fast, not "search all then drop").
    let allowlist: Option<Vec<u64>> = if filters.is_empty() {
        None
    } else {
        let ids = db.vector_ids_filtered(
            filters.section_types.as_deref(),
            filters.paper_ids.as_deref(),
            filters.tags.as_deref(),
        )?;
        if ids.is_empty() {
            return Ok(empty(mode));
        }
        Some(ids.into_iter().map(|i| i as u64).collect())
    };

    let k = k.unwrap_or(match mode {
        SearchMode::Narrow => config.search.default_k_narrow,
        SearchMode::Wide => config.search.default_k_wide,
    });
    let min_score = match mode {
        SearchMode::Narrow => config.search.default_min_score_narrow,
        SearchMode::Wide => config.search.default_min_score_wide,
    };

    let query_vec = embed_query(config, query).await?;
    let ranked = index.search(&query_vec, k, allowlist.as_deref())?;

    let scores: HashMap<i64, f32> = ranked
        .iter()
        .filter(|(_, s)| *s >= min_score)
        .map(|(id, s)| (*id as i64, *s))
        .collect();
    let ordered_ids: Vec<i64> = ranked
        .iter()
        .filter(|(_, s)| *s >= min_score)
        .map(|(id, _)| *id as i64)
        .collect();

    let records = db.chunks_by_vector_ids(&ordered_ids)?;

    // Group by paper, preserving rank order of first appearance.
    let mut groups: Vec<PaperGroup> = Vec::new();
    let mut group_of: HashMap<String, usize> = HashMap::new();
    let mut total_chunks = 0usize;

    for rec in records {
        let score = match scores.get(&rec.vector_id) {
            Some(s) => *s,
            None => continue,
        };
        let pdf = paths.pdf_path(&rec.paper_id);
        let hit = ChunkHit {
            chunk_id: rec.chunk_id.clone(),
            section_type: rec.section_type.as_str().to_string(),
            score,
            snippet: rec.snippet.clone(),
            page: rec.page,
            deep_link: deep_link(&pdf, rec.page, None),
        };
        total_chunks += 1;

        let gi = match group_of.get(&rec.paper_id) {
            Some(&gi) => gi,
            None => {
                let meta = PaperMetadata::load(&paths.metadata_path(&rec.paper_id))
                    .unwrap_or_else(|_| placeholder_meta(&rec.paper_id));
                groups.push(PaperGroup {
                    paper_id: rec.paper_id.clone(),
                    best_score: score,
                    matched_sections: Vec::new(),
                    chunks: Vec::new(),
                    paper: PaperInfo {
                        title: meta.title,
                        authors: meta.authors,
                        abstract_text: meta.abstract_text,
                        categories: meta.categories,
                        published_at: meta.published_at,
                    },
                    tags: meta.tags,
                });
                let gi = groups.len() - 1;
                group_of.insert(rec.paper_id.clone(), gi);
                gi
            }
        };
        let group = &mut groups[gi];
        group.best_score = group.best_score.max(score);
        if !group.matched_sections.contains(&hit.section_type) {
            group.matched_sections.push(hit.section_type.clone());
        }
        group.chunks.push(hit);
    }

    groups.sort_by(|a, b| b.best_score.total_cmp(&a.best_score));

    Ok(SearchResponse {
        query: query.to_string(),
        mode: mode.as_str().to_string(),
        papers: groups,
        total_chunks,
    })
}

/// A paper folder may be mid-delete or hand-mangled; search shouldn't die
/// on one bad metadata.json (PRD: derived state must never hold canonical
/// state hostage).
fn placeholder_meta(paper_id: &str) -> PaperMetadata {
    PaperMetadata {
        arxiv_id: paper_id.to_string(),
        version: None,
        title: format!("(metadata.json unreadable for {paper_id})"),
        authors: Vec::new(),
        abstract_text: String::new(),
        categories: Vec::new(),
        published_at: String::new(),
        updated_at: String::new(),
        ingested_at: String::new(),
        source_format: crate::SourceFormat::Pdf,
        main_tex: None,
        tags: Vec::new(),
        schema_version: crate::SCHEMA_VERSION,
    }
}
