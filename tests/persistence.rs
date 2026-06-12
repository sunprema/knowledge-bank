//! Persistence invariants from the addendum §12 that are testable without
//! network or the ingest pipeline: round-trip, two-store consistency, and
//! atomic-save hygiene. Ingest-dependent invariants (reindex, crash-mid-add)
//! live in the e2e smoke test.

use kb::index::{consistency_check, MetaDb, NewChunk, VectorIndex};
use kb::{now_rfc3339, SectionType};
use std::path::Path;

const DIM: usize = 8;
const BITS: usize = 4;

fn test_vector(seed: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; DIM];
    v[seed % DIM] = 1.0;
    v[(seed + 3) % DIM] = 0.05 * (seed as f32 + 1.0);
    v
}

fn insert_paper(
    db: &MetaDb,
    index: &mut VectorIndex,
    paper_id: &str,
    n_chunks: usize,
) -> Vec<i64> {
    let mut ids = Vec::new();
    db.begin_immediate().unwrap();
    for ordinal in 0..n_chunks {
        let text = format!("chunk {ordinal} of {paper_id}");
        let vid = db
            .insert_chunk(&NewChunk {
                chunk_id: format!("{paper_id}_other_{ordinal}"),
                paper_id: paper_id.to_string(),
                section_type: SectionType::Other,
                ordinal: ordinal as u32,
                content_hash: kb::content_hash(&text),
                text: text.clone(),
                page: Some(ordinal as u32 + 1),
                snippet: kb::make_snippet(&text),
                embedded_at: now_rfc3339(),
                embedding_model: "test-model".to_string(),
                embedding_version: 1,
            })
            .unwrap();
        index
            .add(&[vid as u64], &test_vector(vid as usize))
            .unwrap();
        ids.push(vid);
    }
    db.commit().unwrap();
    ids
}

fn open_stores(dir: &Path) -> (MetaDb, VectorIndex) {
    let db = MetaDb::open(&dir.join("meta.db")).unwrap();
    let index = VectorIndex::open_or_create(&dir.join("index.tv"), DIM, BITS).unwrap();
    (db, index)
}

/// Addendum §12 "Round-trip": add, save, reopen, search — same results.
#[test]
fn round_trip_same_results_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let index_path = dir.path().join("index.tv");

    let (db, mut index) = open_stores(dir.path());
    let ids = insert_paper(&db, &mut index, "2504.19874", 5);
    index.save_atomic(&index_path).unwrap();

    let query = test_vector(ids[2] as usize);
    let before = index.search(&query, 3, None).unwrap();
    drop(index);
    drop(db);

    let (db, index) = open_stores(dir.path());
    let after = index.search(&query, 3, None).unwrap();
    assert_eq!(before, after, "search results must survive a restart");

    let report = consistency_check(&db, &index, true).unwrap();
    assert!(report.ok, "stores must be consistent after reopen: {report:?}");
    assert_eq!(report.db_chunks, 5);
    assert_eq!(report.index_vectors, 5);
}

/// Addendum §12 "Two-store consistency": M chunks in meta.db ↔ M vectors,
/// and the check actually catches divergence.
#[test]
fn consistency_check_catches_divergence() {
    let dir = tempfile::tempdir().unwrap();
    let (db, mut index) = open_stores(dir.path());
    let ids = insert_paper(&db, &mut index, "2504.19874", 4);

    assert!(consistency_check(&db, &index, true).unwrap().ok);

    // Simulate divergence: a vector vanishes from the index only.
    index.remove(ids[0] as u64);
    let report = consistency_check(&db, &index, true).unwrap();
    assert!(!report.ok);
    assert_eq!(report.missing_in_index, vec![ids[0]]);
}

/// A leftover index.tv.tmp from a crash mid-write must be harmless
/// (addendum §10: "incomplete write, safe to ignore").
#[test]
fn leftover_tmp_file_does_not_break_load() {
    let dir = tempfile::tempdir().unwrap();
    let index_path = dir.path().join("index.tv");

    let (db, mut index) = open_stores(dir.path());
    insert_paper(&db, &mut index, "2504.19874", 3);
    index.save_atomic(&index_path).unwrap();

    // Crash artifact: truncated tmp next to the good file.
    std::fs::write(dir.path().join("index.tv.tmp"), b"garbage from a crash").unwrap();

    let reopened = VectorIndex::open_or_create(&index_path, DIM, BITS).unwrap();
    assert_eq!(reopened.len(), 3);
}

/// Paper removal updates both stores and survives a reopen
/// (addendum §5 remove sequence at the store level).
#[test]
fn remove_paper_updates_both_stores() {
    let dir = tempfile::tempdir().unwrap();
    let index_path = dir.path().join("index.tv");

    let (db, mut index) = open_stores(dir.path());
    insert_paper(&db, &mut index, "2504.19874", 3);
    insert_paper(&db, &mut index, "2405.12497", 2);

    db.begin_immediate().unwrap();
    let removed = db.remove_paper("2504.19874").unwrap();
    for vid in &removed {
        assert!(index.remove(*vid as u64));
    }
    index.save_atomic(&index_path).unwrap();
    db.commit().unwrap();

    assert_eq!(removed.len(), 3);
    drop(index);
    drop(db);

    let (db, index) = open_stores(dir.path());
    assert_eq!(index.len(), 2);
    let report = consistency_check(&db, &index, true).unwrap();
    assert!(report.ok);
    assert!(db.chunks_for_paper("2504.19874").unwrap().is_empty());
    assert_eq!(db.chunks_for_paper("2405.12497").unwrap().len(), 2);
}

/// Filtered search honors the allowlist end-to-end (PRD §5 filtered mode):
/// section/paper filters narrow results, and a filter matching nothing
/// yields empty (not a panic — the turbovec trap).
#[test]
fn filtered_search_allowlist_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let (db, mut index) = open_stores(dir.path());
    insert_paper(&db, &mut index, "2504.19874", 3);
    insert_paper(&db, &mut index, "2405.12497", 3);

    let allow: Vec<u64> = db
        .vector_ids_filtered(None, Some(&["2504.19874".to_string()]), None)
        .unwrap()
        .into_iter()
        .map(|i| i as u64)
        .collect();
    assert_eq!(allow.len(), 3);

    let query = test_vector(allow[0] as usize);
    let hits = index.search(&query, 10, Some(&allow)).unwrap();
    assert_eq!(hits.len(), 3, "allowlist caps the result set");
    let returned: std::collections::HashSet<u64> = hits.iter().map(|(id, _)| *id).collect();
    assert!(returned.iter().all(|id| allow.contains(id)));

    // Stale allowlist ids (already removed from the index) must not panic.
    for id in &allow {
        index.remove(*id);
    }
    let hits = index.search(&query, 10, Some(&allow)).unwrap();
    assert!(hits.is_empty());
}
