//! The two derived stores (persistence addendum §2): the turbovec vector
//! index (`index.tv`) and SQLite metadata (`meta.db`). Joined by
//! `chunks.id` = turbovec external id.

pub mod meta_db;
pub mod turbovec_index;

pub use meta_db::{CortexEdgeRow, MetaDb, NewChunk, NewCortexEdge};
pub use turbovec_index::VectorIndex;

use crate::KbError;

/// Result of the addendum §7 consistency check.
#[derive(Debug, serde::Serialize)]
pub struct ConsistencyReport {
    pub db_chunks: usize,
    pub index_vectors: usize,
    pub checked: usize,
    pub missing_in_index: Vec<i64>,
    pub ok: bool,
}

/// Two-store consistency check (addendum §7). `deep` checks every chunk id;
/// otherwise a sample of up to 10. Query-mode callers refuse to serve when
/// `!ok`; ingest-mode callers proceed (the next ingest writes fresh state).
pub fn consistency_check(
    db: &MetaDb,
    index: &VectorIndex,
    deep: bool,
) -> Result<ConsistencyReport, KbError> {
    let ids = db.all_vector_ids()?;
    let db_chunks = ids.len();
    let index_vectors = index.len();

    let to_check: Vec<i64> = if deep {
        ids
    } else {
        // Spot-check a spread of 10 (cheap, catches most corruption).
        let step = (ids.len() / 10).max(1);
        ids.iter().step_by(step).take(10).copied().collect()
    };

    let mut missing = Vec::new();
    for id in &to_check {
        if !index.contains(*id as u64) {
            missing.push(*id);
        }
    }

    Ok(ConsistencyReport {
        db_chunks,
        index_vectors,
        checked: to_check.len(),
        ok: db_chunks == index_vectors && missing.is_empty(),
        missing_in_index: missing,
    })
}
