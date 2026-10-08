# Context assembly

`kb context` answers one question: *which accepted knowledge must an agent or a person have
before working on this task, and is that answer complete?* It selects applicable obligations
by scope (never by text similarity), follows their required dependencies, adds a small ranked
set of explanations, packs whole units into a budget and returns them with provenance, a
completeness status and a receipt.

Normative contract: [architecture.md §5](architecture.md#5-context-assembly),
[ADR 0003](adr/0003-applicability-vs-relevance.md) and
[ADR 0007](adr/0007-budgets-and-receipts.md). Record fields are described in
[format.md](format.md); snapshot selection and freshness in
[snapshots-and-trust.md](snapshots-and-trust.md).

Long output excerpts below were captured from the schema-1 synthetic baseline; their
historical hashes, counts and skill-protocol headers are illustrative, not current-run
claims. The options and behavior described here include schema/skill protocol 2. The baseline used
(`kb init --example synthetic-multirepo --apply`, committed and published to a local bare
repository used as `origin`). The local origin path is shown as `<kb-origin.git>`.

## Command

```sh
./kbw context --intent implement \
  --task "Retry the token refresh after a network error" \
  --repo mobile --path mobile:app/auth/TokenRefresher.kt
```

| Option | Meaning |
|---|---|
| `--intent <i>` | required: `implement`, `refactor`, `debug`, `diagnose`, `review`, `explain` |
| `--task <text>` | free text in any script, at most 8 KiB; used for concepts, aliases, id mentions and full-text search |
| `--repo <id>` | registry repo (repeatable); default: the detected host repo |
| `--path <p>` | host-relative file or directory, or `repo:path` (repeatable, at most 512) |
| `--module <id>`, `--feature <id>`, `--concept <id>` | explicit registry ids (repeatable, at most 512 each) |
| `--host-version <repo=x.y.z>` | host code version (repeatable); overrides the version read from `version_file` |
| `--budget <n>`, `--budget-unit tokens-est\|bytes` | default: `[context] default_budget` (8000) and `default_budget_unit` (`tokens-est`) of `project.toml` |
| `--max-supplementary <n>` | cap on ranked supplementary records; default `[context] max_supplementary` (12), at most 200 |
| `--sections none\|mandatory\|all` | add optional Markdown sections as separate units (default `none`) |
| `--include-proposals` | add local, unreviewed knowledge changes as a labeled overlay |
| `--changed`, `--base`, `--head`, `--working-tree` | use actual host diff paths, including old/deleted paths (not subject to the `--path` limit); no-base default is local changes since HEAD |
| `--change-type <id>` | explicit category (repeatable); only known explicit categories can exclude mismatched obligations |
| `--as-of <date\|revision>` | filter all lookups at a historical host point; incompatible with proposal overlays |
| `--with-code`, `--provider`, `--provider-arg`, `--provider-file` | opt-in commit-bound static code evidence; shared provider deadline/bounds apply |
| `--since-receipt <id>` | reference unchanged content from an explicitly retained, verified local receipt |
| `--core-receipt <id> --core-source <file>` | reuse matching installed always-on rules with verified bundle/lock/file evidence |
| `--stale <days> --on <date>` | dated freshness warnings; absent `--on` uses a selected commit date, not today's clock |
| `--explain` | append the explain part (reasons, scores, exclusions, anchors); not counted in the budget |

Global options that matter here: `--format compact|terse|human|json` (`--json`), `--offline`,
`--snapshot`, `--host`, `--quiet`, `--skill-protocol`. Explicit registry ids that do not exist
fail with `UNKNOWN_SCOPE` (exit 15):

```text
error[UNKNOWN_SCOPE]: unknown registry ids: repos: nosuch
  details: {"repos":["nosuch"]}
  hint: check project/registry/*.toml; ids are case-sensitive
```

## Pipeline

1. **Preflight.** Load the local profile config, check `--skill-protocol`, verify freshness
   (fetch the approved ref unless `--offline`), select the snapshot, check that this engine
   can interpret it, and build or reuse its index entry, including its validation
   diagnostics ([snapshots-and-trust.md](snapshots-and-trust.md)).
2. **Task scope resolution**: host repo, paths → modules and features, aliases → concepts.
3. **Mandatory selection**: every accepted `policy`, `invariant`, `contract` and `gap` whose
   applicability is *applies*. Undetermined ones are listed, not made mandatory (one may still
   appear as a supplementary unit labeled `applicability-undetermined`).
4. **Requires closure** over `links.requires` from the mandatory records.
5. **Effective settings** of the policies in the mandatory tier.
6. **Supplementary candidates and ranking**, including ambiguity candidates.
7. **Proposals** (only with `--include-proposals`).
8. **Completeness**: status reasons from snapshot provenance, validity and scope.
9. **Packing** of whole units into the budget.
10. **Output** with the receipt.

The assembly itself is pure: it reads only the snapshot view and the facts gathered in step 1,
never the clock, so identical logical inputs and snapshot give byte-identical results. Timings
appear only in the envelope `meta`.

## Task scope resolution

### Early discovery and changed scope

```sh
.kb/kbw context --intent diagnose --task "Refresh retries leave checkout pending"
.kb/kbw context --intent implement --path app/auth/TokenRefresher.kt
.kb/kbw context --intent review --changed --base origin/main --working-tree
```

Tracked filenames and unambiguous identifier spelling can discover paths before the caller
knows them. No source parser is embedded in the engine. Discovered paths (reported in
`scope.inferred_paths`, together with code symbols named in the task under `--with-code`) are
candidates: they feed path candidates, ranking and change-type hints, and add the modules and
features of the files they name to a scope that explicit paths, modules or a diff made known.
On their own they never make the module or feature scope known, so module-scoped obligations
stay undetermined. An identifier that names more than 8 files is skipped
(`IDENTIFIER_AMBIGUOUS`); at most 64 discovered paths are used, sorted by path
(`INFERRED_PATHS_TRUNCATED`), and they never count against the `--path` limit. Tracked names
that are not UTF-8 or not safe relative paths are skipped (`TRACKED_NAMES_SKIPPED`). Diagnose
also consults feature/gap aliases, but remains partial with `DIAGNOSE_SCOPE_PROVISIONAL`
until explicit paths, modules or a real diff establish scope. Discovery is not a substitute
for a scoped pre-edit query.

An explicit empty diff is known-empty; old/deleted/renamed paths retain their applicability.
Diff paths are bounded by the diff, not by the `--path` limit. A patch larger than 8 MiB (a
generated lockfile or dump) skips the changed-text identifier hints (`CHANGED_TEXT_SKIPPED`)
instead of failing.
`change-types.toml` aliases/path/symbol hints can add candidates. Such lexical hints cannot
prune unknown categories. Explicit `--change-type` supplies that knowledge; exclusions are
reported in `pruned_change_types` and required dependencies remain reachable.

### Historical context

```sh
.kb/kbw context --intent review --repo mobile --path app/auth/TokenRefresher.kt \
  --snapshot FROZEN_KB_SHA --as-of HOST_BASE_SHA --offline
```

`introduced` is inclusive and `retired` exclusive. Date bounds use the Gregorian calendar;
commit bounds use ancestry in the selected host. Revision queries also resolve a UTC date;
date queries resolve the last first-parent host commit before the end of that day.
Undated accepted knowledge is withheld and reported as `AS_OF_UNDATED`. Commit bounds of
records whose repos, modules or features all belong to other registry repos are not resolved
in the host: such records are withheld and reported as `AS_OF_BOUND_UNRESOLVED`; a missing
commit of a product-wide or host-repo record remains an error. Dependencies outside the slice
stay missing. Ranking statistics, aliases, ids and source filenames all use the same slice,
so future corpus entries cannot change past lexical order; full-text terms fold diacritics as
the index does (`cafe` matches `café`).

This filters author-declared validity; it does not reconstruct older text, registries or
review status. Replay must independently freeze the KB, host and generated/provider inputs.
See [ADR 0012](adr/0012-temporal-context.md) and [evaluation.md](evaluation.md).

### Scope dimensions

Every task dimension is either *known* (a set, possibly empty) or *unknown*.

| Dimension | Known when |
|---|---|
| repos | `--repo` is given; or the host repo is identified; plus repos named explicitly: a `repo:path` qualifier, the repo of an explicit `--module`, the only repo of an explicit `--feature` that declares exactly one repo. Explicit `--repo` values take precedence over the host repo. |
| modules | `--path`, `--module` or `--changed` is given (discovered paths alone do not count) |
| features | `--feature`, `--path`, `--module` or `--changed` is given: explicit features, features of resolved modules, features whose `paths` match |
| concepts | explicit `--concept` ids, plus concept aliases found in the normalized task text |

**Paths.** A relative `--path` is taken relative to the current directory when that lies in
the host work tree, else relative to the host root; `repo:path` values are used as given.
An absolute path inside the host is made host-relative, also when it spells the host root
through a symlinked prefix (macOS `/tmp` or `/var`, a symlinked checkout): kb resolves only the
longest existing ancestor directory and keeps the last component as given. A path that
resolves outside the host is `UNSAFE_PATH` (exit 63); `.` alone (the host root) is
`INVALID_INPUT` (exit 64). A plain path belongs to the host repo, else to the single `--repo`,
else it is matched against every registry repo.

A path may name a file or a directory. It maps to the modules and features whose globs match
the path as given, match its directory form `path/` (`app/src/auth` for `app/src/auth/**`),
or may match inside that directory:

* a glob whose literal directory prefix starts with `path/` (`app/src` contains
  `app/src/auth/**`);
* for a path that does not look like a file, a glob whose literal directory prefix is an
  ancestor of `path/` and whose remaining segments can descend into it: `app/src/main` may
  contain `app/src/**/auth/**`. Wildcard segments are matched segment by segment, so
  `app/src/*/auth/**` does not reach `app/src/main/ui`; a glob with a `{...}` alternation
  after its literal prefix counts as possible.

This over-approximates on purpose: kb cannot look at the disk, so every module and feature
that may lie inside the directory is a candidate.

When a path maps to no module, the modules dimension stays known (and empty) only if the path
looks like a file (no trailing `/` and an extension on its last segment, such as `Main.kt`).
Otherwise modules and features become unknown, the warning `PATH_SCOPE_UNKNOWN` is added to
`issues`, module-scoped obligations become undetermined and the result is `partial`:

```text
scope: repos=mobile; modules=unknown; features=unknown; concepts=none
path: mobile:app/payments -> no registry module
status: PARTIAL
- [partial] UNDETERMINED_OBLIGATIONS: 7 obligation(s) may apply: example.contract.error-envelope, ...
```

A host repo named in `.kbw.toml` but missing from the registry produces the warning
`HOST_REPO_UNKNOWN` and leaves the repo dimension to other sources.

## Applicability

For each non-empty scope dimension of a record: a known task dimension *applies* when the sets
intersect and is *not applicable* otherwise; an unknown task dimension is *undetermined*
(AND across dimensions, OR within). Product-wide records apply to every task.

* **Registry narrowing.** When the task's modules (or features) are unknown but its repos are
  known, a record whose modules (features) all belong to other repos is *not applicable*
  instead of undetermined, because the registry proves it cannot apply.
* **Versions.** Each `applicability.versions` entry is checked whenever a version for its repo
  is known (explicit `--host-version`, or the host repo's `version_file`): mismatch → not
  applicable; unknown version → undetermined. For mandatory selection only, an entry for a
  repo outside the known task repos whose version is unknown is skipped. Records reached
  through `requires` check every entry, including a record that is mandatory by its own
  scope (see [requires closure](#requires-closure)).

## Mandatory selection

Every accepted `policy`, `invariant`, `contract` and `gap` whose applicability is *applies*
enters the mandatory tier, whatever the task text says. Weak or missing keyword overlap can
never drop one. Undetermined obligations are listed under `undetermined` with the unknown
dimension and make the result `partial` (`UNDETERMINED_OBLIGATIONS`); narrow the scope with
`--repo`, `--path`, `--module` or `--host-version` to decide them.

An accepted gap in the mandatory tier tells the reader that the answer is unknown and must be
asked, not guessed.

## Requires closure

Breadth-first over `links.requires`, starting from the mandatory records. Targets of any kind
are followed regardless of their scope, so cross-repository contracts stay reachable. A record
that is already mandatory stays in the mandatory tier with "also required by". Reached records
form the `dependency` tier.

| Target | Result |
|---|---|
| missing | `REQUIRES_MISSING` → `incomplete` |
| `draft` or `superseded` | `REQUIRES_NOT_ACCEPTED` → `incomplete`; not included |
| `deprecated` | warning `REQUIRES_DEPRECATED` in `issues`; included with label `deprecated`; completeness unchanged |
| version not satisfied | `INCOMPATIBLE_DEPENDENCY` → `conflict`; included with label `version-incompatible` |
| version unknown | `DEPENDENCY_VERSION_UNDETERMINED` → `partial`; included with label `version-undetermined` |
| outside the task scope | included with label `outside-task-scope` |

A record that is already mandatory and is also reached through `requires` has its version
constraints re-checked once like a dependency (mandatory selection skips unknown versions of
repos outside the task): an unknown version gives `DEPENDENCY_VERSION_UNDETERMINED`
(`partial`) and the label `version-undetermined`, a mismatch `INCOMPATIBLE_DEPENDENCY`
(`conflict`) and the label `version-incompatible`; the record stays in the mandatory tier.

This matches `kb validate`, which rejects an accepted record that requires a draft or
superseded record (`REQUIRES_NOT_ACCEPTED`) and only warns about a deprecated target
(`REQUIRES_DEPRECATED`).

## Effective settings

For every setting of every policy in the mandatory tier, the effective value is the base value
unless valid overrides from accepted, applicable policies exist. Then:

* the most specific applicable overrides decide: those for which no other applicable
  override is strictly more specific (its scope within theirs but not the other way round);
* if they all carry the same value (string sets compare as sets), that value wins, credited
  to the first of them by id;
* if they disagree, the value is unset, `SETTING_CONFLICT` is reported between exactly those
  overrides and completeness is `conflict`; broader overrides are not candidates.

`kb validate` rejects the same situation for accepted policies as `OVERRIDE_AMBIGUOUS`, so a
validated knowledge base has no setting conflicts.

Overrides that break an [override rule](format.md#policies-settings-and-overrides) are ignored
with a warning in `issues`. Overrides from policies whose applicability is undetermined are
listed per setting as `undetermined_overrides`. Example (payments task):

```text
effective settings:
  - example.common.code-review#min-reviewers = 2 (override by example.backend.payments-review; base 1; reason: Payment code moves money; two approvals catch more mistakes.)
  - example.common.code-review#require-green-ci = true (base)
```

## Supplementary candidates and ranking

Candidates are accepted records outside the mandatory tier found by at least one source:
an id mentioned verbatim in the task text, a path selector matching a task path, a selector
concept among the task concepts, a record alias matching the task text, the feature record of
a task feature, the full-text index (titles, aliases, body and normative text; the top 20
hits), a `rationale` target of a mandatory-tier record, or one hop of `related` from a
mandatory-tier record. Candidates whose applicability is *not applicable* are excluded, except
rationale targets, which ignore scope. Candidates that are not accepted are excluded
(`not-accepted`).

Points are integers and summed:

| Signal | Points |
|---|---|
| id mentioned in the task | 1000 |
| path selector match | 300 + min(literal prefix length of the best matching glob, 100) |
| concept match | 200 each, at most 2 |
| record alias match | 150 |
| rationale of a mandatory-tier record | 250 |
| feature record for a task feature | 250 |
| related to a mandatory-tier record (one hop) | 80 |
| best record for a candidate concept of an ambiguous phrase | 200 |
| intent selector match | 40 |
| kind prior by intent (table below) | 0–40 |
| scope applies (not product-wide) | 50 |
| title token overlap | 20 each, at most 3 |
| full-text rank position p (0-based, top 20) | 100 − 5p |

Intent selectors, the kind prior, scope and title overlap only add points to records found by
a source. Title overlap and full-text terms ignore tokens shorter than 3 characters and a fixed
list of 44 English and Russian function words; at most 32 terms are taken from the task.
Full-text matching uses SQLite FTS5 with each term quoted; the score is computed from the
selected snapshot's own statistics so that results do not depend on other cached snapshots.

Kind prior (`context/rank.rs`):

| Kind | implement | refactor | debug | review | explain |
|---|---|---|---|---|---|
| policy | 30 | 30 | 20 | 40 | 20 |
| feature | 40 | 30 | 30 | 30 | 40 |
| invariant | 30 | 40 | 30 | 40 | 20 |
| contract | 30 | 30 | 30 | 30 | 20 |
| decision | 10 | 30 | 10 | 30 | 40 |
| procedure | 20 | 10 | 40 | 10 | 10 |
| reference | 10 | 10 | 20 | 10 | 30 |
| gap | 20 | 20 | 30 | 30 | 20 |

Order: score descending, then id ascending. A candidate below the **minimum score 80** is
excluded (`below-min-score`); beyond `max_supplementary` it is excluded (`max-supplementary`).
Ambiguity candidates bypass both. A supplementary record whose applicability is undetermined
carries the label `applicability-undetermined`; it is still never mandatory.

## Ambiguity

A normalized task phrase that matches aliases of more than one concept is resolved, in order,
by an explicit `--concept`, by a concept already matched unambiguously in the same task, or by
a single concept whose `paths` hints match a task path. Otherwise it is reported in
`ambiguities` with its candidates, and the best applicable record per candidate concept is
offered (label `ambiguity-candidate:<concept>`). kb does not pretend to understand the
sentence. In the synthetic example the alias `композици*` belongs to both `ui-composition`
and `object-composition`:

```sh
./kbw context --intent explain --task "Как устроена композиция?" --repo mobile
```

```text
scope: repos=mobile; modules=unknown; features=unknown; concepts=none
status: PARTIAL
...
ambiguous: "композиция" (alias композици*) -> object-composition, ui-composition; offered: ui-composition=example.mobile.state-hoisting; pass --concept or a --path to disambiguate
```

No record is offered for `object-composition` because its only record is scoped to the
backend, which is not applicable to a `mobile` task. With `--path mobile:app/ui/ProfileScreen.kt`
the `ui-composition` path hint resolves the phrase and no ambiguity is reported.

## Proposals

With `--include-proposals`, local knowledge changes relative to the approved snapshot
(committed but unapproved, staged, unstaged and untracked; see
[proposal overlay](snapshots-and-trust.md#proposal-overlay)) are added as a separate tier:

* `proposal:new`: a new record whose scope does not rule it out for the task (applies or
  undetermined);
* `proposal:modifies`: a changed accepted record that is included, or whose proposed scope
  does not rule it out;
* `proposal:removes`: a deleted accepted record that is included;
* `stale`: the approved tip changed the same file after the proposal's base.

Proposals never replace accepted records, never count as mandatory and do not change
completeness. An unparsable proposal is reported as `PROPOSAL_INVALID` (warning) and excluded
(`invalid`); unrelated ones are excluded as `not-relevant`.

```text
### proposal example.mobile.token-storage (policy, accepted): Refresh tokens live only in the platform keystore
why: local proposal modifying accepted `example.mobile.token-storage` (not reviewed; the accepted version stays authoritative and is not overridden); labels: proposal:modifies; source: project/knowledge/repos/mobile/token-storage.md
```

## Packing and budgets

Output consists of whole units. A record unit contains all typed normative content of the
record (statements with conditions and exceptions, settings, overrides, parties, obligations
and so on); units and normative sentences are never truncated. Sections are separate optional
units. Tiers are packed in this order:

| Tier | Content | Order within the tier |
|---|---|---|
| `mandatory` | applicable accepted policies, invariants, contracts, gaps | kind (policy, invariant, contract, gap), then id |
| `dependency` | records reached through `requires` | kind (policy, invariant, contract, gap, feature, decision, procedure, reference), then id |
| `proposal` | local proposals (`--include-proposals`) | id |
| `supplementary` | ranked records | rank order |
| `section` | Markdown sections (`--sections mandatory`: of mandatory and dependency units; `all`: of every included unit) | unit order |

The header and the mandatory and dependency units must fit. If they do not, the command fails
with `CONTEXT_BUDGET_EXCEEDED` (exit 31) and the exact amount required; it never returns a
truncated answer:

```text
error[CONTEXT_BUDGET_EXCEEDED]: the header and mandatory knowledge need 1261 tokens-est but the budget is 500
  details: {"format":"compact","limit":500,"required":1261,"unit":"tokens-est"}
  hint: re-run with --budget 1261 or more; mandatory knowledge is never truncated
```

Otherwise the optional units are added first-fit in tier order: a unit that does not fit the
remaining budget is skipped and listed in the receipt as excluded by `budget`, and later
smaller units may still fit. With `--budget 1261` the same task returns `complete` with the
three supplementary records excluded by budget.

### Units of measurement

| Unit | Definition |
|---|---|
| `bytes` | exact UTF-8 bytes of the rendered payload in the selected format |
| `tokens-est` | deterministic estimate `ceil(ascii_bytes / 4) + ceil(non_ascii_chars / 2)`, computed per rendered piece and summed |

`tokens-est` is **an estimate, not a tokenizer count**; the output says so
(`tokens-est (estimate, not a tokenizer count)`). Summing per piece gives an upper bound of the
same formula applied to the whole text, but a real model tokenizer may count more or fewer
tokens; keep headroom when a hard model limit matters.

The budget covers the rendered payload: the header (snapshot, host, scope, status reasons,
effective settings, undetermined obligations, ambiguities, issues, snapshot diagnostics,
notes), the units and the receipt summary. The
`--explain` part and the JSON `meta` object are not counted. The receipt summary is reserved
at its largest possible size, so the measured total never exceeds the limit (a little of the
budget can remain unused) and `required` is the exact minimum under that rule.

In JSON format, `bytes` is the exact size of the `result` member as printed by
`kb context --json` (pretty-printed inside the `kb.cli.v1` envelope, from its `{` to its
`}`, without `explain`); the envelope framing, `error` and `meta` are not counted. In compact
and human format the payload is the printed text before the explain part. Both were checked
on the example: 18846 bytes of printed `result` for `used: 18846`, and 7005 bytes of compact
output for `budget: 7005/100000 bytes`. The format is part of the measurement, so the same
task can pack differently in compact and JSON.

## Completeness

| Status | Meaning |
|---|---|
| `complete` | every applicable obligation and dependency is resolved and included, the snapshot is approved, verified and valid, and nothing is undetermined |
| `partial` | something may apply but could not be decided, or the snapshot's provenance is weaker than a verified approved revision |
| `conflict` | the knowledge contradicts itself for this task (setting conflict, incompatible dependency) |
| `incomplete` | required knowledge is missing, draft or superseded, or the snapshot has validation errors |

The overall status is the worst of all reasons (`complete` < `partial` < `conflict` <
`incomplete`). Reasons are listed in `status_reasons`:

| Reason | Status | Cause |
|---|---|---|
| `FRESHNESS_UNVERIFIED` | partial | the approved ref was not checked against the remote in this call (`--offline`) |
| `WORKING_TREE` | partial | `--snapshot working-tree`: local files, not an approved revision |
| `NOT_APPROVED` | partial | the selected revision is not reachable from the approved tip |
| `APPROVAL_UNKNOWN` | partial | no approved tip is known, so approval cannot be decided |
| `REPO_UNKNOWN` | partial | no `--repo` and no identified host repo |
| `AS_OF_BOUND_UNRESOLVED` | partial | `--as-of` withheld accepted records scoped to other repositories whose commit bounds cannot be resolved in this host |
| `UNDETERMINED_OBLIGATIONS` | partial | obligations whose applicability is undetermined |
| `DEPENDENCY_VERSION_UNDETERMINED` | partial | a required record's version constraint cannot be checked |
| `SETTING_CONFLICT` | conflict | the most specific applicable overrides disagree |
| `INCOMPATIBLE_DEPENDENCY` | conflict | a required record does not apply to the host version |
| `REQUIRES_MISSING` | incomplete | a required record does not exist |
| `REQUIRES_NOT_ACCEPTED` | incomplete | a required record is draft or superseded |
| `SNAPSHOT_INVALID` | incomplete | the snapshot has validation errors (listed under `diagnostics`) |

`kb context` exits 0 only for `complete`. For any other status it prints the full result on
stdout and exits 30 (`CONTEXT_INCOMPLETE`), with the reasons repeated in the error on stderr
(or in the envelope `error` with `--json`). Treat such a result as what it says: use what was
delivered, and resolve or report what is missing.

"Complete" is relative to the declared, validated knowledge base only. When nothing applies,
the result carries the note that it proves nothing about the project. Routing fixtures compare
their `expect_status` with the *knowledge status*, which ignores the provenance reasons
(`FRESHNESS_UNVERIFIED`, `WORKING_TREE`, `NOT_APPROVED`, `APPROVAL_UNKNOWN`).

Other findings are reported in `issues` without changing the status by themselves:
`PATH_SCOPE_UNKNOWN`, `HOST_REPO_UNKNOWN`, `REQUIRES_DEPRECATED` (a deprecated required
record, included with label `deprecated`), ignored overrides (`OVERRIDE_*`, warnings),
`PROPOSAL_INVALID`, and the discovery notes `IDENTIFIER_AMBIGUOUS`, `TRACKED_NAMES_SKIPPED`,
`CHANGED_TEXT_SKIPPED` (info) and `INFERRED_PATHS_TRUNCATED` (warning); every reason above
that concerns a record is repeated there too.

## Receipts

Every result ends with a receipt: counts of included units per tier, the optional units
excluded by budget, the number of other exclusions, the budget and the receipt id:

```text
receipt.id = "sha256:" + hex(sha256(canonical JSON of the default result without receipt.id))
```

The default result is the `result` of `kb context --json` without `--explain`. Canonical JSON
sorts object keys recursively and has no whitespace (`kb::context::receipt_id_of`). To verify
`--explain` output, drop its top-level `explain` member first; the id is the same with and
without `--explain`. With `jq`, this reproduced the id of the example result:

```sh
./kbw --json context ... > ctx.json
jq -r '.result.receipt.id' ctx.json
jq -S -c '.result | del(.explain) | del(.receipt.id)' ctx.json | tr -d '\n' | shasum -a 256
```

The receipt is computed over the canonical JSON form of the result as packed for the selected
output format. In compact or human output that form's `budget` records the compact or human
measurement (`format`, `used`), so the printed receipt differs from the `--json` receipt of
the same task. Only a `--json` receipt can be recomputed from the printed output; verify a
compact or human receipt by re-running the same request in the same format against the same
snapshot and comparing the ids.

A receipt proves which knowledge was **delivered**, from which snapshot, under which scope. It
does not prove that the reader understood or followed it. Request context again after context
compaction, in a new session, after a hand-off to another agent, and whenever the scope, the
contracts involved or the snapshot change.

Normal CLI responses additionally carry `receipt.protocol = kb.receipt.v2`,
`snapshot.content_digest` and `units[].content_sha256`. Content identity covers sorted
source identities, diagnostics and proposal overlay independently of cache location.
Working-tree and Git sources deliberately use distinct identity domains.

`--since-receipt ID` verifies the saved complete response and each unit hash for the same
KB/host/profile. `--core-receipt ID --core-source FILE` additionally checks the actual
managed instruction file, installation lock and current accepted always-on core. Core
reuse is unavailable with `--as-of`. Neither mechanism infers a receipt from a session id.
Applicability and dependency closure are recomputed first; eligible unchanged units retain
their ids/tier/hash and become `delivery` references. New or changed units are delivered in
full, and so is a unit that the receipt held only as a `core` reference unless current core
proof covers it again. Missing, corrupt or mismatched proof fails; re-run without reuse instead of silently
omitting obligations. Reuse assumes the caller actually retains that content in this task.

Every context call also appends private local delivery metadata, not task/source text, for
`usage report`. Select this task's receipts when joining the final diff; see
[knowledge-lifecycle.md](knowledge-lifecycle.md#local-delivery-observations).

## Explain

`--explain` appends, after the counted payload and marked as not counted:

* `included`: every unit with its tier and reason, and for supplementary units the signals and
  points;
* `requires`: the `requires` edges followed from the mandatory tier;
* `excluded`: every excluded candidate with its reason (`budget`, `max-supplementary`,
  `below-min-score`, `not-applicable`, `not-accepted`, `not-relevant`, `invalid`);
* `undetermined` obligations, `warnings`, and `anchors` of the included records.

```text
== explain (not counted in the budget) ==
included:
- example.common.code-review [mandatory] applies (product-wide)
...
- example.feature.login [supplementary] score 500: feature record for task feature `login`, full-text rank 9, related to example.contract.token-refresh, title terms: refresh
  signals: feature-record +250 (feature record for task feature `login`), full-text +60 (full-text rank 9), related +80 (related to example.contract.token-refresh), kind-prior +40 (feature for implement), scope-applies +50 (scope applies), title-tokens +20 (title terms: refresh)
requires:
- example.contract.token-refresh -> example.contract.error-envelope
- example.mobile.token-storage -> example.contract.token-refresh
excluded:
- example.contract.payment-intent [not-applicable] not applicable: modules
- example.decision.token-in-preferences [not-accepted] status superseded
anchors:
- example.mobile.token-storage: test mobile:app/auth/TokenStoreTest.kt (A test anchor does not prove the test runs in CI.)
```

## Output formats

### Terse, outline and code evidence

`--format terse` renders compact typed values under tier/id labels and defers Markdown
bodies. Statements, conditions, exceptions and settings remain intact; `--explain` is
incompatible with terse. Use `show ID --sections` for all deferred bodies. Budgeting measures
the selected renderer, so its receipt and measured cost differ from other formats.

`outline --intent ...` uses the same scope inputs and returns an inventory of ids, kinds,
mandatory flags, reasons and estimated sizes. It has a hard ceiling of 2000 estimated
tokens in addition to the requested budget. Optional inventory may be omitted and counted;
mandatory ids cause an explicit budget failure if they cannot fit. It never supplies a
delivery receipt and cannot replace `context` before editing.

`context --with-code` requires a provider or pinned response file. Validated symbols,
reference/consumer facts and precedent candidates are optional code units after KB
supplementary records, sharing the same budget. They are labeled observed/provider, never
accepted knowledge. The response retains tool/version, full commit, digest, completeness
and limitations. A pinned response file supplies no precedent candidates, and a unit whose
source is not UTF-8 is omitted; limitations report both. Name-based static edges are not
runtime reachability. See
[reference adapters](../core/providers/README.md).

### Compact (default, for agents)

Excerpt of the command at the top of this page (freshness verified, exit 0):

```text
# kb context intent=implement engine=0.1.0 skill_protocol=1 protocol=kb.cli.v1
snapshot: 8541b350d6b6 (latest, freshness=verified); approved=yes; ref=origin refs/heads/main; source=<kb-origin.git>; latest=8541b350d6b6; key=3bc2e6131ef0
host: repo=unknown; head=unknown; versions=unknown
scope: repos=mobile; modules=mobile.auth; features=login; concepts=auth-token (alias `token refresh`)
path: mobile:app/auth/TokenRefresher.kt -> modules mobile.auth
status: COMPLETE
effective settings:
- example.common.code-review#min-reviewers = 1 (base)
- example.common.code-review#require-green-ci = true (base)
### mandatory example.mobile.token-storage (policy, accepted): Refresh tokens live only in the platform keystore
why: applies (modules); source: project/knowledge/repos/mobile/token-storage.md
scope: modules=mobile.auth
- MUST [keystore-only] Store refresh tokens only in the platform keystore.
- MUST NOT [no-plaintext] Write refresh tokens to logs, preferences, files or crash reports.
  except [local-fake-server]: Debug builds that talk to the local fake auth server may log the fake token prefix.
requires: example.contract.token-refresh
rationale: example.decision.secure-token-storage
...
### mandatory example.contract.token-refresh (contract, accepted): Token refresh API between mobile and backend
why: applies (modules); also required by example.mobile.token-storage; source: project/knowledge/contracts/token-refresh.md
scope: modules=mobile.auth,backend.api
party provider: repo backend (backend.api); role: Issues and rotates tokens
party consumer: repo mobile (mobile.auth); role: Refreshes tokens before they expire
interface: POST /v2/auth/refresh (synthetic)
- provider MUST [rotate-on-refresh] Issue a new refresh token on every successful refresh and invalidate the previous one.
- provider SHOULD [grace-window] Accept the previous refresh token for 30 seconds after rotation.
  if: the previous token was issued to the same device
...
### supplementary example.decision.secure-token-storage (decision, accepted): Keep refresh tokens in the platform keystore
why: score 425: full-text rank 2, rationale of example.mobile.token-storage, title terms: refresh; source: project/knowledge/decisions/secure-token-storage.md
...
-- receipt sha256:11dd63ea37bcd41e85e8b1e54c9f14bca8eabde5edc7e6d25061a05166969427; included: mandatory 5, dependencies 0, proposals 0, supplementary 3, sections 0; excluded by budget: none; other exclusions: 5 (--explain); budget: 1764/8000 tokens-est (estimate, not a tokenizer count), measured as compact
-- a receipt proves delivery, not understanding or compliance; re-request context after compaction, a new session, a hand-off, or a scope or snapshot change
```

The same task with `--offline --snapshot working-tree` returns the same units with:

```text
status: PARTIAL
- [partial] FRESHNESS_UNVERIFIED: the approved ref was not checked against the remote in this call
- [partial] WORKING_TREE: knowledge comes from the local working tree, not an approved revision
```

### Human

Longer labels and one block per unit, including the dependency tier (payments task:
`--repo backend --path backend:src/payments/ChargeService.kt`):

```text
KB CONTEXT (intent: implement)
engine 0.1.0, protocol kb.cli.v1, skill protocol 1

snapshot: 8541b350d6b6 (latest, freshness=verified); approved=yes; ...
scope: repos=backend; modules=backend.payments; features=checkout; concepts=none
path: backend:src/payments/ChargeService.kt -> modules backend.payments

status: COMPLETE
...
== MANDATORY example.gap.offline-checkout: Checkout behavior when the device goes offline is undefined
   kind gap | status accepted | origin accepted | owner architecture
   why: applies (features)
   source: project/knowledge/gaps/offline-checkout.md
   scope: features=checkout
   gap (missing): Nobody has specified what the app shows or retries when connectivity is lost after the payment request was sent but before the response arrived.
   ...
== DEPENDENCY example.contract.error-envelope: Common error envelope for all public APIs
   kind contract | status accepted | origin accepted | owner architecture
   why: required by example.contract.payment-intent; labels: outside-task-scope
```

Text output escapes control characters (other than newline and tab) and Unicode
bidirectional controls as `\u{..}`, so record text cannot inject terminal sequences or reorder
displayed text.

### JSON (`--json`)

stdout carries only the `kb.cli.v1` envelope; progress and diagnostics go to stderr. The
`result` object has these members: `protocol`, `engine_version`, `skill_protocol`, `intent`,
`request`, `snapshot`, `freshness`, `host`, `scope`, `completeness`, `status_reasons`,
`effective_settings`, `undetermined`, `ambiguities`, `issues`, `diagnostics`, `notes`,
`units`, `budget`, `receipt` (and `explain` with `--explain`). Optional additions are `code`,
`delivery`, `freshness_reference`, `pruned_change_types` and `request.as_of`.
Historical schema-1 excerpt:

```json
{
  "completeness": "complete",
  "freshness": "verified",
  "status_reasons": [],
  "snapshot": {
    "approved": true,
    "approved_ref": "refs/heads/main",
    "engine_version": "0.1.0",
    "freshness": "verified",
    "key": "3bc2e6131ef0f3e88dc7f8f92f1a40ea44cc6edd92a99caa3eb1944e35ab3ab6",
    "latest_approved": "8541b350d6b6efb586079fa74f9c02e07aa40124",
    "overlay": null,
    "pin": null,
    "profile": "project",
    "remote": "origin",
    "revision": "8541b350d6b6efb586079fa74f9c02e07aa40124",
    "selection": "latest",
    "source": "<kb-origin.git>"
  },
  "scope": {
    "repos": { "state": "known", "values": ["mobile"] },
    "modules": { "state": "known", "values": ["mobile.auth"] },
    "features": { "state": "known", "values": ["login"] },
    "concepts": [ { "id": "auth-token", "sources": ["alias `token refresh`"] } ],
    "paths": [ { "repo": "mobile", "path": "app/auth/TokenRefresher.kt", "modules": ["mobile.auth"], "features": [] } ]
  },
  "units": [
    {
      "id": "example.feature.login",
      "record": "example.feature.login",
      "tier": "supplementary",
      "kind": "feature",
      "status": "accepted",
      "origin": "accepted",
      "title": "Login: sign-in and silent session refresh",
      "path": "project/knowledge/features/login.md",
      "score": 500,
      "labels": [],
      "why": "score 500: feature record for task feature `login`, full-text rank 9, related to example.contract.token-refresh, title terms: refresh",
      "content": { "...": "typed record fields" }
    }
  ],
  "budget": {
    "counted": ["header", "units", "receipt"],
    "not_counted": ["explain", "meta"],
    "estimate": true,
    "format": "json",
    "limit": 8000,
    "unit": "tokens-est",
    "used": 4715
  },
  "receipt": {
    "id": "sha256:3130132b09a5c7b5f69f1dc053c1777fce59b72652244effe8793aec2764703b",
    "included": { "mandatory": 5, "dependencies": 0, "proposals": 0, "supplementary": 3, "sections": 0 },
    "excluded_budget": [],
    "excluded_other": 5,
    "note": "a receipt proves delivery, not understanding or compliance; re-request context after compaction, a new session, a hand-off, or a scope or snapshot change"
  }
}
```

(The real output is pretty-printed with sorted keys; members are reordered and shortened here.)
The envelope `meta` carries non-deterministic facts such as `elapsed_ms`, the index build
statistics and `diagnostics` like `INDEX_RECOVERED`; it is never part of the receipt.

## Search is not context

`kb search <query>` ranks records of every status by id mention (1000), concepts from registry
aliases in the query (200 each, at most 2), record aliases (150), title overlap (20 each, at
most 3) and full-text rank (max(100 − 5p, 5)); ties by id. Options: `--kind` (repeatable; an
unknown kind is `INVALID_INPUT`), `--limit` (default 20, at most 200), `--include-proposals`.
It does not select obligations, follow `requires`, apply scope or report completeness, and
every result says so. Like `show` and `impact`, its text output starts with the snapshot
line (revision, selection, freshness, approval, approved tip and pin):

```text
snapshot: 8541b350d6b6 (latest, freshness=verified); approved=yes; latest=8541b350d6b6
# kb search "обновление токена": 4 of 4 hit(s)
note: search is not a context assembly: mandatory obligations, required dependencies and completeness are not checked; use `kb context` before planning or changing code
300 example.mobile.single-refresh (invariant, accepted): At most one token refresh in flight per session
  matched: concept auth-token, full-text rank 1
295 example.mobile.token-storage (policy, accepted): Refresh tokens live only in the platform keystore
  matched: concept auth-token, full-text rank 2
```

Use search to find an id; use context before planning or changing code.

## Show

`kb show <id>` prints one full record of any status from the selected snapshot, verbatim and
untruncated. `kb show <id>#<section>` (or `--section <section>`) prints one body section;
`--raw` prints the authoritative file bytes exactly; `--include-proposals` also lists local
proposals that change the record. Superseded and deprecated records are shown with their
status and successors:

```text
snapshot: 8541b350d6b6 (latest, freshness=verified); approved=yes; latest=8541b350d6b6
note: superseded: kept addressable for history, not current knowledge; superseded by example.decision.secure-token-storage
# example.decision.token-in-preferences (decision, superseded): Store refresh tokens in shared preferences (superseded)
owner: team-mobile; origin: accepted; source: project/knowledge/decisions/token-in-preferences.md
```

An unknown record or section is `NOT_FOUND` (exit 16); the details list the available
sections. Like `context` and `search`, `show` verifies freshness on every call unless
`--offline`, and its text output starts with the snapshot line shown above (in JSON:
`result.snapshot`). `--raw` prints only the file bytes on stdout; when freshness is
unverified or the revision is not approved, the snapshot line goes to stderr as
`kb: snapshot: ...`, even with `--quiet`.
