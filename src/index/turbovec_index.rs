//! Wrapper around `turbovec::IdMapIndex` (addendum §3) with the atomic
//! save discipline (addendum §5).

use crate::KbError;
use std::path::Path;
use turbovec::IdMapIndex;

pub struct VectorIndex {
    inner: IdMapIndex,
}

impl VectorIndex {
    /// Load from `path` if it exists, else create empty with the given
    /// shape. A load failure (corrupted file) ⇒ `Index` error — callers
    /// implement the startup policy (addendum §7), not this type.
    /// After load, verify dim/bit_width match the config; mismatch ⇒
    /// `Index` error naming both ("run `kb reindex`").
    pub fn open_or_create(path: &Path, dim: usize, bit_width: usize) -> Result<Self, KbError> {
        let _ = (path, dim, bit_width);
        todo!("implemented in the storage slice")
    }

    /// Create an empty in-memory index (used by `kb reindex`).
    pub fn create(dim: usize, bit_width: usize) -> Result<Self, KbError> {
        let _ = (dim, bit_width);
        todo!("implemented in the storage slice")
    }

    /// Add vectors (flattened row-major, `ids.len() * dim` floats).
    /// In-memory only — call [`Self::save_atomic`] afterwards.
    pub fn add(&mut self, ids: &[u64], vectors_flat: &[f32]) -> Result<(), KbError> {
        let _ = (ids, vectors_flat);
        todo!("implemented in the storage slice")
    }

    /// Remove by external id; false if absent. In-memory only.
    pub fn remove(&mut self, id: u64) -> bool {
        let _ = id;
        todo!("implemented in the storage slice")
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
        let _ = (query, k, allowlist);
        todo!("implemented in the storage slice")
    }

    /// Persist with the non-negotiable atomic-rename pattern (addendum §5):
    /// write to `{path}.tmp` (same directory), fsync the file, rename onto
    /// `path`. A crash mid-write must never leave a truncated `index.tv`.
    pub fn save_atomic(&self, path: &Path) -> Result<(), KbError> {
        let _ = path;
        todo!("implemented in the storage slice")
    }
}
