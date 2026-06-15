//! Cortex — the persistent associative layer (the "brain").
//!
//! Retrieval (see [`crate::search`]) answers *"what is relevant to this
//! query?"*. Cortex answers a different question: *"what unexpected connection
//! is worth noticing?"*. It is the difference between a library and a mind —
//! a mind keeps forming links between things it has read, and the interesting
//! links are rarely the obvious ones.
//!
//! On every ingest, Cortex materializes edges between the new document's chunks
//! and the rest of the corpus and persists the **surprising** ones to
//! `meta.db`'s `cortex_edges` table. Surprise here is deliberately *not* raw
//! nearest-neighbor similarity — that only ever surfaces near-duplicates.
//! Innovation lives where two passages are semantically close yet structurally
//! distant, so we score two such signals (both API-free; they reuse the
//! embedding cache):
//!
//! - **need→solution** (directed): a chunk's `future_work`/`limitations` (a
//!   stated need) sits close to another chunk's `method`/`experiments`/
//!   `applications` (a delivered capability). "Someone wished for this;
//!   someone else built it." Uses the corpus's section types — a structural
//!   signal most RAG systems throw away.
//! - **cross-domain** (undirected): two chunks are close in meaning but their
//!   papers share no arXiv category — the same idea echoing across fields,
//!   i.e. a transfer-of-ideas opportunity.
//!
//! The edge store is derived state: `kb reindex` and `kb cortex rebuild`
//! reconstruct it from the embeddings, so it honors the project's "files are
//! forever, indexes are disposable" invariant. Surface the connections with
//! `kb spark` or the web app's Sparks view.

use crate::config::{Config, CortexConfig, KbPaths};
use crate::index::{MetaDb, NewCortexEdge, VectorIndex};
use crate::{ChunkRecord, KbError, PaperMetadata, SectionType};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Section types that express a *need* — a gap the authors wish were filled.
const NEED: [SectionType; 2] = [SectionType::FutureWork, SectionType::Limitations];
/// Section types that express a *delivered capability* — a candidate answer.
const SOLUTION: [SectionType; 3] = [
    SectionType::Method,
    SectionType::Experiments,
    SectionType::Applications,
];

const KIND_NEED_SOLUTION: &str = "need_solution";
const KIND_CROSS_DOMAIN: &str = "cross_domain";

fn is_need(s: SectionType) -> bool {
    NEED.contains(&s)
}
fn is_solution(s: SectionType) -> bool {
    SOLUTION.contains(&s)
}

/// Cosine similarity of two equal-length vectors (0.0 if either is degenerate
/// or the lengths disagree). Cached embeddings aren't guaranteed unit-norm, so
/// we divide by the norms rather than assuming a plain dot product.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Domain distance between two papers' arXiv categories: `1 - Jaccard`. `None`
/// when either side has no categories (ideas, reflections, local PDFs, web
/// pages) — domain can't be judged, so the cross-domain signal doesn't apply.
fn domain_distance(a: &[String], b: &[String]) -> Option<f32> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let sa: HashSet<&str> = a.iter().map(String::as_str).collect();
    let sb: HashSet<&str> = b.iter().map(String::as_str).collect();
    let inter = sa.intersection(&sb).count() as f32;
    let union = sa.union(&sb).count() as f32;
    if union <= 0.0 {
        return None;
    }
    Some(1.0 - inter / union)
}

fn make_edge(
    src: &ChunkRecord,
    dst: &ChunkRecord,
    kind: &str,
    similarity: f32,
    surprise: f32,
) -> NewCortexEdge {
    NewCortexEdge {
        src_chunk: src.vector_id,
        dst_chunk: dst.vector_id,
        src_paper: src.paper_id.clone(),
        dst_paper: dst.paper_id.clone(),
        kind: kind.to_string(),
        similarity,
        surprise,
    }
}

/// Score one cross-paper chunk pair against both signals, returning the edges
/// it earns (zero, one, or both kinds). `sim` is the exact cosine, already
/// known to clear the proximity floor.
fn evaluate(
    cfg: &CortexConfig,
    src: &ChunkRecord,
    src_cats: &[String],
    dst: &ChunkRecord,
    dst_cats: &[String],
    sim: f32,
) -> Vec<NewCortexEdge> {
    let mut out = Vec::new();

    // need→solution is directed; check both orientations (cosine is symmetric,
    // so connecting either paper must yield the same directed edge).
    if is_need(src.section_type) && is_solution(dst.section_type) {
        out.push(make_edge(src, dst, KIND_NEED_SOLUTION, sim, sim));
    }
    if is_need(dst.section_type) && is_solution(src.section_type) {
        out.push(make_edge(dst, src, KIND_NEED_SOLUTION, sim, sim));
    }

    // cross-domain is undirected; store canonical (lower vector id first) so
    // discovering it from either endpoint writes the same PK.
    if let Some(dd) = domain_distance(src_cats, dst_cats)
        && dd >= cfg.min_domain_distance
    {
        let (a, b) = if src.vector_id <= dst.vector_id {
            (src, dst)
        } else {
            (dst, src)
        };
        out.push(make_edge(a, b, KIND_CROSS_DOMAIN, sim, sim * dd));
    }

    out
}

/// A paper's categories, loaded once and cached. A missing/mangled
/// metadata.json yields empty categories (no cross-domain edges, never an
/// error — a bad folder must not sink the connect pass).
fn categories_of<'m>(
    paths: &KbPaths,
    metas: &'m mut HashMap<String, Vec<String>>,
    paper_id: &str,
) -> &'m [String] {
    metas
        .entry(paper_id.to_string())
        .or_insert_with(|| {
            PaperMetadata::load(&paths.metadata_path(paper_id))
                .map(|m| m.categories)
                .unwrap_or_default()
        })
}

/// Find and insert every surprising edge incident to `paper_id`'s chunks.
/// Does not clear first (callers do, when needed); relies on the edge table's
/// `INSERT OR REPLACE` PK to stay idempotent. Returns edges written.
fn scan_and_insert(
    paths: &KbPaths,
    cfg: &CortexConfig,
    db: &MetaDb,
    index: &VectorIndex,
    metas: &mut HashMap<String, Vec<String>>,
    paper_id: &str,
) -> Result<usize, KbError> {
    let src_chunks = db.chunks_for_paper(paper_id)?;
    if src_chunks.is_empty() {
        return Ok(0);
    }
    let src_cats = categories_of(paths, metas, paper_id).to_vec();

    let mut written = 0usize;
    // Over-fetch so same-paper crowding doesn't starve the cross-paper budget.
    let pool = (cfg.neighbors + 1).saturating_mul(4).clamp(cfg.neighbors + 1, 200);

    for src in &src_chunks {
        let Some(src_vec) =
            db.cache_get(&src.content_hash, &src.embedding_model, src.embedding_version)?
        else {
            continue; // no cached vector (e.g. after `kb cache clear`)
        };

        let neighbor_ids: Vec<i64> = index
            .search(&src_vec, pool, None)?
            .into_iter()
            .map(|(id, _)| id as i64)
            .filter(|&id| id != src.vector_id)
            .collect();

        let mut considered = 0usize;
        for dst in db.chunks_by_vector_ids(&neighbor_ids)? {
            if dst.paper_id == *paper_id {
                continue; // intra-paper links aren't connections worth noting
            }
            considered += 1;
            if considered > cfg.neighbors {
                break;
            }
            let Some(dst_vec) =
                db.cache_get(&dst.content_hash, &dst.embedding_model, dst.embedding_version)?
            else {
                continue;
            };
            let sim = cosine(&src_vec, &dst_vec);
            if sim < cfg.min_similarity {
                continue;
            }
            let dst_cats = categories_of(paths, metas, &dst.paper_id).to_vec();
            for edge in evaluate(cfg, src, &src_cats, &dst, &dst_cats, sim) {
                db.insert_cortex_edge(&edge)?;
                written += 1;
            }
        }
    }
    Ok(written)
}

/// (Re)establish the connections incident to one paper, using caller-held
/// stores (the watcher and ingest tails already have them open). Deletes the
/// paper's existing edges first — its chunk ids change on every re-embed, so a
/// stale edge could otherwise point at a vanished chunk — then rescans. The
/// whole thing is one transaction. A no-op when `cortex.enabled = false`.
pub fn connect_paper_with(
    paths: &KbPaths,
    config: &Config,
    db: &MetaDb,
    index: &VectorIndex,
    paper_id: &str,
) -> Result<usize, KbError> {
    if !config.cortex.enabled {
        return Ok(0);
    }
    db.begin_immediate()?;
    let result = (|| -> Result<usize, KbError> {
        db.delete_cortex_edges_for_paper(paper_id)?;
        let mut metas = HashMap::new();
        scan_and_insert(paths, &config.cortex, db, index, &mut metas, paper_id)
    })();
    match result {
        Ok(n) => {
            db.commit()?;
            Ok(n)
        }
        Err(e) => {
            let _ = db.rollback();
            Err(e)
        }
    }
}

/// Open the stores and connect one paper. Convenience wrapper around
/// [`connect_paper_with`] for callers that don't already hold handles.
pub fn connect_paper(paths: &KbPaths, config: &Config, paper_id: &str) -> Result<usize, KbError> {
    if !config.cortex.enabled {
        return Ok(0);
    }
    let db = MetaDb::open(&paths.meta_db_path())?;
    let index = VectorIndex::open_or_create(
        &paths.index_path(),
        config.embedding.dimensions,
        config.turbovec.bit_width,
    )?;
    connect_paper_with(paths, config, &db, &index, paper_id)
}

/// Rebuild the entire edge store from scratch, using caller-held stores (the
/// reindex path already has them). Clears all edges, then scans every paper —
/// idempotent PKs collapse the two discoveries of each pair into one row.
/// Returns the resulting distinct edge count.
pub fn rebuild_all_with(
    paths: &KbPaths,
    config: &Config,
    db: &MetaDb,
    index: &VectorIndex,
) -> Result<usize, KbError> {
    db.begin_immediate()?;
    let result = (|| -> Result<usize, KbError> {
        db.clear_cortex_edges()?;
        if config.cortex.enabled {
            let mut metas = HashMap::new();
            for id in paths.list_paper_ids()? {
                scan_and_insert(paths, &config.cortex, db, index, &mut metas, &id)?;
            }
        }
        db.cortex_edge_count()
    })();
    match result {
        Ok(n) => {
            db.commit()?;
            Ok(n)
        }
        Err(e) => {
            let _ = db.rollback();
            Err(e)
        }
    }
}

/// Open the stores and rebuild the whole edge layer (`kb cortex rebuild`).
pub fn rebuild_all(paths: &KbPaths, config: &Config) -> Result<usize, KbError> {
    let db = MetaDb::open(&paths.meta_db_path())?;
    let index = VectorIndex::open_or_create(
        &paths.index_path(),
        config.embedding.dimensions,
        config.turbovec.bit_width,
    )?;
    rebuild_all_with(paths, config, &db, &index)
}

/// One endpoint of a spark, with enough to render and to deep-link.
#[derive(Debug, Clone, Serialize)]
pub struct SparkEnd {
    pub paper_id: String,
    pub title: String,
    pub section_type: String,
    pub chunk_id: String,
    pub snippet: String,
}

/// A surfaced connection (`kb spark`, `GET /sparks`). For `need_solution`,
/// `directed` is true and `src` is the need, `dst` the solution.
#[derive(Debug, Clone, Serialize)]
pub struct Spark {
    pub kind: String,
    pub directed: bool,
    pub surprise: f32,
    pub similarity: f32,
    pub src: SparkEnd,
    pub dst: SparkEnd,
}

/// Top surprising connections, most surprising first. `limit == 0` falls back
/// to `cortex.max_sparks`; `kind` optionally restricts to one signal.
pub fn list_sparks(
    paths: &KbPaths,
    config: &Config,
    limit: usize,
    kind: Option<&str>,
) -> Result<Vec<Spark>, KbError> {
    let db = MetaDb::open(&paths.meta_db_path())?;
    let limit = if limit == 0 {
        config.cortex.max_sparks
    } else {
        limit
    };
    let rows = db.top_sparks(limit, kind)?;

    let mut titles: HashMap<String, String> = HashMap::new();
    let mut title_of = |paths: &KbPaths, id: &str| -> String {
        titles
            .entry(id.to_string())
            .or_insert_with(|| {
                PaperMetadata::load(&paths.metadata_path(id))
                    .map(|m| m.title)
                    .unwrap_or_else(|_| id.to_string())
            })
            .clone()
    };

    Ok(rows
        .into_iter()
        .map(|r| Spark {
            directed: r.kind == KIND_NEED_SOLUTION,
            kind: r.kind,
            surprise: r.surprise,
            similarity: r.similarity,
            src: SparkEnd {
                title: title_of(paths, &r.src_paper),
                paper_id: r.src_paper,
                section_type: r.src_section,
                chunk_id: r.src_chunk_id,
                snippet: r.src_snippet,
            },
            dst: SparkEnd {
                title: title_of(paths, &r.dst_paper),
                paper_id: r.dst_paper,
                section_type: r.dst_section,
                chunk_id: r.dst_chunk_id,
                snippet: r.dst_snippet,
            },
        })
        .collect())
}

/// Validate a `--kind` / `?kind=` filter string. `None`/`"all"` ⇒ no filter.
pub fn parse_kind_filter(s: Option<&str>) -> Result<Option<&str>, KbError> {
    match s {
        None | Some("all") | Some("") => Ok(None),
        Some(KIND_NEED_SOLUTION) => Ok(Some(KIND_NEED_SOLUTION)),
        Some(KIND_CROSS_DOMAIN) => Ok(Some(KIND_CROSS_DOMAIN)),
        Some(other) => Err(KbError::Usage(format!(
            "unknown spark kind '{other}' (expected need_solution, cross_domain, or all)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(vid: i64, paper: &str, section: SectionType) -> ChunkRecord {
        ChunkRecord {
            vector_id: vid,
            chunk_id: format!("{paper}_{}_{vid}", section.as_str()),
            paper_id: paper.to_string(),
            section_type: section,
            ordinal: 0,
            content_hash: "h".into(),
            text: "t".into(),
            page: None,
            snippet: "s".into(),
            embedded_at: crate::now_rfc3339(),
            embedding_model: "m".into(),
            embedding_version: 1,
        }
    }

    fn cfg() -> CortexConfig {
        CortexConfig::default()
    }

    #[test]
    fn cosine_is_one_for_parallel_half_for_orthogonal_mix() {
        assert!((cosine(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        // Degenerate inputs never panic or divide by zero.
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn domain_distance_disjoint_is_one() {
        let a = vec!["cs.CL".to_string(), "cs.LG".to_string()];
        let b = vec!["q-bio.NC".to_string()];
        assert_eq!(domain_distance(&a, &b), Some(1.0));
        // Full overlap ⇒ 0.
        assert_eq!(domain_distance(&a, &a), Some(0.0));
        // Partial overlap ⇒ between.
        let c = vec!["cs.CL".to_string(), "stat.ML".to_string()];
        let d = domain_distance(&a, &c).unwrap();
        assert!(d > 0.0 && d < 1.0);
        // Missing categories ⇒ no judgement.
        assert_eq!(domain_distance(&a, &[]), None);
    }

    #[test]
    fn need_solution_is_directed_need_to_solution() {
        let need = rec(1, "p1", SectionType::FutureWork);
        let sol = rec(2, "p2", SectionType::Method);
        // Same categories ⇒ no cross-domain edge, only the directed one.
        let cats = vec!["cs.LG".to_string()];
        let edges = evaluate(&cfg(), &need, &cats, &sol, &cats, 0.6);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].kind, KIND_NEED_SOLUTION);
        assert_eq!(edges[0].src_chunk, 1, "src is the need");
        assert_eq!(edges[0].dst_chunk, 2, "dst is the solution");

        // Orientation is independent of argument order.
        let edges2 = evaluate(&cfg(), &sol, &cats, &need, &cats, 0.6);
        assert_eq!(edges2[0].src_chunk, 1);
        assert_eq!(edges2[0].dst_chunk, 2);
    }

    #[test]
    fn cross_domain_requires_disjoint_categories_and_is_canonical() {
        let a = rec(5, "p1", SectionType::Background);
        let b = rec(3, "p2", SectionType::Introduction);
        let ca = vec!["cs.CL".to_string()];
        let cb = vec!["q-bio.NC".to_string()];
        let edges = evaluate(&cfg(), &a, &ca, &b, &cb, 0.7);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].kind, KIND_CROSS_DOMAIN);
        // Canonical: lower vector id first regardless of argument order.
        assert_eq!(edges[0].src_chunk, 3);
        assert_eq!(edges[0].dst_chunk, 5);
        assert!((edges[0].surprise - 0.7).abs() < 1e-6, "disjoint ⇒ surprise == sim");

        // Shared category ⇒ no cross-domain edge under the default floor (1.0).
        let shared = vec!["cs.CL".to_string()];
        assert!(evaluate(&cfg(), &a, &shared, &b, &shared, 0.7).is_empty());
    }

    #[test]
    fn a_pair_can_earn_both_signals() {
        // A future_work need that is also from a different field: both edges.
        let need = rec(1, "p1", SectionType::FutureWork);
        let sol = rec(2, "p2", SectionType::Method);
        let edges = evaluate(
            &cfg(),
            &need,
            &["cs.CL".to_string()],
            &sol,
            &["q-bio.NC".to_string()],
            0.65,
        );
        let kinds: HashSet<&str> = edges.iter().map(|e| e.kind.as_str()).collect();
        assert!(kinds.contains(KIND_NEED_SOLUTION));
        assert!(kinds.contains(KIND_CROSS_DOMAIN));
    }

    #[test]
    fn parse_kind_filter_accepts_known_and_rejects_unknown() {
        assert_eq!(parse_kind_filter(None).unwrap(), None);
        assert_eq!(parse_kind_filter(Some("all")).unwrap(), None);
        assert_eq!(
            parse_kind_filter(Some("need_solution")).unwrap(),
            Some("need_solution")
        );
        assert!(parse_kind_filter(Some("bogus")).is_err());
    }
}
