# ADR 0012: Explicit temporal context slices

Status: proposed for the upstream upgrade

## Decision

`context --as-of <date|revision>` evaluates knowledge at a stated host point without
checking out or scrubbing files. Date bounds use the Gregorian calendar: introduced is
inclusive, retired exclusive. Commit bounds use ancestry in the explicitly selected host
repository. Revision queries use that commit's UTC date for date-bounded records. Date
queries use the host's last first-parent commit before the end of the requested UTC day
for commit-bounded records. Missing or ambiguous Git objects are errors, not string-order
comparisons. Mixed date/commit bounds on one record are invalid.

An undated record cannot establish that its knowledge existed at the cut-off. Temporal
queries withhold such records, list the exclusion and report partial context. They never
quietly include undated knowledge in a historical evaluation. Required records outside
the slice remain missing dependencies, not waived obligations. Ordinary queries retain
schema-1 behavior and do not infer dates.

The Git adapter resolves temporal facts; a pure view filters all lookup paths, including
full-text, aliases, explicit ids, dependencies and proposals. The query, resolved point
and exclusions contribute to the context receipt. A historical slice does not certify the
truth of author-supplied dates. Replay evaluation additionally freezes the host, KB,
registries, tool inputs and hidden-test boundary and rejects incomplete temporal slices.

## Consequences

No query-time model, mutable checkout or current wall clock is needed for temporal
retrieval. Legacy knowledge can be migrated losslessly, but must acquire reviewed validity
evidence before it can be used in historical replay. Future/deleted records remain
addressable in the ordinary snapshot and can be inspected outside the slice.
