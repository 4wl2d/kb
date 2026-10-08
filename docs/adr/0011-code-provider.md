# ADR 0011: Commit-bound external code intelligence

Status: proposed for the upstream upgrade

## Decision

Providers are one-shot commands that read one `kb.code.v1` JSON request on stdin and
write one JSON response on stdout. Requests select symbols, references, dependents or
similar examples. They name the host repository, immutable commit and query scope.
Responses identify the tool/version, supported operations, completeness and limitations.
Symbols carry safe repository-relative paths, exact source spans and SHA-256 stamps.
References name returned symbols and distinguish resolved from possible relationships.

The engine validates the response, commit/repo identity, limits, references and source
bytes against Git objects. It does not load a source parser, embed text, call an LLM or
start a daemon. It never treats provider facts as accepted KB obligations. Repeated output
is normalized with stable ordering. External commands have byte/deadline limits and owned
process groups. Fixtures can supply identical responses without invoking a tool.

Reference adapters live in `core/providers`, outside the engine crate. They support
ast-index and CodeGraph through version-checked inputs and frozen host content. Any native
query limit or unsupported resolution remains visible. Deep impact traverses incoming
relationships to depth 2 and labels possible/file-level relationships. Coverage uses
provider fan-in only when supplied, reporting the absence of that evidence otherwise.

Composed context is opt-in. Code examples, consumers and candidate analogs are budgeted
supplementary units, never replacements for mandatory knowledge. Default-on delivery
requires the downstream A/B gate; static tests cannot establish task-quality gains.

## Consequences

Hosts can choose an installed code tool without adding its parser dependencies to kb.
Static graphs do not establish runtime reachability or the truth of a contract. Provider
failures, incomplete graphs and mismatched commits cannot silently become complete
results. Historical analysis uses the requested Git state rather than mutable source.
