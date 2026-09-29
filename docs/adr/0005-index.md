# ADR 0005: Content-addressed SQLite FTS5 index as a rebuildable cache

Status: accepted

## Decision

A single SQLite database per KB checkout and profile stores parsed documents keyed by content
id (Git blob id or SHA-256) and parser version, plus per-snapshot membership. Builds run in
one `BEGIN IMMEDIATE` transaction and reuse unchanged documents, so a new snapshot parses
only changed files. Readers use WAL snapshots. Corruption is handled by moving the file aside
and rebuilding. User search text is tokenized and quoted; raw FTS syntax is never accepted.

## Consequences

* Warm queries never walk or parse Markdown.
* Branches, snapshots and proposal overlays never mix: membership rows are keyed by snapshot
  and origin.
