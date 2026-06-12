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
  - "user_notes" for "what did I think about this"
- Always include the deep_link in your response so the user can
  verify against the source
- Use kb_get_paper when a chunk snippet isn't enough context
- Use kb_add_note when the user shares an insight about a paper worth
  keeping for future synthesis

## Output discipline

When citing papers:
1. Use the paper title (not just the arxiv id)
2. Note the section: "TurboQuant (section 3.2 — Method)"
3. Include the deep_link as a markdown link
4. If retrieval scores are low (<0.7), note that you're stretching
