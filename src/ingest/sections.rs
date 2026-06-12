//! Section classification and chunking (PRD §4 step 6) — the single
//! trickiest part of the pipeline.

use crate::{KbError, RawChunk, SectionType, TocEntry};

/// Deterministic heading classifier (PRD §4 step 6, locked decision:
/// ambiguous ⇒ `Other`, no ML). Match is case-insensitive `contains`,
/// checked in PRD order (abstract, introduction, background/related/prior,
/// method/approach/algorithm/design/model, experiment/evaluation/result/
/// benchmark, application/use case, limitation/threat/weakness,
/// future work/future direction/open problem, conclusion/discussion).
pub fn classify_heading(heading: &str) -> SectionType {
    let _ = heading;
    todo!("implemented in the ingest slice")
}

/// Strip HTML comments (`<!-- … -->`, including multi-line) from markdown.
/// Locked decision: the notes.md template prompts live in HTML comments and
/// must never be embedded.
pub fn strip_html_comments(md: &str) -> String {
    let _ = md;
    todo!("implemented in the ingest slice")
}

/// Build the full chunk list for a paper:
///
/// - `abstract` chunk always comes from `abstract_text` (arXiv API copy is
///   cleaner than pandoc's — PRD §4 step 6 special case)
/// - `sections_md` (when present) is split at H1/H2/H3 boundaries; each
///   section is classified by its heading; preamble text before the first
///   heading is `Other`. Content that pandoc emitted for the abstract
///   (a section whose heading classifies as `Abstract`) is skipped in
///   favor of the API abstract.
/// - `notes_md` (when present) becomes `user_notes` chunks after
///   [`strip_html_comments`]; if nothing but whitespace remains, no chunk.
/// - Any chunk over `chunk_max_tokens` (see [`crate::approx_tokens`]) is
///   split at paragraph boundaries (`\n\n`), sub-chunks keep the section
///   type and heading, ordinals increment within the section type.
/// - Empty/whitespace-only sections produce no chunk.
///
/// Ordinals are 0-based per section type, so ids look like
/// `2504.19874_method_0`, `2504.19874_method_1`.
pub fn build_chunks(
    sections_md: Option<&str>,
    abstract_text: &str,
    notes_md: Option<&str>,
    chunk_max_tokens: usize,
) -> Result<Vec<RawChunk>, KbError> {
    let _ = (sections_md, abstract_text, notes_md, chunk_max_tokens);
    todo!("implemented in the ingest slice")
}

/// Page mapping for one chunk (PRD §4 step 9): find the TOC entry whose
/// title best matches `heading` (case-insensitive; strip leading numbering
/// like "3.2 " from both sides; exact normalized match, then substring
/// containment either way). Returns `(page, named_dest)` on a match, None
/// otherwise — the caller falls back to the nearest preceding chunk's page.
pub fn page_for_heading(
    heading: Option<&str>,
    toc: &[TocEntry],
) -> Option<(u32, Option<String>)> {
    let _ = (heading, toc);
    todo!("implemented in the ingest slice")
}
