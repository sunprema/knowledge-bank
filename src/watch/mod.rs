//! ArXiv Watch — the corpus grows itself.
//!
//! A watch is a standing interest (an arXiv category, author, or free-text
//! query). On [`refresh`] each enabled watch is polled for recent submissions;
//! every paper not already in the corpus is **scored by how strongly it
//! connects to what the KB already holds** — its title+abstract is run through
//! the normal retrieval pass, and the strength of the resulting corpus matches
//! becomes the relevance score. The matched papers/reflections double as the
//! "why this matters to you" explanation, so no extra LLM call is needed.
//!
//! This is the Sparks idea pointed outward: instead of surprising connections
//! *within* the corpus, it ranks the outside world by its connection *to* the
//! corpus. Relevance auto-tracks the user's interests as the KB grows.
//!
//! [`brief`] aggregates the day's output — new candidate papers, fresh Sparks,
//! and one resurfaced past reflection — into the macOS app's landing surface
//! and the `kb_brief` MCP tool ("today's synthesis compounds into tomorrow's
//! context").

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::{Value, json};

use crate::config::{Config, KbPaths};
use crate::index::{Candidate, MetaDb, NewCandidate};
use crate::ingest::arxiv;
use crate::search::retrieval;
use crate::{DocKind, KbError, PaperMetadata};

/// Recent papers pulled from arXiv per watch on each refresh.
const DEFAULT_PER_WATCH: usize = 30;
/// Corpus connections kept as the "why this matters" explanation.
const MAX_CONNECTIONS: usize = 4;
/// Default number of candidate papers shown in the daily brief.
pub const DEFAULT_BRIEF_PAPERS: usize = 12;

/// The three watch kinds. Stored as the `kind` column; composed into an arXiv
/// `search_query` at refresh time.
pub const WATCH_KINDS: [&str; 3] = ["category", "author", "query"];

/// Validate a watch kind, returning a usage error listing the valid kinds.
pub fn validate_kind(kind: &str) -> Result<(), KbError> {
    if WATCH_KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(KbError::Usage(format!(
            "unknown watch kind {kind:?}; expected one of: {}",
            WATCH_KINDS.join(", ")
        )))
    }
}

/// Compose a watch into an arXiv `search_query` expression.
pub fn search_query_for(kind: &str, value: &str) -> String {
    match kind {
        "category" => format!("cat:{value}"),
        "author" => format!("au:{value}"),
        // free text — `all:` searches title/abstract/authors/comments.
        _ => format!("all:{value}"),
    }
}

/// What [`refresh`] did this pass.
#[derive(Debug, Default, Serialize)]
pub struct RefreshSummary {
    pub watches_refreshed: usize,
    pub fetched: usize,
    pub new_candidates: usize,
    /// Per-watch / per-candidate failures (non-fatal: refresh continues).
    pub errors: Vec<String>,
}

/// Poll every enabled watch, score the un-ingested results against the corpus,
/// and persist the candidates. Failures on one watch or one candidate are
/// collected into the summary rather than aborting the whole refresh.
pub async fn refresh(paths: &KbPaths, config: &Config) -> Result<RefreshSummary, KbError> {
    let db = MetaDb::open(&paths.meta_db_path())?;
    let watches: Vec<_> = db.list_watches()?.into_iter().filter(|w| w.enabled).collect();
    let ingested: HashSet<String> = paths.list_paper_ids()?.into_iter().collect();

    let client = reqwest::Client::builder()
        .user_agent("kb-arxiv-watch/0.1")
        .build()
        .map_err(|e| KbError::Network(format!("building http client: {e}")))?;

    let mut summary = RefreshSummary::default();

    for w in &watches {
        let q = search_query_for(&w.kind, &w.value);
        let papers = match arxiv::fetch_search(&client, &q, DEFAULT_PER_WATCH).await {
            Ok(p) => p,
            Err(e) => {
                summary.errors.push(format!("watch #{} ({q}): {e}", w.id));
                continue;
            }
        };
        summary.fetched += papers.len();

        for p in papers {
            if ingested.contains(&p.arxiv_id) {
                continue;
            }
            let (score, why) = match score_candidate(paths, config, &p).await {
                Ok(v) => v,
                Err(e) => {
                    summary.errors.push(format!("scoring {}: {e}", p.arxiv_id));
                    continue;
                }
            };
            let cand = NewCandidate {
                arxiv_id: p.arxiv_id.clone(),
                watch_id: w.id,
                title: p.title.clone(),
                abstract_text: p.abstract_text.clone(),
                authors_json: serde_json::to_string(&p.authors).unwrap_or_else(|_| "[]".into()),
                categories_json: serde_json::to_string(&p.categories)
                    .unwrap_or_else(|_| "[]".into()),
                published_at: p.published_at.clone(),
                score,
                why_json: why.to_string(),
            };
            if db.upsert_candidate(&cand)? {
                summary.new_candidates += 1;
            }
        }
        db.set_watch_refreshed(w.id, &crate::now_rfc3339())?;
        summary.watches_refreshed += 1;
    }

    Ok(summary)
}

/// Score one candidate by its connection to the corpus, returning the score
/// and a JSON `why` (the connecting papers/reflections). Runs the candidate's
/// title+abstract through the normal retrieval pass — zero new embedding
/// plumbing, and the matches are the explanation.
async fn score_candidate(
    paths: &KbPaths,
    config: &Config,
    p: &PaperMetadata,
) -> Result<(f32, Value), KbError> {
    let query = format!("{}\n\n{}", p.title, p.abstract_text);
    // Raw cosine connection strength (not the blended/RRF search score, whose
    // scale is tiny and doesn't separate candidates).
    let connections = retrieval::connection_strength(paths, config, &query, 8).await?;

    let connects_to_synthesis = connections
        .iter()
        .take(5)
        .any(|c| c.kind == "reflection" || c.kind == "note");

    let why_connections: Vec<Value> = connections
        .iter()
        .take(MAX_CONNECTIONS)
        .map(|c| {
            json!({
                "paper_id": c.paper_id,
                "title": c.title,
                "kind": c.kind,
                "score": c.score,
                "sections": c.section_types,
            })
        })
        .collect();

    let scores: Vec<f32> = connections.iter().map(|c| c.score).collect();
    let score = aggregate_score(&scores, connects_to_synthesis);

    let why = json!({
        "connections": why_connections,
        "connects_to_synthesis": connects_to_synthesis,
    });
    Ok((score, why))
}

/// Blend the corpus cosine-similarity scores into a single 0..1 relevance for a
/// candidate. The inputs are raw cosines (related passages sit ~0.45–0.65 for
/// text-embedding-3-small), so the output lands on the same scale the rest of
/// the UI grades against. Weighted toward the single best match (a sharp
/// connection matters most), tempered by the mean of the top three (breadth),
/// with a small bonus when the candidate connects to the user's *own* synthesis
/// (a reflection or idea) rather than only to raw paper text. The bonus is
/// deliberately small relative to the cosine signal so it only breaks near-ties.
fn aggregate_score(scores: &[f32], connects_to_synthesis: bool) -> f32 {
    if scores.is_empty() {
        return 0.0;
    }
    let top = scores[0];
    let n = scores.len().min(3);
    let mean_top = scores.iter().take(3).sum::<f32>() / n as f32;
    let bonus = if connects_to_synthesis { 0.05 } else { 0.0 };
    (0.7 * top + 0.3 * mean_top + bonus).clamp(0.0, 1.0)
}

// ---- the daily brief ----

/// One resurfaced past synthesis (a reflection or idea), rotated so each is
/// revisited in turn.
#[derive(Debug, Clone, Serialize)]
pub struct Resurfaced {
    pub paper_id: String,
    pub kind: String,
    pub title: String,
    pub snippet: String,
}

/// Headline counts for the brief.
#[derive(Debug, Clone, Serialize)]
pub struct BriefStats {
    pub papers: usize,
    pub new_candidates: usize,
    pub watches: usize,
}

/// The assembled daily brief — "the KB comes to you".
#[derive(Debug, Clone, Serialize)]
pub struct Brief {
    pub generated_at: String,
    pub new_papers: Vec<Candidate>,
    pub sparks: Vec<Value>,
    pub resurfaced: Option<Resurfaced>,
    pub stats: BriefStats,
}

/// Assemble the brief: top new candidate papers, a few fresh Sparks, one
/// resurfaced reflection, and headline stats. Pure read except for advancing
/// the resurfacing rotation cursor.
pub fn brief(paths: &KbPaths, papers_limit: usize) -> Result<Brief, KbError> {
    let db = MetaDb::open(&paths.meta_db_path())?;

    let new_papers = db.list_candidates(Some("new"), papers_limit)?;

    let sparks = db
        .top_sparks(5, None)
        .unwrap_or_default()
        .iter()
        .map(|e| {
            json!({
                "kind": e.kind,
                "surprise": e.surprise,
                "similarity": e.similarity,
                "src": { "paper": e.src_paper, "section": e.src_section, "snippet": e.src_snippet },
                "dst": { "paper": e.dst_paper, "section": e.dst_section, "snippet": e.dst_snippet },
            })
        })
        .collect();

    let resurfaced = pick_resurfaced(paths, &db)?;

    let new_candidates = db
        .candidate_status_counts()?
        .into_iter()
        .find(|(s, _)| s == "new")
        .map(|(_, n)| n)
        .unwrap_or(0);

    let stats = BriefStats {
        papers: paths.list_paper_ids()?.len(),
        new_candidates,
        watches: db.list_watches()?.len(),
    };

    Ok(Brief {
        generated_at: crate::now_rfc3339(),
        new_papers,
        sparks,
        resurfaced,
        stats,
    })
}

/// Choose the least-recently-surfaced reflection (falling back to ideas) and
/// advance the rotation cursor. The cursor is a `{paper_id: last_surfaced_at}`
/// map in the meta KV — a never-surfaced doc sorts first (empty string), so the
/// rotation visits everything before repeating.
fn pick_resurfaced(paths: &KbPaths, db: &MetaDb) -> Result<Option<Resurfaced>, KbError> {
    let mut ids = db.paper_ids_by_kind(DocKind::Reflection)?;
    ids.extend(db.paper_ids_by_kind(DocKind::Note)?);
    if ids.is_empty() {
        return Ok(None);
    }

    let mut seen: HashMap<String, String> = db
        .meta_get("brief_resurfaced")?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let chosen = ids
        .iter()
        .min_by(|a, b| {
            let sa = seen.get(*a).map(String::as_str).unwrap_or("");
            let sb = seen.get(*b).map(String::as_str).unwrap_or("");
            sa.cmp(sb).then(a.cmp(b))
        })
        .cloned()
        .expect("ids is non-empty");

    seen.insert(chosen.clone(), crate::now_rfc3339());
    if let Ok(json) = serde_json::to_string(&seen) {
        db.meta_set("brief_resurfaced", &json)?;
    }

    let meta = PaperMetadata::load(&paths.metadata_path(&chosen)).ok();
    let kind = meta
        .as_ref()
        .map(|m| m.kind.as_str().to_string())
        .unwrap_or_else(|| "reflection".to_string());
    let title = meta
        .as_ref()
        .map(|m| m.title.clone())
        .unwrap_or_else(|| chosen.clone());

    // Body lives in reflection.md (reflections) or idea.md (notes).
    let body_path = if kind == "note" {
        paths.idea_path(&chosen)
    } else {
        paths.reflection_path(&chosen)
    };
    let snippet = std::fs::read_to_string(&body_path)
        .ok()
        .map(|body| snippet_of(&body, 280))
        .unwrap_or_default();

    Ok(Some(Resurfaced {
        paper_id: chosen,
        kind,
        title,
        snippet,
    }))
}

/// First `max_chars` of a body, trimmed at a char boundary, with an ellipsis
/// if truncated.
fn snippet_of(body: &str, max_chars: usize) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut s: String = trimmed.chars().take(max_chars).collect();
    s.push('…');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_query_composition() {
        assert_eq!(search_query_for("category", "cs.LG"), "cat:cs.LG");
        assert_eq!(search_query_for("author", "Vaswani"), "au:Vaswani");
        assert_eq!(
            search_query_for("query", "retrieval augmented"),
            "all:retrieval augmented"
        );
    }

    #[test]
    fn validate_kind_accepts_known_rejects_unknown() {
        for k in WATCH_KINDS {
            assert!(validate_kind(k).is_ok());
        }
        assert!(validate_kind("nonsense").is_err());
    }

    #[test]
    fn aggregate_score_empty_is_zero() {
        assert_eq!(aggregate_score(&[], false), 0.0);
        assert_eq!(aggregate_score(&[], true), 0.0);
    }

    #[test]
    fn aggregate_score_rewards_strong_and_synthesis() {
        let weak = aggregate_score(&[0.3, 0.2, 0.1], false);
        let strong = aggregate_score(&[0.8, 0.7, 0.6], false);
        assert!(strong > weak);
        // synthesis bonus lifts the score, clamped at 1.0.
        let plain = aggregate_score(&[0.5, 0.4, 0.3], false);
        let synth = aggregate_score(&[0.5, 0.4, 0.3], true);
        assert!((synth - plain - 0.05).abs() < 1e-5);
        assert!(aggregate_score(&[1.0, 1.0, 1.0], true) <= 1.0);
        // a strongly-related candidate (cosine ~0.55) clears the UI's green
        // threshold (0.45); an unrelated one (cosine ~0.2) stays well below.
        assert!(aggregate_score(&[0.58, 0.52, 0.49], false) >= 0.45);
        assert!(aggregate_score(&[0.22, 0.18, 0.15], false) < 0.30);
    }

    #[test]
    fn snippet_truncates_on_char_boundary() {
        assert_eq!(snippet_of("hello", 10), "hello");
        let s = snippet_of("a longer body of text here", 8);
        assert!(s.ends_with('…'));
        assert_eq!(s.chars().count(), 9); // 8 + ellipsis
    }
}
