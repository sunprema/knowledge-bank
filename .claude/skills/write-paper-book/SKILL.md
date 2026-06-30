---
name: write-paper-book
description: Turn a paper in the arxiv-kb library into a stunning, self-contained multi-page HTML book — a cover + table of contents, one page per important idea, and a cheatsheet — grounded in the paper's full body (sections.md / paper.pdf) with a bespoke visual design that fits the subject. Light theme, two-page open-book spread, page-turn animations, inline-SVG diagrams. The book is written into $KB_ROOT/<id>/book/ so the KB app can read it in-place. Use when the user wants to "make a book", "build a paper book", "turn this paper into a book", "write a book for <arxiv_id>", "build books for all papers", or "expand/add a chapter" in an existing paper book. Triggers include "book 2504.19874", "make a beautiful book for this paper", "build paper books for all".
---

# write-paper-book

Turn a single paper into a **rich, beautiful, multi-page HTML book** the KB app
reads in an embedded WebView (and the user can also open in a browser). This is the
**generative** sibling of `analyze-paper` / `clean-paper`: where those write text
notes, this builds a designed, paginated *book* — cover, chapters, cheatsheet — with
a bespoke skin that fits the paper. The value is **accurate, well-organized content
drawn from the real paper body** in a **gorgeous, topic-fitting design**.

Think like a great technical-book author who has *just read this paper closely* and
is now teaching it: lay out the ideas in the order that makes them click, draw the
diagrams the paper only describes in prose, and keep every claim faithful to the
text.

## Arguments

- `/write-paper-book <arxiv_id>` — build (or rebuild) the book for one paper
  (e.g. `2504.19874`). This is the path the app's right-click "Build paper book"
  uses.
- `/write-paper-book all` — sweep every paper in the library that doesn't yet have
  a `book/` (skip ones already built unless asked to regenerate).
- `/write-paper-book expand "<chapter>" for <arxiv_id>` — add and generate one new
  chapter page in an existing book (don't regenerate the whole book).
- `/write-paper-book` (no arg) — ask which paper, or offer `all`.

## Where things live

- **Library root = `$KB_ROOT`. Resolve it FIRST — do not assume `~/arxiv-kb`.**
  The repo exports it in `/Volumes/x/kb/.env.local.sh`
  (`export KB_ROOT=/Volumes/x/arxiv-kb`). Read that file (or `echo $KB_ROOT` if
  already exported). A stale copy may exist at `~/arxiv-kb` with far fewer papers —
  use the `$KB_ROOT` value.
- **Per paper input:** `$KB_ROOT/<id>/` containing
  - `metadata.json` — `title`, `authors`, `abstract` (the arXiv summary, *not* an
    analysis), `categories`, `published_at`. Read it.
  - `sections.md` — the extracted + classified paper body. **This is your primary
    source — read it in full** (it can be large; page through it, don't build from
    the first screen).
  - `paper.pdf` — fallback if `sections.md` is missing/empty (read the PDF directly).
  - `notes.md` — may already hold a `## Claude Code analysis` (from `analyze-paper`)
    or a Clean Read; skim it for structure, but ground the book in the paper itself.
- **The book you generate goes in `$KB_ROOT/<id>/book/`** — right next to the
  paper's data, so it travels with the paper and the app finds it by convention:
  ```
  $KB_ROOT/<id>/book/
    book.json                  # the manifest (you write this)
    index.html                 # cover + table of contents (the landing page)
    concepts/01-<slug>.html    # one page per chapter, in order
    concepts/02-<slug>.html
    cheatsheet.html            # a one-page quick reference
    assets/book.css            # the book's bespoke stylesheet
    assets/book.js             # the two-page pager (below) + any small extras
    assets/img/*               # images the user drops onto image slots (offline)
  ```
  The KB app shows the book when `$KB_ROOT/<id>/book/index.html` exists.

## book.json schema

```json
{
  "paper_id": "2504.19874",
  "title": "Tree of Thoughts, As a Field Guide",
  "paper_title": "Tree of Thoughts: Deliberate Problem Solving with Large Language Models",
  "status": "ready",
  "created": "2026-06-30",
  "summary": "A first-principles tour of ToT: framing, the search abstraction, the four design choices, and where it wins.",
  "concepts": [
    {
      "id": "the-idea",
      "title": "The Idea: Thinking as Search",
      "file": "concepts/01-the-idea.html",
      "status": "ready",
      "source": "claude"
    }
  ],
  "images": []
}
```

Field rules:
- `paper_id` — the library id (the folder name). Set once, never change.
- `paper_title` — the real paper title from `metadata.json`; `title` is the book's
  (you may give the book a punchier title, but stay honest).
- `status` (book): `requested` → `ready` once `index.html` exists and every chapter
  is written. Use `building` only if you stop midway.
- `concepts[].status`: `requested` until its `file` is written, then `ready`.
- `concepts[].file`: path **relative to the book folder** (`concepts/NN-slug.html`).
- `concepts[].source`: `user` (the person asked for it) or `claude` (you chose it).
  **Never drop or reorder user chapters.**
- `created`: ISO `yyyy-MM-dd`; set once and leave it.
- `images[]`: image slots (see Images & diagrams).

## Procedure (building a book)

1. **Resolve `$KB_ROOT`** (see above), then confirm `$KB_ROOT/<id>/` exists.
2. **Read the FULL paper.** Read `sections.md` end to end (fall back to `paper.pdf`
   if absent). Read `metadata.json` for title/authors/abstract. Skim `notes.md` for
   any existing analysis. You must understand the paper's actual mechanism, results,
   and limitations — not just its abstract.
3. **Light web context (allowed, secondary).** You may use WebSearch/WebFetch to
   nail background a reader needs (a prerequisite definition, what a baseline is, a
   related-work pointer) and to get version-accurate facts right. **Ground every
   claim in the paper;** use the web for teaching context, never to invent results
   the paper doesn't report. Prefer primary sources. The *generated book* must stay
   offline (no external links/fonts/scripts) — see Self-contained.
4. **Decide the chapters.** Choose ~6–12 chapters that make the paper *click*, in an
   order where each builds on the last. A strong default arc for a research paper:
   - **The Problem** — what's broken / the question, in concrete terms.
   - **The Idea** — the core insight in one breath, then unpacked.
   - **How It Works** — the mechanism/architecture, step by step (the heart — draw
     it as SVG).
   - **The Design Choices** — the non-obvious decisions and why they matter.
   - **The Math / Guarantees** — key formulation, bounds, or objective (only as deep
     as the paper goes; keep it legible).
   - **Does It Work?** — the experiments and the *real numbers* (quote them).
   - **Limits & Open Questions** — caveats, failure modes, what's next.
   - **So What** — why it matters / how you'd use it.
   Adapt to the paper — a systems paper, a theory paper, and a survey want different
   arcs. Include every `source: "user"` chapter.
5. **Design a bespoke skin that fits *this* paper** (see Design). The *structure* is
   consistent across all paper books; the *skin* is the paper's — a diffusion paper
   shouldn't look like a database paper.
6. **Write the pages:**
   - `assets/book.css` — the full, self-contained stylesheet.
   - `assets/book.js` — the two-page pager (reference below) + any tiny extras.
   - `concepts/NN-slug.html` — one page per chapter (`NN` = 01, 02, … in order).
   - `index.html` — the cover + linked table of contents.
   - `cheatsheet.html` — a dense quick reference: the 30-second summary, the key
     equation(s), the headline numbers, the glossary, and the citation.
   - Draw explanatory diagrams as **inline SVG**; for real images you can't draw,
     leave an **image slot** (see Images & diagrams).
7. **Write `book.json`** — every chapter's `file` + `status: "ready"`, the book
   `status: "ready"`, a one-line `summary`, and any image slots in `images[]`.
8. **Verify it turns from `file://`** before declaring done (open `index.html`, press
   → a few times). Tell the user to open the paper in the KB app and click **Book**
   (or press ⌘R / reopen if it was already open).

## Expanding a chapter

To fulfil `expand "<chapter>" for <id>`: read that one topic from the paper, write
its `concepts/NN-slug.html` (next number in sequence), **add it to the table of
contents in `index.html`** and into the prev/next chain, append it to `book.json`'s
`concepts[]` (`source: "user"`, `status: "ready"`), and stop. Don't regenerate the
rest.

## Design — beautiful, paper-fitting, self-contained

The bar is "an exciting HTML book about this paper", not a plain doc. Within a
**consistent house structure**, give each book a **bespoke theme derived from the
paper's subject**.

- **House structure (consistent):** every page shares `assets/book.css`; a sticky
  top bar with the book title + links to Contents and Cheatsheet; each chapter page
  has a heading, the body, and **prev / next / contents** navigation at the bottom.
  `index.html` is a cover hero + a numbered, linked table of contents.
- **Light theme.** The KB reader and a plain browser both expect a light page;
  design a light, paper-like background (a faint grain/grid is nice). Keep strong
  contrast and a clear type scale.

### Navigation contract (required)

Mark the nav links so the app's page-turn keyboard shortcuts work. The KB app binds
**→ next page**, **← previous page**, **↑ first page** by following the page's nav
anchors. On every page give them the right `rel`:
- next-page link → `rel="next"` (omit on the last page)
- previous-page link → `rel="prev"` (omit on the first page)
- the Contents/cover link → `rel="home"` (points to `index.html`)

e.g. `<a rel="next" href="03-how-it-works.html">Next ›</a>`,
`<a rel="home" href="../index.html">Contents</a>`. Chapter pages live in
`concepts/`, so `home` is `../index.html`; on `index.html` it's `index.html`. Order
the chain `index.html → 01 → 02 → … → cheatsheet.html`. Keep the links visible too,
for mouse users and plain-browser viewing.

### Two-page spread (the open-book layout) — default

Render each page as an **open physical book**: two pages side by side with a center
spine, filling the viewport, with a **page-flip** when turning. Long content
paginates into multiple spreads *within* the same file; turning past the last/first
spread moves to the next/previous file.

How it works (self-contained, works from `file://`):
- **Paginate with CSS multicolumn.** Put the page's readable content in a tall,
  fixed-height column box; the browser flows it into page-width columns. Two columns
  visible = one spread; translate the box horizontally to turn spreads.
- **CRITICAL — pin the leaf to ONE column wide.** `book.js` must set
  `leaf.style.width = colW` (one page wide), not leave it at viewport width. If the
  leaf is full-width, the browser fits a *single* column into it and stretches that
  column to the whole viewport, so content (capped by `max-width`) pins to the LEFT
  half and **the right page stays blank** while the page counter inflates. With the
  leaf pinned to `colW`, overflow produces real `colW`-wide columns spaced by
  `colW+gap` that correctly land on alternating left/right pages.
- **A pager (`assets/book.js`) exposes `window.kbPager`** with `next/prev/home`. The
  KB app's page-turn keys prefer this pager (it turns spreads within a page and
  jumps files at the edges); the same `book.js` also wires its own arrow-key handler
  so it works in a plain browser. To avoid double-turning when the app is hosting,
  the in-page handler early-returns if `window.__kbNav` is set (the app sets it).

Structure:

```html
<div class="book-viewport">           <!-- clips to the visible spread -->
  <article class="book-leaf">         <!-- content; multicolumn flows it into pages -->
    …all the page's content (headings, prose, code, figures, image slots)…
  </article>
  <div class="book-spine" aria-hidden="true"></div>   <!-- center gutter shadow -->
</div>
<nav class="book-nav">                <!-- visible controls + the rel contract -->
  <a rel="prev" href="01-the-problem.html">‹ Prev</a>
  <a rel="home" href="../index.html">Contents</a>
  <span class="book-pageno"></span>
  <a rel="next" href="03-how-it-works.html">Next ›</a>
</nav>
```

CSS skeleton (theme the colors/fonts per paper; keep the mechanics):

```css
.book-viewport{ position:fixed; inset:0; overflow:hidden; }
.book-leaf{
  height:100vh; box-sizing:border-box; padding:6vh 0;  /* NO horizontal padding — it fights the pinned column width */
  column-fill:auto;                      /* fill each column fully, overflow rightward */
  transition: transform .5s cubic-bezier(.6,.02,.2,1);   /* the page-flip */
  /* width + column-width are set by book.js to ONE column wide (see pager) */
}
/* centre every block in its column so text isn't pinned to the gutter and both
   pages read evenly. !important beats per-element `margin` shorthands while
   leaving their top/bottom intact. min(...,100%) so a narrow column never overflows */
.book-leaf > *{ max-width:min(34rem,100%); margin-left:auto !important; margin-right:auto !important; }
/* keep ATOMIC blocks whole across the gutter; let paragraphs break so columns fill */
.book-leaf figure, .book-leaf .callout, .book-leaf table,
.book-leaf pre, .book-leaf h1, .book-leaf h2, .book-leaf h3{ break-inside:avoid; }
.book-spine{ position:fixed; top:0; bottom:0; left:50%; width:2px; transform:translateX(-1px);
  box-shadow:0 0 22px 10px rgba(0,0,0,.10); pointer-events:none; }
.book-nav{ position:fixed; left:0; right:0; bottom:0; display:flex; gap:1.5rem;
  justify-content:center; align-items:center; padding:.6rem; }
```

Pager reference (`assets/book.js`) — adapt, but keep the contract and the
`window.kbPager` + `window.__kbNav` names:

```js
(function(){
  var leaf = document.querySelector('.book-leaf');
  var vp   = document.querySelector('.book-viewport');
  if(!leaf || !vp) return;
  var i = 0, total = 1, spread = 1;
  function layout(){
    var W = vp.clientWidth, gap = Math.round(W * 0.07), colW = Math.floor((W - gap) / 2);
    // CRITICAL: pin the leaf to ONE column wide. Without this the single in-box
    // column stretches to the full viewport and only the LEFT page ever fills.
    leaf.style.width = colW + 'px';
    leaf.style.columnWidth = colW + 'px';
    leaf.style.columnGap = gap + 'px';
    var step = colW + gap;                                  // distance between columns
    spread = 2 * step;                                     // distance per two-page spread
    var cols = Math.max(1, Math.round((leaf.scrollWidth + gap) / step));
    total = Math.max(1, Math.ceil(cols / 2));
    i = Math.min(i, total - 1);
    render();
  }
  function render(){
    leaf.style.transform = 'translateX(' + (-i * spread) + 'px)';
    var n = document.querySelector('.book-pageno');
    if(n) n.textContent = (i + 1) + ' / ' + total;
  }
  function href(rel){ var a = document.querySelector('a[rel~="' + rel + '"]'); return a && a.href; }
  window.kbPager = {
    next: function(){ if(i < total-1){ i++; render(); } else { var h=href('next'); if(h) location.href = h; } },
    prev: function(){ if(i > 0){ i--; render(); } else { var h=href('prev'); if(h) location.href = h + '#last'; } },
    home: function(){ var h=href('home'); if(h) location.href = h; }
  };
  window.addEventListener('resize', layout);
  window.addEventListener('load', function(){ layout(); if(location.hash === '#last'){ i = total-1; render(); } });
  layout();
  // Plain-browser keyboard support; the KB app handles keys itself (it sets
  // window.__kbNav), so defer to it to avoid turning twice.
  document.addEventListener('keydown', function(e){
    if(window.__kbNav || e.metaKey || e.ctrlKey || e.altKey) return;
    if(e.key === 'ArrowRight'){ kbPager.next(); e.preventDefault(); }
    else if(e.key === 'ArrowLeft'){ kbPager.prev(); e.preventDefault(); }
    else if(e.key === 'ArrowUp'){ kbPager.home(); e.preventDefault(); }
  });
})();
```

Notes: give the content generous inside margins so text never crowds the spine; size
the type so a typical chapter is a few spreads, not twenty; keep figures/code/equations
from splitting (`break-inside:avoid`). `index.html` can use the same open-book frame
for the cover (left page) + table of contents (right page). The skin (palette, spine
texture, flip easing, paper grain) is the paper's; the mechanics above stay constant.

**Verify the spread before declaring done.** Open a chapter from `file://` and confirm
content actually fills BOTH pages (not just the left) and the page counter is sane. A
fast headless check: `"…/Google Chrome" --headless=new --window-size=1440,900
--screenshot=out.png file://…/concepts/01-*.html` then look at `out.png`. Also keep
inline-SVG `<text>` inside its `viewBox` (e.g. a 360-wide box) — long bottom captions
overflow and get clipped; center them (`text-anchor="middle"`) and shorten to fit.

- **Bespoke skin (per paper):** pick a palette, typography, and motif that evoke the
  paper's subject (a vision paper → optical/lens motifs; an RL paper → trajectories
  and rewards; a systems paper → racks and pipelines; a theory paper → crisp serif +
  chalkboard). Be tasteful and legible — strong type scale, generous spacing.
- **Rich content blocks:** syntax-highlighted code (highlight inline with simple
  `<span>` classes + CSS — **no CDN**), callouts (key-idea / note / caveat),
  comparison tables, equations (write math as styled HTML/MathML or clean SVG — do
  **not** load KaTeX/MathJax from a CDN), figures/diagrams, and "cheatsheet" cards.
  Cite the paper's own results at the foot of a page where useful.
- **Self-contained & offline:** all CSS/JS/assets local and relative. No external
  `<link>`/`<script>` to CDNs, no web fonts that need network — the book must render
  from `file://` with no connection (use system font stacks). Keep `book.js` tiny.
- **Accuracy first.** Beautiful but wrong is a failure. Quote the paper's real
  numbers and equations; show the mechanism as the paper actually defines it. If you
  genuinely don't know something, say so rather than invent it.

## Images & diagrams

Images make a book sing — but it must stay **offline and self-contained**: never
hotlink a remote URL in `<img>`. Pick the right tool for each visual:

- **Inline SVG — draw it yourself, preferred for anything explanatory.** The paper's
  architecture, its pipeline, a state machine, an attention pattern, a search tree, a
  results bar chart. Crisp at any size, no files, themeable to the palette. If you
  can express it as SVG, do — don't make the user source a diagram you could draw.
  **Recreating the paper's key figure as clean SVG is often the best page on the
  book.**
- **Real/illustrative images you can't draw → declare an image slot** (below). You
  don't have the image, so you write a prompt for the user's external image agent
  and leave a placeholder they drop the result onto.
- **CSS/Unicode decoration** for badges, dividers, glyphs — no files.

Rules for every image: **relative paths, mind the depth** — chapter pages live in
`concepts/`, so they reference `../assets/img/x.png`; `index.html` / `cheatsheet.html`
use `assets/img/x.png`. Wrap in a `<figure>` with a caption and `alt`, and size
responsively (`figure img{max-width:100%;height:auto}`).

### Image slots — prompt-and-drop (the user supplies the artwork)

Where a real/generated image genuinely helps, create an **image slot** (the KB book
reader turns these into live drop targets):

1. **Record it in `book.json`** under `images[]`:
   ```json
   {
     "id": "search-tree",
     "prompt": "A clean editorial illustration of a branching search tree of thoughts: a root prompt fanning into candidate thoughts, some pruned (dimmed) and one path highlighted to a solution. Flat vector, calm blue/ink palette, generous whitespace, no baked-in text.",
     "file": "assets/img/search-tree.png",
     "alt": "Deliberate search over a tree of intermediate thoughts",
     "caption": "ToT explores and prunes a tree of intermediate thoughts.",
     "concept": "how-it-works",
     "aspect": "16:9"
   }
   ```
   Write a **standalone, vivid prompt** (style, subject, palette, mood, aspect) any
   image model can run with no other context. Keep `file` = `assets/img/<id>.<ext>`.
2. **Emit a placeholder** at that spot. Use this exact structure so the app can wire
   it up (class names + `data-img-slot` / `data-img-file` matter; `data-img-file` is
   **relative to the book folder**, while `<img src>` is relative to the page):
   ```html
   <figure class="img-slot" data-img-slot="search-tree" data-img-file="assets/img/search-tree.png">
     <img class="img-real" src="../assets/img/search-tree.png"
          alt="Deliberate search over a tree of intermediate thoughts"
          onerror="this.closest('.img-slot').classList.add('img-missing')" />
     <div class="img-drop"><div class="img-drop-inner">
       <strong>Image needed</strong>
       <p class="img-prompt">A clean editorial illustration of a branching search tree … no baked-in text.</p>
       <button type="button" class="img-copy">Copy prompt</button>
       <p class="img-hint">Generate this with your image agent, then drop the file here.</p>
     </div></div>
     <figcaption>ToT explores and prunes a tree of intermediate thoughts.</figcaption>
   </figure>
   ```
   The `<img>` starts broken (the file doesn't exist yet); its `onerror` flags the
   slot and the app's injected CSS reveals the drop prompt. Once the user drops an
   image, the app writes it to `file`, reloads, the `<img>` resolves, and the
   placeholder hides — no rebuild needed.
3. **Style the slot** in `book.css` (dashed-card `.img-drop`, readable `.img-prompt`
   quote, the `.img-copy` button). The app only injects the show/hide behavior + drag
   wiring + "Copy prompt"; opened in a plain browser the prompt still shows.

Keep slots **purposeful** — at most one strong illustration per chapter; lean on SVG
for the explanatory diagrams. The cover can be a slot (`"concept": null`) if you want
custom cover art; otherwise design a typographic cover.

## Privacy / locality note

Everything stays **local**: the book is files on disk under `$KB_ROOT/<id>/book/`,
read directly by the app with no server. Web research happens only during generation;
the generated book has no network dependencies.
