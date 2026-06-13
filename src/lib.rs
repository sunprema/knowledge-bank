//! arxiv-kb core library.
//!
//! Design invariant (PRD §1): the user's canonical files — `metadata.json`,
//! `source/*.tex`, `paper.pdf`, `notes.md` — are the source of truth. The
//! turbovec index and meta.db are derived artifacts, rebuildable at any time
//! via `kb reindex`.

pub mod commands;
pub mod config;
pub mod embed;
pub mod error;
pub mod index;
pub mod ingest;
pub mod search;
pub mod server;
pub mod watcher;

pub use error::KbError;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

/// Schema version written to metadata.json, config.toml and meta.db.
/// Policy (PRD §16, resolved): refuse to read anything newer, with a clear
/// error naming both versions and the remedies (upgrade binary / kb reindex).
pub const SCHEMA_VERSION: u32 = 1;

/// Bumped manually if the embedding API's output ever changes semantically
/// under the same model name. Part of the embedding-cache key.
pub const EMBEDDING_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceFormat {
    Latex,
    Pdf,
    /// Standalone ideas (`kb idea add`) — the body is markdown, no PDF.
    Markdown,
    /// Web pages (`kb add --url`) — readability-extracted, no PDF.
    Html,
}

/// Document kind: arXiv/local papers vs standalone idea notes vs cross-paper
/// reflections. Defaults to `Paper` so pre-existing metadata.json files need
/// no migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocKind {
    #[default]
    Paper,
    Note,
    Reflection,
}

impl DocKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            DocKind::Paper => "paper",
            DocKind::Note => "note",
            DocKind::Reflection => "reflection",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "paper" => Some(DocKind::Paper),
            "note" => Some(DocKind::Note),
            "reflection" => Some(DocKind::Reflection),
            _ => None,
        }
    }
}

/// The contents of a paper's `metadata.json`. Canonical and user-owned:
/// written at ingest, modified only by `kb update` (re-fetch) and `kb tag`
/// (tags are canonical here, mirrored into meta.db for query speed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperMetadata {
    pub arxiv_id: String,
    /// `paper` (default) or `note` (a standalone idea).
    #[serde(default)]
    pub kind: DocKind,
    /// Notes only: the project this idea is keyed to (`global` = applies
    /// across every project). Mirrored into meta.db for filtered search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Notes only: ids of related papers/notes (`[[id]]` links).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
    #[serde(default)]
    pub version: Option<String>,
    pub title: String,
    pub authors: Vec<String>,
    #[serde(rename = "abstract")]
    pub abstract_text: String,
    pub categories: Vec<String>,
    pub published_at: String,
    pub updated_at: String,
    pub ingested_at: String,
    pub source_format: SourceFormat,
    /// HTML docs only (`kb add --url`): the page this was ingested from.
    /// Canonical identity for a web page — `kb update` re-fetches it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_tex: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub schema_version: u32,
}

impl PaperMetadata {
    pub fn load(path: &Path) -> Result<Self, KbError> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            KbError::NotFound(format!("cannot read {}: {e}", path.display()))
        })?;
        let meta: Self = serde_json::from_str(&raw).map_err(|e| {
            KbError::Extraction(format!("malformed {}: {e}", path.display()))
        })?;
        if meta.schema_version > SCHEMA_VERSION {
            return Err(KbError::Config(format!(
                "{} has schema_version {} but this binary supports {}; upgrade kb or run `kb reindex`",
                path.display(),
                meta.schema_version,
                SCHEMA_VERSION
            )));
        }
        Ok(meta)
    }

    pub fn save(&self, path: &Path) -> Result<(), KbError> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| KbError::Index(format!("serialize metadata: {e}")))?;
        std::fs::write(path, json + "\n")
            .map_err(|e| KbError::Index(format!("write {}: {e}", path.display())))?;
        Ok(())
    }
}

/// Closed section-type enum (PRD §4 step 6). Ambiguous headings fall through
/// to `Other` (resolved decision — deterministic, no ML). `Reflection` is a
/// synthetic type produced by `kb reflect` / `kb_create_reflection` — not
/// classified from headings but written directly by the user or Claude.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionType {
    Abstract,
    Introduction,
    Background,
    Method,
    Experiments,
    Applications,
    Limitations,
    FutureWork,
    Conclusion,
    UserNotes,
    Reflection,
    Other,
}

impl SectionType {
    pub const ALL: [SectionType; 12] = [
        SectionType::Abstract,
        SectionType::Introduction,
        SectionType::Background,
        SectionType::Method,
        SectionType::Experiments,
        SectionType::Applications,
        SectionType::Limitations,
        SectionType::FutureWork,
        SectionType::Conclusion,
        SectionType::UserNotes,
        SectionType::Reflection,
        SectionType::Other,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            SectionType::Abstract => "abstract",
            SectionType::Introduction => "introduction",
            SectionType::Background => "background",
            SectionType::Method => "method",
            SectionType::Experiments => "experiments",
            SectionType::Applications => "applications",
            SectionType::Limitations => "limitations",
            SectionType::FutureWork => "future_work",
            SectionType::Conclusion => "conclusion",
            SectionType::UserNotes => "user_notes",
            SectionType::Reflection => "reflection",
            SectionType::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| t.as_str() == s)
    }
}

impl fmt::Display for SectionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A chunk produced by the section classifier, before embedding.
/// `ordinal` counts within a section type (0-based), so the stable chunk id
/// is `{paper_id}_{section_type}_{ordinal}` (PRD §4 step 8).
#[derive(Debug, Clone, PartialEq)]
pub struct RawChunk {
    pub section_type: SectionType,
    /// The markdown heading this chunk came from, if any (used for TOC
    /// page mapping). Sub-chunks of a split section share the heading.
    pub heading: Option<String>,
    pub ordinal: u32,
    pub text: String,
}

impl RawChunk {
    pub fn chunk_id(&self, paper_id: &str) -> String {
        format!("{paper_id}_{}_{}", self.section_type.as_str(), self.ordinal)
    }
}

/// One row of meta.db's `chunks` table. `vector_id` is the SQLite
/// autoincrement PK and doubles as the turbovec external id (addendum §3).
#[derive(Debug, Clone)]
pub struct ChunkRecord {
    pub vector_id: i64,
    pub chunk_id: String,
    pub paper_id: String,
    pub section_type: SectionType,
    pub ordinal: u32,
    pub content_hash: String,
    pub text: String,
    pub page: Option<u32>,
    pub snippet: String,
    pub embedded_at: String,
    pub embedding_model: String,
    pub embedding_version: u32,
}

/// One entry of a PDF's outline (PRD §4 step 5).
#[derive(Debug, Clone, PartialEq)]
pub struct TocEntry {
    pub title: String,
    /// 1-indexed page number.
    pub page: u32,
    /// PDF named destination for `#nameddest=` deep links, if available.
    pub named_dest: Option<String>,
}

/// SHA-256 hex digest of chunk text — the embedding-cache key component.
pub fn content_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// First ~200 chars of chunk text, cut on a char boundary, for previews.
pub fn make_snippet(text: &str) -> String {
    const MAX: usize = 200;
    let trimmed = text.trim();
    if trimmed.len() <= MAX {
        return trimmed.to_string();
    }
    let mut end = MAX;
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &trimmed[..end])
}

/// Crude token estimate (~4 chars per token). Good enough for the
/// chunk_max_tokens split threshold; we deliberately avoid a tokenizer dep.
pub fn approx_tokens(text: &str) -> usize {
    text.len() / 4
}

/// `file://` deep link into a paper's PDF (PRD §5 result shape).
pub fn deep_link(pdf_path: &Path, page: Option<u32>, named_dest: Option<&str>) -> String {
    let base = format!("file://{}", pdf_path.display());
    match (named_dest, page) {
        (Some(dest), _) => format!("{base}#nameddest={dest}"),
        (None, Some(p)) => format!("{base}#page={p}"),
        (None, None) => base,
    }
}

/// RFC 3339 timestamp for "now" (UTC) — single definition so all stores
/// format timestamps identically.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
