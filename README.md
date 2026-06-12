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

## Daily use

```bash
kb add 2504.19874            # fetch, extract sections, embed, index (~15s)
kb search "online vector quantization"
kb search "what could I build with this" --wide
kb search "limitations" --section limitations --tag consumer
kb note 2504.19874           # your notes get embedded too
kb tag 2504.19874 +consumer +quantization
kb open 2504.19874_method_0  # PDF at the right page
kb list / kb show / kb stats / kb status
```

The folder of PDFs, LaTeX sources, and notes is the source of truth;
the vector index and metadata DB are disposable derived state. If
anything ever looks wrong: `kb verify`, then `kb reindex` (the
embedding cache makes rebuilds nearly free).

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
