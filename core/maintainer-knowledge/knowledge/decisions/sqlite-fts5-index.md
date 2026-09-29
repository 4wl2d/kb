+++
schema = 1
id = "kb.decision.sqlite-fts5-index"
kind = "decision"
title = "Embedded SQLite with FTS5 as a rebuildable index"
status = "accepted"
owner = "maintainers"
context = "Warm queries over tens of thousands of records must not re-read and re-parse every Markdown file."
decision = "Use bundled SQLite with FTS5 as a content-addressed, transactional cache derived from Git-tracked text."
reasons = [
  "Transactional builds give readers a complete old or new state.",
  "FTS5 provides ranked full-text search without extra services.",
]
consequences = ["The database is never committed and can always be rebuilt from Git content."]

[scope]
modules = ["kb.index"]

[selectors]
aliases = ["sqlite", "fts5", "index", "индекс"]

[[alternatives]]
option = "Embeddings or a vector database"
rejected_because = "Excluded by design: no runtime model calls, results must be deterministic and explainable."
+++
