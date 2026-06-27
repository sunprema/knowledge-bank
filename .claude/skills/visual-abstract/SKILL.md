---
name: visual-abstract
description: Generate a one-glance Mermaid concept diagram (visual abstract) of a paper in the arxiv-kb library — capturing its core mechanism/architecture as a flowchart — render it to an image, and store it with the paper. Use when the user wants a "visual abstract", "concept diagram", "mermaid diagram", "diagram this paper", "show the architecture", or "one-glance summary". Triggers include "visual abstract 2602.03359", "diagram tree of thoughts", "concept map for all papers".
---

# visual-abstract

Produce a **single, one-glance Mermaid concept diagram** that captures a paper's
core mechanism (data/control flow, architecture, or algorithm loop), render it to
a PNG, and store both the editable source and the image with the paper.

Why an image: the macOS app renders the Clean Read with a **custom native
`MarkdownText` renderer that does NOT draw `mermaid` fences** — so the diagram
must be delivered as `concept.png` (like `cover.png`). The `.md` source is kept
too, so it's editable and future-proof.

## Arguments

- `/visual-abstract <arxiv_id>` — one paper.
- `/visual-abstract all` — sweep the library.
- `/visual-abstract` (no arg) — ask which paper, or offer `all`.

## Where things live

- **Library root = `$KB_ROOT`** — read `/Volumes/x/kb/.env.local.sh`
  (`export KB_ROOT=/Volumes/x/arxiv-kb`) or `echo $KB_ROOT`. Never assume
  `~/arxiv-kb` (stale copy).
- Per paper: read `$KB_ROOT/<id>/sections.md` (body; fall back to `paper.pdf`),
  `metadata.json` (title). Write `$KB_ROOT/<id>/concept.md` (caption + source)
  and `$KB_ROOT/<id>/concept.png` (rendered image).

## Procedure (per paper)

1. **Resolve `$KB_ROOT`**; confirm `$KB_ROOT/<id>/` exists. For `all`, list dirs
   with a `metadata.json` (skip `.arxiv-kb/`).
2. **Read the FULL body** so the diagram reflects the real mechanism, not the
   abstract.
3. **Design ONE concept diagram** (guidance below).
4. **Write `concept.md`**: a one-line caption, then a fenced ```mermaid block with
   the source.
5. **Render to `concept.png`** (commands below).
6. **Show the user** the PNG with SendUserFile. For `all`, render each and end with
   a one-line summary.

## What makes a good one-glance diagram

- **≤ ~12 nodes.** Capture the CORE mechanism — the thing you'd whiteboard — not
  every detail. Ruthlessly drop minor pieces.
- **Show the flow**: `flowchart TD`/`LR` for architectures & pipelines; label edges
  with the transformation when it clarifies (`-- "top-b" -->`).
- **Use `subgraph`s to separate regimes/phases** (e.g. Training vs Inference,
  Encoder vs Decoder, per-step loop) — this is usually what makes it click.
- **Highlight the novel step** with a `classDef` so the eye lands on it.
- **Quote every label**; prefer ASCII-ish math (`x_t`, `M^l`, `e_t`, `σ`) over
  fragile combining-unicode; keep labels short. Use `<br/>` for line breaks.
- **One caption line** stating the key idea in plain words.
- Loops (e.g. tree/iterative search) read well as a cycle back to the start with a
  labeled exit edge.

## Rendering

Extract the mermaid source (the fenced block's contents) to a temp `.mmd`, then:

**Default — mermaid.ink (HTTP, fast, no local browser; NOTE: sends the diagram to
an external render service — fine for derived concept content):**
```
B64=$(base64 < concept.mmd | tr -d '\n')
curl -s -o "$KB_ROOT/<id>/concept.png" "https://mermaid.ink/img/$B64?type=png"
# SVG variant: https://mermaid.ink/svg/$B64
```
(Use `dangerouslyDisableSandbox` for the network call. Verify with `file` that the
output is a real PNG; if mermaid.ink returns a non-image, the source has a syntax
error — fix and retry.)

**Offline/private fallback — local mermaid-cli (downloads a headless browser on
first run; macOS has `gtimeout`, not `timeout`):**
```
npx -y @mermaid-js/mermaid-cli -i concept.mmd -o "$KB_ROOT/<id>/concept.png"
```

## In-app display (follow-up, not required to produce the artifact)

`concept.png` is stored next to `cover.png` but the Reader doesn't show it yet.
Surfacing it in-app is a small Swift addition (an image banner in
`ReaderView`/`PaperDetailView`, mirroring how `cover.png` is loaded). Mention this
as an optional next step; the diagram is still delivered to the user via
SendUserFile and saved on disk.
