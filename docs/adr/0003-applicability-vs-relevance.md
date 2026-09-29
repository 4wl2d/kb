# ADR 0003: Separate mandatory applicability (scope) from relevance (selectors)

Status: accepted

## Decision

`scope` (product, repos, modules, features; AND across dimensions, OR within) decides whether
an accepted policy, invariant, contract or gap is **mandatory** for a task. `selectors`
(paths, concepts, intents, aliases) only rank supplementary context. Task dimensions that
the request does not determine are `Unknown`; obligations constrained on an unknown
dimension are reported as undetermined and make completeness `partial` instead of being
guessed.

## Consequences

* Weak keyword overlap can never exclude an applicable obligation.
* Callers get explicit guidance (pass `--path`/`--module`) instead of silent omissions.
