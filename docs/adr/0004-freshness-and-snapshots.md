# ADR 0004: Per-call freshness, explicit selection, isolated mirror

Status: accepted

## Decision

`context`, `show`, `search` and `impact` fetch the configured approved ref into an isolated
bare mirror on every call (no TTL, no automatic offline fallback). Failure is
`FRESHNESS_UNVERIFIED` unless `--offline` is explicit, in which case the result is labeled
`freshness = unverified`. Selection is separate from freshness: `latest`, `pinned` (host
submodule gitlink or `.kbw.toml pin`), an explicit revision, or `working-tree`. With a host
pin that differs from the approved tip, `auto` returns `UPDATE_REQUIRED` unless the host
binding declares a selection.

Snapshot content is read from Git objects (`ls-tree` + `cat-file --batch`); the developer's
checkouts are never modified. Before interpreting a snapshot, its `core/release.toml` is
compared with the running engine (`UPDATE_REQUIRED` on mismatch).

## Consequences

* Every knowledge query pays one fetch round trip; local retrieval stays in tens of
  milliseconds and is measured separately.
* The approved ref is a trust boundary maintained by the review process outside Git; kb does
  not call provider APIs to verify approvals.
