//! LaTeX e-print download, main-tex detection, pandoc invocation
//! (PRD §4 steps 2-3).

use crate::KbError;
use std::path::{Path, PathBuf};

/// What the e-print archive turned out to contain.
#[derive(Debug, PartialEq)]
pub enum EprintContent {
    /// LaTeX sources were extracted into `source/`; `main_tex` is the
    /// detected entry point (relative to the source dir).
    Latex { main_tex: PathBuf },
    /// The e-print was a bare PDF (no `.tex`) — caller takes the PDF
    /// fallback path and sets `source_format: "pdf"`.
    PdfOnly,
}

/// Download `https://arxiv.org/e-print/{id}` and extract into `source_dir`.
///
/// The payload may be: a `.tar.gz` of a source tree, a bare `.gz` of a
/// single `.tex`, or a raw PDF. Detect by content (magic bytes), not by
/// headers. Reject archive members that would escape `source_dir`
/// (path traversal) — skip them with a warning.
pub async fn download_and_extract_source(
    client: &reqwest::Client,
    arxiv_id: &str,
    source_dir: &Path,
) -> Result<EprintContent, KbError> {
    let _ = (client, arxiv_id, source_dir);
    todo!("implemented in the ingest slice")
}

/// Find the main `.tex` file (PRD §4 step 2 heuristics):
/// 1. candidates = files containing `\documentclass`
/// 2. prefer `main.tex`, `paper.tex`, `manuscript.tex`, then one with
///    `\begin{document}`
/// 3. still ambiguous ⇒ largest candidate
/// No candidates ⇒ `Extraction` error.
pub fn find_main_tex(source_dir: &Path) -> Result<PathBuf, KbError> {
    let _ = source_dir;
    todo!("implemented in the ingest slice")
}

/// Verify pandoc is runnable (`pandoc --version`). Missing ⇒ `Config`
/// error (exit 10) with install instructions (https://pandoc.org/installing.html).
pub fn check_pandoc(pandoc_path: &str) -> Result<(), KbError> {
    let _ = pandoc_path;
    todo!("implemented in the ingest slice")
}

/// Run `pandoc {main_tex} -o {out_md} --from latex --to gfm --wrap=none`
/// (plus `--bibliography` if a `.bib` exists next to the source).
/// Run with cwd = the main tex's directory so relative `\input`s resolve.
/// Non-zero exit ⇒ `Extraction` error carrying pandoc's stderr (caller
/// decides to fall back to PDF extraction).
pub async fn run_pandoc(
    pandoc_path: &str,
    main_tex: &Path,
    out_md: &Path,
) -> Result<(), KbError> {
    let _ = (pandoc_path, main_tex, out_md);
    todo!("implemented in the ingest slice")
}
