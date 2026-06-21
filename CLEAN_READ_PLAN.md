# Clean Read — Faithful, Citation-Free Reader Document (`reader.md`)

## Context

Research papers are cluttered with inline citations ("Bakalova et al., 2025;
Geva et al., 2023"), bracketed numeric refs ("[12]"), and cross-reference
scaffolding ("as shown in Section 4", "see Figure 2"). For a reader trying to
follow the actual argument, this is noise. The goal is to produce a **clean,
readable rewrite** of a paper — the same content and technical substance, with
the citation/cross-ref clutter removed — and store it as a derived "reader
document" (`reader.md`) alongside the PDF. It is **not embedded** into the
vector index; it's purely a reading surface.

This fits the existing derived-file model (`sections.md`, `notes.md` already
live in `<root>/<id>/`) and the existing Reader UI (which already renders
markdown via WKWebView + marked.js + KaTeX).

### Decisions (confirmed with user)
- **Style:** Faithful clean rewrite — section-by-section, preserves all
  technical detail, definitions, and quantitative claims. NOT a summary.
- **Model:** Chosen in the UI per request and passed in the request body
  (reuse the existing model picker; provider routing already dispatches
  `claude-*` → Anthropic vs. OpenAI by model-id prefix).
- **Trigger:** On-demand (button in Reader mode), cached to disk and reused on
  reopen. Matches the PRD's anti-precompute stance.

---

## Backend (Rust)

### 1. New path — `src/config.rs`
Add next to `sections_path` (line 473) / `notes_path` (476):
```rust
pub fn reader_path(&self, arxiv_id: &str) -> PathBuf {
    self.paper_dir(arxiv_id).join("reader.md")
}
```
Existence is detected by `reader_path(id).exists()` — **disk-check only**, no
`PaperMetadata` field (no schema bump). Precedent: `link_target` (config.rs:491)
branches on `pdf_path.exists()`; `ReaderView` reads `sections.md` from disk.

### 2. Per-call output token budget — `src/anthropic.rs`
`const MAX_TOKENS: u32 = 2048` (line 119) truncates a full clean read. **Do not
mutate the shared const** (it governs roundtable/chat budgets). Instead add a
per-instance cap:
```rust
pub fn with_max_tokens(mut self, n: u32) -> Self { self.max_tokens = n; self }
```
Add a `max_tokens` field (default = current 2048 in `new`) and read
`self.max_tokens` in the three request builders (lines ~221, 264, 376). Mirror
in `src/chat.rs` `OpenAiChat` for the OpenAI path. This path requests ~4096.

### 3. Generation function — new module `src/reader.rs`
```rust
pub async fn generate_reader<F: FnMut(&str)>(
    paths: &KbPaths, config: &Config,
    paper_id: &str, model: &str, mut on_delta: F,
) -> Result<String, KbError>
```
Logic:
1. `NotFound` if `metadata_path(id)` or `sections_path(id)` missing.
2. **Segment** `sections.md` by top-level headings (`^#{1,2}\s` — same detection
   as `ReaderView.parseOutline`). Merge adjacent `## Page N` segments (PDF-only
   papers) into ~2–4k-token windows. **Skip** References / Bibliography /
   Acknowledgments segments.
3. For each segment, build the client **directly** (branch on
   `model.starts_with("claude")`, as `retrieval::chat_stream` does at
   retrieval.rs:965–973) so `.with_max_tokens(4096)` applies; call
   `complete_stream`, forwarding deltas to `on_delta`. Section-wise generation
   solves both the long-input problem (sections.md can be 30k+ tokens) and
   output truncation, and gives natural streaming progress.
4. **Atomic write:** accumulate full text, write to `reader.md.tmp`, rename to
   `reader_path(id)` only on full success. An interrupted stream never leaves a
   partial canonical file (leave any prior `reader.md` intact; remove tmp on
   error).

#### Prompt (inline const in `src/reader.rs`, mirroring `DEFAULT_CHAT_SYSTEM`)
- Role: "Rewrite a section of a research paper into clean, readable prose."
- Faithfulness: "Preserve the technical substance, argument, definitions, and
  quantitative claims exactly. This is a faithful rewrite, NOT a summary — do
  not omit steps or drop detail."
- Citation stripping: "Remove parenthetical author-year citations, bracketed
  numeric refs, and cross-reference scaffolding ('as shown in Section 4', 'see
  Figure 2'). Keep load-bearing *claims* in prose; drop the citation marker. Do
  not invent citations."
- Formatting: "Keep the section heading. Output GitHub-flavored markdown.
  Preserve display math (`$$...$$`, `\[...\]`) verbatim. No preamble ('Here is
  the rewrite'), no commentary — output only the rewritten prose."

### 4. HTTP routes — `src/server/http.rs`
In `router()` protected group (line ~106):
```rust
.route("/papers/{paper_id}/reader", post(generate_reader_stream).get(get_reader))
```
- **`generate_reader_stream`** (SSE, modeled on `chat_stream` lines 559–595):
  read `model` from the JSON body (default to `config.chat.model`), spawn a task
  emitting a new `ReaderStreamWire` (add next to `ChatStreamWire` at line 544):
  `Generating` → `Delta { text }`* → `Done { reader }` | `Error { message }`.
  Missing API key or missing `sections.md` surface as an `Error` event (not a
  500), matching `chat_stream`'s contract. Async/network-bound → run directly in
  the spawned task (no `run_blocking`).
- **`get_reader`** (GET): return `{ "reader": "<markdown>" }` from `reader.md`,
  404 if not yet generated.
- In `get_paper` (lines 340–355) add `"has_reader": paths.reader_path(&id).exists()`
  (single `exists()`, no read) so the UI knows generate-vs-view.

---

## Swift UI

### 5. `KBClient` — `macos/Sources/API/KBClient.swift`
- `func reader(_ id: String) async throws -> String?` — GET, body or nil on 404.
- `func readerStream(_ id: String, model: String) -> AsyncThrowingStream<ReaderStreamEvent, Error>`
  — POST with `{ "model": model }` body; SSE consumer copied from `chatStream`
  (lines 83–138): same `data:` framing, decode `ReaderStreamWire`, yield
  `.generating` / `.delta(text)` / `.done(reader:)`, throw on `.error`. Set
  `req.timeoutInterval = 600`.
- Add `ReaderStreamEvent` enum + `ReaderStreamWire` decodable next to the
  `ChatStream*` types. Add `hasReader: Bool` to `PaperDetail` (snake_case maps
  `has_reader`).

### 6. Clean-read toggle — `macos/Sources/Features/Reader/ReaderView.swift`
`ReaderView` already reads `sections.md` and renders via `ReaderWebView`. Add:
- `@State cleanRead`, `readerMarkdown: String?`, `generating`, `genProgress`.
- A segmented control in `fontBar` (lines 52–66): **Original** vs **Clean read**.
- A **model picker** for the generate action — reuse the `LLMModel` enum from
  `macos/Sources/Features/Roundtable/RoundtableModels.swift` (claude-opus-4-8,
  claude-sonnet-4-6, claude-haiku-4-5; default Opus).
- In `load()` (lines 98–107): when `cleanRead`, read `kbRoot/{id}/reader.md`
  from disk (same pattern as sections.md). If absent → show a "Generate clean
  read" affordance (button + model picker) instead of an error.
- Generate action: `client.readerStream(id, model:)`, accumulate `.delta` into
  `readerMarkdown` fed live to `ReaderWebView` (re-renders on change,
  `updateNSView` line 151). On `.done`, file is on disk; later toggles read disk.
  `parseOutline` recomputes from generated markdown. No renderer change — math
  preserved by the prompt, marked.js + KaTeX already handle it.
- `ReaderView` needs a `KBClient`: pass `client:` in from `PaperDetailView`.

### 7. `PaperDetailView` — `macos/Sources/Features/Library/PaperDetailView.swift`
Line 87 `ReaderView(paperId:title:)` → add `client:` arg (already holds `client`
at line 93). Optionally use `detail.hasReader` to label the reader toolbar
button.

---

## Critical files
- `src/config.rs` — `reader_path`
- `src/anthropic.rs` (+ `src/chat.rs`) — per-call `with_max_tokens`
- `src/reader.rs` (new) — `generate_reader` + prompt + segmenter
- `src/server/http.rs` — routes, `ReaderStreamWire`, `get_reader`, `has_reader`
- `macos/Sources/API/KBClient.swift` — `reader`, `readerStream`, `hasReader`
- `macos/Sources/Features/Reader/ReaderView.swift` — toggle, model picker, stream
- `macos/Sources/Features/Library/PaperDetailView.swift` — pass `client`

## Trickiest parts
1. **Token limits** — per-call `with_max_tokens` (not the shared const) +
   section-wise generation so no single call truncates.
2. **Long inputs** — segment `sections.md` by heading; also yields streaming
   progress and avoids "lost in the middle."
3. **Streaming a file write** — tmp + rename only on success.
4. **Custom token cap** — build the provider client directly in `generate_reader`
   (roundtable helpers construct their client internally and ignore the cap).
5. **Faithful, not summary** — prompt forbids summarization/preamble, preserves
   `$$` math verbatim.

## Verification
1. `cargo build` / `cargo test` (unit-test the segmenter; mirror the SSE mock
   tests in `anthropic.rs` ~line 459 for a streaming reader test).
2. `cargo run -- serve` with `ANTHROPIC_API_KEY` exported.
3. `cargo run -- add <arxiv-id>`; confirm `<root>/<id>/sections.md` exists.
4. SSE generate:
   `curl -N -H "X-KB-Key: $KEY" -X POST -d '{"model":"claude-opus-4-8"}' \
     http://127.0.0.1:<port>/papers/<id>/reader`
   — observe `generating` → `delta`… → `done`; confirm `reader.md` written with
   citations stripped and math preserved.
5. `curl -H "X-KB-Key: $KEY" http://127.0.0.1:<port>/papers/<id>/reader` returns
   the body; `GET /papers/<id>` shows `"has_reader": true`.
6. Atomicity: kill server mid-stream → no truncated `reader.md` (only `.tmp` or
   prior file intact). Re-POST overwrites.
7. App (build via `swiftc -parse-as-library`, macOS 14): open paper → Reader →
   pick model → "Generate clean read" → watch live stream → rendered markdown,
   no citation clutter, math renders; reopen → loads instantly from disk.
8. Negatives: unset API key → `error` SSE event (not 500); paper without
   `sections.md` → `NotFound` as error event.
