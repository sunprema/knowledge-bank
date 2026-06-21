//! Config loading (`.arxiv-kb/config.toml`), env overrides, KB folder paths.
//!
//! Resolution order for the KB root: `--root` flag > `KB_ROOT` env >
//! `~/arxiv-kb`. Env vars override config values (PRD §10).

use crate::{KbError, SCHEMA_VERSION, SectionType};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub schema_version: u32,
    pub embedding: EmbeddingConfig,
    pub chat: ChatConfig,
    pub turbovec: TurbovecConfig,
    pub search: SearchConfig,
    pub cortex: CortexConfig,
    pub ingest: IngestConfig,
    pub server: ServerConfig,
    pub watcher: WatcherConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            schema_version: SCHEMA_VERSION,
            embedding: EmbeddingConfig::default(),
            chat: ChatConfig::default(),
            turbovec: TurbovecConfig::default(),
            search: SearchConfig::default(),
            cortex: CortexConfig::default(),
            ingest: IngestConfig::default(),
            server: ServerConfig::default(),
            watcher: WatcherConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        EmbeddingConfig {
            provider: "openai".into(),
            model: "text-embedding-3-small".into(),
            dimensions: 1536,
        }
    }
}

/// Chat-over-corpus (`POST /chat`, web app "Chat" view): a RAG layer that
/// answers questions over the corpus with inline citations. Uses OpenAI
/// chat-completions so it shares the single `OPENAI_API_KEY` the embedding
/// pipeline already needs — no second provider/credential.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatConfig {
    pub provider: String,
    pub model: String,
    /// How many retrieved chunks to feed the model as numbered sources.
    pub max_context_chunks: usize,
    /// Sampling temperature — low keeps answers grounded in the sources.
    pub temperature: f32,
}

impl Default for ChatConfig {
    fn default() -> Self {
        ChatConfig {
            provider: "openai".into(),
            model: "gpt-4o-mini".into(),
            max_context_chunks: 12,
            temperature: 0.2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TurbovecConfig {
    pub bit_width: usize,
}

impl Default for TurbovecConfig {
    fn default() -> Self {
        TurbovecConfig { bit_width: 4 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    pub default_k_narrow: usize,
    pub default_k_wide: usize,
    pub default_min_score_narrow: f32,
    pub default_min_score_wide: f32,
    pub ranking: RankingConfig,
    pub hybrid: HybridConfig,
    pub graph: GraphRankConfig,
}

impl Default for SearchConfig {
    fn default() -> Self {
        SearchConfig {
            default_k_narrow: 10,
            default_k_wide: 40,
            // Calibrated for text-embedding-3-small + 4-bit turbovec, whose
            // scores for clearly-relevant matches sit around 0.45-0.60 (the
            // PRD's original 0.72 hid everything — see smoke-test finding).
            default_min_score_narrow: 0.30,
            default_min_score_wide: 0.0,
            ranking: RankingConfig::default(),
            hybrid: HybridConfig::default(),
            graph: GraphRankConfig::default(),
        }
    }
}

/// Hybrid retrieval: fuse the dense (vector) ranking with a lexical (BM25 /
/// SQLite FTS5) ranking via Reciprocal Rank Fusion. Dense embeddings miss
/// exact-token queries — an author name, a method name, an arXiv id, a rare
/// symbol — which BM25 nails; fusion gets the strengths of both. The dense
/// side is ranked by the [`RankingConfig`] blend, so recency/importance still
/// flow through into the fused result.
///
/// RRF score for a chunk = `Σ weight_i / (rrf_k + rank_i)` over the rankings
/// it appears in (1-based rank). It combines ranks, not raw scores, so the
/// two incomparable score scales (cosine vs. BM25) never need calibrating.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HybridConfig {
    /// `false` ⇒ dense-only (the [`RankingConfig`] blend, unchanged).
    pub enabled: bool,
    pub dense_weight: f32,
    pub lexical_weight: f32,
    /// RRF dampening constant (the standard value is 60): larger ⇒ flatter,
    /// so deep ranks still contribute and no single list dominates the top.
    pub rrf_k: f32,
}

impl Default for HybridConfig {
    fn default() -> Self {
        HybridConfig {
            enabled: true,
            dense_weight: 1.0,
            lexical_weight: 1.0,
            rrf_k: 60.0,
        }
    }
}

/// Graph-propagated retrieval: a Personalized PageRank pass over a chunk
/// similarity graph, fused into the hybrid RRF as a third ranked list. This is
/// HippoRAG's retrieval mechanism (arXiv:2405.14831, in this corpus) — seed PPR
/// from the query's dense matches, let relevance propagate across edges, rank by
/// the stationary distribution — so a chunk relevant *because it is linked to*
/// relevant material surfaces even when its own text shares no tokens with the
/// query (the single-step multi-hop case pure dense + BM25 both miss).
///
/// The faithful adaptation: HippoRAG builds its graph from LLM-extracted
/// entities (an OpenIE indexing pass + a separate store). We instead walk the
/// graph signal already in the KB — embedding-similarity kNN edges plus the
/// explicit `[[id]]` / `--link` / `--scope` relations — so PPR needs no new
/// index and no extra API calls (seed and neighbor vectors come from the
/// embedding cache). The subgraph is expanded locally around the dense seeds,
/// where PPR-with-restart concentrates its mass anyway.
///
/// Off by default: when `enabled = false` the search path is byte-for-byte
/// unchanged. The RRF score for a chunk gains a `graph_weight / (rrf_k + rank)`
/// term (same `rrf_k` as [`HybridConfig`]) for its rank in the PPR list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphRankConfig {
    /// `false` ⇒ no PPR pass (dense + lexical only, unchanged).
    pub enabled: bool,
    /// Weight of the PPR ranked list in the RRF sum (peer of dense/lexical).
    pub graph_weight: f32,
    /// kNN similarity edges expanded per seed chunk when building the subgraph.
    pub neighbors: usize,
    /// PPR damping (restart probability is `1 - damping`): lower ⇒ mass stays
    /// nearer the seeds, higher ⇒ relevance propagates further across hops.
    pub damping: f32,
    /// Power-iteration steps. ~15 converges the small per-query subgraph.
    pub iterations: usize,
}

impl Default for GraphRankConfig {
    fn default() -> Self {
        GraphRankConfig {
            enabled: false,
            graph_weight: 1.0,
            neighbors: 8,
            damping: 0.5,
            iterations: 15,
        }
    }
}

/// Recency/importance-weighted ranking (cf. Generative Agents,
/// arXiv:2304.03442). The final chunk score is a weighted blend
/// `relevance_weight·cosine + recency_weight·recency + importance_weight·importance`,
/// where `recency` decays exponentially with the chunk's age since it was
/// embedded and `importance` is the section-type prior. Defaults keep
/// relevance dominant — the other terms only break near-ties — so set any
/// weight to 0 to disable that signal (all three 0 ⇒ relevance-only).
///
/// The cosine `default_min_score_*` floor still gates candidates *before*
/// blending, so it remains a true relevance floor regardless of these weights.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RankingConfig {
    pub relevance_weight: f32,
    pub recency_weight: f32,
    pub importance_weight: f32,
    /// Age (in days) at which a chunk's recency term halves.
    pub recency_half_life_days: f32,
    /// Over-fetch factor: the index returns `k · candidate_multiplier`
    /// candidates so recency/importance can pull a chunk into the top `k`
    /// that pure cosine would have ranked just outside it. `1` ⇒ rerank only
    /// within the cosine top-k (cheaper, but the blend can only reorder).
    pub candidate_multiplier: usize,
}

impl Default for RankingConfig {
    fn default() -> Self {
        RankingConfig {
            relevance_weight: 0.85,
            recency_weight: 0.05,
            importance_weight: 0.10,
            recency_half_life_days: 180.0,
            candidate_multiplier: 4,
        }
    }
}

/// Cortex — the persistent associative layer (the "brain"). Where retrieval
/// answers *"what is relevant to this query"*, Cortex answers *"what
/// unexpected connection is worth noticing"*. On every ingest it materializes
/// edges between the new document's chunks and the rest of the corpus, but
/// keeps only the **surprising** ones — connections that are semantically close
/// yet structurally distant, which is where novel ideas live (pure
/// nearest-neighbor similarity surfaces the obvious, not the inventive).
///
/// Two signals are scored (both API-free — they reuse the embedding cache):
///
/// - **need→solution** (directed): one chunk's `future_work`/`limitations`
///   (a stated need) sits close to another chunk's `method`/`experiments`/
///   `applications` (a delivered capability). "Someone wished for this;
///   someone else built it."
/// - **cross-domain** (undirected): two chunks are close in meaning but their
///   papers share no arXiv category — the same idea surfacing in a different
///   field, i.e. a transfer-of-ideas opportunity.
///
/// Edges live in `meta.db`'s `cortex_edges` table — derived state, rebuilt by
/// `kb reindex` (or `kb cortex rebuild`) from the embeddings, never canonical.
/// Surface them with `kb spark` or the web app's Sparks view.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CortexConfig {
    /// `false` ⇒ no edges materialized on ingest and `kb spark` stays empty.
    /// On by default: it costs no API calls and is the point of the system.
    pub enabled: bool,
    /// Cross-paper nearest neighbors examined per chunk when looking for
    /// connections. Higher ⇒ more candidate edges (and more ingest compute).
    pub neighbors: usize,
    /// Proximity floor: a pair must reach at least this exact cosine to be a
    /// connection at all. Calibrated for text-embedding-3-small, whose related
    /// passages sit around 0.45-0.65 — high enough that an edge means
    /// something, low enough that genuine cross-field echoes survive.
    pub min_similarity: f32,
    /// Cross-domain gate: `1 - Jaccard(categories)` must reach this. `1.0` ⇒
    /// the papers' arXiv categories must be fully disjoint (default); lower it
    /// to also surface partial-overlap connections.
    pub min_domain_distance: f32,
    /// Default number of sparks returned by `kb spark` / the web view.
    pub max_sparks: usize,
}

impl Default for CortexConfig {
    fn default() -> Self {
        CortexConfig {
            enabled: true,
            neighbors: 6,
            min_similarity: 0.5,
            min_domain_distance: 1.0,
            max_sparks: 50,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IngestConfig {
    pub chunk_max_tokens: usize,
    pub prefer_latex: bool,
    pub pandoc_path: String,
    /// LLM fallback for the section classifier: when a heading's deterministic
    /// classification is `other`, ask the chat model (`[chat] model`) to map it
    /// to a real section type. One batched call per paper, only for the
    /// otherwise-`other` headings — the keyword fast-path is unchanged, and the
    /// call is best-effort (a failure or missing `OPENAI_API_KEY` falls back to
    /// `other`). The PRD pre-authorized this once the Other ratio exceeds ~25%.
    pub classify_with_llm: bool,
}

impl Default for IngestConfig {
    fn default() -> Self {
        IngestConfig {
            chunk_max_tokens: 2000,
            prefer_latex: true,
            pandoc_path: "pandoc".into(),
            classify_with_llm: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub http_port: u16,
    pub http_bind: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            http_port: 4321,
            http_bind: "127.0.0.1".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WatcherConfig {
    pub debounce_ms: u64,
    /// Auto-ingest files dropped into `<root>/inbox/` while `kb watch` runs.
    pub inbox_enabled: bool,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        WatcherConfig {
            debounce_ms: 2000,
            inbox_enabled: true,
        }
    }
}

impl Config {
    /// Load `.arxiv-kb/config.toml`, creating the directory and a default
    /// config on first run. Refuses configs with a newer schema_version.
    pub fn load_or_create(paths: &KbPaths) -> Result<Self, KbError> {
        let dot = paths.dot_dir();
        std::fs::create_dir_all(&dot)
            .map_err(|e| KbError::Config(format!("cannot create {}: {e}", dot.display())))?;
        let path = paths.config_path();
        if !path.exists() {
            let cfg = Config::default();
            cfg.save(&path)?;
            return Ok(cfg);
        }
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| KbError::Config(format!("cannot read {}: {e}", path.display())))?;
        let cfg: Config = toml::from_str(&raw)
            .map_err(|e| KbError::Config(format!("malformed {}: {e}", path.display())))?;
        if cfg.schema_version > SCHEMA_VERSION {
            return Err(KbError::Config(format!(
                "config schema_version is {} but this binary supports {}; \
                 upgrade kb, or run `kb reindex` to rebuild at this version",
                cfg.schema_version, SCHEMA_VERSION
            )));
        }
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<(), KbError> {
        let body = toml::to_string_pretty(self)
            .map_err(|e| KbError::Config(format!("serialize config: {e}")))?;
        std::fs::write(path, body)
            .map_err(|e| KbError::Config(format!("write {}: {e}", path.display())))?;
        Ok(())
    }

    /// Fingerprint of settings that make existing *vectors* unusable when
    /// changed (resolved decision: mismatch ⇒ refuse queries until reindex).
    pub fn vector_fingerprint(&self) -> String {
        format!(
            "{}:{}:{}",
            self.embedding.model, self.embedding.dimensions, self.turbovec.bit_width
        )
    }

    /// Fingerprint of chunking-only settings (mismatch ⇒ warn but serve).
    pub fn chunking_fingerprint(&self) -> String {
        format!("chunk_max_tokens={}", self.ingest.chunk_max_tokens)
    }
}

/// All filesystem locations derived from the KB root (PRD §3 folder layout).
#[derive(Debug, Clone)]
pub struct KbPaths {
    pub root: PathBuf,
}

impl KbPaths {
    /// `--root` flag > `KB_ROOT` env > `~/arxiv-kb`.
    pub fn resolve(cli_root: Option<PathBuf>) -> Result<Self, KbError> {
        let root = match cli_root {
            Some(r) => r,
            None => match std::env::var_os("KB_ROOT") {
                Some(r) => PathBuf::from(r),
                None => dirs::home_dir()
                    .ok_or_else(|| KbError::Config("cannot determine home directory".into()))?
                    .join("arxiv-kb"),
            },
        };
        Ok(KbPaths { root })
    }

    pub fn dot_dir(&self) -> PathBuf {
        self.root.join(".arxiv-kb")
    }
    pub fn config_path(&self) -> PathBuf {
        self.dot_dir().join("config.toml")
    }
    pub fn index_path(&self) -> PathBuf {
        self.dot_dir().join("index.tv")
    }
    pub fn meta_db_path(&self) -> PathBuf {
        self.dot_dir().join("meta.db")
    }
    pub fn log_path(&self) -> PathBuf {
        self.dot_dir().join("kb.log")
    }
    pub fn pid_path(&self) -> PathBuf {
        self.dot_dir().join("kb.pid")
    }
    pub fn api_key_path(&self) -> PathBuf {
        self.dot_dir().join("api_key")
    }

    /// Drop-folder watched by `kb watch` for auto-ingest of `*.pdf` and
    /// `*.url`/`*.txt` (URL lists). Visible on the drive, beside the paper
    /// folders.
    pub fn inbox_dir(&self) -> PathBuf {
        self.root.join("inbox")
    }
    /// Where files that failed to ingest are parked (so they aren't retried
    /// forever and the user can see what didn't work).
    pub fn inbox_failed_dir(&self) -> PathBuf {
        self.inbox_dir().join("failed")
    }

    pub fn paper_dir(&self, arxiv_id: &str) -> PathBuf {
        self.root.join(arxiv_id)
    }
    pub fn metadata_path(&self, arxiv_id: &str) -> PathBuf {
        self.paper_dir(arxiv_id).join("metadata.json")
    }
    pub fn source_dir(&self, arxiv_id: &str) -> PathBuf {
        self.paper_dir(arxiv_id).join("source")
    }
    pub fn pdf_path(&self, arxiv_id: &str) -> PathBuf {
        self.paper_dir(arxiv_id).join("paper.pdf")
    }
    pub fn sections_path(&self, arxiv_id: &str) -> PathBuf {
        self.paper_dir(arxiv_id).join("sections.md")
    }
    pub fn notes_path(&self, arxiv_id: &str) -> PathBuf {
        self.paper_dir(arxiv_id).join("notes.md")
    }
    /// Derived "Clean Read": a faithful, citation-free rewrite of the paper body,
    /// generated on demand and cached here. Not embedded into the index.
    pub fn reader_path(&self, arxiv_id: &str) -> PathBuf {
        self.paper_dir(arxiv_id).join("reader.md")
    }
    /// Canonical body of a standalone idea (`kind = note`).
    pub fn idea_path(&self, id: &str) -> PathBuf {
        self.paper_dir(id).join("idea.md")
    }
    /// Canonical body of a cross-paper reflection (`kind = reflection`).
    pub fn reflection_path(&self, id: &str) -> PathBuf {
        self.paper_dir(id).join("reflection.md")
    }

    /// Deep-link target for a chunk: the PDF for ingested papers, else the
    /// note/reflection body. Reflections live in `reflection.md`, notes in
    /// `idea.md`, so the body path is chosen by section type.
    pub fn link_target(&self, id: &str, section: SectionType) -> PathBuf {
        let pdf = self.pdf_path(id);
        if pdf.exists() {
            pdf
        } else if section == SectionType::Reflection {
            self.reflection_path(id)
        } else {
            self.idea_path(id)
        }
    }

    /// Paper folders = direct children of root containing a metadata.json
    /// (skips `.arxiv-kb` and anything else). Sorted for stable output.
    pub fn list_paper_ids(&self) -> Result<Vec<String>, KbError> {
        let mut ids = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(_) => return Ok(ids), // no root yet = empty corpus
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && path.join("metadata.json").exists()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                ids.push(name.to_string());
            }
        }
        ids.sort();
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbox_paths_live_under_root() {
        let paths = KbPaths { root: PathBuf::from("/kb") };
        assert_eq!(paths.inbox_dir(), PathBuf::from("/kb/inbox"));
        assert_eq!(paths.inbox_failed_dir(), PathBuf::from("/kb/inbox/failed"));
    }

    #[test]
    fn inbox_is_enabled_by_default() {
        assert!(WatcherConfig::default().inbox_enabled);
    }
}
