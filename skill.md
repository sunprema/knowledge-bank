# Knowledge Bank skill

The user has a personal knowledge base of arXiv papers and captured
ideas. When questions involve research papers, ideas across papers, or
synthesis of technical concepts, use the kb_search tool.

## When to use kb_search

- The user asks about a topic that might be in their papers
- The user asks for synthesis ("ideas combining X and Y")
- The user asks for applications, future work, or open problems
  (but for "what should I build" / "what's unsolved" framed as hunting
  for opportunities, prefer kb_find_problems — see below)
- The user references "my papers", "my ideas", or "what I've saved"
- At the start of project work: query kind="note" with
  project=[<current project>, "global"] to load the user's captured
  ideas for this project plus cross-project ones

## How to use it well

- Default to mode="narrow" for direct lookups
- Use mode="wide" with k=30+ for synthesis queries
- Use section_types filter to focus on what matters:
  - "applications" for "what could be built"
  - "future_work" for "open problems"
  - "method" for "how does X work"
  - "user_notes" for "what did I think about this"
- Use kind="note" + project=[...] to recall captured ideas; results
  with "kind": "note" are the user's own ideas, not papers
- Always include the deep_link in your response so the user can
  verify against the source
- Use kb_get_paper when a chunk snippet isn't enough context
- Use kb_add_note when the user shares an insight about a paper worth
  keeping for future synthesis

## Capturing ideas (kb_capture_idea)

- Use when the user states an idea, decision, or insight worth keeping
  that is NOT about a specific paper — kb_add_note stays paper-scoped
- Key it to the project being worked on; use project="global" when it
  applies across projects
- Reference related papers/ideas in the body as [[id]] and list them
  in links
- Re-capturing with the same title (or passing upsert_key with the
  returned id) refines the idea in place — no duplicates, so it's safe
  to capture early and improve later

## Hunting for problems (kb_find_problems)

- Use when the user wants to find problems worth solving — "what should
  I build", "what's unsolved here", "where are the gaps", "mine my
  papers for product/research ideas". This is the corpus-wide hunt;
  kb_search is for looking up a specific thing you already have in mind
- Pass domain to focus the hunt on a topic (e.g. "vector quantization");
  omit it to scan broadly. k controls how many candidates come back
- Each candidate pairs a problem statement (from a paper's limitations
  or future_work) with the nearest method/applications work in OTHER
  papers, and is tagged gap_type:
  - "synthesis_opportunity" — the solution pieces exist across other
    papers but aren't assembled. Lead with these: name the problem, then
    the papers whose methods could combine to solve it
  - "greenfield" — nothing in the corpus addresses it yet. Flag these as
    open/unaddressed, not as ready-to-build
- After judging the candidates, persist the promising ones with
  kb_create_reflection (title + body + scope=[...source paper ids]) so
  the opportunity becomes a first-class, searchable result and the hunt
  compounds across sessions
- Cite both sides as usual: the problem's source paper and each
  candidate solution's paper, with their deep_links

## Catching up (kb_brief)

- Use at the start of a session, or when the user asks "what's new",
  "anything relevant lately", or wants to triage their reading queue.
  This is the ambient digest; kb_search is for a specific lookup
- It returns: new arXiv papers surfaced by the user's standing watches,
  each scored 0..1 by how strongly it connects to the existing corpus —
  with the connecting papers/reflections named under why.connections, so
  you can say WHY a paper matters ("connects to your reflection X and the
  TurboQuant method"). Also: one resurfaced past reflection, a few fresh
  sparks, and corpus stats
- Lead with the highest-scoring papers and explain each via its
  connections. A why.connects_to_synthesis flag means it links to the
  user's own notes/reflections — call that out, it's the strongest signal
- Read-only. To add a surfaced paper to the KB, tell the user to Ingest it
  from the Brief (or use the HTTP /ingest path); don't claim you ingested it
- The brief is empty until the user adds watches (an arXiv category,
  author, or topic) — if so, suggest adding one

## Output discipline

When citing papers:
1. Use the paper title (not just the arxiv id)
2. Note the section: "TurboQuant (section 3.2 — Method)"
3. Include the deep_link as a markdown link
4. If retrieval scores are low (<0.35), note that you're stretching
