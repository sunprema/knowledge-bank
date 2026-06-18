# LOCAL_UI_PRD.md

**Project:** `kb-mac` — a polished native macOS front-end for the KB engine.

**Document status:** Draft v0.1
**Last updated:** June 2026
**Audience:** Claude Code (primary implementer), the author
**Distribution:** Signed `.app` bundle (single user, not the App Store — for now)

> This document governs the **Swift/macOS UI only**. The KB engine, its
> ingest pipeline, retrieval, and persistence remain governed by
> [`KB_PROD_REQUIREMENTS.md`](./KB_PROD_REQUIREMENTS.md) and
> [`KB_PERSISTENCE_ADDENDUM.md`](./KB_PERSISTENCE_ADDENDUM.md). Where the two
> overlap, the engine docs win on engine behavior; this doc wins on UI/UX.

---

## 1. Executive summary

### Context

The KB engine is a single Rust binary that ingests papers/notes/ideas, embeds
them section-by-section into turbovec, and exposes search, synthesis, a
knowledge graph, and a persistent associative layer (Cortex/sparks). It already
speaks three protocols: CLI, MCP (for Claude), and a loopback HTTP API
(`kb serve`).

`kb-mac` is a **native macOS client over that HTTP API**. It does not
reimplement any engine logic. It exists to answer one question:

> *Is there a viable product in a beautiful, local-first personal knowledge
> bank — one that does things a browser tab can't?*

### v0.1 is a consumption surface, not a capture tool

The focus is squarely on **consuming** the knowledge already in the bank —
searching, reading, listening, chatting over it, and exploring its connections.
Getting knowledge *in* stays with the CLI (`kb add`, `kb note`) and the watcher;
the app does not ingest arXiv papers and does not try to be a capture inbox in
v0.1. Every screen earns its place by making the corpus more *legible*, not by
adding ways to write to it. Light mutation (appending a note to a paper you're
reading) is a fast-follow, not part of the first cut.

### Why native, why now

This is a single-user experiment (the author is the only user). The bet is that
escaping browser restrictions unlocks experiences worth the native cost:

- **Read my summaries aloud** — system text-to-speech, background playback.
- **A real PDF reader** — PDFKit with true page deep-linking, sidestepping the
  `file://` vs `http://` deep-link problem the web UI has to work around.
- **Capture from anywhere** — global hotkey + menu-bar item, share extension.
- **System integration** — Spotlight, notifications, Quick Look.

If these don't feel meaningfully better than the existing web UI within a few
weekends of use, that's a valid (and cheap) negative result.

### Non-negotiables inherited from the engine

- **Local-first, loopback-only.** No cloud, no telemetry, no account.
- **Files are forever; indexes are disposable.** The UI never writes to the
  canonical paper folders directly — all mutations go through the engine's
  API (`/papers/{id}/notes`, `/ideas`, `/reflections`).
- **The engine is the source of truth.** The UI holds no durable state beyond
  window layout, the server port, and the API key (in Keychain).

### Success criteria

After a few weeks of the author's own daily use, `kb-mac` has succeeded if:

- It is the author's *default* way to query the KB, over the web UI and CLI.
- "Read this aloud" gets used for real — on search answers or paper summaries.
- Opening a result lands in the right PDF at the right page, every time.
- Capturing an idea via global hotkey is faster than `kb` in a terminal.
- Launching the app "just works" — no manually running `kb serve` first.

### Explicitly out of scope (v0.1)

- App Store distribution, multi-user, sync, sharing.
- PDF annotation/editing (reading + highlighting matched chunks is enough).
- Reimplementing ingest UI — `kb add` from the CLI is fine to start; in-app
  capture is for notes/ideas/reflections, not arXiv ingest (fast-follow).
- Windows/Linux. This is macOS/SwiftUI only.

---

## 2. Architecture

### The shape: native shell over the bundled engine

```
KB.app
├── Contents/MacOS/KB              # the SwiftUI app
├── Contents/Resources/kb         # the bundled Rust engine binary
└── (talks to) ─► kb serve --port <random loopback> ─► 127.0.0.1
```

The app **owns the lifecycle of a `kb serve` child process**:

1. On launch, pick a free loopback port, read/resolve the API key, spawn
   `kb serve --port <p>` as a child process with the right `--root` and
   `OPENAI_API_KEY` in its environment.
2. Poll `GET /health` until ready (with a visible "starting…" state).
3. Hold the API key in memory; send it as `X-KB-Key` on every request.
4. On quit, terminate the child cleanly (it already handles SIGINT →
   graceful shutdown).

Rationale (see brainstorm): this reuses 100% of the engine — turbovec,
HippoRAG PageRank, Cortex, ingest — with one well-understood process boundary.
No FFI surface to maintain, no logic duplicated, and the web UI and Mac app
stay feature-equivalent because both are just clients of the same server.

### Why not FFI / a Swift rewrite

- **FFI (uniffi/cbindgen):** tighter, but requires designing and versioning a
  stable C ABI that has to track engine changes. Not worth it for an experiment.
- **Pure Swift port:** would reimplement turbovec + PageRank + Cortex and they
  would drift. Rejected.

If the experiment proves out and HTTP latency or process management becomes a
real pain, FFI is the documented escalation path — not v0.1.

### Server discovery & coexistence

- Default: the app spawns and manages its *own* `kb serve` on a private port.
- Optional "attach" mode: if `KB_MAC_ATTACH=127.0.0.1:<port>` is set, the app
  connects to an already-running server instead of spawning one (useful when a
  `kb watch` / `kb serve` is already up during development).
- The app must tolerate the engine refusing to start on an out-of-sync index
  (the engine fails fast per addendum §7) — surface that error with a
  "run `kb reindex`" call to action, not a spinner that never resolves.

### Config the app needs

| Setting | Source | Storage |
|---|---|---|
| KB root | env `KB_ROOT` or picker on first run | `UserDefaults` |
| API key | engine's `.arxiv-kb/api_key`, or `KB_API_KEY` | Keychain |
| `OPENAI_API_KEY` | onboarding prompt (search/chat/ingest need it) | Keychain |
| Server port | chosen at launch | in-memory |

---

## 3. The API contract (consumed, not defined here)

The app consumes the existing `kb serve` surface (`src/server/http.rs`). It
defines **no new endpoints** in v0.1; if the UI needs something the API can't
give, that's an engine change with its own PR, noted in §8.

Endpoints used:

```
GET  /health                          # readiness gate at launch
GET  /stats                           # corpus dashboard
GET  /papers            ?tag= &category=
GET  /papers/{id}                     # metadata + notes + pdf_path
POST /papers/{id}/notes               # append a note (re-embeds)
GET  /papers/{id}/similar  ?limit=    # Related panel
POST /search                          # {query, mode, k, filters...}
POST /chat                            # {query, history[]} → answer + sources
POST /compose/assist                  # draft + message → assisted text
GET  /graph             ?neighbors=   # nodes + edges for the graph view
GET  /sparks            ?limit= &kind= # Cortex associative connections
POST /ideas                           # capture an idea
POST /reflections                     # capture a reflection
GET  /chunks/{id}                     # full chunk text
GET  /pdf/{id}                        # PDF bytes (PDFKit loads this directly)
GET  /open/{chunk_id}                 # 302 to deep link (CLI path; UI prefers /pdf)
```

### Response shapes the UI binds to (mirror of the Rust `Serialize` structs)

```
SearchResponse { query, mode, papers: [PaperGroup], total_chunks }
PaperGroup     { paper_id, best_score, matched_sections: [String],
                 chunks: [ChunkHit], paper: PaperInfo, tags: [String] }
ChunkHit       { chunk_id, section_type, score, snippet, page?, deep_link }
PaperInfo      { kind, project?, title, authors, abstract, categories, published_at }

ChatResponse   { answer, sources: [ChatSource] }
ChatSource     { n, paper_id, title, section_type, page?, chunk_id, snippet, has_pdf }

SimilarResponse{ paper_id, papers: [SimilarPaper] }
GraphResponse  { nodes: [GraphNode], edges: [GraphEdge] }
GraphNode      { id, title, kind, project?, tags, categories, published_at, chunks }
GraphEdge      { source, target, kind: "link"|"similar", weight }
```

Define these as `Codable` Swift structs in a single `KBAPI` module. Keep them in
lockstep with the Rust definitions in `src/search/retrieval.rs`; a drift here is
the most likely source of bugs.

---

## 4. The native superpowers (the reason this exists)

Ranked by "is this actually better than the web UI?"

### 4.1 Speech readout — `AVSpeechSynthesizer`

The motivating feature. Any block of text — a chat answer, a paper abstract, a
paper's notes, or a queued "listen to today's sparks" — can be read aloud.

- Play/pause/skip transport in a persistent mini-player.
- Sentence-level highlighting synced to speech where feasible.
- Voice + rate configurable; remembers last choice.
- Keeps playing when the window is backgrounded.

### 4.2 Native PDF reader — `PDFKit`

Loads `GET /pdf/{paper_id}` directly into a `PDFView`. This is the single most
satisfying native win and it **dodges the engine's deep-link limitation**: the
web UI can't open `file://` links from an `http://` page (see `http.rs` notes),
so the app uses the served PDF bytes + `page` from the chunk hit to jump
precisely. Stretch: highlight the matched chunk's text on the page.

### 4.3 Capture from anywhere

- Global hotkey (⌘⇧K) opens a lightweight capture/search palette over any app.
- Menu-bar item for quick search + "new idea."
- Share extension: "Send to KB" an arXiv URL/ID (routes to `kb add` —
  fast-follow once in-app ingest exists).

### 4.4 System integration (fast-follow, not v0.1 MVP)

- Spotlight: index papers/notes as `CSSearchableItem`s.
- Local notifications: "N new sparks surfaced."
- Quick Look support for results.

### 4.5 Native knowledge graph (fast-follow)

The web UI already has a 3D force-directed graph. A native SceneKit/Metal
version (binding `GraphResponse`) would pan/zoom more smoothly, but it is **not**
MVP — the web graph is good enough to defer this.

---

## 5. Information architecture / screens

```
┌─ Sidebar ──────┬─ Main ───────────────────────────────┐
│ Search         │  (content for the selected mode)      │
│ Chat           │                                       │
│ Library        │                                       │
│ Graph          │                                       │
│ Sparks         │                                       │
│ ───────        │                                       │
│ Now Playing ▸  │  (speech mini-player, when active)    │
└────────────────┴───────────────────────────────────────┘
```

- **Search** — query box (narrow/wide toggle, section/tag/kind filters) →
  paper-grouped results (`PaperGroup` cards). Click a chunk → PDF reader at its
  page. "Read aloud" on any result.
- **Chat** — chat-over-corpus (`/chat`); answer renders `[n]` citations that
  link to `ChatSource` → PDF. "Read answer aloud."
- **Library** — `/papers` browse with tag/category filters; detail view shows
  metadata + notes (read-only in v0.1; editing via `POST /papers/{id}/notes` is
  a fast-follow) + Related (`/similar`). "Read notes/abstract aloud."
- **Graph** — web graph embedded in a `WKWebView` for v0.1; native later.
- **Sparks** — `/sparks` feed of cross-document connections, most surprising
  first; each spark links to its two endpoints. "Read today's sparks aloud."

---

## 6. MVP scope & build order

Build in slices, each independently usable. Riskiest plumbing first.

1. **Server lifecycle** — spawn/health-gate/teardown bundled `kb serve`;
   Keychain for keys; onboarding for KB root + `OPENAI_API_KEY`. *(highest risk)*
2. **`KBAPI` client module** — `Codable` structs + async HTTP with `X-KB-Key`.
3. **Search + results list** — the bread and butter.
4. **PDFKit reader pane** — click result → open at the right page.
5. **Speech readout** — the motivating feature, on selected text / answers.
6. **Chat-over-corpus** — wired to `/chat` with citation links.

Fast-follows (post-MVP): Library/notes editing, Sparks feed, global-hotkey
capture, Spotlight, native graph, in-app arXiv ingest.

---

## 7. Distribution & operational notes

Even as a single-user experiment, the app must launch cleanly:

- **Code-signing & notarization** — required so macOS doesn't block the bundled
  binary / child-process spawn. Developer ID, hardened runtime; the child
  process spawn and network entitlements must be declared.
- **Bundled engine binary** — build `kb` for the host arch (arm64 first), copy
  into `Contents/Resources`, mark executable. Document the build step.
- **`pandoc` dependency** — ingest needs it (engine §12). Not required for the
  MVP (which only queries), but in-app ingest later must detect it and guide
  install, exactly as the CLI does.
- **Keys never logged.** `OPENAI_API_KEY` and the KB API key live in Keychain;
  never written to `UserDefaults`, never in logs, never in crash reports.

---

## 8. Likely engine changes this surfaces

Tracked here so UI work doesn't silently fork the engine:

- **Spawn ergonomics:** `kb serve` should optionally print its chosen
  port/key as machine-readable JSON on stdout (or accept `--port 0` and report
  the bound port) so the app doesn't have to scrape stderr.
- **Local embeddings (engine v0.3):** the whole thing depends on
  `OPENAI_API_KEY`. For a frictionless personal tool, landing the fastembed-rs
  local path would let search/chat work key-free. Not a UI task, but the UI is
  the forcing function.
- **Chunk highlight data:** highlighting the matched chunk in the PDF may need
  the chunk's text span/coords; today only `page` is returned.

---

## 9. Open questions

- [x] **In-app arXiv ingest in v0.1?** No — resolved CLI-only. v0.1 is a
      consumption surface (§1). `kb add` / `kb note` / the watcher own getting
      knowledge in; the app only reads. Revisit in-app capture as a fast-follow.
- [ ] Does the app *manage* the server, or assume the user runs `kb serve`?
      (PRD assumes it manages; "attach" mode is the escape hatch.)
- [ ] One KB root, or a switcher for multiple roots (the engine supports
      `--root`)?
- [ ] Web graph in `WKWebView` vs. native graph — confirm the embed is good
      enough to defer native work.
