//! Section classification and chunking (PRD §4 step 6) — the single
//! trickiest part of the pipeline.

use crate::chat::{ChatMessage, OpenAiChat};
use crate::config::Config;
use crate::{KbError, RawChunk, SectionType, TocEntry, approx_tokens};
use std::collections::{HashMap, HashSet};

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

/// LaTeX command tokens pandoc sometimes leaves as bare words in the body
/// (e.g. a mangled `\maketitle\thanks{}` → "maketitle thanks aketitle"). A line
/// is dropped only when *every* token is one of these (or a `\begin{}`/`\end{}`
/// environment marker), so ordinary prose containing one such word survives.
const LATEX_NOISE: &[&str] = &[
    "maketitle", "thanks", "aketitle", "centering", "noindent", "par", "clearpage", "newpage",
    "vspace", "hspace", "bibliographystyle", "bibliography", "footnotesize", "small", "normalsize",
    "bigskip", "medskip", "smallskip", "hfill", "vfill", "tableofcontents", "appendix",
];

/// Strip extraction boilerplate from a paper body before chunking — LaTeX
/// preamble leftovers and figure/markup artifacts that pandoc/PDF conversion
/// leaves behind. They carry no retrievable meaning yet get embedded, polluting
/// search and surfacing as spurious matches (e.g. two `\maketitle` fragments
/// matching each other). Conservative and idempotent: it removes only
/// well-known noise, never ordinary prose, and leaves fenced code blocks
/// untouched. Applied to `sections.md` bodies only — the API abstract, notes,
/// and reflections are already clean.
pub fn clean_extracted_markdown(md: &str) -> String {
    // 1. ACM CCS concept blocks: `<div class="CCSXML"> … </div>`. The inner XML
    //    is escaped, so there is no real nested <div> to confuse the match.
    let without_ccs = remove_block(md, "<div class=\"CCSXML\">", "</div>");

    // 2. Line-level cleanup, preserving fenced code verbatim.
    let mut out = String::with_capacity(without_ccs.len());
    let mut blank_run = 1; // suppress leading blank lines
    let mut in_fence = false;
    for line in without_ccs.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            blank_run = 0;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_fence {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let drop = is_markup_only(line) || is_latex_preamble_noise(line);
        if drop || line.trim().is_empty() {
            if blank_run == 0 {
                out.push('\n'); // collapse a run of blanks to one
            }
            blank_run += 1;
            continue;
        }
        blank_run = 0;
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Remove every `open … close` span from `s` (non-nesting). An unterminated
/// `open` drops the remainder.
fn remove_block(s: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(open) {
        out.push_str(&rest[..i]);
        let after = &rest[i + open.len()..];
        match after.find(close) {
            Some(j) => rest = &after[j + close.len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Drop text between `<` and `>` (HTML/markup tags). Used to decide whether a
/// line is *only* markup.
fn strip_angle_spans(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// A line that is nothing but HTML tags — `<embed …/>`, `<img …>`,
/// `<span …></span>`, `<div …>`, `</div>`, `<figure>` … — carries no text to
/// embed. (Lines mixing tags with real text are left untouched.)
fn is_markup_only(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('<') && strip_angle_spans(t).trim().is_empty()
}

/// True when every whitespace token on the line is a LaTeX-noise command (see
/// [`LATEX_NOISE`]) or a `\begin{}`/`\end{}` marker.
fn is_latex_preamble_noise(line: &str) -> bool {
    let toks: Vec<&str> = line.split_whitespace().collect();
    if toks.is_empty() {
        return false;
    }
    toks.iter().all(|tok| {
        let w = tok.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '{');
        w.starts_with("begin{") || w.starts_with("end{") || LATEX_NOISE.contains(&w)
    })
}

/// Distinct headings in `md` whose *deterministic* classification is `Other` —
/// the candidates an LLM fallback ([`llm_heading_overrides`]) tries to rescue.
pub fn other_headings(md: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (heading, _) in split_at_headings(md) {
        if let Some(h) = heading
            && classify_heading(&h) == SectionType::Other
            && seen.insert(h.clone())
        {
            out.push(h);
        }
    }
    out
}

/// Section types the LLM fallback is allowed to assign. Excludes `abstract`
/// (the API copy always wins), the synthetic `user_notes`/`reflection`, and
/// `other` itself (the default it's trying to improve on).
const LLM_TARGETS: [SectionType; 8] = [
    SectionType::Introduction,
    SectionType::Background,
    SectionType::Method,
    SectionType::Experiments,
    SectionType::Applications,
    SectionType::Limitations,
    SectionType::FutureWork,
    SectionType::Conclusion,
];

/// LLM fallback classifier (PRD §16: sanctioned once the Other ratio exceeds
/// ~25%). Given the headings the deterministic classifier punted to `Other`,
/// ask the chat model to map each to a real section type in ONE batched call,
/// and return only the confident, in-vocabulary results. Best-effort: disabled
/// by config, an empty input, a missing key, or any API/parse error all yield
/// an empty map, leaving those headings as `Other`.
pub async fn llm_heading_overrides(
    config: &Config,
    headings: &[String],
) -> HashMap<String, SectionType> {
    if !config.ingest.classify_with_llm || headings.is_empty() {
        return HashMap::new();
    }
    match try_llm_overrides(config, headings).await {
        Ok(map) => map,
        Err(e) => {
            tracing::warn!("LLM heading classification failed ({e}); keeping deterministic 'other'");
            HashMap::new()
        }
    }
}

async fn try_llm_overrides(
    config: &Config,
    headings: &[String],
) -> Result<HashMap<String, SectionType>, KbError> {
    let client = OpenAiChat::from_env(&config.chat.model)?;
    let allowed = LLM_TARGETS.map(|t| t.as_str()).join(", ");
    let system = format!(
        "You label academic-paper section headings. For each heading, choose the single best \
         category from: {allowed}. Use \"other\" only if none fits (e.g. acknowledgments, \
         references, appendix, notation, ethics statement). Reply with ONLY a JSON object mapping \
         each input heading string (verbatim) to its category. No prose, no code fences."
    );
    let user = format!(
        "Headings:\n{}",
        serde_json::to_string(headings).unwrap_or_default()
    );
    let reply = client
        .complete(
            &[ChatMessage::system(system), ChatMessage::user(user)],
            0.0,
        )
        .await?;

    let json = extract_json_object(&reply)
        .ok_or_else(|| KbError::Network("classifier reply was not a JSON object".into()))?;
    let raw: HashMap<String, String> = serde_json::from_str(json)
        .map_err(|e| KbError::Network(format!("malformed classifier JSON: {e}")))?;

    let mut out = HashMap::new();
    for h in headings {
        if let Some(cat) = raw.get(h)
            && let Some(ty) = SectionType::parse(&cat.to_lowercase())
            && LLM_TARGETS.contains(&ty)
        {
            out.insert(h.clone(), ty);
        }
    }
    Ok(out)
}

/// Slice out the first `{ … }` object from a model reply (tolerates stray prose
/// or ```json fences around it).
fn extract_json_object(reply: &str) -> Option<&str> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    (end > start).then(|| &reply[start..=end])
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
/// - `reflection_md` (when present) becomes `reflection` chunk(s); used for
///   cross-paper synthesis documents created by `kb reflect` /
///   `kb_create_reflection` — no heading classification, whole text is one
///   typed unit.
/// - Any chunk over `chunk_max_tokens` (see [`crate::approx_tokens`]) is
///   split at paragraph boundaries (`\n\n`), sub-chunks keep the section
///   type and heading, ordinals increment within the section type.
/// - Empty/whitespace-only sections produce no chunk.
///
/// Ordinals are 0-based per section type, so ids look like
/// `2504.19874_method_0`, `2504.19874_method_1`.
///
/// Deterministic-only entry point (the LLM fallback contributes no overrides);
/// used by tests and any caller without a [`Config`]/network. Production ingest
/// goes through [`build_chunks_with_overrides`].
pub fn build_chunks(
    sections_md: Option<&str>,
    abstract_text: &str,
    notes_md: Option<&str>,
    reflection_md: Option<&str>,
    chunk_max_tokens: usize,
) -> Result<Vec<RawChunk>, KbError> {
    build_chunks_with_overrides(
        sections_md,
        abstract_text,
        notes_md,
        reflection_md,
        chunk_max_tokens,
        &HashMap::new(),
    )
}

/// As [`build_chunks`], but `overrides` (heading → section type, from
/// [`llm_heading_overrides`]) re-labels headings the deterministic classifier
/// put in `Other`. The paper body is first run through
/// [`clean_extracted_markdown`] to drop extraction boilerplate. The deterministic
/// keyword path is unchanged; overrides only ever upgrade an `Other`.
pub fn build_chunks_with_overrides(
    sections_md: Option<&str>,
    abstract_text: &str,
    notes_md: Option<&str>,
    reflection_md: Option<&str>,
    chunk_max_tokens: usize,
    overrides: &HashMap<String, SectionType>,
) -> Result<Vec<RawChunk>, KbError> {
    let mut builder = ChunkBuilder::new(chunk_max_tokens);

    // The abstract always comes from the API copy.
    builder.add_section(SectionType::Abstract, None, abstract_text);

    if let Some(md) = sections_md {
        let cleaned = clean_extracted_markdown(md);
        for (heading, body) in split_at_headings(&cleaned) {
            match &heading {
                None => builder.add_section(SectionType::Other, None, &body),
                Some(h) => {
                    let mut ty = classify_heading(h);
                    if ty == SectionType::Other
                        && let Some(&forced) = overrides.get(h)
                    {
                        ty = forced;
                    }
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
        // An untouched template is a lone "# Notes on …" heading once the
        // comment prompts are stripped — embedding it would let the paper
        // title masquerade as user intent in user_notes-filtered search.
        let body_without_title = stripped
            .trim_start()
            .strip_prefix('#')
            .map(|rest| rest.split_once('\n').map_or("", |(_, tail)| tail))
            .unwrap_or(&stripped);
        if !body_without_title.trim().is_empty() {
            builder.add_section(SectionType::UserNotes, None, &stripped);
        }
    }

    if let Some(reflection) = reflection_md {
        let text = reflection.trim();
        if !text.is_empty() {
            builder.add_section(SectionType::Reflection, None, text);
        }
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

    // -------- clean_extracted_markdown --------

    #[test]
    fn clean_removes_ccsxml_block() {
        let md = "# Intro\n\n<div class=\"CCSXML\">\n\\<ccs2012\\> \\<concept\\> junk \\</concept\\>\n</div>\n\nReal text.\n";
        let out = clean_extracted_markdown(md);
        assert!(!out.contains("CCSXML"));
        assert!(!out.contains("ccs2012"));
        assert!(out.contains("# Intro"));
        assert!(out.contains("Real text."));
    }

    #[test]
    fn clean_drops_markup_only_lines_but_keeps_mixed() {
        let md = "<embed src=\"figures/teaser.pdf\" />\n<span id=\"fig:t\" data-label=\"fig:t\"></span>\nThe success rate <span>x</span> improved.\n";
        let out = clean_extracted_markdown(md);
        assert!(!out.contains("<embed"));
        assert!(!out.contains("data-label"));
        // A line mixing tags with real prose is preserved verbatim.
        assert!(out.contains("The success rate <span>x</span> improved."));
    }

    #[test]
    fn clean_drops_latex_preamble_noise() {
        let md = "maketitle thanks aketitle\n\nGenerative agents simulate behavior.\n\n\\begin{figure}\n";
        let out = clean_extracted_markdown(md);
        assert!(!out.contains("maketitle"));
        assert!(!out.contains("begin{figure}"));
        assert!(out.contains("Generative agents simulate behavior."));
    }

    #[test]
    fn clean_preserves_code_fences_and_is_idempotent() {
        let md = "# Method\n\n```html\n<div>kept as code</div>\nmaketitle\n```\n\nProse.\n";
        let once = clean_extracted_markdown(md);
        assert!(once.contains("<div>kept as code</div>"), "fenced markup must survive");
        assert!(once.contains("maketitle"), "fenced latex word must survive");
        assert_eq!(clean_extracted_markdown(&once), once, "cleaning is idempotent");
    }

    // -------- other_headings + overrides --------

    #[test]
    fn other_headings_lists_only_unclassified_distinct() {
        let md = "# Introduction\n\nx\n\n# Generative Agent Architecture\n\ny\n\n## Memory and Retrieval\n\nz\n\n# Generative Agent Architecture\n\ndup\n";
        let others = other_headings(md);
        assert_eq!(others, vec!["Generative Agent Architecture".to_string(), "Memory and Retrieval".to_string()]);
        assert!(!others.iter().any(|h| h == "Introduction"), "classified headings excluded");
    }

    #[test]
    fn overrides_upgrade_other_headings_only() {
        let md = "# Generative Agent Architecture\n\nThe architecture has memory.\n\n# Introduction\n\nIntro.\n";
        let mut ov = HashMap::new();
        ov.insert("Generative Agent Architecture".to_string(), SectionType::Method);
        // An override for an already-classified heading must NOT override it.
        ov.insert("Introduction".to_string(), SectionType::Method);
        let chunks = build_chunks_with_overrides(Some(md), "Abs.", None, None, 2000, &ov).unwrap();
        let arch = chunks.iter().find(|c| c.heading.as_deref() == Some("Generative Agent Architecture")).unwrap();
        assert_eq!(arch.section_type, SectionType::Method, "Other heading upgraded");
        let intro = chunks.iter().find(|c| c.heading.as_deref() == Some("Introduction")).unwrap();
        assert_eq!(intro.section_type, SectionType::Introduction, "deterministic result not overridden");
    }

    #[test]
    fn extract_json_object_tolerates_fences_and_prose() {
        assert_eq!(extract_json_object("```json\n{\"a\":\"b\"}\n```"), Some("{\"a\":\"b\"}"));
        assert_eq!(extract_json_object("here: {\"x\":1} done"), Some("{\"x\":1}"));
        assert_eq!(extract_json_object("no object"), None);
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
        let chunks = build_chunks(Some(md), "The clean API abstract.", None, None, 2000).unwrap();
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
        let chunks = build_chunks(Some(md), "Abs.", None, None, 2000).unwrap();
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
        let chunks = build_chunks(Some(md), "Abs.", Some("My note."), None, 2000).unwrap();
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
        let chunks = build_chunks(None, "Abs.", Some(notes), None, 2000).unwrap();
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
        let chunks = build_chunks(None, "Abs.", Some(notes), None, 2000).unwrap();
        assert!(
            chunks.iter().all(|c| c.section_type != SectionType::UserNotes),
            "whitespace-only notes must not produce a chunk"
        );
    }

    #[test]
    fn empty_sections_are_dropped() {
        let md = "# Method\n\n\n# Conclusion\n\nDone.\n";
        let chunks = build_chunks(Some(md), "Abs.", None, None, 2000).unwrap();
        assert!(chunks.iter().all(|c| c.section_type != SectionType::Method));
        assert!(chunks.iter().any(|c| c.section_type == SectionType::Conclusion));
    }

    #[test]
    fn empty_abstract_is_dropped() {
        let chunks = build_chunks(None, "   \n ", None, None, 2000).unwrap();
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
        let chunks = build_chunks(Some(&md), "Abs.", None, None, 150).unwrap();
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
        let chunks = build_chunks(Some(&md), "Abs.", None, None, 60).unwrap();
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
        let chunks = build_chunks(Some(&md), "Abs.", None, None, 100).unwrap();
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
        let chunks = build_chunks(Some(md), "Abs.", None, None, 2000).unwrap();
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
        let chunks = build_chunks(Some(md), "Abs.", None, None, 2000).unwrap();
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
        let chunks = build_chunks(Some(md), "Abs.", None, None, 2000).unwrap();
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
        let chunks = build_chunks(Some(md), "Abs.", None, None, 2000).unwrap();
        let intro = chunks
            .iter()
            .find(|c| c.section_type == SectionType::Introduction)
            .unwrap();
        assert_eq!(intro.heading.as_deref(), Some("Introduction"));
    }

    #[test]
    fn no_sections_md_yields_abstract_and_notes_only() {
        let chunks = build_chunks(None, "Abs.", Some("A thought."), None, 2000).unwrap();
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
