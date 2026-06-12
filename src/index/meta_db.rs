//! SQLite metadata store (`meta.db`) — chunks, pdf_toc, tags mirror,
//! embedding cache, and a small meta KV (schema version, config
//! fingerprints). See PRD §4 steps 5/8 and addendum §9.

use crate::{ChunkRecord, KbError, SectionType, TocEntry};
use std::path::Path;

/// Insert payload for one chunk. `vector_id` is assigned by SQLite
/// (autoincrement) and returned from [`MetaDb::insert_chunk`].
#[derive(Debug, Clone)]
pub struct NewChunk {
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

/// Corpus stats for `kb stats` (includes the Other-ratio health signal —
/// resolved decision: revisit classifier if it exceeds ~25%).
#[derive(Debug, Default, serde::Serialize)]
pub struct DbStats {
    pub papers: usize,
    pub chunks: usize,
    pub chunks_per_section: Vec<(String, usize)>,
    pub other_ratio: f32,
    pub cache_entries: usize,
}

pub struct MetaDb {
    conn: rusqlite::Connection,
}

impl MetaDb {
    /// Open (creating if missing), set WAL mode + foreign keys, apply the
    /// schema idempotently (`CREATE TABLE IF NOT EXISTS`), and check the
    /// stored schema version: newer than [`crate::SCHEMA_VERSION`] ⇒
    /// `Config` error naming both versions and the remedies.
    ///
    /// Schema:
    /// ```sql
    /// CREATE TABLE chunks (
    ///   id              INTEGER PRIMARY KEY AUTOINCREMENT, -- = turbovec external id
    ///   chunk_id        TEXT NOT NULL UNIQUE,
    ///   paper_id        TEXT NOT NULL,
    ///   section_type    TEXT NOT NULL,
    ///   ordinal         INTEGER NOT NULL,
    ///   content_hash    TEXT NOT NULL,
    ///   text            TEXT NOT NULL,
    ///   page            INTEGER,
    ///   snippet         TEXT NOT NULL,
    ///   embedded_at     TEXT NOT NULL,
    ///   embedding_model TEXT NOT NULL,
    ///   embedding_version INTEGER NOT NULL
    /// );
    /// CREATE INDEX idx_chunks_paper ON chunks(paper_id);
    /// CREATE INDEX idx_chunks_section ON chunks(section_type);
    ///
    /// CREATE TABLE pdf_toc (
    ///   paper_id   TEXT NOT NULL,
    ///   section    TEXT NOT NULL,
    ///   page       INTEGER NOT NULL,
    ///   named_dest TEXT,
    ///   PRIMARY KEY (paper_id, section)
    /// );
    ///
    /// CREATE TABLE paper_tags (
    ///   paper_id TEXT NOT NULL,
    ///   tag      TEXT NOT NULL,
    ///   PRIMARY KEY (paper_id, tag)
    /// );
    ///
    /// CREATE TABLE embedding_cache (
    ///   content_hash      TEXT NOT NULL,
    ///   embedding_model   TEXT NOT NULL,
    ///   embedding_version INTEGER NOT NULL,
    ///   vector_bytes      BLOB NOT NULL,   -- f32 little-endian
    ///   cached_at         TEXT NOT NULL,
    ///   PRIMARY KEY (content_hash, embedding_model, embedding_version)
    /// );
    ///
    /// CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    /// ```
    pub fn open(path: &Path) -> Result<Self, KbError> {
        let _ = path;
        todo!("implemented in the storage slice")
    }

    // ---- explicit transactions (the pipeline drives the addendum §5
    // write sequence across both stores, so these are plain methods) ----

    pub fn begin_immediate(&self) -> Result<(), KbError> {
        todo!("implemented in the storage slice")
    }
    pub fn commit(&self) -> Result<(), KbError> {
        todo!("implemented in the storage slice")
    }
    pub fn rollback(&self) -> Result<(), KbError> {
        todo!("implemented in the storage slice")
    }

    // ---- chunks ----

    /// Insert and return the autoincrement id (= turbovec external id).
    pub fn insert_chunk(&self, chunk: &NewChunk) -> Result<i64, KbError> {
        let _ = chunk;
        todo!("implemented in the storage slice")
    }

    pub fn chunk_by_chunk_id(&self, chunk_id: &str) -> Result<Option<ChunkRecord>, KbError> {
        let _ = chunk_id;
        todo!("implemented in the storage slice")
    }

    /// Fetch records for the given vector ids, returned in the SAME ORDER
    /// as the input (search ranking must survive the lookup). Ids with no
    /// row are skipped.
    pub fn chunks_by_vector_ids(&self, ids: &[i64]) -> Result<Vec<ChunkRecord>, KbError> {
        let _ = ids;
        todo!("implemented in the storage slice")
    }

    pub fn chunks_for_paper(&self, paper_id: &str) -> Result<Vec<ChunkRecord>, KbError> {
        let _ = paper_id;
        todo!("implemented in the storage slice")
    }

    /// Delete a paper's chunks, returning the removed vector ids (the
    /// caller removes them from the turbovec index too).
    pub fn delete_chunks_for_paper(&self, paper_id: &str) -> Result<Vec<i64>, KbError> {
        let _ = paper_id;
        todo!("implemented in the storage slice")
    }

    pub fn all_vector_ids(&self) -> Result<Vec<i64>, KbError> {
        todo!("implemented in the storage slice")
    }

    pub fn chunk_count(&self) -> Result<usize, KbError> {
        todo!("implemented in the storage slice")
    }

    /// Vector ids matching ALL provided filters (each filter is OR within
    /// itself, AND across filters) — the allowlist source for filtered
    /// search (PRD §5). `None` filters are ignored; all-None ⇒ every id.
    pub fn vector_ids_filtered(
        &self,
        section_types: Option<&[SectionType]>,
        paper_ids: Option<&[String]>,
        tags: Option<&[String]>,
    ) -> Result<Vec<i64>, KbError> {
        let _ = (section_types, paper_ids, tags);
        todo!("implemented in the storage slice")
    }

    // ---- pdf_toc ----

    /// Replace all TOC rows for a paper.
    pub fn replace_toc(&self, paper_id: &str, entries: &[TocEntry]) -> Result<(), KbError> {
        let _ = (paper_id, entries);
        todo!("implemented in the storage slice")
    }

    pub fn toc_for_paper(&self, paper_id: &str) -> Result<Vec<TocEntry>, KbError> {
        let _ = paper_id;
        todo!("implemented in the storage slice")
    }

    // ---- tags mirror (canonical copy lives in metadata.json) ----

    /// Replace the tag mirror for a paper.
    pub fn set_tags(&self, paper_id: &str, tags: &[String]) -> Result<(), KbError> {
        let _ = (paper_id, tags);
        todo!("implemented in the storage slice")
    }

    // ---- embedding cache (addendum §9) ----

    pub fn cache_get(
        &self,
        content_hash: &str,
        model: &str,
        version: u32,
    ) -> Result<Option<Vec<f32>>, KbError> {
        let _ = (content_hash, model, version);
        todo!("implemented in the storage slice")
    }

    pub fn cache_put(
        &self,
        content_hash: &str,
        model: &str,
        version: u32,
        vector: &[f32],
    ) -> Result<(), KbError> {
        let _ = (content_hash, model, version, vector);
        todo!("implemented in the storage slice")
    }

    /// Remove cache entries whose content_hash no longer appears in chunks.
    /// Returns the number removed (`kb cache gc`).
    pub fn cache_gc(&self) -> Result<usize, KbError> {
        todo!("implemented in the storage slice")
    }

    /// Drop all cache entries. Returns the number removed (`kb cache clear`).
    pub fn cache_clear(&self) -> Result<usize, KbError> {
        todo!("implemented in the storage slice")
    }

    // ---- paper-level cleanup ----

    /// Remove every trace of a paper (chunks + toc + tags). Returns the
    /// removed vector ids.
    pub fn remove_paper(&self, paper_id: &str) -> Result<Vec<i64>, KbError> {
        let _ = paper_id;
        todo!("implemented in the storage slice")
    }

    /// Drop and recreate the chunks/pdf_toc/paper_tags tables (NOT the
    /// embedding cache — surviving reindex is its whole point, addendum §8).
    pub fn reset_derived_tables(&self) -> Result<(), KbError> {
        todo!("implemented in the storage slice")
    }

    // ---- meta KV (config fingerprints, schema version) ----

    pub fn meta_get(&self, key: &str) -> Result<Option<String>, KbError> {
        let _ = key;
        todo!("implemented in the storage slice")
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<(), KbError> {
        let _ = (key, value);
        todo!("implemented in the storage slice")
    }

    // ---- stats ----

    pub fn stats(&self) -> Result<DbStats, KbError> {
        todo!("implemented in the storage slice")
    }
}
