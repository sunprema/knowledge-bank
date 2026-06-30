---
name: clean-paper
description: Produce a Clean Read of a paper in the arxiv-kb library — a faithful, citation-free rewrite of the full paper body — and write it to the paper's reader.md so it appears in the app's Clean Read tab. Use when the user wants to "clean read", "clean up a paper", "strip citations", "make a paper readable", "rewrite without references", or "regenerate the clean read". This is the Claude Code alternative to the engine's built-in reader (src/reader.rs); prefer this skill when asked. Triggers include "clean read 2602.03359", "clean read all papers", "make this paper readable".
---

# clean-paper

Rewrite a paper's full body into clean, readable prose with all citation clutter
removed, and save it as the paper's `reader.md` — the same file the macOS app's
**Clean Read** tab reads from disk (`ReaderView.swift` loads
`~/arxiv-kb/<id>/reader.md`; `has_reader` is just that file's existence). So
writing this file makes the result appear in the app immediately, with no engine
call and no embedding/API-key dependency (the Clean Read is a reading surface
only — it is NOT added to the vector index).

This is the Claude Code counterpart to the engine reader (`src/reader.rs`). The
engine processes the paper in ~12k-char windows through an LLM; this skill reads
the whole paper at once and rewrites it agentically, which handles cross-section
context and citation cleanup more reliably. Prefer this skill when the user asks
for a clean read via Claude Code.

## Arguments

- `/clean-paper <arxiv_id>` — clean one paper (e.g. `2602.03359`).
- `/clean-paper all` — sweep every paper in the library.
- `/clean-paper` (no arg) — ask which paper, or offer `all`.

## Where things live

- **Library root = `$KB_ROOT`. Resolve it FIRST — do not assume `~/arxiv-kb`.**
  The repo exports it in `/Volumes/x/kb/.env.local.sh`
  (`export KB_ROOT=/Volumes/x/arxiv-kb`). Read that file (or `echo $KB_ROOT` if
  already exported) to get the path. A stale copy may exist at `~/arxiv-kb` with
  far fewer papers — writing there will NOT show up in the app. Always use the
  `$KB_ROOT` value.
- Per paper: `$KB_ROOT/<id>/` with `sections.md` (extracted + classified body —
  **the source to rewrite**), `paper.pdf` (fallback), `metadata.json` (title).
- Output: write `$KB_ROOT/<id>/reader.md`. No MCP/engine call needed.

## Procedure (per paper)

1. **Resolve the paper.** First resolve `$KB_ROOT` (see "Where things live").
   With an id, confirm `$KB_ROOT/<id>/` exists. For `all`, list directories under
   `$KB_ROOT/` containing a `metadata.json` (skip `.arxiv-kb/`).
2. **Check for an existing `reader.md`** at `$KB_ROOT/<id>/reader.md` — if present,
   tell the user it will be overwritten and proceed (or skip on request). The
   engine writes the same file, so a stale engine-generated read is fine to replace.
3. **Read the FULL body.** Read `$KB_ROOT/<id>/sections.md` in its entirety —
   page through it; long papers span many pages, do not rewrite from the first
   page alone. If `sections.md` is missing/empty, read `paper.pdf` directly.
4. **Get PDF page boundaries** (for demarcation, see below). Run
   `pdftotext -q "$KB_ROOT/<id>/paper.pdf" -` and split the output on form-feed
   (`\f`) — each chunk is one PDF page, in order. For each page, take its first
   substantial line (>25 chars; skip bare page numbers, running headers, author
   footers) as that page's **start anchor**. Flag pages whose anchors are
   author-name lists — those are References/Bibliography and are dropped from the
   Clean Read. If `pdftotext` is unavailable, fall back to the `meta.db`
   section→page mapping (`SELECT section_type, page, substr(text,1,40) FROM chunks
   WHERE paper_id='<id>' ORDER BY page` and `pdf_toc`).
5. **Rewrite** following the rules below, **inserting page-demarcation markers**
   (see "Page demarcation").
6. **Write** the result to `$KB_ROOT/<id>/reader.md` with the Write tool. Open
   the paper's Clean Read tab in the app to see it (the tab reads the file).
7. For `all`, do them sequentially; end with a one-line summary (cleaned /
   skipped / failed).

## Rewrite rules

- **FAITHFUL REWRITE, not a summary.** Preserve the argument, definitions,
  methods, and every quantitative claim and result. Do not omit steps of
  reasoning or drop technical detail. Length should track the original section,
  not collapse it.
- **Remove EVERY citation marker, in all forms**, regardless of brackets or
  punctuation:
  - parenthetical author-year: `(Bakalova et al., 2025; Geva et al., 2023)`
  - bracketed author-year, including comma-separated lists:
    `[Khattab et al., 2021, Smith, 2025, OpenAI, 2025b, Wu et al., 2025]`
  - bracketed numerics and superscript reference numbers: `[12]`, `[3, 7]`
  - inline narrative citations where the names are the grammatical subject:
    `Wu et al. (2021) show that X` / `as Smith (2025) argues, X` → state the claim
    directly (`X`), dropping the names and year.
  - cross-reference scaffolding: `as shown in Section 4`, `see Figure 2`,
    `Table 3 reports`.
- **Repair the text after every deletion.** When a citation sat mid-sentence,
  delete it AND fix the sentence so it stays grammatical and flows — never leave a
  dangling bracket, a doubled/stranded comma, an empty `()`, or an `and .` gap.
  When a cited result is load-bearing, keep the CLAIM and just drop the marker.
  Never invent or add citations.
- **Drop** the References / Bibliography / Acknowledgments sections entirely.
- **Keep section headings.** Output GitHub-flavored Markdown. Preserve display and
  inline math verbatim (`\(...\)`, `\[...\]`, `$...$`, `$$...$$`) so it still
  renders.
- Output ONLY the rewritten prose into `reader.md` — no preamble, no
  meta-commentary, no notes about what was changed (page markers below are the
  only exception).

## Page demarcation

So the reader can line up the Clean Read with the PDF they're reading, insert a
marker into the prose wherever a new PDF page's content begins, using the page
anchors from step 4. Marker format — its own block, blank lines around it:

```
---

**PDF page N**

```

Rules:
- Place each marker at the nearest paragraph or sentence boundary to where that
  page's anchor content appears. NEVER put a marker inside a math block, a table,
  a fenced block, or a heading line, and never mid-sentence.
- Alignment is APPROXIMATE and that is fine — the Clean Read is a rewrite and PDF
  page breaks usually fall mid-paragraph. Prefer the closest clean boundary over
  splitting text to match exactly. (Optional: phrase as `**PDF page N (approx.)**`.)
- Page 1 is usually the title + abstract; the Clean Read body starts at the
  Introduction. Put a marker/note at the very top for it, e.g.
  `**PDF page 1 — title & abstract**`, then the first body page marker where the
  Introduction begins.
- Do NOT emit per-page markers for pages that are entirely References /
  Bibliography (they are omitted from the Clean Read). Where they fall, emit a
  single note instead, e.g. `**PDF pages 13–14 — References (omitted)**`.
- One marker per page boundary; do not repeat a page number.

## Relationship to the engine reader

The engine's `src/reader.rs` (app "Generate" button) and this skill both write the
same `reader.md`, so they are interchangeable surfaces for the same artifact. This
skill is the preferred path when the user wants Claude Code to do the rewrite; the
engine button remains a no-Claude-session fallback.
