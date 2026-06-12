//! PDF download, outline (TOC) extraction, and fallback text extraction
//! (PRD §4 steps 4-5). Pure-Rust via `lopdf`/`pdf-extract` — keeps the
//! single-static-binary promise (no libpdfium dylib).

use crate::{KbError, TocEntry};
use std::path::Path;

/// Download `https://arxiv.org/pdf/{id}` to `dest` (atomic: temp file then
/// rename). Non-200 ⇒ `Network`; a payload that isn't `%PDF` ⇒ `Extraction`.
pub async fn download_pdf(
    client: &reqwest::Client,
    arxiv_id: &str,
    dest: &Path,
) -> Result<(), KbError> {
    let _ = (client, arxiv_id, dest);
    todo!("implemented in the ingest slice")
}

/// Extract the PDF outline as a flat list (depth-first order), with
/// 1-indexed page numbers and named destinations when present.
/// A PDF with no outline returns an empty Vec (PRD §14: page numbers only).
/// A malformed PDF ⇒ `Extraction` error.
pub fn extract_toc(pdf_path: &Path) -> Result<Vec<TocEntry>, KbError> {
    let _ = pdf_path;
    todo!("implemented in the ingest slice")
}

/// Fallback text extraction, one String per page (PRD §4 step 4). The
/// caller assembles `sections.md` with `## Page N` headings. Used only
/// when LaTeX is unavailable or pandoc fails — graceful degradation.
pub fn extract_text_per_page(pdf_path: &Path) -> Result<Vec<String>, KbError> {
    let _ = pdf_path;
    todo!("implemented in the ingest slice")
}
