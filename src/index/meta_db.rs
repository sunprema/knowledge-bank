//! SQLite metadata store (`meta.db`) — chunks, pdf_toc, tags mirror,
//! embedding cache, and a small meta KV (schema version, config
//! fingerprints). See PRD §4 steps 5/8 and addendum §9.

use crate::{ChunkRecord, KbError, SCHEMA_VERSION, SectionType, TocEntry};
use rusqlite::OptionalExtension;
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

#[derive(Debug)]
pub struct MetaDb {
    conn: rusqlite::Connection,
}

/// DDL for the derived tables — the ones `reset_derived_tables` drops and
/// recreates. `embedding_cache` and `meta` are deliberately NOT here
/// (surviving reindex is the cache's whole point, addendum §8).
const DERIVED_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS chunks (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  chunk_id        TEXT NOT NULL UNIQUE,
  paper_id        TEXT NOT NULL,
  section_type    TEXT NOT NULL,
  ordinal         INTEGER NOT NULL,
  content_hash    TEXT NOT NULL,
  text            TEXT NOT NULL,
  page            INTEGER,
  snippet         TEXT NOT NULL,
  embedded_at     TEXT NOT NULL,
  embedding_model TEXT NOT NULL,
  embedding_version INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_chunks_paper ON chunks(paper_id);
CREATE INDEX IF NOT EXISTS idx_chunks_section ON chunks(section_type);

CREATE TABLE IF NOT EXISTS pdf_toc (
  paper_id   TEXT NOT NULL,
  section    TEXT NOT NULL,
  page       INTEGER NOT NULL,
  named_dest TEXT,
  PRIMARY KEY (paper_id, section)
);

CREATE TABLE IF NOT EXISTS paper_tags (
  paper_id TEXT NOT NULL,
  tag      TEXT NOT NULL,
  PRIMARY KEY (paper_id, tag)
);
";

const PERSISTENT_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS embedding_cache (
  content_hash      TEXT NOT NULL,
  embedding_model   TEXT NOT NULL,
  embedding_version INTEGER NOT NULL,
  vector_bytes      BLOB NOT NULL,
  cached_at         TEXT NOT NULL,
  PRIMARY KEY (content_hash, embedding_model, embedding_version)
);

CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

const CHUNK_COLUMNS: &str = "id, chunk_id, paper_id, section_type, ordinal, content_hash, \
                             text, page, snippet, embedded_at, embedding_model, embedding_version";

fn row_to_chunk(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChunkRecord> {
    let section_raw: String = row.get(3)?;
    let section_type = SectionType::parse(&section_raw).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            format!("unknown section_type {section_raw:?}").into(),
        )
    })?;
    Ok(ChunkRecord {
        vector_id: row.get(0)?,
        chunk_id: row.get(1)?,
        paper_id: row.get(2)?,
        section_type,
        ordinal: row.get(4)?,
        content_hash: row.get(5)?,
        text: row.get(6)?,
        page: row.get(7)?,
        snippet: row.get(8)?,
        embedded_at: row.get(9)?,
        embedding_model: row.get(10)?,
        embedding_version: row.get(11)?,
    })
}

/// `?, ?, ?` placeholder list of length `n` (n >= 1).
fn placeholders(n: usize) -> String {
    let mut s = "?,".repeat(n);
    s.pop();
    s
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
        let conn = rusqlite::Connection::open(path)?;
        // journal_mode returns a row ("wal"); use query_row, not execute.
        conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        // Watcher + MCP server may write concurrently (addendum §10);
        // wait briefly instead of failing with SQLITE_BUSY.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(PERSISTENT_SCHEMA)?;
        conn.execute_batch(DERIVED_SCHEMA)?;

        let db = Self { conn };
        match db.meta_get("schema_version")? {
            Some(stored) => {
                let stored: u32 = stored.parse().map_err(|_| {
                    KbError::Index(format!("meta.db: malformed schema_version {stored:?}"))
                })?;
                if stored > SCHEMA_VERSION {
                    return Err(KbError::Config(format!(
                        "meta.db has schema_version {stored} but this binary supports \
                         {SCHEMA_VERSION}; upgrade kb or run `kb reindex`"
                    )));
                }
            }
            None => db.meta_set("schema_version", &SCHEMA_VERSION.to_string())?,
        }
        Ok(db)
    }

    // ---- explicit transactions (the pipeline drives the addendum §5
    // write sequence across both stores, so these are plain methods) ----

    pub fn begin_immediate(&self) -> Result<(), KbError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        Ok(())
    }
    pub fn commit(&self) -> Result<(), KbError> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }
    pub fn rollback(&self) -> Result<(), KbError> {
        self.conn.execute_batch("ROLLBACK")?;
        Ok(())
    }

    // ---- chunks ----

    /// Insert and return the autoincrement id (= turbovec external id).
    pub fn insert_chunk(&self, chunk: &NewChunk) -> Result<i64, KbError> {
        self.conn.execute(
            "INSERT INTO chunks (chunk_id, paper_id, section_type, ordinal, content_hash, \
             text, page, snippet, embedded_at, embedding_model, embedding_version) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                chunk.chunk_id,
                chunk.paper_id,
                chunk.section_type.as_str(),
                chunk.ordinal,
                chunk.content_hash,
                chunk.text,
                chunk.page,
                chunk.snippet,
                chunk.embedded_at,
                chunk.embedding_model,
                chunk.embedding_version,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn chunk_by_chunk_id(&self, chunk_id: &str) -> Result<Option<ChunkRecord>, KbError> {
        let rec = self
            .conn
            .query_row(
                &format!("SELECT {CHUNK_COLUMNS} FROM chunks WHERE chunk_id = ?1"),
                [chunk_id],
                row_to_chunk,
            )
            .optional()?;
        Ok(rec)
    }

    /// Fetch records for the given vector ids, returned in the SAME ORDER
    /// as the input (search ranking must survive the lookup). Ids with no
    /// row are skipped.
    pub fn chunks_by_vector_ids(&self, ids: &[i64]) -> Result<Vec<ChunkRecord>, KbError> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {CHUNK_COLUMNS} FROM chunks WHERE id = ?1"))?;
        let mut out = Vec::with_capacity(ids.len());
        for &id in ids {
            if let Some(rec) = stmt.query_row([id], row_to_chunk).optional()? {
                out.push(rec);
            }
        }
        Ok(out)
    }

    pub fn chunks_for_paper(&self, paper_id: &str) -> Result<Vec<ChunkRecord>, KbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CHUNK_COLUMNS} FROM chunks WHERE paper_id = ?1 ORDER BY id"
        ))?;
        let rows = stmt.query_map([paper_id], row_to_chunk)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Delete a paper's chunks, returning the removed vector ids (the
    /// caller removes them from the turbovec index too).
    pub fn delete_chunks_for_paper(&self, paper_id: &str) -> Result<Vec<i64>, KbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM chunks WHERE paper_id = ?1 ORDER BY id")?;
        let ids: Vec<i64> = stmt
            .query_map([paper_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        self.conn
            .execute("DELETE FROM chunks WHERE paper_id = ?1", [paper_id])?;
        Ok(ids)
    }

    pub fn all_vector_ids(&self) -> Result<Vec<i64>, KbError> {
        let mut stmt = self.conn.prepare("SELECT id FROM chunks ORDER BY id")?;
        let ids = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(ids)
    }

    pub fn chunk_count(&self) -> Result<usize, KbError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
        Ok(n as usize)
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
        // A provided-but-empty filter can match nothing (OR of zero terms).
        if section_types.is_some_and(|s| s.is_empty())
            || paper_ids.is_some_and(|p| p.is_empty())
            || tags.is_some_and(|t| t.is_empty())
        {
            return Ok(Vec::new());
        }

        let mut sql = String::from("SELECT DISTINCT c.id FROM chunks c");
        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(tags) = tags {
            sql.push_str(" JOIN paper_tags pt ON pt.paper_id = c.paper_id");
            clauses.push(format!("pt.tag IN ({})", placeholders(tags.len())));
            for t in tags {
                params.push(Box::new(t.clone()));
            }
        }
        if let Some(sections) = section_types {
            clauses.push(format!(
                "c.section_type IN ({})",
                placeholders(sections.len())
            ));
            for s in sections {
                params.push(Box::new(s.as_str()));
            }
        }
        if let Some(papers) = paper_ids {
            clauses.push(format!("c.paper_id IN ({})", placeholders(papers.len())));
            for p in papers {
                params.push(Box::new(p.clone()));
            }
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY c.id");

        let mut stmt = self.conn.prepare(&sql)?;
        let ids = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(ids)
    }

    // ---- pdf_toc ----

    /// Replace all TOC rows for a paper.
    pub fn replace_toc(&self, paper_id: &str, entries: &[TocEntry]) -> Result<(), KbError> {
        self.conn
            .execute("DELETE FROM pdf_toc WHERE paper_id = ?1", [paper_id])?;
        let mut stmt = self.conn.prepare(
            "INSERT OR REPLACE INTO pdf_toc (paper_id, section, page, named_dest) \
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for e in entries {
            stmt.execute(rusqlite::params![paper_id, e.title, e.page, e.named_dest])?;
        }
        Ok(())
    }

    pub fn toc_for_paper(&self, paper_id: &str) -> Result<Vec<TocEntry>, KbError> {
        let mut stmt = self.conn.prepare(
            "SELECT section, page, named_dest FROM pdf_toc \
             WHERE paper_id = ?1 ORDER BY page, section",
        )?;
        let rows = stmt.query_map([paper_id], |r| {
            Ok(TocEntry {
                title: r.get(0)?,
                page: r.get(1)?,
                named_dest: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---- tags mirror (canonical copy lives in metadata.json) ----

    /// Replace the tag mirror for a paper.
    pub fn set_tags(&self, paper_id: &str, tags: &[String]) -> Result<(), KbError> {
        self.conn
            .execute("DELETE FROM paper_tags WHERE paper_id = ?1", [paper_id])?;
        let mut stmt = self
            .conn
            .prepare("INSERT OR IGNORE INTO paper_tags (paper_id, tag) VALUES (?1, ?2)")?;
        for tag in tags {
            stmt.execute(rusqlite::params![paper_id, tag])?;
        }
        Ok(())
    }

    // ---- embedding cache (addendum §9) ----

    pub fn cache_get(
        &self,
        content_hash: &str,
        model: &str,
        version: u32,
    ) -> Result<Option<Vec<f32>>, KbError> {
        let bytes: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT vector_bytes FROM embedding_cache \
                 WHERE content_hash = ?1 AND embedding_model = ?2 AND embedding_version = ?3",
                rusqlite::params![content_hash, model, version],
                |r| r.get(0),
            )
            .optional()?;
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        if bytes.len() % 4 != 0 {
            return Err(KbError::Index(format!(
                "meta.db: cached vector for {content_hash} has {} bytes (not a multiple of 4)",
                bytes.len()
            )));
        }
        let vec = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        Ok(Some(vec))
    }

    pub fn cache_put(
        &self,
        content_hash: &str,
        model: &str,
        version: u32,
        vector: &[f32],
    ) -> Result<(), KbError> {
        let mut bytes = Vec::with_capacity(vector.len() * 4);
        for v in vector {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        self.conn.execute(
            "INSERT OR REPLACE INTO embedding_cache \
             (content_hash, embedding_model, embedding_version, vector_bytes, cached_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![content_hash, model, version, bytes, crate::now_rfc3339()],
        )?;
        Ok(())
    }

    /// Remove cache entries whose content_hash no longer appears in chunks.
    /// Returns the number removed (`kb cache gc`).
    pub fn cache_gc(&self) -> Result<usize, KbError> {
        let n = self.conn.execute(
            "DELETE FROM embedding_cache \
             WHERE content_hash NOT IN (SELECT content_hash FROM chunks)",
            [],
        )?;
        Ok(n)
    }

    /// Drop all cache entries. Returns the number removed (`kb cache clear`).
    pub fn cache_clear(&self) -> Result<usize, KbError> {
        let n = self.conn.execute("DELETE FROM embedding_cache", [])?;
        Ok(n)
    }

    // ---- paper-level cleanup ----

    /// Remove every trace of a paper (chunks + toc + tags). Returns the
    /// removed vector ids.
    pub fn remove_paper(&self, paper_id: &str) -> Result<Vec<i64>, KbError> {
        let ids = self.delete_chunks_for_paper(paper_id)?;
        self.conn
            .execute("DELETE FROM pdf_toc WHERE paper_id = ?1", [paper_id])?;
        self.conn
            .execute("DELETE FROM paper_tags WHERE paper_id = ?1", [paper_id])?;
        Ok(ids)
    }

    /// Drop and recreate the chunks/pdf_toc/paper_tags tables (NOT the
    /// embedding cache — surviving reindex is its whole point, addendum §8).
    pub fn reset_derived_tables(&self) -> Result<(), KbError> {
        self.conn.execute_batch(
            "DROP TABLE IF EXISTS chunks; \
             DROP TABLE IF EXISTS pdf_toc; \
             DROP TABLE IF EXISTS paper_tags;",
        )?;
        self.conn.execute_batch(DERIVED_SCHEMA)?;
        Ok(())
    }

    // ---- meta KV (config fingerprints, schema version) ----

    pub fn meta_get(&self, key: &str) -> Result<Option<String>, KbError> {
        let v = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(v)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<(), KbError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            [key, value],
        )?;
        Ok(())
    }

    // ---- stats ----

    pub fn stats(&self) -> Result<DbStats, KbError> {
        let papers: i64 =
            self.conn
                .query_row("SELECT COUNT(DISTINCT paper_id) FROM chunks", [], |r| {
                    r.get(0)
                })?;
        let chunks = self.chunk_count()?;
        let mut stmt = self.conn.prepare(
            "SELECT section_type, COUNT(*) FROM chunks \
             GROUP BY section_type ORDER BY COUNT(*) DESC, section_type",
        )?;
        let chunks_per_section: Vec<(String, usize)> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize)))?
            .collect::<rusqlite::Result<_>>()?;
        let other = chunks_per_section
            .iter()
            .find(|(s, _)| s == SectionType::Other.as_str())
            .map_or(0, |(_, n)| *n);
        let other_ratio = if chunks == 0 {
            0.0
        } else {
            other as f32 / chunks as f32
        };
        let cache_entries: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM embedding_cache", [], |r| r.get(0))?;
        Ok(DbStats {
            papers: papers as usize,
            chunks,
            chunks_per_section,
            other_ratio,
            cache_entries: cache_entries as usize,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_hash;

    fn open_temp() -> (tempfile::TempDir, MetaDb) {
        let dir = tempfile::tempdir().unwrap();
        let db = MetaDb::open(&dir.path().join("meta.db")).unwrap();
        (dir, db)
    }

    fn new_chunk(paper_id: &str, section: SectionType, ordinal: u32, text: &str) -> NewChunk {
        NewChunk {
            chunk_id: format!("{paper_id}_{}_{ordinal}", section.as_str()),
            paper_id: paper_id.to_string(),
            section_type: section,
            ordinal,
            content_hash: content_hash(text),
            text: text.to_string(),
            page: Some(ordinal + 1),
            snippet: crate::make_snippet(text),
            embedded_at: crate::now_rfc3339(),
            embedding_model: "text-embedding-3-small".to_string(),
            embedding_version: crate::EMBEDDING_VERSION,
        }
    }

    #[test]
    fn open_creates_schema_and_sets_version() {
        let (_dir, db) = open_temp();
        assert_eq!(
            db.meta_get("schema_version").unwrap().as_deref(),
            Some(SCHEMA_VERSION.to_string().as_str())
        );
        assert_eq!(db.chunk_count().unwrap(), 0);
    }

    #[test]
    fn open_sets_wal_and_foreign_keys() {
        let (_dir, db) = open_temp();
        let mode: String = db
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let fk: i64 = db
            .conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[test]
    fn open_is_idempotent_across_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.db");
        {
            let db = MetaDb::open(&path).unwrap();
            db.insert_chunk(&new_chunk("p1", SectionType::Abstract, 0, "hi"))
                .unwrap();
        }
        let db = MetaDb::open(&path).unwrap();
        assert_eq!(db.chunk_count().unwrap(), 1);
    }

    #[test]
    fn open_rejects_newer_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.db");
        {
            let db = MetaDb::open(&path).unwrap();
            db.meta_set("schema_version", &(SCHEMA_VERSION + 1).to_string())
                .unwrap();
        }
        let err = MetaDb::open(&path).unwrap_err();
        match err {
            KbError::Config(msg) => {
                assert!(msg.contains(&(SCHEMA_VERSION + 1).to_string()), "got: {msg}");
                assert!(msg.contains(&SCHEMA_VERSION.to_string()), "got: {msg}");
                assert!(msg.contains("upgrade kb") && msg.contains("kb reindex"), "got: {msg}");
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn insert_chunk_returns_autoincrement_ids() {
        let (_dir, db) = open_temp();
        let a = db
            .insert_chunk(&new_chunk("p1", SectionType::Abstract, 0, "a"))
            .unwrap();
        let b = db
            .insert_chunk(&new_chunk("p1", SectionType::Method, 0, "b"))
            .unwrap();
        assert!(b > a, "ids must be monotonically increasing");
    }

    #[test]
    fn chunk_roundtrip_all_fields() {
        let (_dir, db) = open_temp();
        let c = new_chunk("p1", SectionType::Experiments, 2, "some experiment text");
        let id = db.insert_chunk(&c).unwrap();
        let rec = db.chunk_by_chunk_id(&c.chunk_id).unwrap().unwrap();
        assert_eq!(rec.vector_id, id);
        assert_eq!(rec.chunk_id, c.chunk_id);
        assert_eq!(rec.paper_id, "p1");
        assert_eq!(rec.section_type, SectionType::Experiments);
        assert_eq!(rec.ordinal, 2);
        assert_eq!(rec.content_hash, c.content_hash);
        assert_eq!(rec.text, c.text);
        assert_eq!(rec.page, Some(3));
        assert_eq!(rec.snippet, c.snippet);
        assert_eq!(rec.embedded_at, c.embedded_at);
        assert_eq!(rec.embedding_model, c.embedding_model);
        assert_eq!(rec.embedding_version, c.embedding_version);
    }

    #[test]
    fn chunk_by_chunk_id_missing_is_none() {
        let (_dir, db) = open_temp();
        assert!(db.chunk_by_chunk_id("nope").unwrap().is_none());
    }

    #[test]
    fn chunks_by_vector_ids_preserves_input_order() {
        let (_dir, db) = open_temp();
        let mut ids = Vec::new();
        for i in 0..4 {
            ids.push(
                db.insert_chunk(&new_chunk("p1", SectionType::Other, i, &format!("t{i}")))
                    .unwrap(),
            );
        }
        // Reversed + shuffled order, with a missing id interleaved.
        let req = vec![ids[2], 9999, ids[0], ids[3], ids[1]];
        let recs = db.chunks_by_vector_ids(&req).unwrap();
        let got: Vec<i64> = recs.iter().map(|r| r.vector_id).collect();
        assert_eq!(got, vec![ids[2], ids[0], ids[3], ids[1]], "order must match input; missing skipped");
    }

    #[test]
    fn chunks_for_paper_and_delete() {
        let (_dir, db) = open_temp();
        let a = db.insert_chunk(&new_chunk("p1", SectionType::Abstract, 0, "a")).unwrap();
        let b = db.insert_chunk(&new_chunk("p1", SectionType::Method, 0, "b")).unwrap();
        db.insert_chunk(&new_chunk("p2", SectionType::Abstract, 0, "c")).unwrap();

        let recs = db.chunks_for_paper("p1").unwrap();
        assert_eq!(recs.len(), 2);

        let removed = db.delete_chunks_for_paper("p1").unwrap();
        assert_eq!(removed, vec![a, b]);
        assert!(db.chunks_for_paper("p1").unwrap().is_empty());
        assert_eq!(db.chunk_count().unwrap(), 1, "p2 untouched");
    }

    #[test]
    fn all_vector_ids_lists_everything() {
        let (_dir, db) = open_temp();
        let a = db.insert_chunk(&new_chunk("p1", SectionType::Abstract, 0, "a")).unwrap();
        let b = db.insert_chunk(&new_chunk("p2", SectionType::Abstract, 0, "b")).unwrap();
        assert_eq!(db.all_vector_ids().unwrap(), vec![a, b]);
    }

    #[test]
    fn vector_ids_filtered_combinations() {
        let (_dir, db) = open_temp();
        let a = db.insert_chunk(&new_chunk("p1", SectionType::Abstract, 0, "a")).unwrap();
        let b = db.insert_chunk(&new_chunk("p1", SectionType::Method, 0, "b")).unwrap();
        let c = db.insert_chunk(&new_chunk("p2", SectionType::Method, 0, "c")).unwrap();
        let d = db.insert_chunk(&new_chunk("p3", SectionType::Conclusion, 0, "d")).unwrap();
        db.set_tags("p1", &["transformers".into(), "nlp".into()]).unwrap();
        db.set_tags("p2", &["vision".into()]).unwrap();

        // All-None ⇒ every id.
        assert_eq!(db.vector_ids_filtered(None, None, None).unwrap(), vec![a, b, c, d]);

        // Single section filter.
        assert_eq!(
            db.vector_ids_filtered(Some(&[SectionType::Method]), None, None).unwrap(),
            vec![b, c]
        );

        // OR within a filter: two sections.
        assert_eq!(
            db.vector_ids_filtered(Some(&[SectionType::Abstract, SectionType::Conclusion]), None, None)
                .unwrap(),
            vec![a, d]
        );

        // Paper filter.
        assert_eq!(
            db.vector_ids_filtered(None, Some(&["p1".to_string()]), None).unwrap(),
            vec![a, b]
        );

        // Tags join through paper_tags; OR within tags.
        assert_eq!(
            db.vector_ids_filtered(None, None, Some(&["nlp".to_string(), "vision".to_string()]))
                .unwrap(),
            vec![a, b, c]
        );

        // AND across filters: method AND tagged-nlp ⇒ only p1's method chunk.
        assert_eq!(
            db.vector_ids_filtered(
                Some(&[SectionType::Method]),
                None,
                Some(&["nlp".to_string()])
            )
            .unwrap(),
            vec![b]
        );

        // All three filters together.
        assert_eq!(
            db.vector_ids_filtered(
                Some(&[SectionType::Method, SectionType::Abstract]),
                Some(&["p1".to_string(), "p2".to_string()]),
                Some(&["transformers".to_string()])
            )
            .unwrap(),
            vec![a, b]
        );

        // A provided-but-empty filter matches nothing.
        assert!(db.vector_ids_filtered(Some(&[]), None, None).unwrap().is_empty());
        assert!(db.vector_ids_filtered(None, None, Some(&[])).unwrap().is_empty());

        // No duplicate ids when a paper has multiple matching tags.
        let both = db
            .vector_ids_filtered(None, None, Some(&["nlp".to_string(), "transformers".to_string()]))
            .unwrap();
        assert_eq!(both, vec![a, b], "DISTINCT must dedupe the tag join");
    }

    #[test]
    fn toc_replace_and_fetch() {
        let (_dir, db) = open_temp();
        let entries = vec![
            TocEntry { title: "Introduction".into(), page: 1, named_dest: Some("sec.1".into()) },
            TocEntry { title: "Method".into(), page: 3, named_dest: None },
        ];
        db.replace_toc("p1", &entries).unwrap();
        assert_eq!(db.toc_for_paper("p1").unwrap(), entries);

        // Replace wipes previous rows.
        let entries2 = vec![TocEntry { title: "Only".into(), page: 2, named_dest: None }];
        db.replace_toc("p1", &entries2).unwrap();
        assert_eq!(db.toc_for_paper("p1").unwrap(), entries2);
        assert!(db.toc_for_paper("p2").unwrap().is_empty());
    }

    #[test]
    fn set_tags_replaces() {
        let (_dir, db) = open_temp();
        let id = db.insert_chunk(&new_chunk("p1", SectionType::Abstract, 0, "a")).unwrap();
        db.set_tags("p1", &["old".into()]).unwrap();
        db.set_tags("p1", &["new".into()]).unwrap();
        assert!(
            db.vector_ids_filtered(None, None, Some(&["old".to_string()])).unwrap().is_empty()
        );
        assert_eq!(
            db.vector_ids_filtered(None, None, Some(&["new".to_string()])).unwrap(),
            vec![id]
        );
    }

    #[test]
    fn cache_roundtrip_f32_le() {
        let (_dir, db) = open_temp();
        let vec = vec![0.0f32, 1.5, -2.25, f32::MIN_POSITIVE, 1e30, -0.0];
        db.cache_put("hash1", "model-a", 1, &vec).unwrap();
        let got = db.cache_get("hash1", "model-a", 1).unwrap().unwrap();
        assert_eq!(got.len(), vec.len());
        for (a, b) in got.iter().zip(&vec) {
            assert_eq!(a.to_bits(), b.to_bits(), "bit-exact roundtrip");
        }
        // Raw bytes really are f32 little-endian.
        let bytes: Vec<u8> = db
            .conn
            .query_row(
                "SELECT vector_bytes FROM embedding_cache WHERE content_hash='hash1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bytes.len(), vec.len() * 4);
        assert_eq!(&bytes[4..8], &1.5f32.to_le_bytes());
    }

    #[test]
    fn cache_key_includes_model_and_version() {
        let (_dir, db) = open_temp();
        db.cache_put("h", "model-a", 1, &[1.0]).unwrap();
        assert!(db.cache_get("h", "model-b", 1).unwrap().is_none());
        assert!(db.cache_get("h", "model-a", 2).unwrap().is_none());
        assert!(db.cache_get("miss", "model-a", 1).unwrap().is_none());
        assert!(db.cache_get("h", "model-a", 1).unwrap().is_some());
        // Same key overwrites, no constraint violation.
        db.cache_put("h", "model-a", 1, &[2.0]).unwrap();
        assert_eq!(db.cache_get("h", "model-a", 1).unwrap().unwrap(), vec![2.0]);
    }

    #[test]
    fn cache_gc_removes_only_orphans() {
        let (_dir, db) = open_temp();
        let live = new_chunk("p1", SectionType::Abstract, 0, "live text");
        db.insert_chunk(&live).unwrap();
        db.cache_put(&live.content_hash, "m", 1, &[1.0]).unwrap();
        db.cache_put("orphan-hash-1", "m", 1, &[2.0]).unwrap();
        db.cache_put("orphan-hash-2", "m", 1, &[3.0]).unwrap();

        assert_eq!(db.cache_gc().unwrap(), 2);
        assert!(db.cache_get(&live.content_hash, "m", 1).unwrap().is_some());
        assert!(db.cache_get("orphan-hash-1", "m", 1).unwrap().is_none());
        assert_eq!(db.cache_gc().unwrap(), 0, "second gc is a no-op");
    }

    #[test]
    fn cache_clear_counts() {
        let (_dir, db) = open_temp();
        db.cache_put("a", "m", 1, &[1.0]).unwrap();
        db.cache_put("b", "m", 1, &[2.0]).unwrap();
        assert_eq!(db.cache_clear().unwrap(), 2);
        assert_eq!(db.cache_clear().unwrap(), 0);
    }

    #[test]
    fn remove_paper_wipes_all_traces() {
        let (_dir, db) = open_temp();
        let a = db.insert_chunk(&new_chunk("p1", SectionType::Abstract, 0, "a")).unwrap();
        let keep = db.insert_chunk(&new_chunk("p2", SectionType::Abstract, 0, "k")).unwrap();
        db.replace_toc("p1", &[TocEntry { title: "Intro".into(), page: 1, named_dest: None }])
            .unwrap();
        db.set_tags("p1", &["t".into()]).unwrap();

        let removed = db.remove_paper("p1").unwrap();
        assert_eq!(removed, vec![a]);
        assert!(db.chunks_for_paper("p1").unwrap().is_empty());
        assert!(db.toc_for_paper("p1").unwrap().is_empty());
        assert!(db.vector_ids_filtered(None, None, Some(&["t".to_string()])).unwrap().is_empty());
        assert_eq!(db.all_vector_ids().unwrap(), vec![keep]);
    }

    #[test]
    fn reset_derived_tables_preserves_cache_and_meta() {
        let (_dir, db) = open_temp();
        db.insert_chunk(&new_chunk("p1", SectionType::Abstract, 0, "a")).unwrap();
        db.replace_toc("p1", &[TocEntry { title: "T".into(), page: 1, named_dest: None }])
            .unwrap();
        db.set_tags("p1", &["tag".into()]).unwrap();
        db.cache_put("h", "m", 1, &[1.0, 2.0]).unwrap();
        db.meta_set("config_fingerprint", "abc123").unwrap();

        db.reset_derived_tables().unwrap();

        // Derived tables empty but usable.
        assert_eq!(db.chunk_count().unwrap(), 0);
        assert!(db.toc_for_paper("p1").unwrap().is_empty());
        assert!(db.vector_ids_filtered(None, None, Some(&["tag".to_string()])).unwrap().is_empty());
        let id = db.insert_chunk(&new_chunk("p1", SectionType::Abstract, 0, "a")).unwrap();
        assert_eq!(id, 1, "autoincrement restarts after drop — fresh reindex ids");

        // The whole point: cache and meta survive.
        assert_eq!(db.cache_get("h", "m", 1).unwrap().unwrap(), vec![1.0, 2.0]);
        assert_eq!(db.meta_get("config_fingerprint").unwrap().as_deref(), Some("abc123"));
        assert_eq!(
            db.meta_get("schema_version").unwrap().as_deref(),
            Some(SCHEMA_VERSION.to_string().as_str())
        );
    }

    #[test]
    fn explicit_transactions_commit_and_rollback() {
        let (_dir, db) = open_temp();
        db.begin_immediate().unwrap();
        db.insert_chunk(&new_chunk("p1", SectionType::Abstract, 0, "a")).unwrap();
        db.rollback().unwrap();
        assert_eq!(db.chunk_count().unwrap(), 0, "rollback discards the insert");

        db.begin_immediate().unwrap();
        db.insert_chunk(&new_chunk("p1", SectionType::Abstract, 0, "a")).unwrap();
        db.commit().unwrap();
        assert_eq!(db.chunk_count().unwrap(), 1, "commit persists the insert");
    }

    #[test]
    fn meta_kv_roundtrip() {
        let (_dir, db) = open_temp();
        assert!(db.meta_get("k").unwrap().is_none());
        db.meta_set("k", "v1").unwrap();
        assert_eq!(db.meta_get("k").unwrap().as_deref(), Some("v1"));
        db.meta_set("k", "v2").unwrap();
        assert_eq!(db.meta_get("k").unwrap().as_deref(), Some("v2"));
    }

    #[test]
    fn stats_counts_and_other_ratio() {
        let (_dir, db) = open_temp();
        let empty = db.stats().unwrap();
        assert_eq!(empty.papers, 0);
        assert_eq!(empty.chunks, 0);
        assert_eq!(empty.other_ratio, 0.0, "0.0 when empty, not NaN");

        db.insert_chunk(&new_chunk("p1", SectionType::Abstract, 0, "a")).unwrap();
        db.insert_chunk(&new_chunk("p1", SectionType::Other, 0, "b")).unwrap();
        db.insert_chunk(&new_chunk("p2", SectionType::Other, 0, "c")).unwrap();
        db.insert_chunk(&new_chunk("p2", SectionType::Method, 0, "d")).unwrap();
        db.cache_put("h", "m", 1, &[1.0]).unwrap();

        let s = db.stats().unwrap();
        assert_eq!(s.papers, 2);
        assert_eq!(s.chunks, 4);
        assert_eq!(s.cache_entries, 1);
        assert!((s.other_ratio - 0.5).abs() < f32::EPSILON);
        let other = s
            .chunks_per_section
            .iter()
            .find(|(name, _)| name == "other")
            .unwrap();
        assert_eq!(other.1, 2);
        let total: usize = s.chunks_per_section.iter().map(|(_, n)| n).sum();
        assert_eq!(total, 4);
    }

    #[test]
    fn duplicate_chunk_id_is_an_index_error() {
        let (_dir, db) = open_temp();
        let c = new_chunk("p1", SectionType::Abstract, 0, "a");
        db.insert_chunk(&c).unwrap();
        let err = db.insert_chunk(&c).unwrap_err();
        assert!(matches!(err, KbError::Index(_)), "UNIQUE violation maps to Index");
    }
}
