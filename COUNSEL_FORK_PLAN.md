# Counsel — Forking KB into a Legal Contract-Review App

## Context

KB is really a reusable engine: a generic core (OpenAI embeddings, TurboVec
vector index, hybrid RRF + graph-rank retrieval, two-store atomic commit,
streaming SSE, the agent harness, and the entire macOS client) wrapped in a
**research-domain shell**. The research coupling concentrates in exactly one
primitive — the `SectionType` taxonomy + its classifier — plus the features
built on it (Cortex sparks, Problem Hunting, Clean Read prompt) and the arXiv
ingest/watch source adapters.

We are pointing that same engine at **legal contract review**. The product
(working name **Counsel**) helps a solo / small-firm lawyer drop in contracts,
search clauses semantically, ask questions grounded in their own contracts with
citations, and get a per-contract risk review — all **local-first** (privacy is
a professional requirement, which is the moat vs. pasting client contracts into
a cloud chatbot).

### Confirmed decisions
- **Fork into a standalone app.** Copy the repo to a sibling; the research KB
  stays untouched and the two products diverge.
- **MVP = three workflows:** (1) ingest contracts (PDF) + clause-aware search;
  (2) ask-my-contracts RAG chat with clickable citations; (3) per-contract risk
  review (flag risky / non-standard / missing clauses for one contract).
- **Playbook comparison is OUT of MVP** — but keep the clause taxonomy and the
  review schema playbook-friendly so it slots in later.
- **PDF-first ingestion** — reuse the existing PDF + OCR path; DOCX deferred.

> Status: **planning only.** Pick up when ready to start the fork.

## Key architectural finding (governs the whole plan)

The identifier `paper_id` / `arxiv_id` is load-bearing across four layers:
`meta.db` columns (`chunks.paper_id`, `pdf_toc.paper_id`, `paper_tags.paper_id`,
`documents.paper_id`), HTTP route params (`/papers/{paper_id}`), the Rust
`PaperMetadata.arxiv_id` field, and the Swift `PaperMetadata.arxivId` (+ the
`.convertFromSnakeCase` contract).

**Rename only at the two human-facing seams** — the `metadata.json` field
(`arxiv_id` → `contract_id`) and the Swift model. **Do NOT rename the DB columns
or route params** (`paper_id`): it's a schema migration touching every query +
FTS triggers for zero user-visible benefit. Treat internal "paper" vocabulary
(`/papers`, `paper_id`, `PaperInfo`) as engine jargon and leave it.

---

## 1. Clause taxonomy (replaces `SectionType`) — `src/lib.rs`, `src/ingest/sections.rs`

`SectionType` is referenced in 7+ files. **Keep the Rust type name** (optionally
`type ClauseType = SectionType;` for readability), **replace its variants**:

```
Parties, Recitals, Definitions, Term, Payment, Confidentiality,
IpOwnership, Indemnification, LimitationOfLiability, Warranties,
Termination, GoverningLaw, DisputeResolution, Assignment,
ForceMajeure, Notices, Other        + keep UserNotes
```

`as_str()` values snake_case (`limitation_of_liability`, `ip_ownership`, …).
Update `SectionType::ALL` (it's a fixed `[SectionType; 12]` — **change the count
or it won't compile**; `ChunkBuilder.ordinals` is sized from `ALL.len()`).

**`classify_heading()` (sections.rs ~15)** — same case-insensitive, first-match
cascade (the existing numbered-heading tolerance handles "11. Limitation of
Liability"). Order most-specific first — `Termination` before bare `term`,
`LimitationOfLiability` before `liability`/`warranty`:

```
recital|witnesseth|whereas → Recitals;  definition → Definitions;
confidential|non-disclosure|nda → Confidentiality;
indemnif|hold harmless → Indemnification;
limitation of liability|liability cap → LimitationOfLiability;
intellectual property|ownership|\bip\b → IpOwnership;
warrant|representation → Warranties;  terminat → Termination;
governing law|choice of law → GoverningLaw;
dispute|arbitration|jurisdiction|venue → DisputeResolution;
assign → Assignment;  force majeure → ForceMajeure;  notice → Notices;
payment|fees|compensation|invoic → Payment;  term → Term;  part → Parties;
else → Other
```

**`importance_prior()` (lib.rs ~215)** — rank the clauses lawyers scrutinize:
`LimitationOfLiability` / `Indemnification` / `Termination` / `IpOwnership`
high; `Recitals` / `Parties` / `Definitions` low. (`UserNotes` highest.)

**Drop the Abstract special-case** in `build_chunks_with_overrides`
(sections.rs ~340/354): contracts have no abstract; there is no Abstract
variant.

**Cascades:** chunk ids become `acme-msa_indemnification_0`; extend
`Theme.sectionColor` (Swift) for the 17 clause types (`sectionLabel` already
does snake→Title generically); rewrite the sections.rs classifier unit tests
(structure reusable, vocabulary replaced).

## 2. Metadata model — `src/lib.rs` `PaperMetadata` + Swift `API/Models.swift`

- **Rename** Rust field `arxiv_id` → `contract_id` (≈15 constructor sites;
  mechanical). Leave DB columns alone.
- **Drop:** `version`, `categories`, `main_tex`; keep `source_format` dormant
  (always `Pdf`). Repurpose `abstract_text` → optional `summary`.
- **Add** (all `#[serde(default, skip_serializing_if)]` so existing folders need
  no migration — same discipline as today's `source_url`/`project`):
  `contract_type`, `counterparty`, `parties: Vec<String>`, `effective_date`,
  `expiry_date`, `governing_law`, `contract_value`, `summary`.
- Bump `SCHEMA_VERSION` → 2 (lib.rs ~33).
- On-disk layout + canonical/derived invariant unchanged:
  `<root>/<contract_id>/{metadata.json, paper.pdf, sections.md, notes.md,
  reader.md, review.json}`.
- **Swift mirror (Models.swift ~52):** `arxivId`→`contractId` (CodingKeys + `id`
  + inits), drop `version`/`categories`, add the new fields; coordinate
  `PaperInfo` (~149) drops with the Rust `Serialize` struct in
  `src/search/retrieval.rs`.

> **Riskiest part:** Rust↔Swift struct mirroring — `try?` decodes fail silently
> into defaults. **Change Rust serializers first, `curl` the JSON, then mirror
> in Swift.**

## 3. Ingestion — `src/ingest/pipeline.rs`, `sections.rs`

**Primary path = local PDF.** `ingest_local_pdf` / `materialize_local_pdf`
(~135) already validates + copies the PDF, extracts text via
`write_sections_from_pdf` (page-segmented `## Page N` markdown, OCR fallback via
`kb-ocr`), derives a slug, writes metadata + notes, then `index_and_report`.
Reuse all of it; edit only the `PaperMetadata` constructor (~187).

**Central ingestion risk — clause headings aren't markdown headings.** PDF
extraction yields clause titles as inline prose inside `## Page N` blocks. Two
mechanisms, mirroring the existing deterministic-then-LLM design:
- **(a) Heading-promotion pass (backbone):** before chunking, promote lines that
  look like clause headings (ALL-CAPS short lines, or `^\d+\.\s+[A-Z]`) to `###`
  so `split_at_headings` + `classify_heading` work unchanged. New helper beside
  `clean_extracted_markdown`.
- **(b) LLM clause fallback:** extend `try_llm_overrides` (~215) — change the
  prompt from "academic-paper section headings" → "contract clause headings"
  with the new vocabulary (`LLM_TARGETS`). Already wired via
  `config.ingest.classify_with_llm`.

**Delete:** `src/ingest/arxiv.rs`, `src/ingest/latex.rs`, `ingest_paper` (the
arXiv flow ~49), the `prefer_latex`/`pandoc_path` config + pandoc dependency.
**Leave dormant:** `ingest_url` / `src/ingest/html.rs` (cheap future hook; hide
its tab). **Keep:** the inbox `watcher/` (drag-a-PDF-into-a-folder is a good
legal workflow) — but cut arXiv *watches* (§5).

## 4. The three MVP features

**(a) Clause-aware search** — almost free once §1 lands. Reuses `search()`,
hybrid RRF, `/search`, `SearchView`. Wire the clause-type filter + Theme colors;
drop `categories`/`abstract` from results. **Flag:** `default_min_score_narrow`
(0.30) was tuned for academic prose — legal text is more formulaic; smoke-test
and re-tune if relevant clauses get cut.

**(b) Ask-my-contracts chat** — lowest-risk; reuses `/chat/stream`,
`chat_stream`/`prepare_chat`, and the whole Swift `ChatView` + SSE plumbing
unchanged. Two edits:
1. New `DEFAULT_CHAT_SYSTEM` (retrieval.rs ~851): "legal assistant answering
   over the user's contracts… answer ONLY from the numbered sources… cite the
   contract + clause inline, e.g. `[Acme MSA §11.2]`… not a substitute for a
   licensed attorney."
2. Citation format: change the `prepare_chat` source preamble (~929) from
   `[n] "title" — section` to label each source `{title} — {clause_label}` and
   instruct citation by that label. `ChatSource` already carries `title`,
   `section_type`, `page`, `chunk_id` → clickable deep-link via `/open/{chunk_id}`
   already exists.

**(c) Per-contract risk review — the one net-new feature.** Use a
**deterministic-orchestration + single-LLM-call** design (NOT the agent harness:
the input is bounded — all of one contract's clauses, already in meta.db keyed by
`paper_id`). Keep the harness for the future playbook feature.

- **New `src/review.rs`** modeled on `src/reader.rs` (windowing, provider routing
  via `complete_stream_capped`, atomic cache write, streaming `on_delta`). Load
  the contract's chunks grouped by clause type, build one (windowed for long
  contracts) prompt, stream via Claude (`claude-opus-4-8` by id-prefix), cache to
  `<root>/<id>/review.json` (new `KbPaths::review_path` near `reader_path`).
- **Output schema** (`done` payload + cached file):
  ```json
  { "contract_id": "...", "generated_at": "...", "model": "...",
    "findings": [ { "clause": "Limitation of Liability",
      "clause_type": "limitation_of_liability",
      "risk_level": "high|medium|low|missing",
      "why": "...", "suggested_action": "...",
      "chunk_id": "acme-msa_limitation_of_liability_0", "page": 7 } ] }
  ```
  `risk_level: "missing"` covers absent standard clauses (the prompt enumerates
  the expected clause taxonomy) — this is what makes it playbook-ready later.
- **System prompt:** "contract-review assistant… identify risky / non-standard /
  one-sided clauses AND missing standard clauses… return ONLY a JSON array of
  {clause, clause_type, risk_level, why, suggested_action, chunk_id}… cite the
  contract's own language… not a substitute for a licensed attorney."
- **JSON robustness (main risk):** stream raw text for progress, parse the
  accumulated buffer at `done` with a tolerant extractor (generalize
  `extract_json_object`, sections.rs ~257, to arrays); low temperature.
- **New endpoint** `POST /papers/{paper_id}/review` (SSE) — **copy
  `generate_reader_stream` (http.rs ~752) almost verbatim** (same tagged
  `ReviewStreamWire { Reviewing, Delta, Done, Error }`, spawned-task-over-channel,
  `Sse::keep_alive`). Add `GET /papers/{paper_id}/review` (copy `get_reader`) +
  `has_review` on `get_paper` (like `has_reader`).
- **New Swift `Features/Review/ReviewView.swift`** — copy `ReaderView.swift` +
  `KBClient.readerStream` (the SSE consumer is identical). New `Models.swift`
  types `ReviewFinding`/`ReviewResult`/`ReviewStreamWire`. Render findings as
  risk-graded `Card`s (reuse `ScoreBadge`/`Chip`), each deep-linking to its
  `chunk_id` page. Belongs as a **tab on the contract detail** (like the Reader
  tab in `PaperTabs.swift`), since it's per-contract.

## 5. Cut vs. keep-but-hide (keep the MVP lean)

| Feature | Decision |
|---|---|
| Cortex / Sparks | **Cut** — `src/cortex/`, `/sparks`, `SparksView`; arXiv-category signal is meaningless for contracts. |
| Problem Hunting | **Cut** — `/problems`, `tool_find_problems`, `ProblemsView`. |
| arXiv Watches + Daily Brief | **Cut** — `src/watch/`, `/watches*`+`/brief`, `WatchesView`/`BriefView`, `watches`/`watch_candidates` tables. Replace the Brief landing surface with **Library** as home. |
| Roundtable / Personas | **Cut for MVP** — `src/agents/roundtable.rs`, `/brainstorm*`. **Keep `src/agents/harness/`** (future playbook). |
| Explore canvas | **Cut for MVP** — `Features/Explore/`. |
| Graph | **Keep-but-hide** — generic (link/similar edges); a "related contracts" view is plausible later. |
| Library / Search / Chat / Add / Reader | **Keep — the spine.** |

`AppSection` (RootView.swift ~158) shrinks to ~`library, search, add, chat,
review` (+ `graph` hidden). Update its title/subtitle/icon maps and the
`MainView.body` switch (~108) or it won't compile. Sidebar header "Knowledge
Bank" → "Counsel".

## 6. Branding sweep (concentrated)

- `config.rs`: `.arxiv-kb` → `.counsel` (`KbPaths::dot_dir`); default root
  `~/arxiv-kb` → `~/Counsel`.
- `mcp.rs`: `serverInfo.name` `arxiv-kb` → `counsel`; tool descriptions
  "papers/arXiv" → "contracts"; cut `kb_find_problems`/`kb_create_reflection` if
  reflections go.
- `pipeline.rs ~35`: HTTP user-agent `arxiv-kb/…` → `counsel/…`.
- `Info.plist`: bundle id `com.sunprema.kb` → `com.sunprema.counsel`; app name
  "KB"/"Knowledge Bank" → "Counsel"; menu-bar icon `books.vertical.fill` →
  `doc.text.magnifyingglass`.
- `ServerController.swift ~89–148`: root path; failure copy `kb add <arxiv-id>`
  → `kb add --pdf <file>`.
- `AddView.swift`: remove arXiv + URL tabs; PDF-drop is the primary tab.
- Long tail: `grep -ri "paper\|arxiv\|research" macos/Sources`.

Keep the Rust binary name `kb` for MVP (renaming cascades into the Swift launch
path) — rename in a later cosmetic pass.

## 7. Sequencing & verification

- **Phase 0 — Fork & compile-green skeleton.** `cp -R /Volumes/x/kb
  /Volumes/x/counsel`; repoint git remote; fresh `main`. Bump `SCHEMA_VERSION`.
  Delete arxiv/latex/cortex/watch/roundtable + their routes/call-sites until
  `cargo build` is green. (Rust deletions before Swift — engine must build first.)
- **Phase 1 — Taxonomy + metadata (engine).** Replace `SectionType` variants,
  rewrite `classify_heading` + heading-promotion pass, update `importance_prior`,
  drop the Abstract special-case, update `PaperMetadata` + the
  `materialize_local_pdf` constructor, rewrite classifier tests. `cargo test`.
- **Phase 2 — Vertical slice via CLI/curl (no UI).**
  `kb add --pdf sample-contract.pdf` → check `<root>/<id>/` artifacts +
  `sqlite3 ~/.counsel/meta.db "select distinct section_type from chunks"`.
  `kb serve`, then `curl -H "X-KB-Key: $(cat ~/.counsel/api_key)" -d
  '{"query":"liability cap","mode":"narrow"}' localhost:<port>/search` and a
  `/chat/stream` legal question → verify clause-typed hits + cited answers.
- **Phase 3 — Review feature (engine).** Add `src/review.rs`, `POST/GET
  /papers/{id}/review`, `has_review`. `curl -N …/papers/<id>/review` → streamed
  JSON findings + cached `review.json`.
- **Phase 4 — macOS app.** Mirror metadata + review models in `Models.swift`;
  extend `Theme.sectionColor`; shrink `AppSection` + `MainView` switch; build
  `Features/Review/ReviewView.swift` + `KBClient.reviewStream`; fix
  `ServerController` paths/copy; delete cut feature folders; rebrand. Build via
  the project's `macos/build.sh`.
- **Phase 5 — Branding polish.** Dot-dir/root/MCP/bundle-id/user-agent/icons,
  app name, the grep-driven string tail.

**End-to-end:** launch the app → it spawns `kb serve` against `~/Counsel` →
drag-drop a PDF in Add → see it in Library with typed clauses → search a clause
term → chat a question with `[Contract §X]` citations → open a contract → Review
tab streams risk findings, each deep-linking to its PDF page.

## Riskiest / most-uncertain parts
1. **Clause heading detection from PDF text** (§3) — heuristic, contract-format
   dependent; validate early with real contracts, LLM fallback is the safety net.
2. **Review JSON robustness** (§4c) — malformed/partial model JSON; low temp +
   tolerant extraction.
3. **Rust↔Swift struct drift** (§2) — silent `try?` decode; curl-verify before
   mirroring.
4. **Score-floor recalibration** for legal text (§4a).
5. **`SectionType::ALL` fixed-size array** — update the length constant.

## Critical files
- `src/lib.rs` — clause enum, `PaperMetadata`, `importance_prior`, `SCHEMA_VERSION`
- `src/ingest/sections.rs` — `classify_heading`, heading-promotion, LLM fallback
- `src/ingest/pipeline.rs` — PDF ingest path (reuse), delete arXiv flow
- `src/search/retrieval.rs` — `DEFAULT_CHAT_SYSTEM`, `prepare_chat` citations
- `src/reader.rs` — structural template for new `src/review.rs`
- `src/server/http.rs` — router; copy reader-stream → review-stream SSE handlers
- `macos/Sources/API/Models.swift` — Rust↔Swift mirror; new review + metadata types
- `macos/Sources/App/RootView.swift` — `AppSection` trim
- `macos/Sources/Design/Theme.swift` — clause color/label map

## Deferred (post-MVP)
- **Playbook comparison** — the biggest differentiator; the review schema's
  `missing` risk-level + expected-clause list are already the hook. Likely uses
  the retained agent harness.
- DOCX ingestion (pandoc `docx→md`); URL ingest (already dormant).
- Renaming internal `paper`/`/papers` vocabulary to `contract`.
