# CONFIG.md — configuring arxiv-kb

How the KB locates its data, every knob in `config.toml`, and the
portable USB-drive setup this KB actually runs in.

---

## 1. Where the KB lives

Resolution order for the KB root (PRD §10):

1. `--root <path>` flag (per command)
2. `KB_ROOT` environment variable
3. `~/arxiv-kb` (default)

Everything — canonical paper folders AND derived state — lives under
that one root:

```
<KB_ROOT>/
├── 2504.19874/              # canonical, yours (one folder per paper)
│   ├── metadata.json        # managed via commands (kb tag)
│   ├── notes.md             # the file you hand-edit (kb note)
│   ├── paper.pdf
│   ├── source/*.tex
│   └── sections.md          # derived; regenerated on reindex
└── .arxiv-kb/               # derived, machine-managed; never hand-edit
    ├── index.tv             # turbovec vector index
    ├── meta.db (+-wal,-shm) # chunk metadata + embedding cache (SQLite)
    ├── config.toml          # per-KB configuration (section 3 below)
    ├── kb.log               # diagnostics
    └── kb.pid               # present only while `kb watch` runs
```

Deleting `.arxiv-kb/` is always recoverable: `kb reindex` rebuilds it
from the paper folders. The embedding cache lives in `meta.db`, so a
reindex with an intact cache costs zero API calls.

## 2. Environment variables

| variable | purpose |
|---|---|
| `KB_ROOT` | KB folder path (this setup: `/Volumes/x/arxiv-kb`) |
| `OPENAI_API_KEY` | required for embeddings; never logged, never in errors |
| `KB_LOG_LEVEL` | `error` / `warn` / `info` / `debug` (overrides `--verbose`) |

This repo's convention: keep both exports in `.env.local.sh` at the
repo root (gitignored via `.env*`) and `source` it before working:

```bash
export OPENAI_API_KEY='sk-proj-…'
export KB_ROOT=/Volumes/x/arxiv-kb
```

## 3. config.toml reference

Created with defaults on first run at `<KB_ROOT>/.arxiv-kb/config.toml`.
Defaults as shipped:

```toml
schema_version = 1

[embedding]
provider = "openai"                 # "local" planned for v0.3
model = "text-embedding-3-small"
dimensions = 1536

[turbovec]
bit_width = 4                       # 2 | 4 (4 recommended)

[search]
default_k_narrow = 10
default_k_wide = 40
default_min_score_narrow = 0.3      # see note below
default_min_score_wide = 0.0

[ingest]
chunk_max_tokens = 2000
prefer_latex = true
pandoc_path = "pandoc"

[server]
http_port = 4321                    # HTTP server lands in v0.2
http_bind = "127.0.0.1"

[watcher]
debounce_ms = 2000
```

**Score floor note:** `text-embedding-3-small` scores clearly-relevant
matches around 0.45–0.60 (not near 1.0), hence the 0.30 narrow floor.
A KB initialized by an older build may still carry `0.72` in its own
config.toml — that hides everything; lower it by hand.

**Config-change policy (locked design decision):** changing
`embedding.model`, `dimensions`, or `bit_width` makes existing vectors
unusable — queries refuse with "run `kb reindex`". Changing
`chunk_max_tokens` only warns; results reflect old chunking until you
reindex. Nothing reindexes automatically.

## 4. This setup: portable KB on a USB drive

The KB root here is **`/Volumes/x/arxiv-kb`** on a removable APFS
volume carried between Macs. The layout is fully self-contained, so
the corpus, index, notes, and embedding cache all travel together.

### Why APFS matters

APFS (journaled) gives real `fsync` and atomic `rename`, which is what
the crash-safe persistence model assumes (addendum §5). Status: ideal,
nothing degraded. If the drive ever needs to serve a Windows/Linux
machine it would have to be exFAT — that still works, but with weaker
sync guarantees and per-OS builds of the binary.

### Per-machine setup (one-time, each Mac)

1. Install the binary: `cargo install --path /Volumes/x/kb`
   (release build → `~/.cargo/bin/kb`)
2. Install pandoc: `brew install pandoc`
3. `source` the env file (or add `KB_ROOT` to that machine's
   `~/.zshrc`). macOS mounts named volumes at the same path, so
   `/Volumes/x/arxiv-kb` is stable across machines.

### MCP registration (per machine)

Claude Code launches `kb mcp` as a subprocess that does not source
your shell profile — pin the env explicitly:

```bash
claude mcp add arxiv-kb \
  --env KB_ROOT=/Volumes/x/arxiv-kb \
  --env OPENAI_API_KEY=sk-proj-… \
  -- kb mcp
```

(Alternative: `-- kb --root /Volumes/x/arxiv-kb mcp` hardcodes the
path and needs only the key.)

### Habits that keep a removable KB safe

- **Eject properly**; never yank during `kb add` or while `kb watch`
  is running. Stop the watcher (ctrl-c) before ejecting — it holds the
  index open and owns `kb.pid`.
- **`kb verify` after plugging into a new machine** — a sub-second
  consistency check. If it ever complains, `kb reindex` is the
  always-correct fix (near-free thanks to the cache).
- If the volume isn't mounted, `kb` sees an empty root — commands
  don't break, `kb list` is just empty until you mount.
- Back up by copying the folder: `tar -czf kb-backup.tar.gz
  /Volumes/x/arxiv-kb/`. The paper folders are the only irreplaceable
  part.

### Optional: version the notes

The canonical layout doesn't fight git. Inside `/Volumes/x/arxiv-kb`:
`git init`, gitignore `.arxiv-kb/`, and your notes/tags history is
versioned on the drive too.
