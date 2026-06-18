# NEW_SWIFT_FEATURES.md

Roadmap of features to make `kb-mac` (see [`LOCAL_UI_PRD.md`](./LOCAL_UI_PRD.md)) a
world-class research reader. Check a box when the feature ships. Grouped by the
three loops a great reader must nail — **read**, **connect**, **capture** — plus
native ergonomics and output.

> Status legend: `[ ]` planned · `[x]` shipped · `[~]` partial / follow-ups remain

---

## 0. The differentiator — annotation → notes loop  `[~]`

The killer feature: annotations flow back into the searchable KB.

- [x] Select text in the PDF → **Add to KB Notes** (quoted + page citation), re-embedded by the engine
- [x] Visual **highlight** on the PDF selection (in-session)
- [x] Notes panel refreshes to show the new note immediately
- [x] **Persist highlights** across sessions (JSON sidecar in Application Support; never touches `paper.pdf`; right-click a highlight to remove)
- [x] Select text → **"Explain this"** (passage → chat model → plain-language sheet, with read-aloud)
- [ ] Highlight → margin note tied to a specific chunk

## 1. Read — comprehension

- [x] **Reader mode** — reflowable typographic view of `sections.md` (adjustable font, light/dark)
- [x] **Math rendering** — render LaTeX in the reader via KaTeX (GitHub `` $`…`$ `` + `$$`/`\(`/`\[`)
  - [ ] _follow-up:_ bundle KaTeX/marked assets for offline rendering (currently CDN-loaded)
- [x] **Outline / section navigation** — native rail parsed from headings; click to scroll the reader
- [ ] **Reading progress & resume** — remember scroll position per paper; "continue reading" shelf
- [ ] **Figures & tables** — a per-paper figure gallery / jump-to-figure
- [ ] **Speech upgrades** — word-synced highlighting, skip-by-section, "listen to unread" queue, background playback

## 2. Connect — synthesis (KB's unfair advantage)

- [x] **Native knowledge graph** — interactive render of `/graph` (PPR edges); click node → open paper
- [ ] **Multi-paper synthesis** — select N tabs → "synthesize across these" → cited answer
- [x] **Connections panel** per paper — Similar + explicit Links + Sparks, segmented; click to open
- [ ] **Today / serendipity feed** — new sparks, papers to revisit, surfaced daily
- [ ] **Question-driven reading** — ask → answer + exact passages → open in reader (deepen existing chat→reader)

## 3. Capture — close the loop

- [x] **Editable notes** — live markdown editor + debounced preview; ⌘S saves via `PUT /notes` (overwrite + re-embed)
- [ ] **Ideas & reflections** capture in-app (engine endpoints exist), with `[[id]]` linking
- [ ] **Global quick-capture hotkey** (⌘⇧K) — capture an idea / save a link from anywhere
- [ ] **Tag / project management** — edit tags in-app, filter by tag/category/project

## 4. Native-Mac ergonomics

- [ ] **Command palette (⌘K)** — fuzzy-jump to any paper, search, or action
- [ ] **Keyboard everything** — ⌘W close tab, ⌘1–9 switch tabs, ⌘\ toggle split
- [ ] **Spotlight integration** — papers/notes as `CSSearchableItem`s
- [ ] **Drag-and-drop ingest** + **Share extension** ("Send to KB" from Safari/Preview)
- [ ] **Persist split divider position** and per-pane PDF state
- [ ] **Menu-bar quick search** + optional widget ("today's spark")

## 5. Output — research workflow

- [ ] **Citation export** — BibTeX / copy citation / send to Zotero
- [ ] **Excerpt / compile** — assemble selected sections into one reading PDF (engine `kb excerpt`)
- [ ] **Cite-while-writing** — writing surface backed by `/compose/assist`, pulling citations from the corpus
- [ ] **Export notes/highlights** to Markdown / Obsidian

---

## Enablers (not UI, but they gate quality)

- [ ] **Section classifier** — corpus is ~69% `other`; section-based features (outline, chips,
      section-filtered search) stay weak until improved (engine; PRD §16 — LLM pass above ~25%)
- [ ] **`kb serve` machine-readable port/key** on stdout, so the app needn't scrape stderr
- [ ] **Local embeddings** (engine v0.3) — make search/chat work without an OpenAI key
