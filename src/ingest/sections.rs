//! Section classification and chunking (PRD §4 step 6) — the single
//! trickiest part of the pipeline.

use crate::{KbError, RawChunk, SectionType, TocEntry, approx_tokens};

/// Deterministic heading classifier (PRD §4 step 6, locked decision:
/// ambiguous ⇒ `Other`, no ML). Match is case-insensitive `contains`,
/// checked in PRD order (abstract, introduction, background/related/prior,
/// method/approach/algorithm/design/model, experiment/evaluation/result/
/// benchmark, application/use case, limitation/threat/weakness,
/// future work/future direction/open problem, conclusion/discussion).
pub fn classify_heading(heading: &str) -> SectionType {
    let h = heading.to_lowercase();
    let s = h.as_str();
    if s.contains("abstract") {
        SectionType::Abstract
    } else if s.contains("introduction") {
        SectionType::Introduction
    } else if s.contains("related work") || s.contains("background") || s.contains("prior") {
        SectionType::Background
    } else if s.contains("method")
        || s.contains("approach")
        || s.contains("algorithm")
        || s.contains("design")
        || s.contains("model")
    {
        SectionType::Method
    } else if s.contains("experiment")
        || s.contains("evaluation")
        || s.contains("result")
        || s.contains("benchmark")
    {
        SectionType::Experiments
    } else if s.contains("application") || s.contains("use case") {
        SectionType::Applications
    } else if s.contains("limitation") || s.contains("threat") || s.contains("weakness") {
        SectionType::Limitations
    } else if s.contains("future work")
        || s.contains("future direction")
        || s.contains("open problem")
    {
        SectionType::FutureWork
    } else if s.contains("conclusion") || s.contains("discussion") {
        SectionType::Conclusion
    } else {
        SectionType::Other
    }
}

/// Strip HTML comments (`<!-- … -->`, including multi-line) from markdown.
/// Locked decision: the notes.md template prompts live in HTML comments and
/// must never be embedded. An unterminated comment runs to end of input
/// (HTML semantics).
pub fn strip_html_comments(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start + 4..].find("-->") {
            Some(end) => rest = &rest[start + 4 + end + 3..],
            None => return out, // unterminated: drop to end
        }
    }
    out.push_str(rest);
    out
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
    let mut builder = ChunkBuilder::new(chunk_max_tokens);

    // The abstract always comes from the API copy.
    builder.add_section(SectionType::Abstract, None, abstract_text);

    if let Some(md) = sections_md {
        for (heading, body) in split_at_headings(md) {
            match &heading {
                None => builder.add_section(SectionType::Other, None, &body),
                Some(h) => {
                    let ty = classify_heading(h);
                    if ty == SectionType::Abstract {
                        continue; // API abstract wins over pandoc's
                    }
                    builder.add_section(ty, Some(h.clone()), &body);
                }
            }
        }
    }

    if let Some(notes) = notes_md {
        let stripped = strip_html_comments(notes);
        builder.add_section(SectionType::UserNotes, None, &stripped);
    }

    Ok(builder.chunks)
}

struct ChunkBuilder {
    chunks: Vec<RawChunk>,
    /// Per-type ordinal counters, indexed parallel to `SectionType::ALL`.
    ordinals: [u32; SectionType::ALL.len()],
    chunk_max_tokens: usize,
}

impl ChunkBuilder {
    fn new(chunk_max_tokens: usize) -> Self {
        ChunkBuilder {
            chunks: Vec::new(),
            ordinals: [0; SectionType::ALL.len()],
            chunk_max_tokens,
        }
    }

    fn next_ordinal(&mut self, ty: SectionType) -> u32 {
        let idx = SectionType::ALL
            .iter()
            .position(|t| *t == ty)
            .expect("SectionType::ALL is exhaustive");
        let ord = self.ordinals[idx];
        self.ordinals[idx] += 1;
        ord
    }

    /// Add one classified section, splitting it at paragraph boundaries if
    /// it exceeds the token budget. Whitespace-only text adds nothing.
    fn add_section(&mut self, ty: SectionType, heading: Option<String>, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        for piece in split_oversized(text, self.chunk_max_tokens) {
            let ordinal = self.next_ordinal(ty);
            self.chunks.push(RawChunk {
                section_type: ty,
                heading: heading.clone(),
                ordinal,
                text: piece,
            });
        }
    }
}

/// Split `text` into pieces of at most `max_tokens` (per [`approx_tokens`])
/// at `\n\n` paragraph boundaries. A single paragraph longer than the budget
/// is kept whole — paragraph boundaries are the finest split we do.
fn split_oversized(text: &str, max_tokens: usize) -> Vec<String> {
    if approx_tokens(text) <= max_tokens {
        return vec![text.to_string()];
    }
    let mut pieces = Vec::new();
    let mut current = String::new();
    for para in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        if !current.is_empty() && approx_tokens(&current) + approx_tokens(para) > max_tokens {
            pieces.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(para);
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

/// Split markdown at ATX H1/H2/H3 boundaries. Returns `(heading, body)`
/// pairs in document order; the preamble before the first heading comes
/// first with `heading = None`. H4+ headings and fenced code blocks do not
/// split sections.
fn split_at_headings(md: &str) -> Vec<(Option<String>, String)> {
    let mut out: Vec<(Option<String>, String)> = Vec::new();
    let mut heading: Option<String> = None;
    let mut body = String::new();
    let mut in_fence = false;

    for line in md.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        }
        if !in_fence
            && let Some(h) = parse_atx_heading(line)
        {
            out.push((heading.take(), std::mem::take(&mut body)));
            heading = Some(h);
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    out.push((heading, body));

    // Drop a preamble entry that is pure whitespace (common: file starts
    // directly with a heading) but keep empty *headed* sections so callers
    // can decide (ChunkBuilder drops them anyway).
    if let Some((None, b)) = out.first()
        && b.trim().is_empty()
    {
        out.remove(0);
    }
    out
}

/// `## 3.2 Lloyd-Max Quantization {#sec:lm}` → `3.2 Lloyd-Max Quantization`.
/// Only H1-H3 count as section boundaries.
fn parse_atx_heading(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
    if !(1..=3).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None; // "#hashtag" is not a heading
    }
    let mut text = rest.trim().trim_end_matches('#').trim_end().to_string();
    // Pandoc may carry attribute blocks: "Introduction {#sec:intro}".
    if text.ends_with('}')
        && let Some(brace) = text.rfind('{')
    {
        text.truncate(brace);
        text = text.trim_end().to_string();
    }
    Some(text)
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
    let needle = normalize_title(heading?);
    if needle.is_empty() {
        return None;
    }
    // Pass 1: exact normalized match.
    for entry in toc {
        if normalize_title(&entry.title) == needle {
            return Some((entry.page, entry.named_dest.clone()));
        }
    }
    // Pass 2: substring containment either way.
    for entry in toc {
        let t = normalize_title(&entry.title);
        if !t.is_empty() && (t.contains(&needle) || needle.contains(&t)) {
            return Some((entry.page, entry.named_dest.clone()));
        }
    }
    None
}

/// Lowercase, strip leading section numbering ("3.2 ", "4. "), collapse
/// whitespace.
fn normalize_title(title: &str) -> String {
    let lower = title.to_lowercase();
    let trimmed = lower.trim();
    let numbering_len = trimmed
        .bytes()
        .take_while(|b| b.is_ascii_digit() || *b == b'.')
        .count();
    let rest = if numbering_len > 0 && trimmed[..numbering_len].bytes().any(|b| b.is_ascii_digit())
    {
        &trimmed[numbering_len..]
    } else {
        trimmed
    };
    rest.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------- classify_heading --------

    #[test]
    fn classifies_every_section_type() {
        assert_eq!(classify_heading("Abstract"), SectionType::Abstract);
        assert_eq!(classify_heading("1 Introduction"), SectionType::Introduction);
        assert_eq!(classify_heading("Related Work"), SectionType::Background);
        assert_eq!(classify_heading("2 Background"), SectionType::Background);
        assert_eq!(classify_heading("Prior Art"), SectionType::Background);
        assert_eq!(classify_heading("3 Method"), SectionType::Method);
        assert_eq!(classify_heading("Our Approach"), SectionType::Method);
        assert_eq!(classify_heading("The Algorithm"), SectionType::Method);
        assert_eq!(classify_heading("System Design"), SectionType::Method);
        assert_eq!(classify_heading("Model Architecture"), SectionType::Method);
        assert_eq!(classify_heading("4 Experiments"), SectionType::Experiments);
        assert_eq!(classify_heading("Evaluation"), SectionType::Experiments);
        assert_eq!(classify_heading("Results"), SectionType::Experiments);
        assert_eq!(classify_heading("Benchmarks"), SectionType::Experiments);
        assert_eq!(classify_heading("Applications"), SectionType::Applications);
        assert_eq!(classify_heading("Use Cases"), SectionType::Applications);
        assert_eq!(classify_heading("Limitations"), SectionType::Limitations);
        assert_eq!(classify_heading("Threats to Validity"), SectionType::Limitations);
        assert_eq!(classify_heading("Weaknesses"), SectionType::Limitations);
        assert_eq!(classify_heading("Future Work"), SectionType::FutureWork);
        assert_eq!(classify_heading("Future Directions"), SectionType::FutureWork);
        assert_eq!(classify_heading("Open Problems"), SectionType::FutureWork);
        assert_eq!(classify_heading("5 Conclusion"), SectionType::Conclusion);
        assert_eq!(classify_heading("Discussion"), SectionType::Conclusion);
    }

    #[test]
    fn classification_is_case_insensitive() {
        assert_eq!(classify_heading("ABSTRACT"), SectionType::Abstract);
        assert_eq!(classify_heading("RELATED WORK"), SectionType::Background);
    }

    #[test]
    fn first_match_in_prd_order_wins() {
        // "introduction" beats "background"
        assert_eq!(
            classify_heading("Introduction and Background"),
            SectionType::Introduction
        );
        // "result" (Experiments) beats "discussion" (Conclusion)
        assert_eq!(
            classify_heading("Results and Discussion"),
            SectionType::Experiments
        );
        // "background" beats "method"
        assert_eq!(
            classify_heading("Background on Quantization Methods"),
            SectionType::Background
        );
    }

    #[test]
    fn numbered_subsection_without_keywords_is_other() {
        assert_eq!(
            classify_heading("3.2 Lloyd-Max Quantization"),
            SectionType::Other
        );
    }

    #[test]
    fn numbered_heading_with_keyword_classifies() {
        assert_eq!(classify_heading("3.1 Proposed Method"), SectionType::Method);
    }

    #[test]
    fn ambiguous_headings_are_other() {
        for h in ["Acknowledgments", "References", "Appendix A", "Notation", "Ethics Statement"] {
            assert_eq!(classify_heading(h), SectionType::Other, "{h}");
        }
    }

    // -------- strip_html_comments --------

    #[test]
    fn strips_single_line_comment() {
        assert_eq!(strip_html_comments("a <!-- hidden --> b"), "a  b");
    }

    #[test]
    fn strips_multiline_comment() {
        let md = "before\n<!-- line one\nline two -->\nafter";
        assert_eq!(strip_html_comments(md), "before\n\nafter");
    }

    #[test]
    fn strips_multiple_comments() {
        let md = "<!-- a -->x<!-- b -->y";
        assert_eq!(strip_html_comments(md), "xy");
    }

    #[test]
    fn unterminated_comment_drops_to_end() {
        assert_eq!(strip_html_comments("keep <!-- never closed"), "keep ");
    }

    #[test]
    fn no_comments_is_identity() {
        assert_eq!(strip_html_comments("plain **markdown**"), "plain **markdown**");
    }

    // -------- build_chunks --------

    fn ids(chunks: &[RawChunk]) -> Vec<String> {
        chunks.iter().map(|c| c.chunk_id("2504.19874")).collect()
    }

    #[test]
    fn abstract_always_from_api_text() {
        let md = "# Abstract\n\nPandoc's mangled abstract.\n\n# Introduction\n\nIntro text.\n";
        let chunks = build_chunks(Some(md), "The clean API abstract.", None, 2000).unwrap();
        let abstracts: Vec<_> = chunks
            .iter()
            .filter(|c| c.section_type == SectionType::Abstract)
            .collect();
        assert_eq!(abstracts.len(), 1);
        assert_eq!(abstracts[0].text, "The clean API abstract.");
        assert_eq!(abstracts[0].ordinal, 0);
        assert_eq!(abstracts[0].heading, None);
    }

    #[test]
    fn preamble_before_first_heading_is_other() {
        let md = "Title line emitted by pandoc.\n\n# Introduction\n\nIntro body.\n";
        let chunks = build_chunks(Some(md), "Abs.", None, 2000).unwrap();
        assert_eq!(chunks[0].section_type, SectionType::Abstract);
        assert_eq!(chunks[1].section_type, SectionType::Other);
        assert_eq!(chunks[1].heading, None);
        assert_eq!(chunks[1].text, "Title line emitted by pandoc.");
        assert_eq!(chunks[2].section_type, SectionType::Introduction);
        assert_eq!(chunks[2].heading.as_deref(), Some("Introduction"));
    }

    #[test]
    fn chunk_id_format() {
        let md = "# Method\n\nHow it works.\n\n# Conclusion\n\nThe end.\n";
        let chunks = build_chunks(Some(md), "Abs.", Some("My note."), 2000).unwrap();
        assert_eq!(
            ids(&chunks),
            vec![
                "2504.19874_abstract_0",
                "2504.19874_method_0",
                "2504.19874_conclusion_0",
                "2504.19874_user_notes_0",
            ]
        );
    }

    #[test]
    fn notes_become_user_notes_after_comment_stripping() {
        let notes = "# Notes on TurboQuant\n\n<!-- Why is this interesting to me? -->\n\nGreat for edge embedding search.\n";
        let chunks = build_chunks(None, "Abs.", Some(notes), 2000).unwrap();
        let notes_chunks: Vec<_> = chunks
            .iter()
            .filter(|c| c.section_type == SectionType::UserNotes)
            .collect();
        assert_eq!(notes_chunks.len(), 1);
        assert!(!notes_chunks[0].text.contains("interesting to me"));
        assert!(notes_chunks[0].text.contains("Great for edge embedding search."));
    }

    #[test]
    fn untouched_notes_template_produces_no_chunk() {
        let notes = "<!-- Why is this interesting to me? -->\n\n\n<!-- What would I build with this? -->\n\n\n";
        let chunks = build_chunks(None, "Abs.", Some(notes), 2000).unwrap();
        assert!(
            chunks.iter().all(|c| c.section_type != SectionType::UserNotes),
            "whitespace-only notes must not produce a chunk"
        );
    }

    #[test]
    fn empty_sections_are_dropped() {
        let md = "# Method\n\n\n# Conclusion\n\nDone.\n";
        let chunks = build_chunks(Some(md), "Abs.", None, 2000).unwrap();
        assert!(chunks.iter().all(|c| c.section_type != SectionType::Method));
        assert!(chunks.iter().any(|c| c.section_type == SectionType::Conclusion));
    }

    #[test]
    fn empty_abstract_is_dropped() {
        let chunks = build_chunks(None, "   \n ", None, 2000).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn oversized_section_splits_at_paragraph_boundaries() {
        // ~100 tokens per paragraph (400 chars), budget 150 tokens
        // => two paragraphs per chunk never fit; expect one chunk each.
        let para_a = "a".repeat(400);
        let para_b = "b".repeat(400);
        let para_c = "c".repeat(400);
        let md = format!("# Method\n\n{para_a}\n\n{para_b}\n\n{para_c}\n");
        let chunks = build_chunks(Some(&md), "Abs.", None, 150).unwrap();
        let method: Vec<_> = chunks
            .iter()
            .filter(|c| c.section_type == SectionType::Method)
            .collect();
        assert_eq!(method.len(), 3);
        assert_eq!(method[0].ordinal, 0);
        assert_eq!(method[1].ordinal, 1);
        assert_eq!(method[2].ordinal, 2);
        assert_eq!(method[0].text, para_a);
        assert_eq!(method[1].text, para_b);
        assert_eq!(method[2].text, para_c);
        // Sub-chunks share the heading.
        assert!(method.iter().all(|c| c.heading.as_deref() == Some("Method")));
        assert_eq!(method[1].chunk_id("2504.19874"), "2504.19874_method_1");
    }

    #[test]
    fn oversized_split_packs_paragraphs_greedily() {
        // 4 paragraphs of ~25 tokens each, budget 60 => two per chunk.
        let p = "x".repeat(100);
        let md = format!("# Method\n\n{p}\n\n{p}\n\n{p}\n\n{p}\n");
        let chunks = build_chunks(Some(&md), "Abs.", None, 60).unwrap();
        let method: Vec<_> = chunks
            .iter()
            .filter(|c| c.section_type == SectionType::Method)
            .collect();
        assert_eq!(method.len(), 2);
        assert_eq!(method[0].text, format!("{p}\n\n{p}"));
    }

    #[test]
    fn single_huge_paragraph_stays_whole() {
        let para = "z".repeat(2000); // 500 tokens, no \n\n inside
        let md = format!("# Method\n\n{para}\n");
        let chunks = build_chunks(Some(&md), "Abs.", None, 100).unwrap();
        let method: Vec<_> = chunks
            .iter()
            .filter(|c| c.section_type == SectionType::Method)
            .collect();
        assert_eq!(method.len(), 1);
        assert_eq!(method[0].text, para);
    }

    #[test]
    fn ordinals_increment_across_same_type_sections() {
        let md = "# Method\n\nFirst.\n\n# Our Approach\n\nSecond.\n";
        let chunks = build_chunks(Some(md), "Abs.", None, 2000).unwrap();
        let method: Vec<_> = chunks
            .iter()
            .filter(|c| c.section_type == SectionType::Method)
            .collect();
        assert_eq!(method.len(), 2);
        assert_eq!(method[0].ordinal, 0);
        assert_eq!(method[1].ordinal, 1);
        assert_eq!(method[0].heading.as_deref(), Some("Method"));
        assert_eq!(method[1].heading.as_deref(), Some("Our Approach"));
    }

    #[test]
    fn h1_h2_h3_split_but_h4_does_not() {
        let md = "## Method\n\nTop.\n\n#### Inner detail\n\nStill method.\n\n### Evaluation\n\nEval.\n";
        let chunks = build_chunks(Some(md), "Abs.", None, 2000).unwrap();
        let method: Vec<_> = chunks
            .iter()
            .filter(|c| c.section_type == SectionType::Method)
            .collect();
        assert_eq!(method.len(), 1);
        assert!(method[0].text.contains("#### Inner detail"));
        assert!(method[0].text.contains("Still method."));
        assert!(chunks.iter().any(|c| c.section_type == SectionType::Experiments));
    }

    #[test]
    fn hashes_inside_code_fences_do_not_split() {
        let md = "# Method\n\n```bash\n# not a heading\n```\n\nAfter fence.\n";
        let chunks = build_chunks(Some(md), "Abs.", None, 2000).unwrap();
        let method: Vec<_> = chunks
            .iter()
            .filter(|c| c.section_type == SectionType::Method)
            .collect();
        assert_eq!(method.len(), 1);
        assert!(method[0].text.contains("# not a heading"));
        assert!(method[0].text.contains("After fence."));
    }

    #[test]
    fn pandoc_attribute_blocks_are_stripped_from_headings() {
        let md = "# Introduction {#sec:intro}\n\nBody.\n";
        let chunks = build_chunks(Some(md), "Abs.", None, 2000).unwrap();
        let intro = chunks
            .iter()
            .find(|c| c.section_type == SectionType::Introduction)
            .unwrap();
        assert_eq!(intro.heading.as_deref(), Some("Introduction"));
    }

    #[test]
    fn no_sections_md_yields_abstract_and_notes_only() {
        let chunks = build_chunks(None, "Abs.", Some("A thought."), 2000).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].section_type, SectionType::Abstract);
        assert_eq!(chunks[1].section_type, SectionType::UserNotes);
    }

    // -------- page_for_heading --------

    fn toc() -> Vec<TocEntry> {
        vec![
            TocEntry { title: "1 Introduction".into(), page: 1, named_dest: Some("sec.1".into()) },
            TocEntry { title: "3 Method".into(), page: 3, named_dest: None },
            TocEntry { title: "3.2 Lloyd-Max Quantization".into(), page: 4, named_dest: Some("sec.3.2".into()) },
            TocEntry { title: "Conclusion".into(), page: 11, named_dest: None },
        ]
    }

    #[test]
    fn exact_match_after_number_stripping() {
        // pandoc heading has no number; TOC entry does.
        assert_eq!(
            page_for_heading(Some("Introduction"), &toc()),
            Some((1, Some("sec.1".to_string())))
        );
        // both numbered
        assert_eq!(
            page_for_heading(Some("3.2 Lloyd-Max Quantization"), &toc()),
            Some((4, Some("sec.3.2".to_string())))
        );
    }

    #[test]
    fn match_is_case_insensitive() {
        assert_eq!(page_for_heading(Some("METHOD"), &toc()), Some((3, None)));
    }

    #[test]
    fn substring_containment_matches() {
        // heading ⊂ toc title
        assert_eq!(
            page_for_heading(Some("Lloyd-Max"), &toc()),
            Some((4, Some("sec.3.2".to_string())))
        );
        // toc title ⊂ heading
        assert_eq!(
            page_for_heading(Some("Conclusion and Outlook"), &toc()),
            Some((11, None))
        );
    }

    #[test]
    fn exact_match_preferred_over_substring() {
        let toc = vec![
            TocEntry { title: "Method Overview".into(), page: 2, named_dest: None },
            TocEntry { title: "Method".into(), page: 5, named_dest: None },
        ];
        assert_eq!(page_for_heading(Some("Method"), &toc), Some((5, None)));
    }

    #[test]
    fn no_match_and_no_heading_return_none() {
        assert_eq!(page_for_heading(Some("Zebra Studies"), &toc()), None);
        assert_eq!(page_for_heading(None, &toc()), None);
        assert_eq!(page_for_heading(Some("3.2"), &toc()), None); // pure number normalizes to empty
    }
}
