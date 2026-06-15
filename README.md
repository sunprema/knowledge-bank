# Knowledge Bank

A single Rust binary (the `kb` CLI) that turns a folder of arXiv papers into a
queryable, AI-friendly knowledge base: save a paper in one command,
search across your corpus semantically, let Claude synthesize across
papers via MCP, and browse and analyze everything in a local web app —
with every claim deep-linked back to the source PDF.

▶ **[Watch the 75-second cinematic demo](https://sunprema.github.io/knowledge-bank/)** —
served by GitHub Pages from [`docs/`](./docs/index.html).

Full design: [KB_PROD_REQUIREMENTS.md](./KB_PROD_REQUIREMENTS.md) and
[KB_PERSISTENCE_ADDENDUM.md](./KB_PERSISTENCE_ADDENDUM.md).
Configuration, env vars, and the portable USB-drive setup:
[CONFIG.md](./CONFIG.md).

## Setup

Requirements:

- [pandoc](https://pandoc.org/installing.html) on PATH (LaTeX → markdown)
- `OPENAI_API_KEY` exported (embeddings: `text-embedding-3-small`)

```bash
cargo install --path .
kb init                      # creates ~/arxiv-kb (override: --root / KB_ROOT)
```

## Commands

Every command accepts `--root <path>` (or `KB_ROOT`) and
`--format pretty|json`.

### Capturing papers

```bash
kb add 2504.19874
kb add https://arxiv.org/abs/2504.19874     # URLs work too
kb add --pdf "Attention Is All You Need.pdf"   # any local PDF
kb add --url https://simonwillison.net/2024/Dec/31/llms-in-2024/   # any web page
```

`kb add` does the whole ingest in one shot (~15-20s): fetches metadata
from the arXiv API, downloads the LaTeX source and the PDF, converts
the LaTeX to markdown via pandoc, classifies it into typed sections
(abstract, method, limitations, future_work, …), embeds each section
separately, and indexes them. Papers without LaTeX fall back to PDF
text extraction. It also creates a `notes.md` template for you.

`kb add --pdf` ingests a PDF that isn't on arXiv. Its id is the
slugified filename (`Attention Is All You Need.pdf` →
`attention-is-all-you-need`) and works everywhere an arXiv id does
(`kb note`, `kb tag`, `kb show`, `kb open`, …). The title comes from
the PDF's own metadata when present, else from the filename; there's
no arXiv metadata to fetch, so `kb update` doesn't apply — re-add the
file after `kb remove` if it changes.

`kb add --url` ingests a web page. It fetches the page, extracts the
main article with a readability pass (stripping nav/ads/boilerplate),
converts it to markdown, and indexes it into typed sections like a
paper. Its id is a slug of the URL (`example.com/post` →
`example-com-post-a3f9c2`, the hash suffix keeping distinct URLs
distinct), and the page URL is recorded as the document's canonical
identity — so `kb update <id>` re-fetches it. Works everywhere an
arXiv id does. No PDF is downloaded.

```bash
kb update 2504.19874     # re-fetch (paper got a new arXiv version); keeps your tags
kb remove 2504.19874     # delete from the index AND delete the folder (asks first)
```

### Drop-folder (inbox)

While `kb watch` is running, anything dropped into `<root>/inbox/` is
ingested automatically:

```bash
cp "Some Paper.pdf" "$KB_ROOT/inbox/"     # → ingested like `kb add --pdf`
echo "https://example.com/post" > "$KB_ROOT/inbox/links.txt"   # one URL per line
```

A `*.pdf` is ingested as a local PDF; a `*.url` or `*.txt` is read as a
list of URLs (one per line, `#` comments allowed), each ingested like
`kb add --url`. On success the dropped file is **deleted** (its content
now lives in the KB); on failure it's moved to `inbox/failed/` and the
reason is logged to `kb.log`, so the watcher never retries it in a loop.
Set `inbox_enabled = false` under `[watcher]` in `config.toml` to turn
this off.

Deletion is symmetric: remove a paper's folder (or just its
`metadata.json`) and the running watcher drops its embeddings from both
stores — no orphaned vectors.

### Searching

```bash
kb search "online vector quantization"                  # narrow mode
kb search "what could I build with this" --wide         # synthesis mode
kb search "failure modes" --section limitations,future_work
kb search "quantization" --tag consumer -k 20
kb search "payment lane" --kind note --project kitgig --project global
```

Search is primarily semantic — it matches meaning, so vocabulary drift
across papers doesn't matter (and a lexical pass, below, still catches
exact terms). Two modes: **narrow** (default,
top 10, drops weak matches below the score floor) for "find me the
paper/section about X", and **wide** (`--wide`, top 40, no floor) for
synthesis questions where you want broad material to reason over.
`--section`, `--tag`, `--paper`, `--kind`, and `--project` restrict
the search. Results are grouped per paper, each chunk with its score,
section type, snippet, and a `file://…#page=N` deep link into the PDF.

Retrieval is **hybrid**: a dense (vector) ranking fused with a lexical
**BM25** ranking — a SQLite FTS5 index over chunk text — via Reciprocal
Rank Fusion. Pure semantic search misses exact-token queries (an author
name, a method name like `QJL`, an arXiv id); BM25 nails those, and
fusion gets the strengths of both. The reported `score` is then a small
RRF value — judge results by order, not magnitude. Turn it off
(`[search.hybrid] enabled = false`) for pure dense search.

The dense side isn't cosine-only either. Following the Generative Agents
retrieval model (arXiv:2304.03442, in this very corpus), each candidate
is scored on a blend of **relevance** (cosine), **recency** (recently
added/edited material decays slowly upward), and **importance** (a
section-type prior — your reflections and notes outrank raw paper prose,
`future_work` outranks background). Relevance stays dominant; recency
and importance break near-ties so freshly captured and high-value
material surfaces, and this ordering feeds the fusion above. The cosine
floor still gates dense candidates first, so it remains a true relevance
floor. Tune the weights under `[search.ranking]` / `[search.hybrid]`
(see [CONFIG.md](./CONFIG.md)).

An optional third ranker handles **multi-hop** retrieval. With
`[search.graph] enabled`, search runs a **Personalized PageRank** pass over a
chunk similarity graph seeded by the query's dense matches, fused into the same
RRF — so a chunk relevant *because it links to* relevant material surfaces even
when its own text shares no tokens with the query. This is HippoRAG's mechanism
(arXiv:2405.14831, in this corpus), walking the KB's existing similarity and
`[[id]]` edges instead of an LLM-extracted entity graph — no new index, no extra
API calls. It's **off by default**; flip it on per-corpus to taste.

### Capturing ideas

```bash
kb idea add --project kitgig --title "x402 anon lane" --body "Use x402 for an anonymous per-call payment lane"
kb idea add --project global --title "shared insight" # no --body: opens $EDITOR
echo "piped body" | kb idea add --project kitgig --title "from stdin" --body -
```

Ideas are standalone notes, keyed by project, living in the same index
as papers — one search surface. The id is the slugified title
(`x402 anon lane` → `x402-anon-lane`) and works with `kb show`,
`kb tag`, `kb remove`, and `--paper` search filters. Running
`kb idea add` again with the same title (or id) **updates the idea in
place** — no duplicates, so refine freely. Use project `global` for
ideas that apply across every project, then recall with
`--kind note --project <current> --project global`. `--link <id>`
records related papers/ideas.

### Reflections (cross-paper synthesis)

```bash
kb reflect --title "Memory architectures across agent frameworks" \
           --scope 2304.03442 --scope 2509.25140 \
           --tags memory,agents
kb reflect --title "Quantization trade-offs" --body -   # pipe body via stdin
kb reflect --title "Attention variants"                 # opens $EDITOR with template
```

A reflection is a higher-level synthesis document distilled from several
papers. It is stored with `section_type = reflection` and embedded
immediately, so future `kb_search` calls and `--section reflection`
surface it alongside raw paper chunks — today's synthesis compounds
into tomorrow's context.

`--scope` records the paper ids that informed the reflection (repeatable;
stored as `links` in `metadata.json`). When `--body` is omitted,
`$EDITOR` opens a template pre-seeded with the scoped papers' titles
and guiding sections (Themes, Contradictions, Combined ideas).

The id is derived from the title with a `reflection-` prefix
(`reflection-memory-architectures-across-agent-frameworks`), and
works everywhere an arXiv id does (`kb show`, `kb tag`, `kb remove`).

```bash
kb search "agent memory" --section reflection           # retrieve stored reflections
kb search "agent memory" --wide                         # reflections surface here too
```

### Notes and tags

```bash
kb note 2504.19874                      # opens notes.md in $EDITOR
kb tag 2504.19874 +consumer +quant      # add tags
kb tag 2504.19874 -quant                # remove a tag
```

Whatever you write in `notes.md` is embedded as its own `user_notes`
section — your thoughts become searchable alongside the paper, and
future synthesis queries see them. Re-embedding happens right after
the editor closes. Tags live in `metadata.json` (they survive
reindexes) and power `--tag` filters.

### Exploring the corpus

```bash
kb list                      # all documents (--tag/--kind/--project to filter)
kb show 2504.19874           # metadata, abstract, indexed sections, your notes
kb similar 2504.19874        # documents nearest this one (-k/--limit N)
kb open 2504.19874           # PDF in your default viewer
kb open 2504.19874 --section method     # … at that section's page
kb open 2504.19874_method_0  # … at a specific search hit's page
kb stats                     # corpus totals, chunks per section type, top tags
```

`kb similar` ranks the documents whose content sits closest to a given one in
embedding space (the same signal behind the web app's **Related** panel and the
graph's similarity edges). It reuses the embedding cache, so it costs no API
calls unless the cache was cleared.

### Health and maintenance

```bash
kb status            # root, paper count, index/db counts, watcher liveness
kb verify            # index ↔ meta.db consistency check (--deep checks every chunk)
kb reindex           # rebuild all derived state from the paper folders
kb gc                # drop chunks for deleted papers, prune stale cache entries
kb cache clear       # drop all cached embeddings (forces re-embedding)
```

The paper folders (PDFs, LaTeX, notes) are the source of truth; the
vector index and metadata DB are disposable derived state. `kb
reindex` rebuilds them from scratch, and the embedding cache makes
that nearly free — it only pays the API for text it has never seen.

### Server modes

```bash
kb mcp               # MCP server on stdio (how Claude Code connects)
kb serve             # HTTP API + browser web app on http://127.0.0.1:4321
kb serve --port 8080 # … on a different port
kb rotate-key        # generate a fresh HTTP API key
kb watch             # foreground folder watcher (see below)
```

`kb serve` starts a loopback-only HTTP server for tools that don't speak
MCP — browser extensions, curl scripts, alternative clients — and ships
a self-contained **web app** at `/` for browsing and analyzing the
corpus from your browser. It binds `127.0.0.1` only (never `0.0.0.0`)
and requires an `X-KB-Key` header on every request; the key is generated
on first run, stored mode-0600 in `.arxiv-kb/api_key`, and printed on
startup (override with `KB_API_KEY`, rotate with `kb rotate-key`).

| Method & path | Purpose |
|---|---|
| `GET /` | the web app (no key required for the shell) |
| `GET /health` | liveness probe (no key required) |
| `GET /stats` | corpus stats |
| `GET /papers` | list documents (`?tag=`, `?category=`) |
| `GET /papers/{id}` | metadata + notes + PDF path |
| `POST /search` | semantic search (body: `query`, `mode`, `k`, filters) |
| `GET /papers/{id}/similar` | documents most similar to this one (`?limit=`) |
| `GET /graph` | the corpus as nodes + edges (`?neighbors=` similarity edges per node) |
| `POST /chat` | RAG answer over the corpus, with cited sources (body: `query`, optional `history`) |
| `POST /papers/{id}/notes` | append a note and re-embed |
| `GET /chunks/{id}` | full chunk text + deep link |
| `GET /open/{id}` | 302 redirect to the PDF deep link |

The web app needs no build step. Open the `http://127.0.0.1:4321/?key=…`
link printed by `kb serve` — it seeds the key into your browser
(`localStorage`) — and you get five views:

- **Papers** — filter by text/tag/category/kind; open any document's
  abstract, notes, and PDF deep-link. Each detail panel also shows a
  **Related** list: the documents most similar to this one (by the mean of
  its chunk embeddings), so you can wander the corpus by proximity.
- **Search** — narrow/wide semantic search with per-chunk scores and
  deep-links, cross-linking into the paper detail.
- **Chat** — ask a question and get an answer synthesized over the corpus
  (wide retrieval → the chat model), with every claim cited `[n]` back to the
  source document and PDF page. Citations and the source list open the PDF
  panel at the right page. Multi-turn: follow-ups keep the prior context.
- **Graph** — the whole corpus as a force-directed graph: a node per
  document (sized by indexed chunk count, colored by kind), edges from
  explicit `[[id]]`/`--link`/`--scope` relations plus nearest-neighbor
  *similarity* edges. Drag nodes, pan/zoom, filter edge types, highlight by
  text, click a node to open the document.
- **Analytics** — document/chunk counts, chunks-per-section breakdown, and a
  tag cloud.

**Chat** uses OpenAI chat-completions (so it shares the single
`OPENAI_API_KEY` the embedding pipeline already needs); the model and context
size are configurable under `[chat]` in `config.toml` (default
`gpt-4o-mini`, 12 context chunks). **Related** and **Graph** similarity reuse
the embedding cache, so they cost no API calls.

Planned for v0.2: `kb excerpt` (compile chosen sections into one PDF).

## Claude Code integration

```bash
claude mcp add arxiv-kb -- kb mcp
```

Tools exposed:

| Tool | Purpose |
|---|---|
| `kb_search` | Narrow/wide/filtered semantic search (supports `section_types`, `kind`, `project`) |
| `kb_get_paper` | Full metadata + notes for a specific document |
| `kb_add_note` | Append a note to a paper and re-embed immediately |
| `kb_capture_idea` | Agent-side twin of `kb idea add` — upserts standalone ideas by project |
| `kb_create_reflection` | Save a cross-paper synthesis reflection; indexed as `reflection` chunks and retrieved by future searches |

Drop [skill.md](./skill.md) into your Claude Code plugin folder so
Claude knows when and how to use them.

Typical agent flow for synthesis: `kb_search(mode='wide')` → read
chunks → `kb_create_reflection(title, body, scope=[...paper_ids])`.
The reflection is embedded on save; subsequent `kb_search` calls
retrieve it as a first-class result alongside raw paper sections.

## Background watcher (optional)

```bash
kb watch        # re-embeds notes on save, ingests new folders, prunes deleted ones
```

Everything works without it — `kb add` and `kb note` index inline.
