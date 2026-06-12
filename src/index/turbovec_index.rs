//! Wrapper around `turbovec::IdMapIndex` (addendum §3) with the atomic
//! save discipline (addendum §5).

use crate::KbError;
use std::path::Path;
use turbovec::IdMapIndex;

pub struct VectorIndex {
    inner: IdMapIndex,
}

impl std::fmt::Debug for VectorIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // IdMapIndex has no Debug impl; summarize the shape instead.
        f.debug_struct("VectorIndex")
            .field("len", &self.inner.len())
            .field("dim", &self.inner.dim())
            .field("bit_width", &self.inner.bit_width())
            .finish()
    }
}

impl VectorIndex {
    /// Load from `path` if it exists, else create empty with the given
    /// shape. A load failure (corrupted file) ⇒ `Index` error — callers
    /// implement the startup policy (addendum §7), not this type.
    /// After load, verify dim/bit_width match the config; mismatch ⇒
    /// `Index` error naming both ("run `kb reindex`").
    pub fn open_or_create(path: &Path, dim: usize, bit_width: usize) -> Result<Self, KbError> {
        if !path.exists() {
            return Self::create(dim, bit_width);
        }
        let inner = IdMapIndex::load(path).map_err(|e| {
            KbError::Index(format!(
                "cannot load vector index {}: {e}; run `kb reindex` to rebuild",
                path.display()
            ))
        })?;
        // A non-lazy index always carries its dim, even when empty.
        if inner.dim() != dim {
            return Err(KbError::Index(format!(
                "vector index {} has dim {} but config wants dim {}; run `kb reindex`",
                path.display(),
                inner.dim(),
                dim
            )));
        }
        if inner.bit_width() != bit_width {
            return Err(KbError::Index(format!(
                "vector index {} has bit_width {} but config wants bit_width {}; run `kb reindex`",
                path.display(),
                inner.bit_width(),
                bit_width
            )));
        }
        Ok(Self { inner })
    }

    /// Create an empty in-memory index (used by `kb reindex`).
    pub fn create(dim: usize, bit_width: usize) -> Result<Self, KbError> {
        let inner = IdMapIndex::new(dim, bit_width)
            .map_err(|e| KbError::Index(format!("cannot create vector index: {e}")))?;
        Ok(Self { inner })
    }

    /// Add vectors (flattened row-major, `ids.len() * dim` floats).
    /// In-memory only — call [`Self::save_atomic`] afterwards.
    pub fn add(&mut self, ids: &[u64], vectors_flat: &[f32]) -> Result<(), KbError> {
        self.inner
            .add_with_ids(vectors_flat, ids)
            .map_err(|e| KbError::Index(format!("vector index add failed: {e}")))
    }

    /// Remove by external id; false if absent. In-memory only.
    pub fn remove(&mut self, id: u64) -> bool {
        self.inner.remove(id)
    }

    pub fn contains(&self, id: u64) -> bool {
        self.inner.contains(id)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Single-query search returning ranked `(external_id, score)`.
    ///
    /// Allowlist safety (IdMapIndex::search_with_allowlist PANICS on an
    /// empty allowlist or on ids not present in the index — this wrapper
    /// is the guard): filter the allowlist through `contains` first; if
    /// nothing survives, return `Ok(vec![])` without searching. Also
    /// truncate the result to entries actually returned (k may exceed
    /// index size).
    pub fn search(
        &self,
        query: &[f32],
        k: usize,
        allowlist: Option<&[u64]>,
    ) -> Result<Vec<(u64, f32)>, KbError> {
        if query.len() != self.inner.dim() {
            return Err(KbError::Index(format!(
                "query has {} dims but index has {}",
                query.len(),
                self.inner.dim()
            )));
        }
        // The inner kernel panics on non-finite / huge-magnitude query
        // coordinates; surface that as a typed error instead.
        if let Some(v) = query.iter().find(|v| !v.is_finite() || v.abs() >= 1e16) {
            return Err(KbError::Index(format!(
                "query contains an invalid coordinate ({v}); refusing to search"
            )));
        }
        if k == 0 || self.inner.is_empty() {
            return Ok(Vec::new());
        }

        // Pre-filter the allowlist: search_with_allowlist panics on an
        // empty list or on ids the index doesn't contain.
        let filtered: Option<Vec<u64>> = allowlist.map(|ids| {
            ids.iter()
                .copied()
                .filter(|&id| self.inner.contains(id))
                .collect()
        });
        if let Some(f) = &filtered
            && f.is_empty()
        {
            return Ok(Vec::new());
        }

        let (scores, ids) = self
            .inner
            .search_with_allowlist(query, k, filtered.as_deref());
        // The inner index already truncates to min(k, len, n_allowed);
        // zip handles any residual shape disagreement defensively.
        Ok(ids.into_iter().zip(scores).collect())
    }

    /// Persist with the non-negotiable atomic-rename pattern (addendum §5):
    /// write to `{path}.tmp` (same directory), fsync the file, rename onto
    /// `path`. A crash mid-write must never leave a truncated `index.tv`.
    pub fn save_atomic(&self, path: &Path) -> Result<(), KbError> {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);

        self.inner.write(&tmp).map_err(|e| {
            KbError::Index(format!("write vector index {}: {e}", tmp.display()))
        })?;
        // fsync the temp file so the rename publishes durable bytes.
        let f = std::fs::File::open(&tmp)
            .map_err(|e| KbError::Index(format!("open {} for fsync: {e}", tmp.display())))?;
        f.sync_all()
            .map_err(|e| KbError::Index(format!("fsync {}: {e}", tmp.display())))?;
        drop(f);
        std::fs::rename(&tmp, path).map_err(|e| {
            KbError::Index(format!(
                "rename {} -> {}: {e}",
                tmp.display(),
                path.display()
            ))
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIM: usize = 8;
    const BITS: usize = 4;

    /// n distinct, well-separated unit vectors (one-hot on coordinate i%DIM,
    /// plus a small per-vector perturbation so each is unique).
    fn vectors(n: usize) -> Vec<f32> {
        let mut flat = vec![0.0f32; n * DIM];
        for i in 0..n {
            flat[i * DIM + (i % DIM)] = 1.0;
            flat[i * DIM + ((i + 3) % DIM)] = 0.05 * (i as f32 + 1.0);
        }
        flat
    }

    fn query_for(i: usize, n: usize) -> Vec<f32> {
        let flat = vectors(n);
        flat[i * DIM..(i + 1) * DIM].to_vec()
    }

    fn filled(n: usize) -> VectorIndex {
        let mut idx = VectorIndex::create(DIM, BITS).unwrap();
        let ids: Vec<u64> = (1..=n as u64).collect();
        idx.add(&ids, &vectors(n)).unwrap();
        idx
    }

    #[test]
    fn create_and_add_and_len() {
        let idx = filled(4);
        assert_eq!(idx.len(), 4);
        assert!(!idx.is_empty());
        assert!(idx.contains(1));
        assert!(idx.contains(4));
        assert!(!idx.contains(5));
    }

    #[test]
    fn search_returns_nearest_first() {
        let idx = filled(4);
        let q = query_for(2, 4); // vector with external id 3
        let res = idx.search(&q, 4, None).unwrap();
        assert_eq!(res.len(), 4);
        assert_eq!(res[0].0, 3, "nearest should be the vector itself");
        // Ranked: scores non-increasing.
        for w in res.windows(2) {
            assert!(w[0].1 >= w[1].1, "scores must be descending");
        }
    }

    #[test]
    fn search_k_greater_than_len_truncates() {
        let idx = filled(3);
        let res = idx.search(&query_for(0, 3), 10, None).unwrap();
        // turbovec truncates effective_k to min(k, len) = 3.
        assert_eq!(res.len(), 3);
    }

    #[test]
    fn search_empty_index_returns_empty() {
        let idx = VectorIndex::create(DIM, BITS).unwrap();
        let res = idx.search(&[0.0; DIM], 5, None).unwrap();
        assert!(res.is_empty());
    }

    #[test]
    fn search_k_zero_returns_empty() {
        let idx = filled(2);
        assert!(idx.search(&query_for(0, 2), 0, None).unwrap().is_empty());
    }

    #[test]
    fn search_wrong_query_dim_is_error_not_panic() {
        let idx = filled(2);
        let err = idx.search(&[0.0; DIM + 1], 1, None).unwrap_err();
        assert!(matches!(err, KbError::Index(_)));
    }

    #[test]
    fn search_nan_query_is_error_not_panic() {
        let idx = filled(2);
        let mut q = query_for(0, 2);
        q[0] = f32::NAN;
        assert!(matches!(idx.search(&q, 1, None), Err(KbError::Index(_))));
    }

    #[test]
    fn allowlist_restricts_results() {
        let idx = filled(4);
        let q = query_for(0, 4); // nearest overall is id 1
        let res = idx.search(&q, 4, Some(&[2, 4])).unwrap();
        assert_eq!(res.len(), 2);
        let ids: Vec<u64> = res.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&2) && ids.contains(&4));
        assert!(!ids.contains(&1) && !ids.contains(&3));
    }

    #[test]
    fn allowlist_with_absent_ids_does_not_panic() {
        let idx = filled(3);
        // 99 and 100 are not in the index — raw search_with_allowlist would panic.
        let res = idx.search(&query_for(1, 3), 5, Some(&[2, 99, 100])).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].0, 2);
    }

    #[test]
    fn allowlist_nothing_survives_returns_empty() {
        let idx = filled(3);
        // Empty allowlist.
        assert!(idx.search(&query_for(0, 3), 5, Some(&[])).unwrap().is_empty());
        // Allowlist of only-absent ids.
        assert!(
            idx.search(&query_for(0, 3), 5, Some(&[77, 88]))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn remove_by_external_id() {
        let mut idx = filled(3);
        assert!(idx.remove(2));
        assert!(!idx.remove(2), "second remove returns false");
        assert!(!idx.remove(99));
        assert_eq!(idx.len(), 2);
        assert!(!idx.contains(2));
        // External ids 1 and 3 survive the swap-remove.
        assert!(idx.contains(1) && idx.contains(3));
        let res = idx.search(&query_for(2, 3), 3, None).unwrap();
        assert!(res.iter().all(|(id, _)| *id != 2));
    }

    #[test]
    fn save_atomic_roundtrip_same_results() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.tv");
        let idx = filled(5);
        idx.save_atomic(&path).unwrap();
        assert!(path.exists());
        assert!(!dir.path().join("index.tv.tmp").exists(), "tmp must be renamed away");

        let loaded = VectorIndex::open_or_create(&path, DIM, BITS).unwrap();
        assert_eq!(loaded.len(), 5);
        let q = query_for(3, 5);
        let before = idx.search(&q, 5, None).unwrap();
        let after = loaded.search(&q, 5, None).unwrap();
        assert_eq!(before, after, "search results must survive the roundtrip");
    }

    #[test]
    fn save_atomic_overwrites_previous_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.tv");
        let mut idx = filled(2);
        idx.save_atomic(&path).unwrap();
        idx.add(&[10], &query_for(0, 1)).unwrap();
        idx.save_atomic(&path).unwrap();
        let loaded = VectorIndex::open_or_create(&path, DIM, BITS).unwrap();
        assert_eq!(loaded.len(), 3);
        assert!(loaded.contains(10));
    }

    #[test]
    fn leftover_tmp_never_breaks_subsequent_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.tv");
        let idx = filled(2);
        idx.save_atomic(&path).unwrap();
        // Simulate a crash mid-write on a LATER save: garbage tmp left behind.
        std::fs::write(dir.path().join("index.tv.tmp"), b"truncated garbage").unwrap();
        let loaded = VectorIndex::open_or_create(&path, DIM, BITS).unwrap();
        assert_eq!(loaded.len(), 2, "stray .tmp must not affect the real file");
        // And a fresh save_atomic happily clobbers the stray tmp.
        idx.save_atomic(&path).unwrap();
        let reloaded = VectorIndex::open_or_create(&path, DIM, BITS).unwrap();
        assert_eq!(reloaded.len(), 2);
    }

    #[test]
    fn open_or_create_missing_file_creates_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.tv");
        let idx = VectorIndex::open_or_create(&path, DIM, BITS).unwrap();
        assert!(idx.is_empty());
        assert!(!path.exists(), "create must not write anything");
    }

    #[test]
    fn open_or_create_corrupted_file_mentions_reindex() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.tv");
        std::fs::write(&path, b"not a turbovec file").unwrap();
        let err = VectorIndex::open_or_create(&path, DIM, BITS).unwrap_err();
        match err {
            KbError::Index(msg) => assert!(msg.contains("kb reindex"), "got: {msg}"),
            other => panic!("expected Index error, got {other:?}"),
        }
    }

    #[test]
    fn open_or_create_dim_mismatch_names_both() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.tv");
        filled(2).save_atomic(&path).unwrap();
        let err = VectorIndex::open_or_create(&path, 16, BITS).unwrap_err();
        match err {
            KbError::Index(msg) => {
                assert!(msg.contains('8') && msg.contains("16"), "got: {msg}");
                assert!(msg.contains("kb reindex"), "got: {msg}");
            }
            other => panic!("expected Index error, got {other:?}"),
        }
    }

    #[test]
    fn open_or_create_bit_width_mismatch_names_both() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.tv");
        filled(2).save_atomic(&path).unwrap();
        let err = VectorIndex::open_or_create(&path, DIM, 2).unwrap_err();
        match err {
            KbError::Index(msg) => {
                assert!(msg.contains('4') && msg.contains('2'), "got: {msg}");
            }
            other => panic!("expected Index error, got {other:?}"),
        }
    }

    #[test]
    fn empty_index_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.tv");
        VectorIndex::create(DIM, BITS).unwrap().save_atomic(&path).unwrap();
        let loaded = VectorIndex::open_or_create(&path, DIM, BITS).unwrap();
        assert!(loaded.is_empty());
        assert!(loaded.search(&[0.0; DIM], 3, None).unwrap().is_empty());
    }
}
