//! Clean Read: a faithful, citation-free rewrite of a paper's extracted text.
//!
//! Research papers are dense with inline citations ("(Bakalova et al., 2025)"),
//! bracketed refs ("[12]"), and cross-reference scaffolding ("see Section 4",
//! "as Figure 2 shows"). For a reader trying to follow the argument, this is
//! noise. This module rewrites the engine-extracted `sections.md` into clean,
//! readable prose with that clutter removed — preserving the argument and the
//! technical substance — and persists it as `reader.md` next to the PDF. The
//! result is NOT embedded into the vector index; it's purely a reading surface.
//!
//! Generation is on-demand (the reader clicks "Generate") and cached to disk,
//! matching the PRD's anti-precompute stance. Long papers are processed
//! section-by-section so no single LLM call has to swallow the whole body or
//! emit one huge (truncatable) output — which also yields natural streaming
//! progress as the document builds heading by heading.

use crate::anthropic::AnthropicChat;
use crate::chat::{ChatMessage, OpenAiChat};
use crate::config::{Config, KbPaths};
use crate::KbError;

/// Output token budget for one window's rewrite. Larger than the roundtable
/// default (2048) because a faithful rewrite runs about as long as its input;
/// windows are pre-sized (see [`WINDOW_CHARS`]) to stay within this.
const READER_MAX_TOKENS: u32 = 4096;

/// Target window size in characters (~4 chars/token). `sections.md` is split on
/// headings, then adjacent segments are merged up to this so each LLM call is a
/// meaningful unit rather than a lone heading or a single page.
const WINDOW_CHARS: usize = 12_000;

/// Low temperature for the OpenAI path — a faithful rewrite wants fidelity, not
/// invention. (The Anthropic path sends no sampling params; Opus 4.8 rejects
/// them.)
const READER_TEMPERATURE: f32 = 0.2;

/// System prompt for the per-window rewrite. Mirrors the inline-prompt style of
/// `DEFAULT_CHAT_SYSTEM` in `search::retrieval`.
const SYSTEM_PROMPT: &str = "You are rewriting one section of a research paper into clean, readable prose \
for a focused reader who wants the argument without the citation clutter.\n\n\
Rules:\n\
- This is a FAITHFUL REWRITE, not a summary. Preserve the argument, the \
definitions, the methods, and every quantitative claim and result. Do not omit \
steps of reasoning or drop technical detail.\n\
- Remove inline citation clutter: parenthetical author-year citations like \
\"(Bakalova et al., 2025; Geva et al., 2023)\", bracketed numeric references \
like \"[12]\", and cross-reference scaffolding like \"as shown in Section 4\", \
\"see Figure 2\", \"Table 3 reports\". When a cited result is load-bearing, keep \
the CLAIM in prose and simply drop the citation marker. Never invent or add \
citations.\n\
- Keep the section heading(s). Output GitHub-flavored Markdown. Preserve display \
and inline math verbatim (\\(...\\), \\[...\\], $...$, $$...$$) so it still \
renders.\n\
- Output ONLY the rewritten prose. No preamble (\"Here is the rewrite\"), no \
meta-commentary, and no notes about what you changed.";

/// Generate the Clean Read for `paper_id`, streaming each text fragment to
/// `on_delta`, and write the result atomically to `reader.md`. Returns the full
/// markdown.
///
/// `model` selects the provider (Claude vs. OpenAI by id prefix). Errors are
/// returned, not written: a partial/interrupted run never leaves a truncated
/// canonical `reader.md` (the file is written via a temp + rename only on full
/// success, leaving any prior copy intact).
pub async fn generate_reader<F: FnMut(&str)>(
    paths: &KbPaths,
    config: &Config,
    paper_id: &str,
    model: &str,
    mut on_delta: F,
) -> Result<String, KbError> {
    if !paths.metadata_path(paper_id).exists() {
        return Err(KbError::NotFound(format!("{paper_id} is not in the KB")));
    }
    let source = std::fs::read_to_string(paths.sections_path(paper_id)).map_err(|_| {
        KbError::NotFound(format!(
            "{paper_id} has no extracted text (sections.md) to distill"
        ))
    })?;

    let model = if model.trim().is_empty() {
        config.chat.model.as_str()
    } else {
        model
    };

    let windows = build_windows(&source);
    if windows.is_empty() {
        return Err(KbError::Usage(format!("{paper_id} has no body text to distill")));
    }

    let mut full = String::new();
    for (i, window) in windows.iter().enumerate() {
        if i > 0 {
            // Blank line between rewritten sections so the markdown reflows cleanly.
            on_delta("\n\n");
            full.push_str("\n\n");
        }
        let messages = vec![ChatMessage::system(SYSTEM_PROMPT), ChatMessage::user(window.clone())];
        complete_stream_capped(model, &messages, READER_MAX_TOKENS, |d| {
            full.push_str(d);
            on_delta(d);
        })
        .await?;
    }

    // Atomic write: temp + rename, so an interrupted stream can't leave a
    // half-written reader.md (rename is atomic on the same filesystem).
    let reader_path = paths.reader_path(paper_id);
    let tmp = paths.paper_dir(paper_id).join("reader.md.tmp");
    std::fs::write(&tmp, &full)
        .map_err(|e| KbError::Index(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &reader_path)
        .map_err(|e| KbError::Index(format!("rename {}: {e}", reader_path.display())))?;

    Ok(full)
}

/// Stream a single window through the right provider, applying the larger
/// output cap. Mirrors `roundtable::complete_stream`'s id-prefix routing, but
/// constructs the client directly so the per-call `max_tokens` takes effect.
async fn complete_stream_capped<F: FnMut(&str)>(
    model: &str,
    messages: &[ChatMessage],
    max_tokens: u32,
    on_delta: F,
) -> Result<String, KbError> {
    if model.starts_with("claude") {
        AnthropicChat::from_env(model)?
            .with_max_tokens(max_tokens)
            .complete_stream(messages, on_delta)
            .await
    } else {
        OpenAiChat::from_env(model)?
            .with_max_tokens(max_tokens)
            .complete_stream(messages, READER_TEMPERATURE, on_delta)
            .await
    }
}

/// Split `sections.md` into generation windows: segment at markdown headings,
/// drop reference/bibliography/acknowledgment sections, then pack segments up to
/// [`WINDOW_CHARS`]. An oversized segment (e.g. a long section or a big `## Page
/// N` block from a PDF-only paper) is further split at paragraph boundaries so
/// no single call is unbounded.
fn build_windows(md: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for seg in split_segments(md) {
        if is_reference_heading(&seg) {
            continue;
        }
        if seg.len() > WINDOW_CHARS {
            push_nonempty(&mut cur, &mut out);
            for piece in split_paragraphs(&seg, WINDOW_CHARS) {
                out.push(piece);
            }
            continue;
        }
        if !cur.is_empty() && cur.len() + seg.len() > WINDOW_CHARS {
            push_nonempty(&mut cur, &mut out);
        }
        cur.push_str(&seg);
    }
    push_nonempty(&mut cur, &mut out);
    out
}

fn push_nonempty(cur: &mut String, out: &mut Vec<String>) {
    if !cur.trim().is_empty() {
        out.push(std::mem::take(cur));
    } else {
        cur.clear();
    }
}

/// Split markdown into segments that each begin at a heading line (`#`…) and run
/// up to the next heading. Leading content before the first heading is its own
/// segment.
fn split_segments(md: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut cur = String::new();
    for line in md.lines() {
        if line.trim_start().starts_with('#') && !cur.trim().is_empty() {
            segments.push(std::mem::take(&mut cur));
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.trim().is_empty() {
        segments.push(cur);
    }
    segments
}

/// True if the segment's first heading is a references/bibliography/
/// acknowledgments section — content the Clean Read drops entirely.
fn is_reference_heading(seg: &str) -> bool {
    let Some(line) = seg.lines().find(|l| l.trim_start().starts_with('#')) else {
        return false;
    };
    let h = line.trim_start_matches('#').trim().to_lowercase();
    h.starts_with("reference")
        || h.starts_with("bibliography")
        || h.starts_with("acknowledg")
}

/// Split an oversized segment into <= `max`-char pieces at blank-line (paragraph)
/// boundaries, so a single huge section/page still streams in bounded calls.
fn split_paragraphs(seg: &str, max: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut cur = String::new();
    for para in seg.split("\n\n") {
        if !cur.is_empty() && cur.len() + para.len() + 2 > max {
            pieces.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push_str("\n\n");
        }
        cur.push_str(para);
    }
    if !cur.trim().is_empty() {
        pieces.push(cur);
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_split_at_headings() {
        let md = "intro text\n\n# A\nbody a\n\n## B\nbody b\n";
        let segs = split_segments(md);
        assert_eq!(segs.len(), 3);
        assert!(segs[0].starts_with("intro text"));
        assert!(segs[1].starts_with("# A"));
        assert!(segs[2].starts_with("## B"));
    }

    #[test]
    fn reference_sections_are_dropped() {
        let md = "# Method\nwe do x\n\n# References\n[1] foo\n[2] bar\n";
        let windows = build_windows(md);
        let joined = windows.join("\n");
        assert!(joined.contains("we do x"));
        assert!(!joined.contains("[1] foo"));
    }

    #[test]
    fn is_reference_heading_matches_variants() {
        assert!(is_reference_heading("# References\nx"));
        assert!(is_reference_heading("## Bibliography\nx"));
        assert!(is_reference_heading("# Acknowledgments\nx"));
        assert!(!is_reference_heading("# Method\nx"));
    }

    #[test]
    fn small_sections_pack_into_one_window() {
        let md = "# A\nshort\n\n# B\nalso short\n";
        let windows = build_windows(md);
        assert_eq!(windows.len(), 1, "tiny sections should merge into one window");
    }

    #[test]
    fn oversized_segment_splits_by_paragraph() {
        // ~30k chars under one heading, with paragraph breaks to split on.
        let big = format!("# Big\n{}", "lorem ipsum dolor sit.\n\n".repeat(1300));
        let windows = build_windows(&big);
        assert!(windows.len() > 1, "oversized section must be split");
        // Each piece stays near the window budget (one trailing paragraph may
        // nudge it slightly over).
        assert!(windows.iter().all(|w| w.len() <= WINDOW_CHARS + 64));
    }
}
