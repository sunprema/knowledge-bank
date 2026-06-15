//! Graph-propagated retrieval: Personalized PageRank over a per-query chunk
//! similarity graph. This is HippoRAG's retrieval mechanism (arXiv:2405.14831,
//! in this corpus) — seed PPR from the query's dense matches and rank chunks by
//! the stationary distribution, so a chunk relevant *because it links to*
//! relevant material surfaces even when its own text shares no tokens with the
//! query. See [`GraphRankConfig`] for the faithful-adaptation note (we walk the
//! KB's existing similarity + `[[id]]` edges instead of an LLM-extracted entity
//! graph). The output is a ranked list the caller fuses into the hybrid RRF.

use crate::config::{GraphRankConfig, KbPaths};
use crate::index::{MetaDb, VectorIndex};
use crate::{ChunkRecord, KbError, PaperMetadata};
use std::collections::HashMap;

/// Add an undirected weighted edge; weights accumulate if a pair is discovered
/// more than once (e.g. a mutual kNN edge that is also an explicit link).
fn add_edge(adj: &mut HashMap<i64, HashMap<i64, f32>>, a: i64, b: i64, w: f32) {
    if a == b || w <= 0.0 {
        return;
    }
    *adj.entry(a).or_default().entry(b).or_insert(0.0) += w;
    *adj.entry(b).or_default().entry(a).or_insert(0.0) += w;
}

/// Rank chunks by Personalized PageRank over a similarity subgraph grown around
/// the dense seeds. Returns `vector_id`s ordered by PPR score (descending).
///
/// `seeds` are `(vector_id, query relevance)` pairs (cosine) — the
/// personalization vector. `seed_records` supplies the seeds' cache keys and
/// paper ids; records hydrated for newly discovered neighbor nodes are written
/// to `sink` so the caller can fuse them without re-querying.
#[allow(clippy::too_many_arguments)]
pub fn ppr_rank(
    paths: &KbPaths,
    db: &MetaDb,
    index: &VectorIndex,
    seeds: &[(i64, f32)],
    seed_records: &HashMap<i64, ChunkRecord>,
    allowlist: Option<&[u64]>,
    cfg: &GraphRankConfig,
    sink: &mut HashMap<i64, ChunkRecord>,
) -> Result<Vec<i64>, KbError> {
    if !cfg.enabled || seeds.is_empty() {
        return Ok(Vec::new());
    }

    // Personalization: query relevance (cosine) over the seed nodes, ≥0 and
    // normalized to sum 1. A non-positive total means no usable seed.
    let mut personalization: HashMap<i64, f32> = HashMap::new();
    let mut seed_sum = 0.0f32;
    for &(id, w) in seeds {
        let w = w.max(0.0);
        if w > 0.0 {
            *personalization.entry(id).or_insert(0.0) += w;
            seed_sum += w;
        }
    }
    if seed_sum <= 0.0 {
        return Ok(Vec::new());
    }
    for v in personalization.values_mut() {
        *v /= seed_sum;
    }

    // Grow a local similarity graph: each seed's nearest neighbors become nodes,
    // edges weighted by cosine. PPR-with-restart concentrates mass near the
    // seeds, so a one-hop neighborhood approximates full-graph PPR.
    let mut adj: HashMap<i64, HashMap<i64, f32>> = HashMap::new();
    let want = cfg.neighbors.saturating_add(1).max(1);
    let mut discovered: Vec<i64> = Vec::new();
    for &(seed_id, _) in seeds {
        let Some(rec) = seed_records.get(&seed_id) else {
            continue;
        };
        let Some(vec) =
            db.cache_get(&rec.content_hash, &rec.embedding_model, rec.embedding_version)?
        else {
            // No cached vector (e.g. after `kb cache clear`) — keep the seed as
            // a node via personalization, just don't expand from it.
            continue;
        };
        for (nid, score) in index.search(&vec, want, allowlist)? {
            let nid = nid as i64;
            add_edge(&mut adj, seed_id, nid, score.max(0.0));
            if nid != seed_id
                && !seed_records.contains_key(&nid)
                && !sink.contains_key(&nid)
            {
                discovered.push(nid);
            }
        }
    }

    // Hydrate records for the discovered neighbors (their paper id is needed to
    // lift explicit document links, and the caller fuses them in by id).
    discovered.sort_unstable();
    discovered.dedup();
    if !discovered.is_empty() {
        for rec in db.chunks_by_vector_ids(&discovered)? {
            sink.insert(rec.vector_id, rec);
        }
    }

    // Lift explicit `[[id]]` / `--link` / `--scope` relations: if two papers are
    // linked, connect the chunks of theirs that are already nodes. A curated
    // link is a strong relation, so weight it at 1.0 (cosine edges are < 1).
    let paper_of = |id: i64| -> Option<String> {
        seed_records
            .get(&id)
            .or_else(|| sink.get(&id))
            .map(|r| r.paper_id.clone())
    };
    let mut chunks_by_paper: HashMap<String, Vec<i64>> = HashMap::new();
    for &id in adj.keys() {
        if let Some(p) = paper_of(id) {
            chunks_by_paper.entry(p).or_default().push(id);
        }
    }
    let papers: Vec<String> = chunks_by_paper.keys().cloned().collect();
    for paper in &papers {
        let Ok(meta) = PaperMetadata::load(&paths.metadata_path(paper)) else {
            continue;
        };
        let Some(src_chunks) = chunks_by_paper.get(paper).cloned() else {
            continue;
        };
        for target in &meta.links {
            let Some(tgt_chunks) = chunks_by_paper.get(target) else {
                continue;
            };
            for &a in &src_chunks {
                for &b in tgt_chunks {
                    add_edge(&mut adj, a, b, 1.0);
                }
            }
        }
    }

    if adj.is_empty() {
        return Ok(Vec::new());
    }

    // Power-iteration PPR: r = (1-d)·p + d·Wᵀr, with W row-normalized over each
    // node's incident edge weights (the graph is undirected, so weights are
    // symmetric). Renormalize r each step so mass that would leak at isolated
    // nodes is redistributed, keeping Σr = 1.
    let nodes: Vec<i64> = adj.keys().copied().collect();
    let outdeg: HashMap<i64, f32> =
        nodes.iter().map(|&n| (n, adj[&n].values().sum::<f32>())).collect();
    let p = |id: i64| personalization.get(&id).copied().unwrap_or(0.0);

    let d = cfg.damping.clamp(0.0, 0.99);
    let mut r: HashMap<i64, f32> = nodes.iter().map(|&n| (n, p(n))).collect();
    for _ in 0..cfg.iterations.max(1) {
        let mut next: HashMap<i64, f32> =
            nodes.iter().map(|&n| (n, (1.0 - d) * p(n))).collect();
        for &j in &nodes {
            let rj = r[&j];
            let od = outdeg[&j];
            if rj == 0.0 || od <= 0.0 {
                continue;
            }
            for (&i, &w) in &adj[&j] {
                *next.get_mut(&i).unwrap() += d * rj * (w / od);
            }
        }
        let sum: f32 = next.values().sum();
        if sum > 0.0 {
            for v in next.values_mut() {
                *v /= sum;
            }
        }
        r = next;
    }

    let mut ranked: Vec<(i64, f32)> = r.into_iter().collect();
    // Descending score; tiebreak on id for deterministic ranks.
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(ranked.into_iter().map(|(id, _)| id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // PPR power iteration on a hand-built graph, bypassing the index/db. The
    // shape that matters for multi-hop: a node with no query relevance of its
    // own, reachable only through a relevant neighbor, must outrank an
    // unrelated node. We replicate the iteration here over an explicit `adj`.
    fn power_iterate(
        adj: &HashMap<i64, HashMap<i64, f32>>,
        personalization: &HashMap<i64, f32>,
        d: f32,
        iters: usize,
    ) -> Vec<(i64, f32)> {
        let nodes: Vec<i64> = adj.keys().copied().collect();
        let outdeg: HashMap<i64, f32> =
            nodes.iter().map(|&n| (n, adj[&n].values().sum::<f32>())).collect();
        let p = |id: i64| personalization.get(&id).copied().unwrap_or(0.0);
        let mut r: HashMap<i64, f32> = nodes.iter().map(|&n| (n, p(n))).collect();
        for _ in 0..iters {
            let mut next: HashMap<i64, f32> =
                nodes.iter().map(|&n| (n, (1.0 - d) * p(n))).collect();
            for &j in &nodes {
                let rj = r[&j];
                let od = outdeg[&j];
                if rj == 0.0 || od <= 0.0 {
                    continue;
                }
                for (&i, &w) in &adj[&j] {
                    *next.get_mut(&i).unwrap() += d * rj * (w / od);
                }
            }
            let sum: f32 = next.values().sum();
            for v in next.values_mut() {
                *v /= sum;
            }
            r = next;
        }
        let mut out: Vec<(i64, f32)> = r.into_iter().collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    }

    #[test]
    fn multi_hop_node_outranks_unrelated() {
        // Graph: seed 1 —(0.9)— node 2 —(0.9)— node 3 ; isolated node 4 links
        // only to a far node 5. Only node 1 carries query relevance.
        let mut adj: HashMap<i64, HashMap<i64, f32>> = HashMap::new();
        add_edge(&mut adj, 1, 2, 0.9);
        add_edge(&mut adj, 2, 3, 0.9);
        add_edge(&mut adj, 4, 5, 0.9);
        let personalization = HashMap::from([(1i64, 1.0f32)]);

        let ranked = power_iterate(&adj, &personalization, 0.5, 50);
        let score: HashMap<i64, f32> = ranked.iter().copied().collect();

        // The seed ranks first; its 2-hop neighbors (2, 3) get propagated mass;
        // the disconnected component (4, 5) gets none.
        assert_eq!(ranked[0].0, 1);
        assert!(score[&2] > score[&4], "linked neighbor must beat unrelated node");
        assert!(score[&3] > score[&4], "2-hop neighbor must beat unrelated node");
        assert!(score[&4].abs() < 1e-6, "disconnected node gets no PPR mass");
    }

    #[test]
    fn closer_neighbor_scores_higher() {
        // Two neighbors of the seed, different edge weights: the stronger edge
        // should carry more propagated relevance.
        let mut adj: HashMap<i64, HashMap<i64, f32>> = HashMap::new();
        add_edge(&mut adj, 1, 2, 0.9);
        add_edge(&mut adj, 1, 3, 0.3);
        let personalization = HashMap::from([(1i64, 1.0f32)]);

        let ranked = power_iterate(&adj, &personalization, 0.5, 50);
        let score: HashMap<i64, f32> = ranked.iter().copied().collect();
        assert!(score[&2] > score[&3], "stronger edge ⇒ more propagated mass");
    }

    #[test]
    fn add_edge_is_symmetric_and_accumulates() {
        let mut adj: HashMap<i64, HashMap<i64, f32>> = HashMap::new();
        add_edge(&mut adj, 1, 2, 0.4);
        add_edge(&mut adj, 2, 1, 0.5); // same undirected pair, accumulates
        assert!((adj[&1][&2] - 0.9).abs() < 1e-6);
        assert!((adj[&2][&1] - 0.9).abs() < 1e-6);
        // Self-loops and non-positive weights are ignored.
        add_edge(&mut adj, 1, 1, 1.0);
        add_edge(&mut adj, 1, 3, 0.0);
        assert!(!adj[&1].contains_key(&1));
        assert!(!adj[&1].contains_key(&3));
    }
}
