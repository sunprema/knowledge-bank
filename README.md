# arxiv-kb

A single Rust binary that turns a folder of arXiv papers into a
queryable, AI-friendly knowledge base: save a paper in one command,
search across your corpus semantically, and let Claude synthesize
across papers via MCP — with every claim deep-linked back to the
source PDF.

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

```bash
kb update 2504.19874     # re-fetch (paper got a new arXiv version); keeps your tags
kb remove 2504.19874     # delete from the index AND delete the folder (asks first)
```

### Searching

```bash
kb search "online vector quantization"                  # narrow mode
kb search "what could I build with this" --wide         # synthesis mode
kb search "failure modes" --section limitations,future_work
kb search "quantization" --tag consumer -k 20
```

Search is semantic — it matches meaning, not keywords, so vocabulary
drift across papers doesn't matter. Two modes: **narrow** (default,
top 10, drops weak matches below the score floor) for "find me the
paper/section about X", and **wide** (`--wide`, top 40, no floor) for
synthesis questions where you want broad material to reason over.
`--section`, `--tag`, and `--paper` restrict the search. Results are
grouped per paper, each chunk with its score, section type, snippet,
and a `file://…#page=N` deep link into the PDF.

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
kb list                      # all papers (--tag to filter)
kb show 2504.19874           # metadata, abstract, indexed sections, your notes
kb open 2504.19874           # PDF in your default viewer
kb open 2504.19874 --section method     # … at that section's page
kb open 2504.19874_method_0  # … at a specific search hit's page
kb stats                     # corpus totals, chunks per section type, top tags
```

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
kb watch             # foreground folder watcher (see below)
```

Planned for v0.2: `kb similar` (papers near this one), `kb excerpt`
(compile chosen sections into one PDF), `kb serve` (HTTP API).

## Claude Code integration

```bash
claude mcp add arxiv-kb -- kb mcp
```

Tools exposed: `kb_search` (narrow/wide/filtered), `kb_get_paper`,
`kb_add_note`. Drop [skill.md](./skill.md) into your Claude Code
plugin folder so Claude knows when and how to use them.

## Background watcher (optional)

```bash
kb watch        # re-embeds notes on save, ingests new folders, prunes deleted ones
```

Everything works without it — `kb add` and `kb note` index inline.
