# Knowledge Bank skill

The user has a personal knowledge base of arXiv papers and captured
ideas. When questions involve research papers, ideas across papers, or
synthesis of technical concepts, use the kb_search tool.

## When to use kb_search

- The user asks about a topic that might be in their papers
- The user asks for synthesis ("ideas combining X and Y")
- The user asks for applications, future work, or open problems
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

## Output discipline

When citing papers:
1. Use the paper title (not just the arxiv id)
2. Note the section: "TurboQuant (section 3.2 — Method)"
3. Include the deep_link as a markdown link
4. If retrieval scores are low (<0.35), note that you're stretching
