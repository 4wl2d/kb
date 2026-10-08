# ADR 0015: Declarative verification with explicit evidence limits

Status: proposed for the upstream upgrade

## Decision

Statements may carry schema-2 probes for commit-message and branch-name regexes, path
naming, forbidden static import edges and banned source patterns. Regexes use Rust's
bounded regex engine. Probe records cannot name shell commands, interpreters or scripts.
No text from a record, commit message or source line is executed.

`verify --diff BASE` examines the current work tree; `--staged` examines the index;
`--head REV` examines committed bytes and supports a commit-bound code provider. Version
applicability is read from that same target. The first commit has an explicit empty-tree
baseline. Renames, deleted paths and new untracked files retain their Git meaning.

Commit, branch and naming regexes are positive constraints. Naming sees final
repo-relative paths. Banned-API regexes must not match added source lines; their matches
remain lexical, including comments and strings. Import probes require resolved static
edges for positive violations and complete evidence for a negative conclusion. Partial
graphs, possible edges, cross-repository targets or uncommitted import changes remain
unverifiable. Findings identify paths/lines/commits without echoing source or message text.

Record scope, versions and declared change types use the same applicability logic as
context. Natural-language conditions and exceptions cannot be inferred by a regex engine;
`--applicable RECORD#STATEMENT` explicitly asserts that the conditions hold and no exception
applies. That assertion is retained in the report. Unknown applicability cannot silently
pass. Must/must-not findings block by default; `--strict` also blocks advisory failures.

The supplied commit-msg hook checks only pending message and branch probes against the
staged scope: the index Git is committing (`verify --index-file`, also for `commit -a` and
pathspec commits), the message as Git will commit it (scissors cut, comments stripped as
Git would) and, during a rebase, the rebased branch. Like host MR jobs it reads the pinned
KB revision. Host MR jobs check the committed change. Both return native exit codes.
Existing hooks and branch-protection settings remain under team control. These mechanical
Git hooks are distinct from the empirical, optional harness pre-edit/stop gates.

## Provenance and freshness

Anchor stamping edits only explicitly selected records, after preview and fresh-byte
preconditions, preserving status, content and comments; a change anchor keeps its reviewed
commit and is stamped only at that commit. It uses Git objects, optionally
narrowing to a unique stamped provider definition. Otherwise it stamps the entire file
conservatively. Source checks establish bytes and availability, not a statement's truth,
review approval or test execution. Qualified symbol identity is not re-resolved by a
Git-only stamp check. Moved spans remain reviewable drift rather than being silently healed.

Freshness uses an explicit `--on YYYY-MM-DD` or the selected commit's UTC date. This is a
declared deterministic reference, not a hidden clock; calendar SLA jobs supply today's
date explicitly. `verified_at` is caller-reported review evidence, never inferred from
an anchor check. Warnings retain applicable obligations. Drift groups review work by owner;
ledger reports supported/stale/unverifiable evidence and reproducibly samples drafts for
human audit without accepting them.

Local usage is append-only and stores ids, scopes, costs and provenance rather than task
text or source snippets. Scope joins are relevance proxies, not semantic quality or model
consultation measurements. Logs never leave the machine through an engine command.
