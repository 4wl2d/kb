# KB architecture (engineering contract)

This document is the normative engineering contract for the `kb` engine. User-facing
guides in `docs/` explain the same behavior for authors and operators; when they
disagree, this document and the code win and the guide is a bug.

Versions described here: engine `0.1.0`, document schema `1`, CLI protocol `1`,
index schema `1`, skill protocol `1`, bootstrap manifest `1`; parser version `2` (code only,
`PARSER_VERSION` in `core/cli/src/versions.rs`).

## 1. Distribution model and ownership

* One **upstream** repository contains the engine (`core/`, `kbw`, Cargo files),
  schemas, migrations, templates, shared skills, adaptation instructions and release
  machinery. It contains no real third-party project data; `project/` holds only a
  README explaining the uninitialized state.
* Each product creates exactly one **downstream** fork/private copy that keeps upstream
  Git history. The downstream holds the engine *and* the product knowledge
  (`project/`). One downstream may describe many host repositories.
* The downstream is mounted into each host repository as a Git submodule (conventional
  path `.kb`) or used as a separate checkout. Users run `<kb>/kbw`; there is no global
  `kb` binary to manage.
* Engine-owned paths are listed in `core/release.toml` (`engine_paths`). Project-owned
  paths: `project/`. Engine changes flow through upstream; project adaptation flows
  through `project/`. `kb update divergence` reports unintended engine drift against
  the recorded upstream base (`project/upstream.toml`). The root `SECURITY.md` is
  engine-owned; a downstream publishes its own policy at `.github/SECURITY.md` (outside
  `engine_paths`), rendered from `core/templates/security/SECURITY.md.tmpl`.
* Process conventions (GitFlow etc.) are not hard-coded into the executable.

## 2. Repository layout

```
kbw                       POSIX sh launcher (runtime selection, source bootstrap, artifact install)
Cargo.toml / Cargo.lock   workspace (members: core/cli, core/benchmarks)
rust-toolchain.toml       pinned toolchain
core/cli/                 crate `kb`: library + `kb` executable
core/schemas/             generated JSON Schemas (drift-checked by tests and `kb schema --check`)
core/migrations/          migration docs + synthetic legacy fixtures (code lives in core/cli/src/migrate.rs)
core/templates/           project skeleton, record templates, CI/MR templates, synthetic example
core/skills/              canonical skill instructions + references (rendered by `kb integrate --generate`)
core/maintainer-knowledge/ opt-in knowledge about kb itself (profile `maintainer`)
core/tests/               shared fixtures + routing evaluations used by core/cli/tests
core/benchmarks/          crate `kb-bench`: corpus generator + measurements
core/release.toml         bootstrap/compatibility manifest (line-parseable by kbw)
project/                  project-owned knowledge (created by `kb init`)
.cache/                   generated (runtime binaries, git mirror, SQLite index); git-ignored
```

After `kb init --apply`:

```
project/project.toml            profile config (trusted config: namespace, source, trust)
project/registry/{owners,repos,modules,features,concepts}.toml
project/knowledge/<group>/*.md  typed records (groups are free-form directories)
project/skill-config/skill.toml project settings for generated skills
project/skill-config/generated/ rendered skill bundle (generated, drift-checked, committed)
project/routing-tests/*.toml    routing fixtures
project/upstream.toml           upstream base (url parameter + revision) for divergence/update
project/README.md               replaced by init with a project README
```

Files named `README.md` inside knowledge roots are documentation and are never parsed as
records. Every other `*.md` file under a knowledge root must be a record. Non-`.md` files
under knowledge roots are ignored with a warning. Symlinks under knowledge roots are an
error (never followed).

## 3. Record format (document schema 1)

A record is a Markdown file whose first line is exactly `+++`, followed by strict TOML
front matter, a closing line `+++`, then an optional Markdown body.

* Unknown fields, duplicate keys, wrong types and unknown enum values are errors.
* `schema` must equal `1`. Other values: `UNSUPPORTED_SCHEMA_VERSION` (if a migration
  exists, the message names `kb migrate`).
* Body: optional explanatory Markdown split into sections by `## ` headings. Section id =
  slug of the heading (lowercase, alphanumerics kept incl. Unicode letters, other runs →
  `-`). Duplicate section slugs are an error. Text before the first `## ` heading is the
  `intro` section. Body sections are **non-normative**; a lint warning
  (`NORMATIVE_LANGUAGE_IN_BODY`) is emitted when a body contains RFC 2119 uppercase
  keywords (`MUST`, `MUST NOT`, `SHALL`, `SHALL NOT`, `SHOULD NOT`, `REQUIRED`).

### 3.1 Common fields

| field | type | rules |
|---|---|---|
| `schema` | int | `1` |
| `id` | string | `^[a-z][a-z0-9]*(-[a-z0-9]+)*(\.[a-z0-9]+(-[a-z0-9]+)*)+$`, ≤128 bytes; first segment = profile namespace |
| `kind` | enum | `policy feature invariant contract decision procedure reference gap` |
| `title` | string | 1..=200 chars, single line |
| `status` | enum | `draft accepted deprecated superseded` |
| `owner` | string | must exist in `registry/owners.toml` |
| `scope` | table | see 3.3 (required) |
| `selectors` | table | optional; see 3.4 |
| `links` | table | optional; see 3.5 |
| `applicability` | table | optional: `versions = { <repo> = "<semver req>" }` |
| `anchors` | array of tables | optional provenance anchors, see 3.6 |

### 3.2 Kind-specific fields

Normative content is typed and atomic. A `Statement` is
`{ id, level, text, conditions = [..], exceptions = [{ id, text }] }` with `level` in
`must must-not should should-not may`. Local ids (`statement.id`, exception ids, item ids,
setting names, party ids) match `^[a-z0-9]+(-[a-z0-9]+)*$` and are unique per record.
Exceptions that change an obligation's meaning must be typed `exceptions`; they are
rendered together with the statement and are never dropped by budgeting.

* **policy**: `rules: [Statement]`, `settings: [Setting]`, `overrides: [Override]`; at least one
  non-empty. `Setting = { name, type = integer|boolean|string|string-set, value,
  override = forbidden|stricter|any (default forbidden), stricter = lower|higher|true|false|superset|subset
  (required iff override = stricter; must fit the type), override_owners = [..], description }`.
  `Override = { target = "<policy-id>#<setting-name>", value, reason }`.
* **feature**: `feature` (registry feature id), `summary`, `behaviors: [Item]` (≥1),
  `boundaries: [Item]`. `Item = { id, text }`.
* **invariant**: `statements: [Statement]` (≥1).
* **contract**: `parties: [{ id, repo, modules = [..], role }]` (≥2), `interface` (optional
  string), `obligations: [Statement + party]` (≥1; `party` must be a declared party id).
* **decision**: `context`, `decision`, `reasons` (≥1), `alternatives: [{ option, rejected_because }]`,
  `consequences`.
* **procedure**: `preconditions`, `steps: [Item]` (≥1), `expected` (≥1). Procedures are data;
  kb never executes them.
* **reference**: `summary`, `sources: [{ title, url?, path? }]`.
* **gap**: `gap = missing|ambiguity|contradiction`, `description`, `affects = [record ids]`,
  `questions = [..]`.

### 3.3 Scope (mandatory applicability)

```toml
[scope]
product = false            # true => product-wide; then repos/modules/features must be empty
repos = ["mobile"]
modules = ["mobile.auth"]
features = ["login"]
```

* `product = true` XOR at least one non-empty dimension (an accidental empty scope is an error).
* Semantics: **AND across dimensions, OR within a dimension**. Empty dimension = unconstrained.
* All ids must exist in the registry. Unsatisfiable combinations are errors (a module whose
  repo is not in `repos`; a feature whose repos do not intersect `repos`).

Task scope has, per dimension, either `Known(set)` or `Unknown`:

* repos: Known when `--repo` is given, the host repo is identified, or the task names a repo
  explicitly (a `repo:path` qualifier, the owning repo of an explicit `--module`, the single
  repo of an explicit `--feature` that declares exactly one repo); otherwise Unknown.
* modules: Known when `--path` or `--module` is given. A `--path` may name a file or a
  directory; it maps to the modules (and features) whose globs (a) match the path as given,
  (b) match its directory form `path/` (the module root), or (c) may match inside that
  directory: a glob whose literal directory prefix starts with `path/`, or, for a path that
  does not look like a file, a glob whose literal directory prefix is an ancestor of `path/`
  and whose remaining segments can descend into it (`app/src/main` may contain
  `app/src/**/auth/**`; wildcard segments are matched segment by segment, so
  `app/src/*/auth/**` does not reach `app/src/main/ui`; a `{...}` alternation counts as
  possible). Rule (c) over-approximates: such modules and features are candidates. When a
  path maps to no module, the dimension stays Known only if the path looks like a file (no
  trailing `/` and an extension on its last segment); otherwise modules and features are
  Unknown (warning `PATH_SCOPE_UNKNOWN`), module-scoped obligations are Undetermined and the
  result is `partial` — never silently dropped.
* features: Known when `--feature`, `--path` or `--module` is given (explicit features ∪
  features of resolved modules ∪ features whose `paths` match).

Record applicability: for every non-empty record dimension D: task D Known → applies in D iff
the sets intersect, otherwise NotApplicable(D); task D Unknown → Undetermined(D). Result is
NotApplicable if any dimension is, else Undetermined if any is, else Applies. Product-wide →
Applies. Version applicability (`applicability.versions`) is evaluated against host versions
(`--host-version repo=x.y.z` or the repo's `version_file`): a constraint is always checked
when a version for its repo is known; non-matching → NotApplicable(version). For mandatory
selection only, a constraint on a repo outside the Known task repos whose version is unknown
is skipped; records reached through `requires` check every constraint (unknown →
`DEPENDENCY_VERSION_UNDETERMINED`, partial; mismatch → `INCOMPATIBLE_DEPENDENCY`, conflict),
including a record that is also mandatory by its own scope (re-checked once; it stays in the
mandatory tier and gets the label `version-undetermined` or `version-incompatible`).

### 3.4 Selectors (relevance only)

```toml
[selectors]
paths = ["app/src/**/auth/**", "backend:src/api/**"]   # optional "repo:" qualifier
concepts = ["auth-token"]                              # registry concept ids
intents = ["implement", "debug"]
aliases = ["token refresh", "обновление токена"]        # extra query phrases
```

Selectors never exclude an applicable obligation. Paths are globs (`*` within a segment,
`**` across segments, `?`, `[...]`), repo-relative, `/`-separated; they may not be
absolute, contain `..`, backslashes or NUL.

### 3.5 Links

```toml
[links]
requires = []     # mandatory transitive dependencies (any kind); acyclic
rationale = []    # explanations (usually decisions); optional context, not recursive
related = []      # one-hop suggestions, never followed transitively
supersedes = []   # ids this record replaces; acyclic
```

Validation: every target exists (any status); no self links; `requires` graph acyclic;
`supersedes` graph acyclic; every superseded target has status `superseded`; every
`superseded` record is superseded by ≥1 record; an accepted record may not `require` a
draft or superseded record (error) and gets a warning for deprecated targets
(`REQUIRES_DEPRECATED`). Context matches this: a deprecated `requires` target is included
with label `deprecated` and a `REQUIRES_DEPRECATED` warning in `issues` and does not lower
completeness; draft and superseded targets are `REQUIRES_NOT_ACCEPTED` → `incomplete`. With
`kb validate --base <rev>`: ids present at base must still exist (historical ids stay
addressable) and must keep their kind (no reuse for other facts).

### 3.6 Anchors (provenance)

`{ kind = source|test|change|doc, repo?, path?, symbol?, commit?, change?, note? }`.
`source`/`test` need `repo` and `path`; `change` needs `change` (e.g. `"!42"`) or `commit`.
A test anchor does not prove the test runs; a valid schema does not prove the text is true.

### 3.7 Policies, settings and overrides

Policies accumulate (all applicable rules apply). Only a named setting with
`override != forbidden` may be overridden, by an accepted policy that:

1. targets an existing setting (`OVERRIDE_TARGET_UNKNOWN`),
2. whose `override` is not `forbidden` (`OVERRIDE_FORBIDDEN`),
3. with a value of the declared type (`OVERRIDE_TYPE_MISMATCH`),
4. that is not weaker when `override = stricter` (`OVERRIDE_WEAKENS`),
5. whose scope is subsumed by the target record's scope (`OVERRIDE_SCOPE_EXCEEDS`),
6. whose owner is listed in `override_owners` when that list is non-empty (`OVERRIDE_NOT_AUTHORIZED`).

Two overrides of one setting with overlapping scopes, different values and neither scope
strictly more specific than the other (incomparable or equivalent scopes) are
`OVERRIDE_AMBIGUOUS` at validation time; equal values are valid whatever the scopes. At
context time the effective value is the base value unless applicable overrides exist; then the
most specific applicable overrides decide: those for which no other applicable override is
strictly more specific (subsumed but not subsuming). If they all carry the same value (string
sets compare as sets), that value wins (source `by` = the first of them in id order); if they
disagree, the setting is a conflict between exactly those overrides (`completeness =
conflict`) and broader overrides are not candidates. Validation prevents this for accepted
overrides.

Scope subsumption `A ⊆ B`: `B.product` → true. Otherwise with implied repos
`R(X) = X.repos` if non-empty, else the repos implied by X's modules (and features),
else all: `R(A) ⊆ R(B)` when B constrains repos; `A.modules ⊆ B.modules` (A non-empty)
when B constrains modules; `A.features ⊆ B.features` (A non-empty) when B constrains
features.

## 4. Registries and profile config

`project/project.toml` (schema 1):

```toml
schema = 1
[project]
name = "Example"
namespace = "example"
[source]
remote = "origin"
approved_ref = "refs/heads/main"
allowed_protocols = ["https", "ssh"]      # explicit transport policy for fetch
[knowledge]
roots = ["knowledge"]                     # relative to the profile directory
[context]
default_budget = 8000
default_budget_unit = "tokens-est"
```

Registries (`project/registry/*.toml`, each with `schema = 1`, missing file = empty):
`owners.toml` `[[owner]] id, title, repos = [..], product = bool` (authority: an owner with
non-empty `repos` may only own records whose implied repos are within them; product-wide
records need an owner with `product = true`); `repos.toml` `[[repo]] id, title, remotes = [..],
version_file?`; `modules.toml` `[[module]] id, repo, title, paths = [..], features = [..]`;
`features.toml` `[[feature]] id, title, repos = [..], paths = [..]`; `concepts.toml`
`[[concept]] id, title, aliases = [..], paths = [..]` (paths are disambiguation hints).

Registry ids: `^[a-z0-9]+([-_.][a-z0-9]+)*$`, ≤64 bytes, unique per registry.

### 4.1 Alias normalization

1. Unicode NFKC, 2. lowercase (Unicode), 3. `ё`→`е`, 4. every char that is not alphanumeric
→ space, 5. collapse whitespace, trim. Matching is on whole-token sequences of the normalized
task text. An alias ending in `*` (e.g. `композици*`) matches tokens with that prefix
(last token only). No stemming; morphological variants must be listed or use `*`.

The maintainer profile (`--profile maintainer`) uses `core/maintainer-knowledge/profile.toml`
with the same schema; namespace `kb`.

## 5. Context assembly

Request: `intent` (`implement refactor debug review explain`), optional `task`, `repo[]`,
`path[]`, `module[]`, `feature[]`, `concept[]`, `budget`, `budget_unit`, `include_proposals`,
`sections` (`none|mandatory|all`, default none), `max_supplementary` (default 12),
`host_version[]`, `explain`.

Pipeline:

1. **Preflight**: profile config, runtime/manifest compatibility, snapshot freshness and
   selection (§6), snapshot engine compatibility, snapshot validity. A snapshot with
   validation errors yields `completeness = incomplete` plus diagnostics (never `complete`).
2. **Scope resolution**: host repo, paths → modules/features, alias → concepts. Unknown
   registry ids given explicitly → `UNKNOWN_SCOPE` error. A plain `--path` is made
   host-relative (relative to the current directory inside the host, else to the host root);
   an absolute path that spells the host root through a symlinked prefix is accepted (only
   the longest existing ancestor directory is resolved, the last component is kept as given);
   a path that resolves outside the host → `UNSAFE_PATH`.
3. **Mandatory selection**: every *accepted* `policy`, `invariant`, `contract` and `gap` whose
   applicability is `Applies`. Undetermined obligations are listed and make completeness
   `partial`. Independent of full-text ranking.
4. **Requires closure**: breadth-first over `requires` from mandatory records, any kind,
   ignoring repo filters (cross-repo contracts stay reachable). Missing target → `incomplete`
   (`REQUIRES_MISSING`); draft or superseded target → `incomplete` (`REQUIRES_NOT_ACCEPTED`);
   deprecated target → included with label `deprecated` and warning `REQUIRES_DEPRECATED`
   (completeness unchanged); every version constraint of a reached record is checked, also
   for a mandatory record reached through `requires` (§3.3): NotApplicable by version →
   `conflict` (`INCOMPATIBLE_DEPENDENCY`), unknown version → `partial`
   (`DEPENDENCY_VERSION_UNDETERMINED`); never silently dropped.
5. **Supplementary candidates** (accepted, not NotApplicable, except rationale targets which
   ignore scope): explicit id mentions in task text, path selector matches, concept matches,
   record alias matches, title term matches, FTS5 over title/aliases/body/normative text,
   `rationale` of mandatory records, one hop of `related` from mandatory records.
6. **Ranking** (integer points, deterministic): id mention 1000; path selector match 300 +
   min(literal-prefix-length,100); concept match 200 each (max 2); record alias match 150;
   rationale of mandatory 250; feature record for a task feature 250; related one hop 80;
   intent selector match 40; kind prior by intent (table in `context/rank.rs`) 0–40; scope
   Applies (non-product) 50; title token overlap 20 each (max 3); FTS rank position p (0-based,
   top 20) → 100 − 5p. Minimum score 80. Order: score desc, then id asc.
7. **Packing**: tiers in order: mandatory (kind order policy, invariant, contract, gap, then
   required dependencies; id asc within), proposals (only with `--include-proposals`),
   supplementary (rank order, first-fit), sections (if requested). Units are whole: a record
   core unit contains all typed normative content; sections are separate optional units.
   If header + mandatory units exceed the budget → `CONTEXT_BUDGET_EXCEEDED` with the required
   amount. Otherwise units that do not fit are listed as excluded (`budget`).
8. **Output** with snapshot provenance, scope, completeness, effective settings, units,
   ambiguities, diagnostics, receipt.

Completeness: `complete` (all mandatory resolved, snapshot valid, nothing undetermined),
`partial` (undetermined obligations, unknown repo, or non-approved snapshot content),
`conflict` (setting conflicts, incompatible dependency), `incomplete` (missing, draft or
superseded required records, invalid snapshot). Unverified freshness (offline) is reported separately
(`freshness = unverified`) and makes the status at best `partial`. "Complete" is relative to
the declared, validated knowledge base only; an empty corpus proves nothing about the project.

Ambiguity: a normalized task phrase that matches aliases of more than one concept is resolved
by explicit `--concept`, then by concept `paths` hints matching task paths; otherwise it is
reported (`ambiguities[]`) with candidates, and the best supplementary record per candidate
concept is offered (tagged `ambiguity-candidate`).

### 5.1 Budget

Units: `bytes` (exact UTF-8 bytes of the rendered payload in the selected format) and
`tokens-est` (deterministic estimate: `ceil(ascii_bytes/4) + ceil(non_ascii_chars/2)`,
computed per unit and summed, which upper-bounds the whole-text estimate). The estimate is not
a tokenizer count. The budget covers the rendered context payload (header, units, provenance,
receipt summary); `--explain` details and the JSON `meta` object are not counted and are marked
as such. In JSON format, `bytes` is the exact UTF-8 size of the `result` member as printed by
`kb context --json` (pretty-printed inside the `kb.cli.v1` envelope, without `explain`), from
its `{` to its `}`; the envelope framing, `error` and `meta` are not counted.

### 5.2 Receipt

`receipt.id = "sha256:" + hex(sha256(canonical JSON of the result as packed for the selected
output format — its JSON form without `explain` and with receipt.id removed))`; canonical JSON sorts keys
recursively and has no whitespace (`kb::context::receipt_id_of`). To verify `--explain` output,
drop its top-level `explain` member first. Because the budget records the format and the
measured size, compact/human receipts differ from the `--json` receipt of the same task: only
the `--json` receipt can be recomputed from printed output; a compact/human receipt is verified
by re-running the same request in the same format against the same snapshot. The receipt
lists included ids with reasons, requires edges, excluded
candidates with reasons, undetermined obligations, warnings and source anchors. A receipt
proves delivery, not understanding or compliance. Agents must re-request context after
compaction, a new session, a hand-off, or a scope/snapshot change.

## 6. Freshness and snapshots

* **Freshness** (per call to `context`, `show`, `search`, `impact`): unless `--offline`,
  kb fetches the configured `approved_ref` from the configured remote into an isolated bare
  mirror (`.cache/git/<16 hex of a hash of the normalized source identity and ref>.git`,
  ref `refs/kb/approved`), using argument arrays,
  `GIT_TERMINAL_PROMPT=0`, `protocol.allow=never` plus the configured allowed protocols,
  `--no-tags --no-recurse-submodules --no-write-fetch-head`. Failure → `FRESHNESS_UNVERIFIED`.
  No TTL, no automatic offline fallback. `--offline` uses the last fetched approved revision
  (mirror), else (for `refs/heads/*` approved refs) the KB checkout's remote-tracking ref,
  and reports `freshness = unverified`.
* **Selection** (`--snapshot`): `auto` (default), `latest`, `pinned`, `working-tree`, or a
  revision. `auto`: host binding `selection` (`.kbw.toml`) if set; else if a host pin exists and
  differs from the approved tip → `UPDATE_REQUIRED` (both revisions reported); else latest.
  `pinned` uses the host pin (submodule gitlink in host `HEAD`, or `.kbw.toml pin`) and still
  reports the approved tip. A pin is never called latest. Revisions not reachable from the
  approved tip are `approved = false` and make completeness at best `partial`.
* The KB working tree, index, HEAD, branches and submodule pointers are never modified by
  reading commands: no pull/merge/rebase/reset/checkout/stash/submodule update.
* **Engine compatibility**: before interpreting a snapshot, kb reads `core/release.toml` at
  that revision; a different `engine_version`, `document_schema`, `index_schema` or `protocol`
  → `UPDATE_REQUIRED`. `kb doctor` (check `snapshot-engine`) applies the same test to the
  approved tip and the host pin from the mirror: `fail` for the revision `--snapshot auto`
  selects, `warn` for the other, `skip` when neither is known.
* **Snapshot key** = sha256 over profile, profile config path, normalized source identity,
  approved ref, revision, engine
  version, index schema, parser version and overlay digest. A working-tree snapshot is built
  from the same frozen listing that produced its key; every working-tree read is checked
  against the listed content id, and a file that changes, appears or disappears during the
  call fails the command with a retryable `IO_ERROR` — nothing is stored, so a key never names
  other content.
* **Proposal overlay** (`--include-proposals`): files under the profile's knowledge/registry
  paths that differ between `merge-base(HEAD, approved)` and the KB working tree (committed,
  staged, unstaged, untracked). Overlay records are labeled `proposal` (`new`, `modifies <id>`,
  `removes <id>`), never override accepted records and never count as mandatory.
* Host binding file `.kbw.toml` in the host root (optional): `repo`, `pin`, `selection`.

## 7. Index

SQLite (bundled, FTS5) at `.cache/index/<profile>.sqlite`, WAL mode, `busy_timeout`.
Content-addressed `docs` (key = content id: git blob oid for snapshots, sha256 for working
tree files, + parser version) with parsed record JSON, meta JSON, FTS row, path-selector rows
(`dir_prefix` indexed) and term rows (concepts/aliases/intents indexed); `snapshots`
(key, revision, registry/profile JSON, diagnostics JSON); `snapshot_docs(snapshot, path,
origin, doc)`. Building a snapshot runs in one `BEGIN IMMEDIATE` transaction: unchanged
content ids are reused, only new blobs are parsed; readers see the old or the new state.
Documents are stored only under the content id of the bytes they were parsed from (sha256 ids
are verified at insert; Git blob ids are immutable). The `meta` table records
`index_schema`, `layout` (table layout revision), `parser_version`, `engine_version` and
`profile`; a missing or different value means an outdated but well-formed index, which is
rebuilt in place (info `INDEX_REBUILT`, recovery kind `outdated`). Only real damage moves the
file aside to `<name>.corrupt-<pid>-<nanos>` (warning `INDEX_RECOVERED`, kind `corrupt`):
corrupt or not a database (`SQLITE_CORRUPT`/`SQLITE_NOTADB`), no `meta` table (a foreign
database), unreadable metadata, a required table missing while the metadata matches, WAL mode
unavailable, or damage detected with `PRAGMA quick_check` after a query fails; the snapshot
is then rebuilt and the query retried once. The same retry covers a snapshot collected by
another process. A duplicated id is served from its first file by path (`DUPLICATE_ID` is
still reported). A proposal file byte-identical to the accepted file is not a proposal. GC
keeps the most recent 8 snapshots. User search strings are tokenized and each token is quoted
as an FTS5 string; raw FTS syntax is never passed through.
Working-tree content ids reuse a stat cache (size, mtime, ctime, inode; entries modified
within 2 s of the cache write are re-hashed, as in Git's index); bytes read during a build are
verified against the listed content id, and a stat-cache entry found wrong is dropped so a
retry re-hashes the file.

## 8. CLI

Global options: `--root`, `--config`, `--profile project|maintainer`, `--format
compact|human|json` (`--json`), `--offline`, `--snapshot`, `--host`, `--quiet`,
`--skill-protocol <n>` (mismatch → `SKILL_OUTDATED`). Unknown arguments are rejected.

Commands: `init`, `doctor`, `validate`, `index`, `context`, `search`, `show`, `sync`,
`impact`, `integrate`, `migrate`, `update {check,prepare,divergence,abandon}`, `schema`,
`version`. Mutating commands are dry-run by default and write only with `--apply`
(`update prepare` writes only to its own branch/worktree; `update abandon <branch>` reports
what it would remove and removes the worktree and branch only with `--apply`, refusing a
worktree with uncommitted, untracked or conflicted files with `CONFLICT` unless
`--apply --force`). `doctor --online` fetches the approved ref; combined with `--offline` it
is `INVALID_INPUT`. `kb version` prints the versions and `build <fingerprint>` (§9).

Text output (compact, human) of `show`, `search` and `impact` starts with one snapshot
provenance line in the wording of the `context` header, without ref, source and key:
`snapshot: <rev12|working-tree> (<selection>, freshness=<verified|unverified>);
approved=<yes|no|unknown>[; latest=<tip12>][; pin=<pin12>][; overlay=<digest12> (<n> files)]`.
`show --raw` keeps stdout byte-exact and prints that line to stderr as `kb: <line>` (even with
`--quiet`) when freshness is unverified or `approved` is not `yes`.

Machine mode: stdout carries only the protocol document; progress/diagnostics go to stderr.

Envelope: `{"protocol":"kb.cli.v1","command":..,"ok":bool,"result":..|null,"error":{code,
exit_code,message,details?,hint?,diagnostics?}|null,"meta":{..non-deterministic..}}`;
optional error members are omitted when absent. Argument errors in JSON mode (`--json`,
`--format json` or `--format=json` among the raw arguments) still print one envelope with
error `USAGE` (exit 2); `command` is the subcommand named in the arguments, or null when none
is. `--help`/`--version` in JSON mode print an ok envelope with `result.help`.

Exit codes: 0 ok; 1 internal; 2 usage; 10–19 project/config (`PROJECT_NOT_INITIALIZED`=10,
`CONFIG_INVALID`=11, `UNSUPPORTED_SCHEMA_VERSION`=13 (12 is unused and not reassigned:
invalid records are `validate` diagnostics and the context reason `SNAPSHOT_INVALID`),
`ALREADY_INITIALIZED`=14, `UNKNOWN_SCOPE`=15, `NOT_FOUND`=16); 20–29 freshness/snapshot
(`FRESHNESS_UNVERIFIED`=20, `UPDATE_REQUIRED`=21, `SNAPSHOT_NOT_FOUND`=22,
`SKILL_OUTDATED`=23); 30–39 context (`CONTEXT_INCOMPLETE`=30, `CONTEXT_BUDGET_EXCEEDED`=31);
40–49 checks (`VALIDATION_FAILED`=40, `ROUTING_TESTS_FAILED`=41, `DRIFT_DETECTED`=42,
`ENGINE_DIVERGED`=43, `IMPACT_UNACKNOWLEDGED`=44, `CONFLICT`=45); 50–59 runtime/update
(`RUNTIME_INCOMPATIBLE`=50, `UPDATE_CONFLICT`=51, `MIGRATION_FAILED`=52, `UPDATE_FAILED`=53);
60–69 environment (`GIT_ERROR`=60, `INDEX_ERROR`=61, `IO_ERROR`=62, `UNSAFE_PATH`=63,
`INVALID_INPUT`=64).

`context` exits 0 only when completeness is `complete`; otherwise it prints the full result
and exits 30 (`CONTEXT_INCOMPLETE`).

## 9. Launcher and runtime

`kbw` (POSIX sh) reads `core/release.toml` with line-based parsing, computes the engine
fingerprint = sha256 over sorted `sha256  path` lines of the build inputs (`Cargo.toml`,
`Cargo.lock`, `rust-toolchain.toml`, `core/release.toml`, `core/cli/Cargo.toml`,
`core/cli/src/**`), and executes `.cache/runtime/<fingerprint>/kb` with `KB_ROOT` set.
A stamp file + `find -newer`/`-cnewer` avoids re-hashing on warm runs.

The runtime of a checkout is created only by explicit, documented actions:

* `kbw --kbw-bootstrap` builds from source (`cargo build --release --locked -p kb`, pinned
  toolchain) and activates it by atomic rename. The fingerprint is passed to Cargo as
  `KBW_BUILD_FINGERPRINT` and compiled in (`option_env!`, a tracked Cargo input, so a changed
  fingerprint always recompiles the engine even when file mtimes look fresh). `kb version`
  prints it as `build <fp>` and `kb --json version` as `build_fingerprint` (`unknown` for a
  build made outside `kbw`). The smoke test (`kb --json version`) requires
  `build_fingerprint` and `engine_version` to match, and the build fails
  (`KBW_BOOTSTRAP_FAILED`) if the engine inputs changed while it ran. Cargo builds into
  `KBW_CARGO_TARGET_DIR` (default `<root>/.cache/cargo-target`; a relative value is resolved
  against the directory `kbw` is run from, as `kb update prepare` does, not against the root);
* `kbw --kbw-install-artifact <archive|https-url> --sha256 <hex>` (or `--sha256-file`)
  verifies the digest before listing/extracting, rejects unsafe entries (absolute, `..`,
  links, devices), checks `BUILD-INFO` (engine version, target, fingerprint must equal the
  local engine), smoke-runs the binary (it must report the same build fingerprint), and
  activates atomically: a new runtime directory by one rename, a reinstall of the same
  fingerprint by renaming `BUILD-INFO` and `kb` over the old files, so `<fp>/kb` resolves at
  every moment and running processes keep their binary; failures leave the previous runtime
  intact;
* `kbw --kbw-package <out-dir>` archives the active source-built runtime of this fingerprint
  (smoke-tested the same way); when the active runtime is an installed artifact or does not
  report this fingerprint, it builds from source instead.

Normal commands never build or install an engine: when the runtime for the current
fingerprint is missing (first use, an engine edit, or a KB checkout moved to another engine),
they fail with `KBW_RUNTIME_NOT_BOOTSTRAPPED` (exit 50). `KBW_AUTO_BOOTSTRAP=1` opts in to
implicit builds (CI). `kb update prepare` bootstraps the target engine inside its own update
worktree as an explicit, reported step.

The engine-owned workflows `.github/workflows/upstream-ci.yml` and `release.yml` are
inherited by every downstream; a fork disables them without editing them (which would be
engine divergence) by setting the repository variable `KB_ENGINE_CI` (skips the CI job
`check`) or `KB_RELEASE` (skips the release jobs `verify`, `build` and `publish`) to
`disabled`; any other value, or no variable, keeps them enabled.

## 10. Integrations

`kb integrate --generate` renders `core/skills/kb/*` with `project/skill-config/skill.toml`
into `project/skill-config/generated/` (skill bundle + managed instruction blocks).
`kb integrate [--check|--apply]` (host level) installs the bundle into the host repository:
`.claude/skills/kb/` (Claude Code), `.agents/skills/kb/` (Codex; Cursor also reads it) and
managed blocks in `CLAUDE.md` / `AGENTS.md` delimited by
`<!-- kb:begin <name> -->` / `<!-- kb:end <name> -->`. Installed hashes are tracked in
`.kbw/integration.lock` in the host; user-modified generated files or blocks are reported as
conflicts and never silently overwritten.
The skill setting `snapshot` (`auto|latest|pinned`) is written as an explicit `--snapshot`
into every generated kbw command; `auto` passes nothing, so a host `.kbw.toml selection`
decides, while `latest`/`pinned` override it. The shipped host CI templates run
`kbw impact --snapshot pinned --check`, which checks against the host pin (freshness is still
verified) and therefore does not fail with `UPDATE_REQUIRED` while a merged knowledge change
waits for the host pin update.
