# KB_PROD_REQUIREMENTS.md

**Project:** `arxiv-kb` — a personal knowledge base for arXiv papers with semantic search and AI-assisted synthesis.

**Document status:** Draft v0.1
**Last updated:** May 2026
**Audience:** Claude Code (primary implementer), maintainers, contributors
**Distribution:** Single static Rust binary

---

## 1. Executive summary

`arxiv-kb` is a single Rust binary that turns a folder of arXiv papers
into a queryable, AI-friendly knowledge base.

### The problem

Research papers are dense, slow to absorb, and impossible to recall in
detail months later. When you read 50 papers across a year on adjacent
topics — vector quantization, agent frameworks, consumer crypto — you
end up with a mental "I remember this is somewhere" feeling but no way
to surface the right paper, let alone the right section. Synthesis
across papers — *"what could combine these ideas into a real product?"*
— is structurally impossible without re-reading them.

Generic knowledge tools (Obsidian, Notion, plain folders) don't help
because:

- They search by keyword, but research vocabulary drifts across papers
- They treat a paper as one unit, but a paper has 6+ semantically
  distinct sections
- They don't expose content to AI assistants in a structured way
- They can't surface non-obvious adjacencies across papers

### The solution

A single Rust binary that:

1. **Ingests arXiv papers from their IDs** (one command per paper)
2. **Extracts structured sections** from LaTeX source (abstract,
   introduction, method, applications, limitations, future work)
3. **Embeds each section separately** into a turbovec vector index
4. **Exposes search and synthesis tools** to Claude via the Model
   Context Protocol (MCP), plus an HTTP interface and a CLI
5. **Preserves the original PDF** with deep-link support so the human
   can always jump to the canonical source

The result: you save papers in 10 seconds, query across them in any
language you like, and get answers that cite specific sections of
specific papers — with a clickable link straight into the PDF at the
right page.

### Why this design

The system is built on four principles, in priority order:

1. **Files are forever; indexes are disposable.** The user's folder of
   PDFs and LaTeX sources is the source of truth. The vector index and
   metadata DB are derived artifacts, rebuildable at any time.
2. **Section-level granularity beats whole-paper.** A 12-page paper
   averaged into one vector matches everything weakly and nothing
   strongly. Sectioned embedding is the difference between "I found a
   relevant paper" and "I found the relevant paragraph."
3. **LaTeX source > PDF extraction.** When LaTeX source is available
   (the common case for arXiv), use it. PDF extraction is the fallback,
   not the primary path. This single decision raises extraction quality
   from ~60% to ~95%.
4. **Human-readable round-trip.** Every search result includes a
   deep-link to the original PDF at the right page. Claude's claims
   are always verifiable against the source in one click.

### Success criteria

After 3 months of use, the system has succeeded if:

- You've ingested 50+ papers without it feeling like a chore
- You routinely ask Claude synthesis questions and the answers
  surprise you with non-obvious adjacencies
- You click deep-links into source PDFs to verify Claude's claims and
  the links land at the right section
- You can rebuild the index from scratch and the system still works
- The binary's memory footprint stays under 50 MB and search latency
  stays under 100 ms

### Out of scope

To keep this focused:

- **Not a paper reader.** It points at your existing PDF viewer; it
  doesn't replace one.
- **Not a citation manager.** No BibTeX export, no Zotero integration
  in v0.1.
- **Not a writing tool.** It doesn't help draft papers, just consume them.
- **Not a multi-user system.** Single-user local-first only.
- **Not a generic knowledge base.** It's specifically tuned for arXiv
  papers. Generalizing would dilute the value.
- **Not a hosted service.** Pure local. No cloud sync. No telemetry.

---

## 2. User stories

These drive the design. Each gets satisfied by the system.

### Capture

> *"I just saw an interesting paper on Twitter. I want to save it to
> my KB in one command and have it indexed by the time I'm back from
> getting coffee."*

```bash
kb add 2504.19874
# fetches LaTeX source + PDF, extracts sections, embeds them, indexes
# done in ~15 seconds
```

### Synthesis

> *"I've been collecting papers on vector quantization and on agent
> infrastructure. What applications could combine the two?"*

Claude (via MCP):
- Calls `kb_search_wide` with the query
- Retrieves 30+ section-level matches across the corpus
- Clusters them into themes
- Proposes specific applications with citations to the source papers
- Includes deep-links to the relevant PDF pages

### Verification

> *"Claude said TurboQuant uses Lloyd-Max quantization in its method.
> I want to read that section myself."*

Claude's response includes:

```
TurboQuant uses Lloyd-Max quantization to find bucket boundaries
that minimize MSE under the rotated Beta distribution.
[→ open at section 3.2](file:///Users/you/arxiv-kb/2504.19874/paper.pdf#page=4)
```

Click → PDF viewer opens at page 4 of the right paper.

### Notes

> *"This paper is interesting because it relates to something I was
> thinking about last month. I want to add a note in my own words
> that becomes part of what Claude sees in future synthesis."*

```bash
kb note 2504.19874
# opens $EDITOR with the paper's notes.md
# whatever you write gets embedded as part of the paper's content
```

### Excerpting

> *"I want to read these 4 specific sections from 3 different papers
> together on my e-reader tonight."*

```bash
kb excerpt result_a result_b result_c result_d --out tonight.pdf
# assembles a PDF with just those page ranges from the source PDFs
```

### Corpus exploration

> *"What papers have I saved? What did I think was interesting about
> each one?"*

```bash
kb list                       # all papers
kb show 2504.19874            # metadata + your notes + tags
kb similar 2504.19874         # papers semantically near this one
```

---

## 3. System architecture

### Single binary, four runtime modes

```
arxiv-kb binary
│
├── CLI mode               (kb add, kb search, kb show, ...)
├── Watcher mode           (kb watch — background, re-indexes folder)
├── MCP server mode        (kb mcp — stdio MCP for Claude Code)
└── HTTP server mode       (kb serve — HTTP for curl, browser, other tools)
```

All four share the same internal modules. You can run multiple modes
simultaneously: `kb watch &` in one terminal, `kb mcp` invoked by
Claude Code in another, plus `kb serve --port 4321` for browser-based
tools.

### Folder layout

```
~/arxiv-kb/                       # configurable via --root flag or KB_ROOT env
├── 2504.19874/                   # one folder per paper, named by arXiv ID
│   ├── metadata.json             # title, authors, abstract, date, categories
│   ├── source/                   # LaTeX source files (if available)
│   │   ├── main.tex
│   │   └── *.tex (other files)
│   ├── paper.pdf                 # the PDF
│   ├── sections.md               # extracted, structured markdown (DERIVED)
│   └── notes.md                  # user-written notes (CANONICAL, hand-edited)
├── 2405.12497/
│   └── ...
└── .arxiv-kb/                    # all derived/managed state — gitignored
    ├── index.tv                  # turbovec index
    ├── id_map.tvim               # turbovec id mapping (stable across rebuilds)
    ├── meta.db                   # SQLite — file→vector mapping, page maps, hashes
    ├── config.toml               # embedding model, bit_width, schema version
    ├── kb.log                    # watcher diagnostics
    └── kb.pid                    # running watcher's PID
```

**Canonical (user-owned, never overwrite):**
- `metadata.json` (well, the binary writes it once, but never overwrites)
- `paper.pdf`
- `source/*.tex`
- `notes.md`

**Derived (regenerable from canonical):**
- `sections.md`
- `.arxiv-kb/*`

If `.arxiv-kb/` is deleted, `kb reindex` reconstructs it from the
canonical files. This invariant is critical.

### Data flow: ingest

```
kb add 2504.19874
  │
  ▼
[arXiv API call] ──► metadata.json
  │
  ▼
[fetch e-print tarball] ──► source/ (LaTeX files)
  │
  ▼  (if no LaTeX available, skip to PDF-only path)
  │
[pandoc latex → markdown] ──► sections.md
  │
  ▼
[fetch PDF] ──► paper.pdf
  │
  ▼
[pdfium TOC extraction] ──► section→page mapping (into meta.db)
  │
  ▼
[section classifier] ──► structured chunks
  │                       [{type: abstract, text: "..."},
  │                        {type: introduction, text: "..."},
  │                        {type: method, text: "..."},
  │                        ...]
  ▼
[embedding API call, one per chunk] ──► vectors
  │
  ▼
[turbovec.add_with_ids] ──► index.tv updated
  │
  ▼
[meta.db updated] ──► chunk_id → (paper_id, section_type, page, snippet)
  │
  ▼
done
```

### Data flow: search

```
kb_search("consumer applications", k=10)
  │
  ▼
[embedding API call for query] ──► query vector
  │
  ▼
[turbovec.search(query, k=10)] ──► [(chunk_id, score), ...]
  │
  ▼
[meta.db lookup] ──► full chunk records with paper metadata
  │
  ▼
[response shaped for client]
  │
  ▼
returned to caller (CLI, MCP, or HTTP)
```

### Module structure

```
src/
├── main.rs                   # clap CLI + dispatch
├── lib.rs                    # public types and orchestration
├── ingest/
│   ├── mod.rs
│   ├── arxiv.rs              # arXiv API client, ID parsing
│   ├── latex.rs              # LaTeX source download, pandoc invocation
│   ├── pdf.rs                # PDF download, TOC extraction via pdfium
│   ├── sections.rs           # section classifier
│   └── pipeline.rs           # the ingest orchestration
├── embed/
│   ├── mod.rs
│   ├── openai.rs             # OpenAI text-embedding-3-small client
│   └── local.rs              # fastembed-rs fallback (v0.3)
├── index/
│   ├── mod.rs
│   ├── turbovec_index.rs     # wraps turbovec::IdMapIndex
│   └── meta_db.rs            # SQLite for chunk metadata
├── search/
│   ├── mod.rs
│   └── retrieval.rs          # search logic, snippets, deduplication
├── server/
│   ├── mod.rs
│   ├── http.rs               # axum HTTP server
│   └── mcp.rs                # MCP server on stdio
├── watcher/
│   ├── mod.rs
│   └── fs_watcher.rs         # notify-based folder watcher
├── excerpt/
│   ├── mod.rs
│   └── assembler.rs          # PDF page-range concatenation (v0.2)
└── config.rs                 # config loading, env vars, defaults
```

Keep modules small. Split when a file exceeds ~400 lines.

---

## 4. The ingest pipeline (the heart of the system)

This is the single most important subsystem. Quality of ingest
determines quality of search, which determines whether the whole
system is worth using.

### Input: an arXiv ID or URL

Accept formats:
- `2504.19874`
- `arxiv:2504.19874`
- `https://arxiv.org/abs/2504.19874`
- `https://arxiv.org/pdf/2504.19874`
- `https://arxiv.org/pdf/2504.19874v2`

Normalize to canonical ID (`2504.19874`). Discard version suffixes
for v0.1 (always fetch latest). Track version in metadata so we know
which version we have.

### Step 1: metadata via arXiv API

URL: `https://export.arxiv.org/api/query?id_list=2504.19874`

Returns Atom XML with title, authors, abstract, categories, published
date, updated date. Parse with `quick-xml` or `roxmltree`. Write to
`metadata.json`:

```json
{
  "arxiv_id": "2504.19874",
  "version": "v2",
  "title": "TurboQuant: Online Vector Quantization with Near-optimal Distortion Rate",
  "authors": ["...", "..."],
  "abstract": "We present...",
  "categories": ["cs.IR", "cs.LG"],
  "published_at": "2024-04-28T...",
  "updated_at": "2024-09-15T...",
  "ingested_at": "2026-05-24T...",
  "source_format": "latex" | "pdf",
  "schema_version": 1
}
```

**Rate-limit-friendly:** arXiv asks for 3-second delays between bulk
queries. For one-paper ingest this doesn't matter, but a future
`kb import bibliography.bib` bulk-add command must respect this.

### Step 2: LaTeX source download

URL: `https://arxiv.org/e-print/2504.19874`

Returns a `.tar.gz` (sometimes a `.gz` of a single `.tex`, sometimes
a single `.pdf` if no LaTeX is available). Extract to `source/`.

If extraction yields only a PDF (no `.tex` files), set
`source_format: "pdf"` in metadata and skip to step 4 (PDF fallback).

Otherwise:

1. Find the main `.tex` file. Heuristics:
   - Look for `\documentclass` in any `.tex` file
   - If multiple have it, prefer one named `main.tex`, `paper.tex`,
     `manuscript.tex`, or the one with `\begin{document}`
   - If still ambiguous, pick the largest file with `\documentclass`
2. Note its filename in `metadata.json` as `main_tex`.

### Step 3: LaTeX → structured markdown via pandoc

Invoke pandoc as a subprocess:

```bash
pandoc source/main.tex -o sections.md \
  --from latex \
  --to gfm \
  --wrap=none \
  --bibliography=source/*.bib    # if present
```

Why `gfm` (GitHub-flavored Markdown) instead of plain markdown:
preserves tables better, handles LaTeX math via `$...$` cleanly.

If pandoc errors (malformed LaTeX, missing packages), fall back to
PDF extraction (step 4) and flag the metadata as `source_format: "pdf"`.

Pandoc is a required system dependency. The binary checks for it on
first run and prints clear install instructions if missing.

### Step 4: PDF extraction fallback

When LaTeX is unavailable or pandoc fails:

1. Download `https://arxiv.org/pdf/2504.19874` as `paper.pdf`
2. Use `pdfium-render` to extract text per page
3. Concatenate into `sections.md` with `## Page N` headings (since
   we have no semantic section info)
4. Quality will be lower; downstream section classifier will mostly
   produce a single "body" chunk for the whole paper

This is graceful degradation, not a primary path.

### Step 5: PDF download + TOC extraction

Always download `paper.pdf` for human-reading purposes.

Use `pdfium-render` to extract the PDF's outline (TOC). For each
entry, capture:

- Section title
- Page number (1-indexed)

ArXiv PDFs almost always have a TOC because LaTeX generates one
automatically. Store as a list in `meta.db`:

```sql
CREATE TABLE pdf_toc (
  paper_id    TEXT NOT NULL,
  section     TEXT NOT NULL,
  page        INTEGER NOT NULL,
  named_dest  TEXT,
  PRIMARY KEY (paper_id, section)
);
```

The `named_dest` is the PDF named destination if available (for
`#nameddest=` URLs); otherwise NULL and we fall back to `#page=N`.

### Step 6: section classification

The single trickiest part of the pipeline. Given a structured
markdown file from pandoc, partition it into chunks tagged by section
type.

Section types (closed enum):

- `abstract`
- `introduction`
- `background` (related work, prior art)
- `method` (algorithm, approach, design)
- `experiments` (evaluation, results, benchmarks)
- `applications` (use cases, when authors propose practical uses)
- `limitations` (failure modes, weaknesses, threats to validity)
- `future_work` (open problems, next steps)
- `conclusion`
- `other` (whatever doesn't fit above)

Classification heuristic (deterministic, no ML):

```rust
fn classify(heading: &str) -> SectionType {
    let h = heading.to_lowercase();
    match h.as_str() {
        s if s.contains("abstract") => Abstract,
        s if s.contains("introduction") => Introduction,
        s if s.contains("related work") || s.contains("background")
          || s.contains("prior") => Background,
        s if s.contains("method") || s.contains("approach")
          || s.contains("algorithm") || s.contains("design")
          || s.contains("model") => Method,
        s if s.contains("experiment") || s.contains("evaluation")
          || s.contains("result") || s.contains("benchmark") => Experiments,
        s if s.contains("application") || s.contains("use case") => Applications,
        s if s.contains("limitation") || s.contains("threat")
          || s.contains("weakness") => Limitations,
        s if s.contains("future work") || s.contains("future direction")
          || s.contains("open problem") => FutureWork,
        s if s.contains("conclusion") || s.contains("discussion") => Conclusion,
        _ => Other,
    }
}
```

Split sections.md at each H1/H2/H3 boundary. Each section becomes a
chunk. Chunks longer than 2000 tokens get split further at paragraph
boundaries, with all sub-chunks retaining the same section type.

Special case: the abstract is always extracted from `metadata.json`
(it came from the arXiv API and is cleaner than what pandoc produces).

### Step 7: notes.md

If `notes.md` doesn't exist, create an empty template:

```markdown
# Notes on TurboQuant

<!-- Why is this interesting to me? -->


<!-- What would I build with this? -->


<!-- Connections to other things I've saved -->


```

Encourage the user to write here. The `kb note <id>` command opens
this file in `$EDITOR`. The watcher re-embeds when it's modified.

When indexing, notes.md becomes its own section type:
`user_notes`. Weighted slightly higher in retrieval than other
sections because it reflects the user's actual intent.

### Step 8: embedding

For each chunk produced by section classification + notes:

1. Compute content hash (SHA-256 of the chunk text)
2. Check `meta.db` — already embedded with this hash?
   - If yes, reuse existing vector ID, skip API call
   - If no, embed and store
3. Call OpenAI `text-embedding-3-small` (1536 dim) via HTTP
4. Store vector via turbovec's `add_with_ids` with a stable chunk ID

Chunk ID scheme: `{paper_id}_{section_type}_{ordinal}`
e.g. `2504.19874_method_0`, `2504.19874_method_1` (when split)

Stored in `meta.db`:

```sql
CREATE TABLE chunks (
  chunk_id        TEXT PRIMARY KEY,
  vector_id       INTEGER NOT NULL UNIQUE,  -- turbovec id
  paper_id        TEXT NOT NULL,
  section_type    TEXT NOT NULL,
  ordinal         INTEGER NOT NULL,
  content_hash    TEXT NOT NULL,
  text            TEXT NOT NULL,           -- the chunk's source text
  page            INTEGER,                  -- PDF page number, derived from TOC
  snippet         TEXT,                     -- first 200 chars for previews
  embedded_at     TEXT NOT NULL,
  embedding_model TEXT NOT NULL,
  embedding_version INTEGER NOT NULL
);
```

The `vector_id` mirrors the ID in turbovec's `IdMapIndex`. They
must stay in sync; treat them as one logical pair.

### Step 9: page mapping

For each chunk, determine the page in the PDF where its content
appears. Strategy:

1. Take the chunk's heading text (e.g. "3.2 Lloyd-Max Quantization")
2. Look up in the `pdf_toc` table: find the entry whose section name
   best matches (case-insensitive, fuzzy if needed)
3. If found, store its `page` and `named_dest` with the chunk
4. If not found, use the nearest preceding TOC entry's page

This isn't perfect — pandoc's section names won't always match the
PDF's exactly — but it's good enough for "open the PDF roughly where
this content is."

### Step 10: index persistence

After all chunks ingested:

1. Save the turbovec index to `.arxiv-kb/index.tv`
2. Commit SQLite transaction
3. Append to `kb.log`: ingest summary (paper_id, N chunks, time taken)

If anything in the pipeline fails midway, the entire ingest is
rolled back — partial papers in the index are worse than missing
papers.

---

## 5. Search and retrieval

### Search modes

Three retrieval modes. Same underlying turbovec call, different
shaping of inputs and outputs.

#### Mode: `narrow` (default for direct lookup)

```rust
kb_search_narrow(query: &str, k: usize) -> Vec<Result>
```

- Embed the query
- Search turbovec with `k=10`
- Filter results by `min_score >= 0.72` (configurable)
- Return top results, ordered by score
- Each result includes paper metadata, snippet, page, deep-link

#### Mode: `wide` (default for synthesis)

```rust
kb_search_wide(query: &str, k: usize) -> Vec<Result>
```

- Embed the query
- Search turbovec with `k=40` (configurable, up to 100)
- No score floor — return everything in the top-k
- Include full metadata for each result
- Results not pre-clustered (Claude does that)

#### Mode: `filtered`

```rust
kb_search_filtered(query: &str, k: usize, filters: Filters) -> Vec<Result>
```

Filters can include:
- `section_types`: only these section types
- `paper_ids`: only these papers
- `categories`: only these arXiv categories (cs.IR, etc.)
- `date_range`: papers ingested in this range
- `tags`: only papers with these user-applied tags

Filtering happens via turbovec's allowlist mechanism. The Rust side
queries `meta.db` for matching chunk vector_ids and passes them as
an `allowlist` to turbovec's search. Turbovec's SIMD kernel honors
the allowlist at 32-vector block granularity — selective filters are
*fast*, not "search all then drop."

### Result shape

Every result, regardless of mode, has this shape:

```json
{
  "chunk_id": "2504.19874_method_0",
  "paper_id": "2504.19874",
  "section_type": "method",
  "score": 0.83,
  "snippet": "Lloyd-Max quantization finds bucket boundaries...",
  "page": 4,
  "deep_link": "file:///Users/you/arxiv-kb/2504.19874/paper.pdf#page=4",
  "paper": {
    "title": "TurboQuant: ...",
    "authors": ["..."],
    "abstract": "We present...",
    "categories": ["cs.IR"],
    "published_at": "2024-04-28"
  },
  "tags": ["consumer", "quantization"]
}
```

Claude sees this and includes the `deep_link` in its response so the
user can verify the source.

### Deduplication

When multiple chunks from the same paper appear in the top results
(common for synthesis queries), the response groups them:

```json
{
  "papers": [
    {
      "paper_id": "2504.19874",
      "best_score": 0.83,
      "matched_sections": ["method", "applications"],
      "chunks": [
        { "chunk_id": "...method_0", "score": 0.83, "page": 4, ... },
        { "chunk_id": "...applications_0", "score": 0.79, "page": 11, ... }
      ],
      "paper": { ... }
    },
    ...
  ]
}
```

Deduplication is at the paper level. Each paper appears once with all
its matching chunks under it. The CLI's pretty-print mode shows one
paper per "card" with the matched sections listed.

### Snippets

Snippets in results are the first ~200 characters of the chunk text,
with the matched query terms NOT highlighted (no string matching;
embeddings don't expose token-level scores). Just a preview.

If Claude needs the full chunk text, it calls `kb_get_chunk(chunk_id)`.

### Caching

Query embeddings are cached in-memory during a single process
lifetime (a hashmap keyed by query string). Same query twice in a
session = one API call. No persistent query cache; queries are too
varied to benefit.

---

## 6. The CLI

Twelve commands. Each maps to a real use. Resist adding more.

### Ingest

```bash
kb add <arxiv-id-or-url>            # ingest a paper
kb add --pdf <path>                 # ingest a local PDF (no LaTeX path)
kb update <arxiv-id>                # re-fetch (in case the paper was updated)
kb remove <arxiv-id>                # remove from index AND delete folder
```

### Notes and tags

```bash
kb note <arxiv-id>                  # opens notes.md in $EDITOR
kb tag <arxiv-id> +tag1 +tag2       # add tags
kb tag <arxiv-id> -tag1             # remove tag
```

### Search

```bash
kb search "<query>"                 # narrow search, pretty-printed
kb search "<query>" --wide          # wide search
kb search "<query>" --json          # JSON output
kb search "<query>" --section method,applications
kb search "<query>" --tag consumer
```

### Exploration

```bash
kb list                             # all papers
kb list --tag consumer              # filtered
kb show <arxiv-id>                  # metadata + notes + sections summary
kb similar <arxiv-id>               # papers near this one in embedding space
kb open <arxiv-id>                  # open PDF in default viewer
kb open <arxiv-id> --section method # open at that section's page
kb open <chunk-id>                  # open at the page for that chunk
```

### Excerpts (v0.2)

```bash
kb excerpt <chunk-id> [<chunk-id>...] --out compiled.pdf
```

### Corpus management

```bash
kb stats                            # corpus summary: N papers, N chunks, top tags
kb status                           # is watcher running? indexed up to date?
kb reindex                          # rebuild index from scratch
kb gc                               # remove orphaned chunks
```

### Server modes

```bash
kb watch [--daemon]                 # background folder watcher
kb mcp                              # MCP server on stdio
kb serve [--port 4321]              # HTTP server
```

### Global flags

```bash
--root <path>                       # KB folder (default: $HOME/arxiv-kb)
--format pretty|json                # output format
--verbose                           # debug logging to stderr
--help, --version
```

The `--root` flag means you can have multiple KBs:

```bash
kb --root ~/research-kb add 2504.19874
kb --root ~/personal-kb add 2401.00001
```

### Exit codes

| code | meaning |
|------|---------|
| 0 | success |
| 1 | usage error (bad arguments) |
| 2 | not found (paper, chunk, etc.) |
| 3 | network error (arXiv API, embedding API) |
| 4 | extraction error (pandoc failed, PDF malformed) |
| 5 | index error (turbovec failed) |
| 10 | configuration error |

---

## 7. The MCP server

When invoked as `kb mcp`, the binary speaks the Model Context Protocol
on stdin/stdout. Claude Code launches it as a subprocess.

### Tools exposed

Three tools in v0.1. More can be added as you learn what's useful.

#### `kb_search`

```json
{
  "name": "kb_search",
  "description": "Search the user's arxiv-kb for sections of papers matching a query. Returns top-k chunks ranked by semantic similarity, with paper metadata and PDF deep-links. Use mode='narrow' for direct lookups (when the user asks about a specific concept), mode='wide' for synthesis or ideation queries (when the user wants to combine ideas across papers).",
  "input_schema": {
    "type": "object",
    "properties": {
      "query": { "type": "string" },
      "mode": { "type": "string", "enum": ["narrow", "wide"], "default": "narrow" },
      "k": { "type": "integer", "default": 10, "minimum": 1, "maximum": 100 },
      "section_types": {
        "type": "array",
        "items": { "type": "string" },
        "description": "Restrict to these section types. Useful for synthesis — e.g. ['applications', 'future_work'] surfaces what authors propose."
      },
      "tags": {
        "type": "array",
        "items": { "type": "string" }
      },
      "paper_ids": {
        "type": "array",
        "items": { "type": "string" }
      }
    },
    "required": ["query"]
  }
}
```

#### `kb_get_paper`

```json
{
  "name": "kb_get_paper",
  "description": "Get full metadata, abstract, and user notes for a specific paper. Use after kb_search when Claude needs more context than a chunk snippet provides.",
  "input_schema": {
    "type": "object",
    "properties": {
      "paper_id": { "type": "string" }
    },
    "required": ["paper_id"]
  }
}
```

#### `kb_add_note`

```json
{
  "name": "kb_add_note",
  "description": "Append a note to a paper's notes.md. Use when the user (during a Claude conversation) shares an insight about a paper that should be captured for future synthesis.",
  "input_schema": {
    "type": "object",
    "properties": {
      "paper_id": { "type": "string" },
      "note": { "type": "string" }
    },
    "required": ["paper_id", "note"]
  }
}
```

### Skill file

The binary should also ship a `skill.md` that the user can drop into
their Claude Code plugin folder. Contents:

```markdown
# arxiv-kb skill

The user has a personal arXiv knowledge base. When questions involve
research papers, ideas across papers, or synthesis of technical
concepts, use the kb_search tool.

## When to use kb_search

- The user asks about a topic that might be in their papers
- The user asks for synthesis ("ideas combining X and Y")
- The user asks for applications, future work, or open problems
- The user references "my papers" or "what I've saved"

## How to use it well

- Default to mode="narrow" for direct lookups
- Use mode="wide" with k=30+ for synthesis queries
- Use section_types filter to focus on what matters:
  - "applications" for "what could be built"
  - "future_work" for "open problems"
  - "method" for "how does X work"
- Always include the deep_link in your response so the user can
  verify against the source

## Output discipline

When citing papers:
1. Use the paper title (not just the arxiv id)
2. Note the section: "TurboQuant (section 3.2 — Method)"
3. Include the deep_link as a markdown link
4. If retrieval scores are low (<0.7), note that you're stretching
```

This skill is what teaches Claude to use the tool well, not just
to know it exists.

---

## 8. The HTTP server

When invoked as `kb serve [--port 4321]`, expose a minimal HTTP API.

This is the alternative for tools that don't speak MCP — browser
extensions, curl scripts, alternative AI clients.

### Endpoints

```
GET  /health                        # liveness
GET  /stats                         # corpus stats
GET  /papers                        # list papers (with ?tag=, ?category=)
GET  /papers/{paper_id}             # paper details
POST /search                        # search (body: query, mode, k, filters)
POST /papers/{paper_id}/notes       # append a note
GET  /chunks/{chunk_id}             # full chunk text
GET  /open/{chunk_id}                # 302 redirect to the deep_link
```

### Auth

Bind to `127.0.0.1` only (never `0.0.0.0`). Require a header
`X-KB-Key: <key>` on every request. Generate the key on first run,
store in `.arxiv-kb/api_key`, print it for the user to add to
clients. Rotate with `kb rotate-key`.

### CORS

Allow requests from `chrome-extension://<id>` and from localhost
origins. Configurable. Default: deny all cross-origin.

---

## 9. The watcher

When invoked as `kb watch [--daemon]`, run a background process that:

1. Watches the KB root folder for changes
2. On change to a paper folder's `notes.md`: re-embed just the notes
   chunk for that paper
3. On change to a paper folder's `sections.md`: re-embed all chunks
   for that paper (rare — only if user manually edits)
4. On new folder appearing with valid `metadata.json` but no
   indexed chunks: ingest it
5. On folder deletion: remove all chunks for that paper

Use `notify` crate. Debounce: a save followed by another within 2
seconds triggers one re-embed.

Lifecycle events logged to `.arxiv-kb/kb.log`:

```
2026-05-24T10:30:00Z INFO  watcher started, monitoring /Users/you/arxiv-kb
2026-05-24T10:31:23Z INFO  detected change in 2504.19874/notes.md, re-embedding
2026-05-24T10:31:25Z INFO  re-embedded 2504.19874_user_notes_0 (1.8s)
2026-05-24T10:35:42Z INFO  detected new folder 2405.12497, ingesting
2026-05-24T10:35:58Z INFO  ingested 2405.12497: 7 chunks (15.2s)
```

The watcher writes a PID file at `.arxiv-kb/kb.pid`. `kb status`
checks if the watcher is alive via `kill -0`.

---

## 10. Configuration

`.arxiv-kb/config.toml`:

```toml
schema_version = 1

[embedding]
provider = "openai"                   # "openai" | "local"
model = "text-embedding-3-small"
dimensions = 1536

[turbovec]
bit_width = 4                          # 2 | 4 (4 recommended for embedding quality)

[search]
default_k_narrow = 10
default_k_wide = 40
default_min_score_narrow = 0.72
default_min_score_wide = 0.0           # no floor in wide mode

[ingest]
chunk_max_tokens = 2000               # split chunks larger than this
prefer_latex = true                    # try LaTeX before PDF
pandoc_path = "pandoc"                 # or absolute path

[server]
http_port = 4321
http_bind = "127.0.0.1"

[watcher]
debounce_ms = 2000
```

Environment variables override config:

- `KB_ROOT` — KB folder path
- `OPENAI_API_KEY` — required for OpenAI embedding
- `KB_API_KEY` — HTTP server auth (otherwise generated)
- `KB_LOG_LEVEL` — `error`, `warn`, `info`, `debug`

---

## 11. Roadmap

### v0.1 — core (target: 3 weekends)

Build order matters; each slice should be testable before the next.

1. **CLI skeleton** — clap with all subcommands stubbed
2. **Config + folder layout** — `kb init`, config.toml, .arxiv-kb/
3. **arXiv metadata fetcher** — `kb add` writes metadata.json only
4. **LaTeX downloader + pandoc invocation** — extract sections.md
5. **PDF download + TOC extraction** — paper.pdf + pdf_toc table
6. **Section classifier** — chunks with section_type assigned
7. **OpenAI embedding client** — embed one chunk
8. **turbovec wiring** — add chunks to index, save, load
9. **meta.db schema** — store chunks with vector_ids
10. **kb search (narrow)** — query → results with metadata
11. **kb show, kb list** — corpus exploration
12. **kb open** — PDF deep-linking via system viewer
13. **MCP server** — kb_search, kb_get_paper, kb_add_note
14. **kb watch (foreground only)** — re-embed on notes.md change

### v0.2 — depth

15. **Wide and filtered search modes**
16. **Tags** — kb tag, search by tag, filter in MCP tool
17. **kb excerpt** — PDF page-range assembly
18. **kb similar** — adjacency search
19. **HTTP server** — full surface, API key auth
20. **Daemon mode for watcher**

### v0.3 — local embeddings

21. **fastembed-rs integration** — local BGE-small-en-v1.5
22. **Multi-model support** — re-embed corpus with new model
23. **Embedding migration** — `kb reindex --embedding-model X`

### v0.4 — bulk and ergonomics

24. **`kb import bibliography.bib`** — bulk-add from BibTeX
25. **`kb import twitter-archive.zip`** — for non-arXiv content (stretch)
26. **`kb sync`** — re-fetch updated versions of papers
27. **Citation graph** — fetch papers' references, surface "papers I've
    saved that cite each other"

### v1.0 — inclusion criteria

- [ ] All v0.1 features stable
- [ ] 50+ papers ingested in a real user's KB
- [ ] Search latency <100ms for queries on a 500-chunk index
- [ ] Memory footprint <50MB idle, <200MB during ingest
- [ ] Documented Pandoc + system dependencies install
- [ ] Single-binary distribution via cargo install and Homebrew tap
- [ ] At least one full end-to-end test (ingest → search → MCP → response)

---

## 12. Distribution

- **Cargo:** `cargo install arxiv-kb`
- **Homebrew (eventually):** `brew install arxiv-kb`
- **GitHub Releases:** macOS arm64 + x86_64, Linux x86_64 + arm64 (musl)
- **Required system dep:** `pandoc` (documented; binary checks at runtime
  with clear install instructions if missing)

Static link via `cargo build --release` with musl on Linux. Strip
symbols. Target ~10MB final binary.

---

## 13. Performance targets

| metric | target |
|--------|--------|
| ingest time per paper | <20s (network-bound on embedding + arXiv) |
| search latency (500 chunks) | <100ms end-to-end including embedding |
| search latency (5000 chunks) | <200ms |
| memory idle | <50MB |
| memory during ingest | <200MB |
| binary size (stripped) | <15MB |
| `kb status` response | <50ms |
| index save (5000 chunks) | <1s |
| index load (5000 chunks) | <100ms |

These are aspirational floors. Profile before shipping v1.0 to
confirm.

---

## 14. Edge cases and failure modes

| situation | behavior |
|-----------|----------|
| arXiv ID doesn't exist | exit 2, message: "paper not found on arxiv" |
| arXiv API rate-limited | wait 3s and retry; on 3rd failure, exit 3 |
| LaTeX source is a single PDF (no .tex) | fall back to PDF path, flag in metadata |
| Pandoc isn't installed | exit 10 on first ingest, with install URL |
| Pandoc errors on weird LaTeX | log warning, fall back to PDF extraction |
| PDF has no TOC | use page numbers only, no named destinations |
| Embedding API rate-limited | exponential backoff, max 3 retries |
| Embedding API down | exit 3 with clear message; don't partial-ingest |
| Disk full during ingest | rollback, exit 5, leave folder in clean state |
| Two `kb add` calls race on same paper | second one detects existing folder, no-op |
| Watcher receives event during shutdown | flush in-flight embeds, save index, exit |
| meta.db corrupted | `kb reindex` rebuilds from canonical files |
| index.tv corrupted | same; reindex rebuilds |
| Both meta.db and index.tv missing | `kb reindex` works (canonical files are intact) |
| User edits sections.md directly | watcher re-embeds; not recommended workflow |
| User edits paper.pdf directly | watcher ignores PDFs (they're not the source of truth for content) |
| Network down during search | embedding fails → exit 3, no partial results |
| Two watchers race | second one's flock fails, exits with message |
| Note added via MCP while user has notes.md open in editor | last-write-wins; user sees Claude's note on next save |

---

## 15. Testing strategy

### Unit tests

- Section classifier: known headings → expected section types
- arXiv ID parser: every URL format → canonical ID
- Chunk splitter: long content → multiple chunks at paragraph boundaries
- meta.db queries: ingest → chunk lookup → search filter

### Integration tests

In `tests/`:

- `ingest.rs` — fixture LaTeX file → ingest → assert sections, embedding cache
- `search.rs` — pre-built fixture index → query → assert results
- `mcp.rs` — spawn MCP subprocess → send tool calls → assert responses
- `http.rs` — start HTTP server → curl → assert responses
- `watcher.rs` — start watcher, modify a notes.md → assert re-embed

### End-to-end smoke test

A `smoke.sh` script that:

1. Creates a temp KB root
2. Runs `kb add 2504.19874` (real network call — gated behind env flag)
3. Asserts the folder structure is correct
4. Runs `kb search "quantization"`
5. Asserts at least one result includes TurboQuant

This is the "does the whole thing actually work" test. Run in CI
behind `ENABLE_E2E=1` so PRs don't hit arXiv every push.

---

## 16. Open questions to resolve before v0.1

These are the design choices not locked in yet. Implementer (Claude
Code) should ask before guessing on these.

- [ ] **Notes.md format**: free-form markdown vs. structured prompts
      (the template with "Why interesting?", "What would I build?")
- [ ] **Section classifier when headings are ambiguous**: fall through
      to `Other`, or use LLM-based classification for ambiguous cases?
- [ ] **Re-embedding on chunk_max_tokens change**: do we reindex
      automatically, prompt the user, or refuse?
- [ ] **Notes chunk weighting**: scale notes vectors slightly so they
      rank higher? Or trust embedding similarity directly?
- [ ] **PDF assembly in v0.2**: include cover page with synthesis
      summary, or just concatenate raw page ranges?
- [ ] **`kb add` blocking vs background**: block by default with a
      progress bar, or fire-and-forget with watcher catching up?
      Leaning blocking with progress (more predictable).
- [ ] **Tag storage**: in metadata.json (canonical, hand-editable) or
      in meta.db (derived, queryable)? Leaning metadata.json.
- [ ] **Schema version 1 vs 2 migration policy**: refuse to read
      newer index, or attempt forward-compatible degraded mode?
      Leaning refuse with a clear error.

---

## 17. Anti-goals (things we explicitly will not do)

These appear in scope-creep brainstorms but should be rejected:

- ❌ A built-in PDF reader / annotation tool
- ❌ Multi-user sharing or cloud sync
- ❌ Real-time collaboration
- ❌ A web UI for the KB itself (CLI + MCP + HTTP is enough)
- ❌ ML-based summarization of papers (Claude does this on demand;
     we don't precompute)
- ❌ Author profile pages, paper recommendations from external services
- ❌ Replacing arXiv as a paper source (Google Scholar, OpenReview,
     etc. are out of scope for v1)
- ❌ Storing PDF excerpts inline in meta.db (the PDFs are on disk;
     just reference them)
- ❌ Anything that requires keeping the binary running 24/7 (the
     watcher is optional; CLI + MCP work fine without it)

---

## 18. References

- TurboQuant paper: https://arxiv.org/abs/2504.19874
- turbovec crate: https://github.com/RyanCodrai/turbovec
- Pandoc: https://pandoc.org
- pdfium-render crate: https://docs.rs/pdfium-render
- notify crate: https://docs.rs/notify
- MCP specification: https://modelcontextprotocol.io
- OpenAI embeddings: https://platform.openai.com/docs/guides/embeddings
- fastembed-rs (for v0.3 local embeddings): https://github.com/Anush008/fastembed-rs

---

## 19. Instructions for the implementer

This document is the source of truth. Where it conflicts with prior
sketches in chat history, this document wins.

Build in the order in section 11 (Roadmap). Don't skip ahead. Each
slice should produce a working artifact testable in isolation before
the next slice begins.

When in doubt about a design choice, consult section 16 (Open
questions). If the question isn't listed there, ask before guessing.

Conventions:

- All output supports `--format json`
- Exit codes per section 6
- `--root` flag respected on every command
- `notify` for filesystem watching
- `tokio` async, one task per long-running concern
- `tracing` for diagnostics (not the JSONL — that's `agent-monitor`'s pattern)
- `serde` for all data types
- `anyhow` at binary edges, `thiserror` in library code

The user's `OPENAI_API_KEY` is sensitive. Never log it. Never include
it in error messages.

The user's KB folder may contain papers they consider personal. Don't
exfiltrate. The only network calls are: arXiv API for metadata,
arXiv for source/PDF downloads, and the embedding API. Nothing else.

When implementing the MCP server, prefer the official `mcp-rs` crate
if mature; otherwise implement the protocol directly. The schemas in
section 7 are the contract.

This README and the SPEC are living documents. Update them when
design decisions shift during implementation.
