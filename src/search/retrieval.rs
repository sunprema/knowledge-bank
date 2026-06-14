//! Retrieval: embed query → turbovec search (allowlist for filters) →
//! meta.db hydration → paper-level grouping (PRD §5).

use crate::config::{Config, KbPaths};
use crate::embed::OpenAiEmbedder;
use crate::index::{consistency_check, MetaDb, VectorIndex};
use crate::{deep_link, ChunkRecord, DocKind, KbError, PaperMetadata, SectionType};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
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
    /// `None` = both papers and notes.
    pub kind: Option<DocKind>,
    /// Restrict notes to these projects (OR). Typical agent query:
    /// `[current_project, "global"]`.
    pub projects: Option<Vec<String>>,
}

impl SearchFilters {
    pub fn is_empty(&self) -> bool {
        self.section_types.is_none()
            && self.paper_ids.is_none()
            && self.tags.is_none()
            && self.kind.is_none()
            && self.projects.is_none()
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
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
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

    if let Some(stored) = db.meta_get("vector_fingerprint")?
        && stored != config.vector_fingerprint()
    {
        return Err(KbError::Index(format!(
            "embedding/index config changed ({} → {}) — existing vectors are unusable; run `kb reindex`",
            stored,
            config.vector_fingerprint()
        )));
    }
    if let Some(stored) = db.meta_get("chunking_fingerprint")?
        && stored != config.chunking_fingerprint()
    {
        eprintln!(
            "warning: chunking config changed ({} → {}); results reflect the old chunking until you run `kb reindex`",
            stored,
            config.chunking_fingerprint()
        );
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
            filters.kind,
            filters.projects.as_deref(),
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

    let ranking = &config.search.ranking;
    // Over-fetch a candidate pool so recency/importance can promote a chunk
    // into the top `k` that pure cosine ranked just outside it (Generative
    // Agents, arXiv:2304.03442, retrieve a pool then rank — not rank-the-top-k).
    let pool_k = k.saturating_mul(ranking.candidate_multiplier.max(1)).max(k);
    let query_vec = embed_query(config, query).await?;
    let ranked = index.search(&query_vec, pool_k, allowlist.as_deref())?;

    // The cosine floor gates candidates *before* blending, so it stays a true
    // relevance floor regardless of the ranking weights.
    let cosine: HashMap<i64, f32> = ranked
        .iter()
        .filter(|(_, s)| *s >= min_score)
        .map(|(id, s)| (*id as i64, *s))
        .collect();
    let ordered_ids: Vec<i64> = ranked
        .iter()
        .filter(|(_, s)| *s >= min_score)
        .map(|(id, _)| *id as i64)
        .collect();

    // Dense candidates, each scored by the recency/importance blend (#1).
    // This ordering is the dense input to fusion, and the standalone ranking
    // when hybrid is off. Records are cached for reuse across both paths.
    let now = Utc::now();
    let mut records: HashMap<i64, ChunkRecord> = HashMap::new();
    let mut dense_scored: Vec<(i64, f32)> = Vec::new();
    for rec in db.chunks_by_vector_ids(&ordered_ids)? {
        let Some(&relevance) = cosine.get(&rec.vector_id) else {
            continue;
        };
        let recency = recency_factor(&rec.embedded_at, now, ranking.recency_half_life_days);
        let importance = rec.section_type.importance_prior();
        let blended = ranking.relevance_weight * relevance
            + ranking.recency_weight * recency
            + ranking.importance_weight * importance;
        dense_scored.push((rec.vector_id, blended));
        records.insert(rec.vector_id, rec);
    }
    // Tiebreak on vector_id so equal scores rank deterministically.
    dense_scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));

    let hybrid = &config.search.hybrid;
    let ranked_records: Vec<(ChunkRecord, f32)> = if !hybrid.enabled {
        // Dense-only: the blended score is the reported score (== #1).
        dense_scored
            .into_iter()
            .take(k)
            .filter_map(|(id, s)| records.remove(&id).map(|rec| (rec, s)))
            .collect()
    } else {
        // Fuse dense + lexical (BM25) rankings via Reciprocal Rank Fusion.
        // The dense rank already carries recency/importance, so those signals
        // survive fusion; BM25 adds the exact-token matches dense embeddings
        // miss. The reported `score` is the RRF score.
        let dense_rank: HashMap<i64, usize> = dense_scored
            .iter()
            .enumerate()
            .map(|(i, (id, _))| (*id, i))
            .collect();

        // Lexical candidates, restricted to the same filter allowlist so
        // --section/--tag/--kind/--project/--paper apply uniformly.
        let allow: Option<HashSet<i64>> =
            allowlist.as_ref().map(|v| v.iter().map(|&x| x as i64).collect());
        let lex_ids: Vec<i64> = db
            .lexical_search(query, pool_k)?
            .into_iter()
            .filter(|id| allow.as_ref().is_none_or(|s| s.contains(id)))
            .collect();
        let lex_rank: HashMap<i64, usize> =
            lex_ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();

        // Hydrate records for lexical-only ids (dense ones are already loaded).
        let missing: Vec<i64> = lex_ids
            .iter()
            .copied()
            .filter(|id| !records.contains_key(id))
            .collect();
        for rec in db.chunks_by_vector_ids(&missing)? {
            records.insert(rec.vector_id, rec);
        }

        // RRF: Σ weight / (rrf_k + 1-based rank) over the lists a chunk is in.
        let mut fused: Vec<(i64, f32)> = records
            .keys()
            .copied()
            .filter(|id| dense_rank.contains_key(id) || lex_rank.contains_key(id))
            .map(|id| {
                let mut s = 0.0;
                if let Some(&r) = dense_rank.get(&id) {
                    s += hybrid.dense_weight / (hybrid.rrf_k + (r + 1) as f32);
                }
                if let Some(&r) = lex_rank.get(&id) {
                    s += hybrid.lexical_weight / (hybrid.rrf_k + (r + 1) as f32);
                }
                (id, s)
            })
            .collect();
        fused.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        fused.truncate(k);
        fused
            .into_iter()
            .filter_map(|(id, s)| records.remove(&id).map(|rec| (rec, s)))
            .collect()
    };

    // Group by paper, preserving rank order of first appearance.
    let mut groups: Vec<PaperGroup> = Vec::new();
    let mut group_of: HashMap<String, usize> = HashMap::new();
    let mut total_chunks = 0usize;

    for (rec, score) in ranked_records {
        // Notes and reflections have no PDF; deep-link to their body file.
        let target = paths.link_target(&rec.paper_id, rec.section_type);
        let hit = ChunkHit {
            chunk_id: rec.chunk_id.clone(),
            section_type: rec.section_type.as_str().to_string(),
            score,
            snippet: rec.snippet.clone(),
            page: rec.page,
            deep_link: deep_link(&target, rec.page, None),
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
                        kind: meta.kind.as_str().to_string(),
                        project: meta.project,
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

/// Exponential recency term in `(0, 1]` from a chunk's `embedded_at`
/// timestamp: `1.0` for something embedded now, `0.5` at `half_life_days`,
/// decaying toward 0. An unparseable or future timestamp is treated as
/// neutral (`0.5`) rather than failing the search.
fn recency_factor(embedded_at: &str, now: DateTime<Utc>, half_life_days: f32) -> f32 {
    let Ok(t) = DateTime::parse_from_rfc3339(embedded_at) else {
        return 0.5;
    };
    let age_days = (now - t.with_timezone(&Utc)).num_seconds() as f32 / 86_400.0;
    let age_days = age_days.max(0.0);
    (-std::f32::consts::LN_2 * age_days / half_life_days.max(1.0)).exp()
}

/// A paper folder may be mid-delete or hand-mangled; search shouldn't die
/// on one bad metadata.json (PRD: derived state must never hold canonical
/// state hostage).
fn placeholder_meta(paper_id: &str) -> PaperMetadata {
    PaperMetadata {
        arxiv_id: paper_id.to_string(),
        kind: DocKind::default(),
        project: None,
        links: Vec::new(),
        version: None,
        title: format!("(metadata.json unreadable for {paper_id})"),
        authors: Vec::new(),
        abstract_text: String::new(),
        categories: Vec::new(),
        published_at: String::new(),
        updated_at: String::new(),
        ingested_at: String::new(),
        source_format: crate::SourceFormat::Pdf,
        source_url: None,
        main_tex: None,
        tags: Vec::new(),
        schema_version: crate::SCHEMA_VERSION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn recency_decays_by_half_life() {
        let now = Utc::now();
        let just_now = now.to_rfc3339();
        let half = (now - Duration::days(180)).to_rfc3339();
        let old = (now - Duration::days(720)).to_rfc3339();

        assert!((recency_factor(&just_now, now, 180.0) - 1.0).abs() < 0.01);
        assert!((recency_factor(&half, now, 180.0) - 0.5).abs() < 0.01);
        // Strictly monotonic: older ⇒ smaller.
        assert!(recency_factor(&old, now, 180.0) < recency_factor(&half, now, 180.0));
    }

    #[test]
    fn recency_handles_bad_and_future_timestamps() {
        let now = Utc::now();
        assert_eq!(recency_factor("not-a-date", now, 180.0), 0.5);
        let future = (now + Duration::days(30)).to_rfc3339();
        // Clamped to age 0 ⇒ full recency, never above 1.0.
        assert!((recency_factor(&future, now, 180.0) - 1.0).abs() < 0.01);
    }

    #[test]
    fn importance_prior_ranks_synthesis_above_prose() {
        assert!(SectionType::Reflection.importance_prior() > SectionType::Method.importance_prior());
        assert!(SectionType::UserNotes.importance_prior() > SectionType::Other.importance_prior());
        assert!(SectionType::FutureWork.importance_prior() > SectionType::Background.importance_prior());
    }

    #[test]
    fn blend_lets_importance_break_a_cosine_tie() {
        // Two chunks at equal cosine and equal recency: the higher-importance
        // section type wins under the default weights.
        let r = crate::config::RankingConfig::default();
        let rel = 0.50_f32;
        let rec = 0.50_f32;
        let blend = |imp: f32| r.relevance_weight * rel + r.recency_weight * rec + r.importance_weight * imp;
        let reflection = blend(SectionType::Reflection.importance_prior());
        let other = blend(SectionType::Other.importance_prior());
        assert!(reflection > other);
        // …but the gap stays small relative to a real relevance difference,
        // so relevance still dominates.
        assert!(reflection - other < 0.10);
    }
}
