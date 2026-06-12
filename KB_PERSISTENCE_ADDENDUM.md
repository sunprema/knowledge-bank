# KB_PERSISTENCE_ADDENDUM.md

**Companion to:** `KB_PROD_REQUIREMENTS.md`
**Document status:** Addendum v0.1
**Scope:** Persistence model for the turbovec index and `meta.db`.
**Audience:** Claude Code (implementer), maintainers.

---

## Why this document exists

The main PRD specifies *what* gets persisted (section 3 — folder layout,
section 4 step 10 — index persistence). It does not specify *how* the
in-memory turbovec index is kept in sync with disk, *when* writes happen,
or *what* should happen when the process exits ungracefully.

This addendum fills that gap. If anything here conflicts with the PRD,
the PRD wins — but nothing here should conflict; it should only clarify.

---

## 1. The persistence model in one paragraph

Turbovec is **in-memory by default**. The index lives in RAM during
execution and must be explicitly serialized with `index.write(path)` to
survive a process restart. On startup, the binary calls
`IdMapIndex::load(path)` to restore state. Between startup and
shutdown, the binary is the sole owner of the index — there is no
file-locking, no shared-memory access, and no automatic background
checkpointing inside turbovec itself. **We do the checkpointing
explicitly, after every mutation.**

---

## 2. The two storage layers

The system has two persistent stores. Understanding what lives where
prevents whole categories of confusion.

| store | type | what it holds | when written | when read |
|-------|------|---------------|--------------|-----------|
| `.arxiv-kb/index.tv` | turbovec serialized index (binary) | quantized vectors, IDs, codebook, calibration scalars | after every successful add/remove; on graceful shutdown | once at process startup; never re-read during a process lifetime |
| `.arxiv-kb/meta.db` | SQLite | chunk metadata (paper_id, section_type, snippet, page, hash, vector_id) | continuously via per-statement transactions | lazily, per query |

**Both are derived.** Both can be rebuilt from the user's canonical
source files (`metadata.json`, `source/*.tex`, `paper.pdf`, `notes.md`)
via `kb reindex`.

**Neither is the source of truth.** If either is deleted, corrupted, or
out of date, the answer is always "rebuild from canonical files,"
never "recover from a backup of the index."

---

## 3. Why `IdMapIndex`, not plain `TurboQuantIndex`

This is a one-time architectural decision that the implementer must
get right. The PRD mentions `IdMapIndex` but doesn't justify it; here
is why.

Plain `TurboQuantIndex` assigns **sequential, position-based** internal
IDs (0, 1, 2, ...). These IDs shift when entries are removed: deleting
ID 5 in a 10-element index either renumbers everything (so old `IDs > 5`
become invalid) or leaves a hole (so search results might reference IDs
that no longer have meaningful data). Either way, an external pointer
like `meta.db.chunks.vector_id = 42` becomes garbage after any deletion.

`IdMapIndex` accepts **caller-supplied external IDs** (uint64) and
maintains a stable mapping internally. You call:

```rust
index.add_with_ids(&vectors, &[external_id_1, external_id_2, ...])?;
index.remove(external_id_1);   // O(1) by external ID
let (scores, ids) = index.search(query, k);   // returns external IDs
```

External IDs survive across:
- Deletions
- Index serialization and reload
- Reindex from scratch (if we keep the same ID-assignment scheme)

Our `meta.db.chunks.vector_id` column stores these external IDs.
They're the joining key between the two stores.

**ID assignment scheme.** Use a monotonically-increasing `INTEGER
PRIMARY KEY AUTOINCREMENT` in `meta.db.chunks`. SQLite gives us
collision-free unique integers. We use that integer as both the
chunk's PK and the vector's external ID in turbovec. This makes the
join trivial and the ID source single.

---

## 4. When we save the index

After every successful mutation:

- After `kb add` completes ingest of one paper
- After `kb remove` deletes a paper
- After `kb update` re-ingests a paper
- After the watcher re-embeds a notes.md change
- After `kb reindex` finishes a rebuild
- On graceful shutdown (SIGINT, SIGTERM) — defensive

We do **not** save:

- After read-only operations (search, list, show)
- During an ingest in progress (only after it completes successfully)
- At a periodic interval (rejected — see section 6)

The save cost at our scale is small: writing ~5000 vectors at 1536-dim
4-bit takes ~50ms and produces ~3MB on disk. We can afford to do it on
every change.

---

## 5. The write sequence (critical correctness detail)

Every mutation involves two stores. We must order writes so that a
crash between them leaves the system in a recoverable state — never a
silently corrupt one.

### For add operations

```
1. Compute all chunks for the paper
2. Call embedding API for each chunk (network calls — slow)
3. BEGIN TRANSACTION on meta.db
4. INSERT chunk rows into meta.db.chunks (get auto-assigned IDs)
5. Call index.add_with_ids(vectors, chunk_ids) — in memory
6. Call index.write(".arxiv-kb/index.tv.tmp") — write to temp file
7. fsync the temp file
8. Rename tmp → final: ".arxiv-kb/index.tv.tmp" → ".arxiv-kb/index.tv"
9. COMMIT meta.db transaction
10. Log success
```

**Why this order:**

- Steps 1-2 do all the network work first; if anything fails here,
  we haven't touched either store
- Step 3 opens a SQLite transaction but doesn't commit
- Step 5 mutates in-memory only — no disk write yet
- Steps 6-8 use the atomic-rename pattern: a partially-written
  `index.tv.tmp` is harmless; only the final rename makes it visible
- Step 9 commits the SQLite transaction only after the index is
  durably on disk

If we crash between steps 8 and 9: index has the new vectors, meta.db
doesn't. Next startup detects the mismatch (see section 7) and
recovers.

If we crash before step 8: meta.db transaction was never committed; on
restart, neither store has the new chunks. Clean state.

If we crash between steps 5 and 6: in-memory state is ahead of disk,
but we never persisted it. Disk is in the pre-add state. On restart,
both stores are pre-add. Clean.

**The atomic rename in steps 7-8 is non-negotiable.** Without it, a
crash mid-write produces a truncated `index.tv` that fails to load and
forces a reindex.

### For remove operations

```
1. BEGIN TRANSACTION on meta.db
2. Look up chunk IDs for the paper
3. Call index.remove(chunk_id) for each — in memory
4. Call index.write(".arxiv-kb/index.tv.tmp") + fsync + rename
5. DELETE rows from meta.db.chunks
6. COMMIT meta.db
```

Same atomic-rename pattern. Same crash safety guarantees.

### For watcher-triggered re-embeds

Same sequence as add, but with `INSERT OR REPLACE` semantics on the
affected chunks. The chunk's `vector_id` stays the same (we look it
up by `paper_id + section_type + ordinal`), so the index update is
`remove` + `add_with_ids` using the same external ID.

---

## 6. Why not periodic checkpointing or write-ahead logging

We considered three alternatives to "save after every change":

**Periodic checkpointing** ("save every 30 seconds if dirty"):
Rejected. Adds complexity (a background timer, dirty-flag tracking)
for a problem we don't have. Our mutation rate is low — maybe one a
day. Per-mutation save is fine.

**Write-ahead log:** Rejected for v0.1. Right answer for high-write
systems, overkill for ours. Could be revisited if we ever do bulk
imports of 1000+ papers where the per-paper save cost adds up.

**Batched saves during bulk import:** Worth doing **only** for `kb
import bibliography.bib` (v0.4). In that mode, save once at the end
instead of after every paper. Until v0.4 ships, not relevant.

Per-mutation save with atomic rename is the right primitive for v0.1.

---

## 7. Startup consistency check

Every process startup performs this check before serving requests:

```
1. Open meta.db (creates if missing)
2. Read schema_version; if newer than binary supports, exit with clear error
3. If .arxiv-kb/index.tv exists:
     a. Try IdMapIndex::load(".arxiv-kb/index.tv")
     b. If load fails (corrupted), log error, fall through to step 4
   Else: fall through to step 4
4. If load succeeded:
     a. Count rows in meta.db.chunks: M
     b. Count vectors in loaded index: N
     c. If M != N: log warning, set "consistency_check_failed = true"
     d. Spot-check: sample 10 chunk_ids from meta.db, verify each is in the index
     e. If any missing: set "consistency_check_failed = true"
5. If load failed OR consistency_check_failed:
     a. In query modes (search, MCP, HTTP): refuse to start, print
        "index out of sync — run `kb reindex` to rebuild"
     b. In ingest modes (add, watch): proceed with whatever's loaded;
        the next ingest will write a fresh consistent state
     c. In reindex mode: proceed (this is the recovery path)
```

The spot-check in step 4d catches the case where the two stores have
the same count but different contents. Sampling 10 entries is cheap
and catches most corruption.

This check runs every startup, takes <50ms on a 5000-chunk index, and
costs nothing. It's the safety net that lets us trust the persistence
model.

---

## 8. `kb reindex` — the recovery path

`kb reindex` is the answer to every persistence problem. It rebuilds
both `index.tv` and `meta.db` from canonical source files alone.

```
1. Confirm with user (unless --yes): "this will rebuild the index from
   N papers and may take ~Ns. Continue?"
2. Move .arxiv-kb/index.tv to .arxiv-kb/index.tv.backup (don't delete yet)
3. Begin a fresh in-memory IdMapIndex with same config (bit_width, dim)
4. Drop and recreate meta.db.chunks table
5. For each paper folder under the KB root:
     a. Read metadata.json
     b. Read sections.md (or PDF-extract if no LaTeX)
     c. Read notes.md
     d. Run the section classifier
     e. For each chunk:
        - INSERT into meta.db.chunks (auto-assigns new vector_id)
        - Compute hash; check cache; embed if needed
        - index.add_with_ids(vector, chunk_id)
     f. Save progress to log every 10 papers
6. index.write(".arxiv-kb/index.tv.tmp") + fsync + rename
7. Commit meta.db
8. Delete .arxiv-kb/index.tv.backup
9. Print summary: N papers, M chunks, S seconds, $C in embedding spend
```

`kb reindex` is idempotent. Running it twice in a row produces the
same final state (assuming no source changes between runs, and using
the embedding cache so we don't repay the API cost).

**The backup in step 2 is the safety net.** If reindex itself crashes,
the user can manually restore `index.tv.backup`. We delete it only
after the new index is safely committed.

---

## 9. Embedding cache (lives in meta.db)

Re-embedding the same text via the OpenAI API is wasteful. We cache.

```sql
CREATE TABLE embedding_cache (
  content_hash      TEXT PRIMARY KEY,
  embedding_model   TEXT NOT NULL,
  embedding_version INTEGER NOT NULL,
  vector_bytes      BLOB NOT NULL,           -- raw f32 little-endian
  cached_at         TEXT NOT NULL
);
```

On every chunk embed:

1. Compute content hash
2. Look up `(content_hash, embedding_model, embedding_version)` in cache
3. If hit: deserialize and use, skip the API call
4. If miss: call API, store in cache, use

This cache survives `kb reindex` — that's the whole point. A reindex
of 100 papers shouldn't re-pay $0.40 in embedding costs just to
rebuild from scratch.

Cache invalidation happens automatically when:

- `embedding_model` changes (configured in `config.toml`)
- `embedding_version` is bumped (manual, when API output changes
  semantically — e.g. OpenAI ships a new model under the same name)
- The user runs `kb cache clear`

The cache is small per entry (~6 KB for a 1536-dim float32 vector) but
grows linearly with corpus size. Pruning is manual via `kb cache gc`
which removes entries with no matching `content_hash` in `chunks`.

---

## 10. What's in `.arxiv-kb/` at rest

After a graceful shutdown:

```
.arxiv-kb/
├── config.toml                 # embedding model, bit_width, schema version
├── index.tv                    # turbovec serialized index (binary)
├── meta.db                     # SQLite — chunks, pdf_toc, tags, embedding_cache
├── meta.db-wal                 # SQLite write-ahead log (may be present)
├── meta.db-shm                 # SQLite shared memory file (may be present)
├── api_key                     # HTTP server auth key (mode 0600)
├── kb.log                      # diagnostics log (rotating)
└── kb.pid                      # only present when watcher is running
```

After a hard crash:

```
.arxiv-kb/
├── ...
├── index.tv                    # last successfully renamed state
├── index.tv.tmp                # MAY exist — incomplete write, safe to ignore
├── meta.db                     # SQLite handles its own crash recovery
├── kb.pid                      # MAY be stale — kb status uses kill -0
└── ...
```

On next startup, the binary cleans up:

- Remove `index.tv.tmp` if present (it's incomplete by definition)
- Check `kb.pid`; if the PID doesn't exist, remove the file
- SQLite recovers from its own WAL automatically

None of this requires user intervention.

---

## 11. Backup and portability

The KB root folder is fully self-contained. To back it up:

```bash
tar -czf arxiv-kb-backup-2026-05-24.tar.gz ~/arxiv-kb/
```

To migrate to a new machine:

```bash
# on new machine
mkdir ~/arxiv-kb
tar -xzf arxiv-kb-backup-2026-05-24.tar.gz -C ~
# done — kb works
```

Two caveats:

1. **The embedding model must match.** If the source machine used
   `text-embedding-3-small` and the new machine's config wants a
   different model, `kb reindex --force` is required. The cache will
   be invalidated and re-embedding will happen automatically.

2. **`.arxiv-kb/api_key` is sensitive.** When sharing a KB folder
   between developers (rare but possible), exclude or regenerate the
   API key. Add `.arxiv-kb/api_key` to `.gitignore` if the KB is in a
   git repo.

The user's canonical files (`metadata.json`, `paper.pdf`, `source/`,
`notes.md`, `sections.md`) are the only thing that *really* needs to
be backed up. Everything in `.arxiv-kb/` is regeneratable.

---

## 12. Implications for testing

The test suite should cover these persistence invariants:

- **Round-trip:** add a paper, exit, restart, search — same results
- **Atomic rename:** kill the process between `index.write(tmp)` and
  rename; on restart, `index.tv` is the pre-write state, not corrupted
- **Two-store consistency:** after a successful add, M chunks in
  meta.db ↔ M vectors in index
- **Crash mid-add:** kill between meta.db transaction begin and
  commit; restart finds clean pre-add state
- **Reindex:** delete `.arxiv-kb/index.tv` and `meta.db.chunks` table,
  run reindex, verify all papers come back with same content
- **Embedding cache hits:** reindex twice, verify zero API calls on
  second run

Integration test fixtures should include a "kill the process mid-write"
helper via SIGKILL plus a controlled write barrier.

---

## 13. Persistence decisions (resolved 2026-06-12)

Resolved with the user before v0.1 implementation began.

- [x] **Historical index snapshots for rollback**: no. Canonical files
      + the embedding cache make `kb reindex` a cheap, complete
      recovery path, and reindex already keeps a `.backup` during the
      rebuild itself.
- [x] **Config changes triggering reindex**: never automatic. A config
      fingerprint is stored in meta.db; vector-incompatible changes
      (embedding model/dim, bit_width) refuse queries until
      `kb reindex`, chunking-only changes (chunk_max_tokens) serve
      with a warning. See PRD section 16.
- [x] **WAL mode for meta.db**: yes. Better write performance, readers
      don't block the writer (matters when `kb watch` + `kb mcp` run
      simultaneously). The `-wal`/`-shm` sidecars are already in the
      folder layout (section 10).
- [x] **`kb verify` command**: yes, in v0.1 alongside `kb status`.
      Exposes the startup consistency check (section 7) on demand,
      plus a full chunk-by-chunk check behind `--deep`. Also gives the
      test suite a natural assertion hook.

---

## 14. Summary for the implementer

What you need to remember:

1. **Use `IdMapIndex`.** Not plain `TurboQuantIndex`.
2. **External IDs come from `meta.db.chunks.id`** (SQLite autoincrement).
3. **Save after every mutation**, using temp-file-plus-rename.
4. **Run the consistency check on every startup**, refuse to serve
   queries if it fails, point user at `kb reindex`.
5. **`kb reindex` rebuilds everything** from canonical files, using
   the embedding cache to avoid re-paying API costs.
6. **The cache lives in `meta.db`** keyed by content hash + model + version.
7. **The user's canonical files are the only thing that really matters.**
   Indexes and DBs are scaffolding.

If you find yourself writing code that violates any of these, stop and
ask before continuing.
