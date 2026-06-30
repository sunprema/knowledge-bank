---
name: implement-paper
description: Turn a paper in the arxiv-kb library into a concrete implementation spark for the user's Elixir/Ash/Phoenix-LiveView/Spark-DSL/Ash-Reactor stack — one deep, best-fit architecture mapping that sparks a buildable prototype — and capture it as an idea (kb_capture_idea) plus a pointer note on the paper. Use when the user wants to "implement a paper", "how would I build this in Elixir/Ash", "spark implementation ideas", "map this paper to my stack", or "turn this paper into a solution". Triggers include "implement 2303.11366", "how do I build this in Ash", "implementation ideas for all papers".
---

# implement-paper

Given a paper, produce ONE deep, best-fit way to build its core idea in the
user's stack — an idea-generative architecture mapping (not a literal port) that
plays to BEAM's strengths — and capture it as a reusable idea in the KB.

## The user's stack (the target of every mapping)

- **Elixir / OTP / BEAM** — GenServers, supervision trees, `DynamicSupervisor`,
  Tasks, massive lightweight concurrency, fault isolation, distribution, PubSub.
- **Ash Framework** — declarative resources (attributes, actions, relationships,
  calculations, policies) as the domain/data layer.
- **Phoenix LiveView** — real-time server-rendered UI, streams, PubSub updates.
- **Spark DSL** — the DSL toolkit Ash is built on; use when a declarative,
  user-facing config surface would beat hand-wired code.
- **Ash Reactor** — saga / dependency-graph workflow orchestration with steps,
  async, and compensation (undo) — the natural home for multi-step agent loops.

(Also recorded in the `user-stack` auto-memory. If that memory has changed,
prefer it.)

## Arguments

- `/implement-paper <arxiv_id>` — one paper (e.g. `2303.11366`).
- `/implement-paper all` — sweep every paper in the library.
- `/implement-paper` (no arg) — ask which paper, or offer `all`.

## Where things live

- **Library root = `$KB_ROOT`. Resolve it FIRST** — read
  `/Volumes/x/kb/.env.local.sh` (`export KB_ROOT=/Volumes/x/arxiv-kb`) or
  `echo $KB_ROOT`. Do NOT assume `~/arxiv-kb` (a stale copy lives there).
- Per paper: `$KB_ROOT/<id>/sections.md` (body — read this), `paper.pdf`
  (fallback), `metadata.json` (title/abstract).

## Procedure (per paper)

1. **Resolve `$KB_ROOT`** and confirm `$KB_ROOT/<id>/` exists. For `all`, list
   dirs under `$KB_ROOT/` with a `metadata.json` (skip `.arxiv-kb/`).
2. **Read the FULL body** (`sections.md` in its entirety; fall back to `paper.pdf`).
   You must understand the actual mechanism, not just the abstract.
3. **Pick the SINGLE best-fit implementation angle.** Choose the framing where
   this paper's mechanism maps most naturally onto the stack (usually:
   orchestration/loops → Reactor; per-entity state → GenServers; domain/memory →
   Ash resources; live observation → LiveView; declarative config → Spark DSL).
   If the paper is a poor fit for the stack (pure numerics, hardware, theory),
   say so honestly and give the most useful angle anyway (often "wrap the
   algorithm as a Rust NIF / external service and orchestrate around it").
4. **Write the spark** following the structure below — go deep on the one angle.
5. **Capture it** (see "Output").
6. For `all`, do them sequentially; end with a one-line summary.

## Spark structure (one angle, deep)

- **Angle** — one line naming the chosen framing.
- **Mapping table** — paper mechanism → stack primitive (Ash resource / Reactor
  step / OTP process / LiveView / Spark DSL), one row per mechanism.
- **Architecture sketch** — concretely:
  - *Ash resources*: name them with key attributes, the important actions, and
    relationships.
  - *Ash Reactor*: the steps, their dependencies, what runs async/in parallel,
    and any compensation (undo) for saga-style steps.
  - *OTP process model*: what is a GenServer/Task, what supervises it, what runs
    concurrently (this is the BEAM payoff — be specific).
  - *LiveView surface*: what the user watches in real time, via PubSub/streams.
  - *Spark DSL* (only if it earns its keep): a short sketch of the declarative
    API it would expose.
- **Why BEAM helps** — the specific leverage for THIS paper (concurrency scale,
  fault isolation, soft-realtime, distribution, hot code reload). Don't be generic.
- **The honest seam** — the parts BEAM is bad at (embeddings, quantization,
  training, GPU matmul). Route them to **Nx/Bumblebee/Axon**, a **Rust NIF
  (Rustler)**, or **the existing Rust KB engine / TurboVec** over a port — do NOT
  pretend pure Elixir is the right tool. Naming this seam is what makes the spark
  real.
- **First vertical slice** — the smallest end-to-end thing to build first (1–2
  resources + 1 Reactor + a minimal LiveView), so it sparks immediate action.
- **Key libraries** — the handful actually needed (e.g. `ash`, `reactor`,
  `ash_reactor`, `phoenix_live_view`, `req`, `bumblebee`, `nx`, `oban`,
  `broadway`, `rustler`).
- *(Optional, one line)* **Runner-up angle** — the next-best framing, named only.

## Output

1. **Capture as an idea** with `kb_capture_idea`:
   - `project`: `"elixir-ash"` (so sparks across papers accumulate and cross-link)
   - `title`: `"Impl spark: <paper short title> → <angle>"`
   - `body`: the full spark (structure above), in Markdown
   - `tags`: `["implementation", "elixir", "ash", "reactor", <paper-topic>]`
   - `upsert_key`: `"<arxiv_id>-impl"` (so re-running updates rather than duplicates)
   - `links`: the paper id / arxiv URL if accepted by the tool
2. **Leave a pointer** on the paper with `kb_add_note(paper_id, ...)`: a one-line
   note like `Implementation spark (Elixir/Ash) captured as idea — project
   'elixir-ash', see kb_search.` Keep the full content in the idea, not the note.

## Embedding note (surface it) — and the capture/note 401 difference

The two write tools behave DIFFERENTLY on an invalid `OPENAI_API_KEY` (HTTP 401):

- **`kb_add_note`** persists to `notes.md` first, then *defers* embedding — the
  content is saved; only search indexing waits. Not a failure.
- **`kb_capture_idea`** embeds as part of creation and **hard-fails on 401 —
  nothing is saved.**

The MCP server uses the env it was LAUNCHED with, so a stale key there can't be
fixed from a shell — but the `kb` CLI picks up a freshly-sourced key. Recovery
order when `kb_capture_idea` returns a 401:

1. **Preferred — capture via the CLI with the live key** (saves AND embeds):
   write the spark body to a temp file, then
   ```
   source /Volumes/x/kb/.env.local.sh && \
     cat <body.md> | kb idea add --project elixir-ash \
       --title "<title>" --tags "implementation,elixir,ash,reactor" \
       --link <arxiv_id> --body -
   ```
   (`.env.local.sh` exports a valid `OPENAI_API_KEY`; re-running with the same
   title/id updates in place. Use `dangerouslyDisableSandbox` for the network call.)
2. **Fallback if the CLI/key is unavailable** — `kb_add_note` the FULL spark into
   the paper's `notes.md` (prefixed with its intended idea project + title so it
   can be re-captured later), and tell the user it's saved but not yet a
   searchable idea until the key is fixed and `/implement-paper <id>` is re-run.

After CLI capture, embedding is already done. To also pick up any previously
deferred notes, run `source /Volumes/x/kb/.env.local.sh && kb reindex --yes`
(embedding cache keeps it cheap).

Reminder: `kb_capture_idea` `upsert_key` must be a slug — no dots (use
`2305-10601-impl`, not `2305.10601-impl`).
