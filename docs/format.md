# Record format

kb has exactly one authoring format for project knowledge: a Markdown file that starts with
strict TOML front matter between `+++` lines. This guide covers the fields, the eight record
kinds, scope and selectors, links, policies and overrides, registries, and every diagnostic
that `kb validate` can report.

The normative contract is [architecture.md §3–§4](architecture.md#3-record-format-document-schemas-1-and-2);
the design decision is [ADR 0002](adr/0002-record-format.md). How records are selected for a
task is described in [context.md](context.md); where they are read from and why they can be
trusted is in [snapshots-and-trust.md](snapshots-and-trust.md).

All examples use the registry of the synthetic example
(`core/templates/examples/synthetic-multirepo`, namespace `example`). They were checked with
`kb validate --strict` in a scratch KB created by `kb init --example synthetic-multirepo --apply`.

## Where records live

| Path (relative to the profile directory, `project/`) | Content |
|---|---|
| `project.toml` | profile config: namespace, approved source, knowledge roots, context defaults |
| `registry/{owners,repos,modules,features,concepts}.toml` | registries; a missing file means an empty registry |
| `<knowledge root>/**/*.md` | records; the default root is `knowledge`, grouping below it is free-form |
| `routing-tests/*.toml` | routing fixtures (run by `kb validate`) |

Rules for files under a knowledge root:

* Every `*.md` file is a record, except files named `README.md` (any case), which are
  documentation and never parsed.
* Other files are ignored with the warning `NON_RECORD_FILE` (`.gitkeep` is ignored silently).
* Symlinks are never followed: `SYMLINK_NOT_ALLOWED` (error).
* Limits: a record file is at most 512 KiB, its front matter at most 128 KiB, any text field
  at most 16 KiB of UTF-8, any list at most 500 entries, a title at most 200 characters.

## Anatomy of a record

```text
+++                      <- first line, exactly `+++` (a UTF-8 BOM and CRLF are tolerated)
schema = 2               <- TOML front matter: typed, normative content
...
+++                      <- closing line
Optional Markdown body.  <- non-normative explanation, split into sections at `## ` headings
```

The front matter is parsed twice: first as generic TOML (syntax, duplicate keys, `schema`,
`kind`), then strictly into the type of its kind. Unknown fields, duplicate keys, wrong types,
unknown enum values and missing required fields are errors. TOML comments are allowed.

**Top-level keys come before the first table.** In TOML every `key = value` line after a
`[table]` header belongs to that table. A top-level field written after `[scope]` becomes
`scope.<field>` and is rejected:

```toml
[scope]
repos = ["mobile"]

context = "..."          # wrong: this is scope.context
```

`kb validate` reports it as:

```text
error FRONT_MATTER_INVALID project/knowledge/bad/order.md:12: TOML parse error at line 12, column 1: unknown field `context`, expected one of `product`, `repos`, `modules`, `features`
```

Line numbers in TOML errors are file lines (the front matter starts at line 2, after the
opening `+++`), and the column is a 1-based character column. The diagnostic's `line` field
is set to the same line, so compact output prints `path:line` and human output
`--> path:line`; JSON diagnostics carry `line`. Errors without a TOML position have no line.

## Common fields

| Field | Type | Required | Rules |
|---|---|---|---|
| `schema` | integer | yes | `1` or `2`; use `2` for new fields, migrate existing records with `kb migrate` |
| `id` | string | yes | `<namespace>.<segment>(.<segment>)*`, see [Identifiers](#identifiers) |
| `kind` | string | yes | `policy`, `feature`, `invariant`, `contract`, `decision`, `procedure`, `reference`, `gap` |
| `title` | string | yes | one non-blank line, 1–200 characters |
| `status` | string | yes | `draft`, `accepted`, `deprecated`, `superseded` |
| `owner` | string | yes | an id from `registry/owners.toml` with authority over the scope |
| `[scope]` | table | yes | mandatory applicability, see [Scope](#scope-mandatory-applicability) |
| `[selectors]` | table | no | relevance signals only, see [Selectors](#selectors-relevance-only) |
| `[links]` | table | no | `requires`, `rationale`, `related`, `supersedes`, see [Links](#links) |
| `[applicability]` | table | no | `versions = { <repo> = "<semver requirement>" }` |
| `[[anchors]]` | array of tables | no | provenance, see [Anchors](#anchors-and-provenance) |

Lifecycle: only `accepted` records are ever mandatory, dependency or supplementary context.
`draft`, `deprecated` and `superseded` records stay addressable through `kb show` and
`kb search`; as ranking candidates they are excluded with reason `not-accepted`, and a local
draft can appear in context only as a labeled proposal (`--include-proposals`). An unknown
status is a parse error, never treated as accepted. Status in a file is not proof of review; see
[Trust model](snapshots-and-trust.md#trust-model).

### Identifiers

| Kind of id | Pattern | Max | Unique within |
|---|---|---|---|
| record id | `^[a-z][a-z0-9]*(-[a-z0-9]+)*(\.[a-z0-9]+(-[a-z0-9]+)*)+$` | 128 bytes | the profile |
| namespace (first segment of a record id) | `^[a-z][a-z0-9]*(-[a-z0-9]+)*$` | 32 bytes | set in `project.toml` |
| local id (statement, exception, item, party, obligation, setting name) | `^[a-z0-9]+(-[a-z0-9]+)*$` | 64 bytes | its list (`behaviors` and `boundaries` together; exceptions within their statement) |
| registry id | `^[a-z0-9]+([-_.][a-z0-9]+)*$` | 64 bytes | its registry file |

Record ids are stable forever. Once an id has been approved it keeps its kind and is never
removed or reused; retire a record with `status = "deprecated"` or supersede it
(checked with `kb validate --base <rev>`).

## Kinds

### Schema 2 additions

Schema 1 remains readable with its original fields and defaults. Schema 2 adds optional
structured domain knowledge; migration only updates declarations and preserves content.
The examples below that use schema 1 remain valid. Versioned editor schemas are shipped as
`core/schemas/record.v1.schema.json` and `record.v2.schema.json`.

| Field | Record kind | Meaning |
|---|---|---|
| `introduced`, `retired` | all | `YYYY-MM-DD` or host commit id; start inclusive, end exclusive; both bounds use the same kind |
| `verified_at`, `review_by` | all | real calendar dates, with the review deadline at or after verification |
| `scope.change_types` | all | change categories such as migration or retry |
| `delivery` | policy, invariant | scoped (default) or always; always needs unconditional product scope |
| `states`, `transitions`, `clocks`, `data_sources`, `scenarios` | feature | subsystem model; transition endpoints must be declared states |
| `scenarios`, `consumers` | contract | test cases and `{repo,path,symbol?}` consumers, using registered repos |
| `terms` | reference | `{term,meaning,source}` glossary; normalized terms must be unique |
| `verify` | statement, obligation | declarative probes; never a shell command |
| `anchors.stamp` | anchor | `{commit,start_line,end_line,sha256}` over a Git source span |

A scenario has `id`, `given` and `expect` strings. A transition has `id`, `from`, `to` and
`when`. Feature states use the same `{id,text}` item format as behaviors. Freshness and
validity are distinct: verification does not introduce or retire knowledge. Dates have no
implicit timezone; commit ancestry is resolved by the host adapter, never by the parser.

Probe kinds: `commit-message`/`branch-name` with `pattern`; `forbidden-import` with nonempty
`from`/`to` globs; `naming`/`banned-api` with nonempty `paths` and a regex `pattern`. Unknown
fields, invalid regexes and unsafe globs are errors. A probe glob (`paths`, `from`, `to`)
whose text before the first `:` contains no `/`, `[` or `{` is `repo:`-qualified (a `:`
inside a class or brace group, as in `[a:b]/x.md`, is pattern text), and that text must
name a registry repo exactly (`UNKNOWN_REPO`, message ``verify: `<glob>` names unknown repo
`<repo>` ``). A typo or a case change such as `Mobile:app/**` would never match, so
validation and `kb verify` fail instead of skipping the probe. A probe declaration is not
evidence that it ran. A synthetic probe on a rule of the example's
`example.mobile.token-storage` policy:

```toml
[[rules]]
id = "no-plaintext"
level = "must-not"
text = "Write refresh tokens to logs, preferences, files or crash reports."

[[rules.verify]]
kind = "banned-api"
paths = ["mobile:app/auth/**"]   # repo-relative globs, optionally "repo:glob"
pattern = 'Log\.[a-z]+\(.*refreshToken'
```

### Original kind fields

Normative content is typed and atomic. A **statement** is
`{ id, level, text, conditions = [..], exceptions = [{ id, text }] }` with `level` one of
`must`, `must-not`, `should`, `should-not`, `may`. An exception that changes what an
obligation means must be a typed `exceptions` entry: it is always rendered with its statement
and never dropped by budgeting. An **item** is `{ id, text }`.

| Kind | Required kind fields | Optional kind fields | Can be mandatory |
|---|---|---|---|
| `policy` | at least one of `rules` (statements), `settings`, `overrides` | the other two | yes |
| `feature` | `feature` (registry feature id), `summary`, `behaviors` (≥ 1 item) | `boundaries` (items) | no |
| `invariant` | `statements` (≥ 1) | | yes |
| `contract` | `parties` (≥ 2: `{ id, repo, role, modules = [..] }`), `obligations` (≥ 1 statement plus `party`, a declared party id) | `interface` | yes |
| `decision` | `context`, `decision`, `reasons` (≥ 1) | `alternatives = [{ option, rejected_because }]`, `consequences` | no |
| `procedure` | `steps` (≥ 1 item), `expected` (≥ 1) | `preconditions` | no |
| `reference` | `summary` | `sources = [{ title, url?, path? }]` | no |
| `gap` | `gap` (`missing`, `ambiguity`, `contradiction`), `description` | `affects` (existing record ids), `questions` | yes |

"Can be mandatory" means that an accepted record of that kind whose scope applies to a task is
always delivered, independent of ranking. Other kinds reach the context through `requires`,
`rationale`, `related` or ranking. Procedures are data: kb shows them and never executes them.

### Minimal valid record of each kind

The shipped templates in [`core/templates/records/`](../core/templates/records/) show every
field of each kind with placeholder text. The records below are the smallest valid form.
Each passes `kb validate --strict` against the synthetic example registry.

```toml
+++
schema = 1
id = "example.min.policy"
kind = "policy"
title = "Tokens never go to plaintext storage"
status = "draft"
owner = "team-mobile"

[scope]
repos = ["mobile"]

[[rules]]
id = "no-plaintext"
level = "must-not"
text = "Store refresh tokens in plaintext storage."
+++
```

```toml
+++
schema = 1
id = "example.min.feature"
kind = "feature"
title = "Sign-in"
status = "draft"
owner = "architecture"
feature = "login"
summary = "Lets a user sign in and keeps the session alive."

[scope]
features = ["login"]

[[behaviors]]
id = "refresh-on-expiry"
text = "An expired access token is refreshed once before the request is retried."
+++
```

```toml
+++
schema = 1
id = "example.min.invariant"
kind = "invariant"
title = "One refresh at a time"
status = "draft"
owner = "team-mobile"

[scope]
modules = ["mobile.auth"]

[[statements]]
id = "single-flight"
level = "must"
text = "At most one token refresh request is in flight per session."
+++
```

```toml
+++
schema = 1
id = "example.min.contract"
kind = "contract"
title = "Token refresh endpoint"
status = "draft"
owner = "architecture"

[scope]
repos = ["mobile", "backend"]

[[parties]]
id = "provider"
repo = "backend"
role = "Serves the refresh endpoint."

[[parties]]
id = "consumer"
repo = "mobile"
role = "Calls the refresh endpoint."

[[obligations]]
id = "rotate"
party = "provider"
level = "must"
text = "Return a new refresh token with every successful refresh."
+++
```

```toml
+++
schema = 1
id = "example.min.decision"
kind = "decision"
title = "Keep tokens in the platform keystore"
status = "draft"
owner = "architecture"
context = "Tokens were readable by other apps on rooted devices."
decision = "Store tokens only in the platform keystore."
reasons = ["The keystore is the only storage protected by the OS."]

[scope]
repos = ["mobile"]
+++
```

```toml
+++
schema = 1
id = "example.min.procedure"
kind = "procedure"
title = "Rotate the token signing key"
status = "draft"
owner = "team-backend"
expected = ["Tokens signed with the new key are accepted."]

[scope]
modules = ["backend.api"]

[[steps]]
id = "publish-key"
text = "Publish the new public key next to the old one."
+++
```

```toml
+++
schema = 1
id = "example.min.reference"
kind = "reference"
title = "Authentication overview"
status = "draft"
owner = "architecture"
summary = "How sign-in, tokens and refresh fit together."

[scope]
features = ["login"]
+++
```

```toml
+++
schema = 1
id = "example.min.gap"
kind = "gap"
title = "Offline refresh behavior is unspecified"
status = "draft"
owner = "architecture"
gap = "missing"
description = "Nothing says what happens when a refresh fails while offline."

[scope]
features = ["login"]
+++
```

## Scope (mandatory applicability)

```toml
[scope]
product = false            # true = product-wide; then repos/modules/features must be empty
repos = ["mobile"]
modules = ["mobile.auth"]
features = ["login"]
```

* Either `product = true` or at least one non-empty dimension. An empty scope is
  `SCOPE_EMPTY`; `product = true` with other dimensions is `SCOPE_INVALID`.
* **AND across dimensions, OR within a dimension.** An empty dimension does not constrain.
  The example above applies to tasks in repo `mobile` *and* module `mobile.auth` *and*
  feature `login`.
* All ids must exist in the registry (`UNKNOWN_REPO`, `UNKNOWN_MODULE`, `UNKNOWN_FEATURE`).
* Combinations no task can satisfy are `SCOPE_UNSATISFIABLE`: a module whose repo is not in
  `repos`, a feature whose repos do not intersect `repos`, or dimensions that imply disjoint
  repos.

A task has, per dimension, either a known set or an unknown value. For each non-empty record
dimension: a known task dimension applies when the sets intersect and is not applicable
otherwise; an unknown task dimension is *undetermined*. A record applies when every
constrained dimension applies; it is not applicable when any dimension is not applicable;
otherwise it is undetermined. Undetermined obligations are listed and make the context
`partial`; they are never silently dropped.

A task `--path` may name a file or a directory. It resolves to the modules and features whose
registry globs match it, match its directory form (`app/auth` for `app/auth/**`) or lie inside
it (`app` contains `app/auth/**`). A path that maps to no module keeps the modules dimension
known only when it looks like a file (`Main.kt`); otherwise modules and features become
unknown (`PATH_SCOPE_UNKNOWN`). Details: [context.md](context.md#task-scope-resolution).

### Version applicability

```toml
[applicability]
versions = { mobile = ">=2.0.0" }
```

Values are semver requirements (`APPLICABILITY_INVALID` otherwise) keyed by registry repo ids.
The host version comes from `--host-version repo=x.y.z` or the first line of the repo's
`version_file` in the host (a leading `v` is accepted). A known version that does not match
makes the record not applicable; an unknown version makes it undetermined. Compatibility is
never inferred from dates. See [context.md](context.md#applicability) for the rules applied to
required dependencies.

## Selectors (relevance only)

```toml
[selectors]
paths = ["app/src/**/auth/**", "backend:src/api/**"]   # optional `repo:` qualifier
concepts = ["auth-token"]                              # registry concept ids
intents = ["implement", "debug"]
aliases = ["token refresh", "обновление токена"]        # extra task phrases
```

Selectors only add ranking points to supplementary candidates. They never exclude an
applicable obligation and never make a record mandatory.

* `paths` are globs: `*` within one segment, `**` across segments, `?` one character,
  `[...]` a class. They are repo-relative and `/`-separated; absolute paths, `.`/`..`
  segments, backslashes and control characters are rejected (`SELECTOR_PATH_INVALID`). An
  unqualified glob matches in any repo; `repo:glob` only in that repo (`UNKNOWN_REPO` for an
  unknown qualifier). The qualifier is the text before the first `:` when it contains no
  `/`, `[` or `{`, checked as written: `Mobile:app/**` names the unknown repo `Mobile`,
  while `[a:b]/x.md` is unqualified.
* `concepts` must exist in `registry/concepts.toml` (`UNKNOWN_CONCEPT`).
* `intents` are `implement`, `refactor`, `debug`, `review`, `explain`.
* `aliases` are matched against the normalized task text with the rules in
  [Alias normalization](#alias-normalization); an alias that normalizes to nothing is
  `SELECTOR_ALIAS_INVALID`.

## Links

```toml
[links]
requires = []     # mandatory transitive dependencies (any kind); must be acyclic
rationale = []    # why this record exists (usually decisions); optional context
related = []      # one-hop suggestions, never followed transitively
supersedes = []   # ids this record replaces; must be acyclic
```

| Link | Effect in context | Validation |
|---|---|---|
| `requires` | followed breadth-first from every mandatory record, across repos and scopes; a missing, draft or superseded target makes the context `incomplete`; a deprecated target is included with label `deprecated` and a `REQUIRES_DEPRECATED` warning | acyclic (`REQUIRES_CYCLE`); an accepted record may not require a `draft` or `superseded` record (`REQUIRES_NOT_ACCEPTED`), and gets `REQUIRES_DEPRECATED` (warning) for a deprecated one |
| `rationale` | accepted targets of mandatory-tier records are supplementary candidates (+250 points), even when their own scope does not apply | target exists |
| `related` | accepted targets of mandatory-tier records are candidates (+80 points), one hop only | target exists |
| `supersedes` | informational; `kb show` of a superseded record names its successors | acyclic (`SUPERSEDES_CYCLE`); every target must have `status = "superseded"` (`SUPERSEDED_STATUS`, reported at the target); every `superseded` record must be named by some `supersedes` (`SUPERSEDED_ORPHAN`) |

All links: every target must exist, whatever its status (`DANGLING_LINK`; this also covers
gap `affects`), and a record may not link to itself (`LINK_SELF`). A cycle is reported once
per strongly connected component, at its smallest id, naming a shortest cycle.

Historical ids: `kb validate --base <rev>` compares with the records at a revision of the KB
checkout. An id that existed there must still exist (`ID_REMOVED`) and keep its kind
(`ID_KIND_CHANGED`). Keep old records with `deprecated` or `superseded` status instead of
deleting or reusing them.

## Policies, settings and overrides

Policies accumulate: every applicable rule of every applicable policy applies. There is no
precedence between rules and no expression language. The only thing that can be changed per
scope is a **named setting** that its policy declares overridable.

```toml
[[settings]]
name = "min-reviewers"
type = "integer"              # integer, boolean, string, string-set
value = 1
override = "stricter"         # forbidden (default), stricter, any
stricter = "higher"           # required iff override = "stricter"
override_owners = ["team-backend", "team-mobile"]   # empty = any owner
description = "Minimum number of approving reviewers per merge request."
```

`stricter` must fit the type: `lower`/`higher` for integers, `true`/`false` for booleans,
`superset`/`subset` for string sets. Strings have no stricter direction, so a string setting
can only be `forbidden` or `any`. An override lives in another policy:

```toml
[[overrides]]
target = "example.common.code-review#min-reviewers"
value = 2
reason = "Payment code moves money; two approvals catch more mistakes."
```

Setting and override declaration rules (single record):

| Code | Rule |
|---|---|
| `SETTING_NAME_INVALID` | the setting name is a local id |
| `SETTING_DUPLICATE` | setting names are unique in the policy |
| `SETTING_TYPE_MISMATCH` | `value` has the declared `type` |
| `SETTING_STRICTER_MISSING` | `override = "stricter"` needs `stricter` |
| `SETTING_STRICTER_INVALID` | the `stricter` direction fits the type |
| `SETTING_STRICTER_UNUSED` | `stricter` only with `override = "stricter"` |
| `SETTING_OWNERS_UNUSED` | `override_owners` only on overridable settings |
| `OVERRIDE_TARGET_INVALID` | `target` has the form `<policy-id>#<setting-name>` |
| `OVERRIDE_SELF` | a policy does not override its own setting |
| `OVERRIDE_DUPLICATE` | a policy overrides a target at most once |

Override rules (checked by `kb validate` for every override, and re-checked at context time,
where an invalid override is ignored with a warning):

| # | Code | The override is valid only if |
|---|---|---|
| 1 | `OVERRIDE_TARGET_UNKNOWN` | the target record exists, is a policy and declares that setting |
| 2 | `OVERRIDE_FORBIDDEN` | the setting's `override` is not `forbidden` |
| 3 | `OVERRIDE_TYPE_MISMATCH` | the value has the setting's type |
| 4 | `OVERRIDE_WEAKENS` | with `override = "stricter"`, the value is not weaker than the base value (equal is allowed; `superset` requires every base element, `subset` allows only base elements) |
| 5 | `OVERRIDE_SCOPE_EXCEEDS` | the overriding policy's scope is within the target policy's scope |
| 6 | `OVERRIDE_NOT_AUTHORIZED` | the overriding policy's owner is in `override_owners` when that list is non-empty |
| – | `OVERRIDE_AMBIGUOUS` | no two valid overrides from accepted policies set different values (string sets compare as sets) with overlapping scopes where neither scope is strictly more specific than the other: neither contains the other, or each contains the other (identical or equivalent scopes, such as `modules = ["mobile.auth"]` and `repos = ["mobile"], modules = ["mobile.auth"]`). Equal values are valid whatever the scopes |

Scope containment ("A within B"): a product-wide B contains everything, and a product-wide A
is within a product-wide B only. Otherwise, when B
constrains repos, the repos implied by A (its `repos`, else the repos of its modules and
features, else any repo) must be a subset of B's repos; when B constrains modules or features,
A must constrain the same dimension with a subset of B's ids.

Real output for rules 1–4, 6 and the ambiguity check against the synthetic
`example.common.code-review` policy:

```text
error OVERRIDE_WEAKENS project/knowledge/ov/weak.md [example.ov.weak]: override `example.common.code-review#min-reviewers`: 0 is weaker than the base value 1 (stricter = higher)
error OVERRIDE_FORBIDDEN project/knowledge/ov/forbidden.md [example.ov.forbidden]: override `example.common.code-review#require-green-ci`: the setting does not allow overrides
error OVERRIDE_NOT_AUTHORIZED project/knowledge/ov/auth.md [example.ov.auth]: override `example.common.code-review#min-reviewers`: owner `team-platform` is not in override_owners [team-backend, team-mobile]
error OVERRIDE_AMBIGUOUS project/knowledge/ov/amb1.md [example.ov.amb1]: overrides of `example.common.code-review#min-reviewers` by `example.ov.amb1` (3) and `example.backend.payments-review` (2, project/knowledge/repos/backend/payments-review.md) have overlapping scopes and neither is strictly more specific
```

How the effective value is chosen for a task is described in
[context.md](context.md#effective-settings).

## Anchors and provenance

```toml
[[anchors]]
kind = "source"          # source, test, change, doc
repo = "mobile"          # registry repo id
path = "app/auth/TokenStore.kt"
symbol = "TokenStore"
# commit = "<7..64 hex>", change = "!42", note = "..."
```

| `kind` | Needs |
|---|---|
| `source`, `test` | `repo` and `path` |
| `change` | `change` (for example a merge request reference) or `commit` |
| `doc` | `path` |

`path` must be a safe relative path (`ANCHOR_PATH_INVALID`), `commit` 7–64 hex characters
(`ANCHOR_COMMIT_INVALID`), `repo` a registry repo (`UNKNOWN_REPO`). Anchors are listed in the
`--explain` receipt, and `kb impact` matches changed host files against `source`, `test` and
`doc` anchor paths. Schema validation records provenance without running anything.
`anchors stamp` and `anchors check` separately inspect immutable Git bytes and optional
definition spans; a matching stamp does not prove the statement's truth or a test run.
`propose submit` verifies every anchor of the draft, so a submitted `change` anchor needs a
`repo` and a `commit` that resolves in it (or a `path` that exists there); an external
reference alone (`change = "!42"`) is not locally verifiable and the submission is refused.
`anchors stamp` never rewrites or adds a `change` anchor's `commit`: one with a commit is
stamped only `--at` that commit, and one without a `path` has no span to stamp.
**A test anchor alone does not prove that the test exists or runs, and a valid schema does
not prove that the text is true.** See [knowledge-lifecycle.md](knowledge-lifecycle.md).

## Body sections

The Markdown body is optional and non-normative. It is split at lines starting with `## `
(outside fenced code blocks). Text before the first heading is the section `intro`. A section
id is the heading's slug: lowercase, letters and digits kept (including non-Latin letters),
every other run of characters becomes `-` (`## Почему так?` → `почему-так`). Duplicate slugs
and headings with an empty slug are `BODY_INVALID`.

Sections are delivered only on request: `kb show <id>#<section>` or
`kb context --sections mandatory|all`. Because they are optional, obligations and exceptions
must not hide there: a body line (outside code blocks) containing the uppercase words `MUST`,
`MUST NOT`, `SHALL`, `SHALL NOT`, `SHOULD NOT` or `REQUIRED` produces the warning
`NORMATIVE_LANGUAGE_IN_BODY` (an error with `kb validate --strict`).

## Registries

Existing registries read `schema = 1` or `2`; new projects use `2`. Each has an array of
tables and rejects unknown fields. `change-types.toml` is a schema-2 addition.

| File | Entry | Fields |
|---|---|---|
| `owners.toml` | `[[owner]]` | `id`, `title`, `repos = [..]`, `product = bool` |
| `repos.toml` | `[[repo]]` | `id`, `title`, `remotes = [..]`, `version_file?` |
| `modules.toml` | `[[module]]` | `id`, `repo`, `title`, `paths = [..]`, `features = [..]` |
| `features.toml` | `[[feature]]` | `id`, `title`, `repos = [..]`, `paths = [..]` |
| `concepts.toml` | `[[concept]]` | `id`, `title`, `aliases = [..]`, `paths = [..]` |
| `change-types.toml` | `[[change_type]]` | `id`, `title`, aliases and optional path/symbol hints for change-category discovery |

* Owner authority: an owner with non-empty `repos` may own only records whose implied repos
  lie within them; product-wide records need an owner with `product = true`
  (`OWNER_NOT_AUTHORIZED`).
* `repos.remotes` identify the host repository by its Git remotes (see
  [host detection](snapshots-and-trust.md#host-detection)); `version_file` is a host-relative
  file whose first line is the host code version.
* Module and feature `paths` map task paths to modules and features. Concept `paths` are
  disambiguation hints for ambiguous aliases.
* Adding a registry entry never requires rebuilding the CLI.

### Alias normalization

Task text, concept aliases and record aliases are normalized the same way:

1. Unicode NFKC;
2. Unicode lowercase;
3. `ё` → `е`;
4. every character that is not a letter or digit becomes a space;
5. whitespace is collapsed and trimmed.

Matching is on whole-token sequences. An alias ending in `*` matches its last token by prefix:
`композици*` matches `композиция`, `композицию` and `композиции`; the plain alias
`композиция` does not match `композицию`. There is no stemming, so morphological variants
must be listed or written with `*`. A concept's own id (with `-`, `_` and `.` read as spaces)
is an implicit alias. An alias shared by several concepts produces an
[ambiguity](context.md#ambiguity), never a guess.

## JSON Schemas

`core/schemas/*.schema.json` (draft 2020-12) are generated from the Rust model and
drift-checked (`kb schema --check`, exit 42 `DRIFT_DETECTED` on drift; `kb schema --write`
regenerates). `record.v1.schema.json` and `record.v2.schema.json` describe the respective
front-matter vocabularies; the others describe
`project.toml`, the registries, routing fixtures, `skill.toml`, `.kbw.toml`, `upstream.toml`,
`core/release.toml`, the `kb.cli.v1` envelope and `kb.code.v1` provider requests/responses.
Routing fixtures, host bindings and skill configuration retain their independent schema 1.

A schema-valid document can still be rejected by `kb validate`. Runtime-only rules include
uniqueness of local ids inside arrays of tables, section slugs, self links and override self
targets, path safety and glob syntax, alias normalization, semver requirements, text limits in
bytes (the schema's `maxLength` counts characters), every cross-record rule (namespace,
duplicate ids, registry references, owner authority, scope satisfiability, contract parties,
links, cycles, lifecycle, overrides) and the `--base` checks. The complete list is in
[`core/schemas/README.md`](../core/schemas/README.md).

## Validating

```sh
./kbw validate                  # working tree: records, registries, links, policies, routing fixtures
./kbw validate --strict         # warnings count as errors
./kbw validate --base <rev>     # also check that ids at <rev> still exist with the same kind
./kbw validate --no-routing     # skip routing fixtures
./kbw validate --templates      # also check core/templates (upstream check)
./kbw validate --stale 90 --on 2026-10-07  # explicit calendar reference for an audit
```

`--templates` parses every project template as its destination file. Registry templates
are parsed strictly (`REGISTRY_PARSE`) and must declare a `schema` a loaded corpus accepts
(`UNSUPPORTED_SCHEMA_VERSION`); `registry/change-types.toml` must declare `schema = 2` and
gets the change-type checks of `kb validate` (ids, duplicates, globs, aliases and symbols),
except repo qualifiers, which name the downstream's repos and are checked there.

With `--templates`, the text report says that the templates were checked, with their own
counts, which are also included in the totals: compact `templates: checked (shipped templates
and examples): 0 error(s), 0 warning(s)`, human `Shipped templates and examples were validated
(--templates): 0 error(s), 0 warning(s)`; JSON has `result.templates = true`.

`kb validate` reads the KB working tree without network access unless `--snapshot`
selects another snapshot; Git can supply a deterministic freshness reference. Explicit
`--on` is required when an audit means today's calendar age. It exits 40 (`VALIDATION_FAILED`) when there are errors (or
warnings with `--strict`), else 41 (`ROUTING_TESTS_FAILED`) when a routing case fails.
Diagnostics are sorted by severity, path, record, line, code and message. Compact output:

```text
validate project: records: 38, files: 41, errors: 7, warnings: 1
error DANGLING_LINK project/knowledge/bad/cross.md [example.bad.cross]: links.requires: target `example.bad.missing` does not exist
error OWNER_NOT_AUTHORIZED project/knowledge/bad/cross.md [example.bad.cross]: owner `team-mobile` may only own records within repos [mobile], but the scope implies [backend]
error REQUIRES_NOT_ACCEPTED project/knowledge/bad/cross.md [example.bad.cross]: accepted record requires `example.min.policy`, which is draft; required knowledge must be accepted
error SCOPE_UNSATISFIABLE project/knowledge/bad/cross.md [example.bad.cross]: module `mobile.auth` belongs to repo `mobile`, which is not in scope.repos [backend]
error FRONT_MATTER_SYNTAX project/knowledge/bad/dup.md:6: TOML parse error at line 6, column 1: duplicate key
error FRONT_MATTER_INVALID project/knowledge/bad/order.md:12: TOML parse error at line 12, column 1: unknown field `context`, expected one of `product`, `repos`, `modules`, `features`
error FRONT_MATTER_INVALID project/knowledge/bad/status.md:6: TOML parse error at line 6, column 10: unknown variant `approved`, expected one of `draft`, `accepted`, `deprecated`, `superseded`
warning NORMATIVE_LANGUAGE_IN_BODY project/knowledge/bad/cross.md [example.bad.cross]: section `intro` uses `MUST NOT`; move obligations and exceptions into typed front matter
```

A file that fails to parse is not loaded, so it also cannot satisfy links from other records.
The same file, record, registry and cross-record diagnostics are computed for every indexed
snapshot (routing fixtures and `--base` checks are not); any error there makes context from
that snapshot `incomplete` (`SNAPSHOT_INVALID`).

## Diagnostic catalogue

Severity is `error` unless noted. "Where" names the source file that emits the code.

### File and front matter (`parse.rs`, `corpus.rs`, `source.rs`, `snapshot/tree.rs`)

| Code | Meaning |
|---|---|
| `RECORD_TOO_LARGE` | the record file exceeds 512 KiB |
| `RECORD_NOT_UTF8` | the record is not valid UTF-8 |
| `FRONT_MATTER_MISSING` | the first line is not `+++`, or the front matter is not closed by a `+++` line |
| `FRONT_MATTER_TOO_LARGE` | the front matter exceeds 128 KiB |
| `FRONT_MATTER_SYNTAX` | TOML syntax error, including duplicate keys |
| `FRONT_MATTER_INVALID` | strict typed parse failed: unknown field, wrong type, unknown enum value, missing required field (also a top-level key written after a table) |
| `SCHEMA_FIELD_MISSING` / `SCHEMA_FIELD_INVALID` | `schema` is absent / not an integer |
| `UNSUPPORTED_SCHEMA_VERSION` | `schema` is not readable by this engine: records and registry files accept `1` or `2`, `registry/change-types.toml` requires `2`, routing fixtures require `1` |
| `KIND_MISSING` / `KIND_INVALID` / `KIND_UNKNOWN` | `kind` is absent / not a string / not one of the eight kinds |
| `BODY_INVALID` | duplicate section slug, or a `## ` heading with an empty slug |
| `NON_RECORD_FILE` (warning) | a non-`.md` file under a knowledge root is ignored |
| `SYMLINK_NOT_ALLOWED` | a symlink under a knowledge root, template tree or Git snapshot |
| `SOURCE_UNREADABLE` | a file cannot be read, or a Git blob exceeds 4 MiB |
| `UNSAFE_PATH` | a Git snapshot contains a path that is not valid UTF-8 or not a safe relative path |

### Single-record rules (`parse.rs`)

| Code | Meaning |
|---|---|
| `ID_INVALID` | the record id does not match the record id pattern or exceeds 128 bytes |
| `TITLE_INVALID` | the title is blank, multi-line or longer than 200 characters |
| `OWNER_MISSING` | `owner` is empty |
| `SCOPE_EMPTY` | no `product = true` and no non-empty dimension |
| `SCOPE_INVALID` | `product = true` together with repos, modules or features |
| `SELECTOR_PATH_INVALID` | a selector glob is unsafe or not valid glob syntax |
| `SELECTOR_ALIAS_INVALID` | a selector alias normalizes to nothing |
| `LINK_SELF` | a link names the record itself |
| `LINK_ID_INVALID` | a link target or gap `affects` entry is not a record id |
| `APPLICABILITY_INVALID` | an `applicability.versions` value is not a semver requirement |
| `ANCHOR_INVALID` | an anchor lacks the fields its kind needs |
| `ANCHOR_PATH_INVALID` | an anchor path is not a safe relative path |
| `ANCHOR_COMMIT_INVALID` | an anchor commit is not 7–64 hex characters |
| `SOURCE_PATH_INVALID` | a reference source `path` is not a safe relative path |
| `LOCAL_ID_INVALID` / `LOCAL_ID_DUPLICATE` | a statement, exception, item, party or obligation id is malformed / repeated |
| `TEXT_EMPTY` / `TEXT_TOO_LONG` | a required text field is blank / exceeds 16 KiB |
| `LIST_TOO_LONG` / `LIST_DUPLICATE` | a list has more than 500 entries / repeats a value |
| `POLICY_EMPTY` | a policy has no rules, settings or overrides |
| `FEATURE_BEHAVIORS_EMPTY` | a feature has no behavior |
| `INVARIANT_EMPTY` | an invariant has no statement |
| `CONTRACT_PARTIES` | a contract has fewer than two parties |
| `CONTRACT_OBLIGATIONS_EMPTY` | a contract has no obligation |
| `CONTRACT_PARTY_UNKNOWN` | an obligation names an undeclared party |
| `DECISION_REASONS_EMPTY` | a decision has no reason |
| `PROCEDURE_STEPS_EMPTY` / `PROCEDURE_EXPECTED_EMPTY` | a procedure has no step / no expected result |
| `SETTING_*`, `OVERRIDE_TARGET_INVALID`, `OVERRIDE_SELF`, `OVERRIDE_DUPLICATE` | see [Policies, settings and overrides](#policies-settings-and-overrides) |
| `NORMATIVE_LANGUAGE_IN_BODY` (warning) | uppercase RFC 2119 keywords in an optional body section |

### Cross-record rules (`validate.rs`)

| Code | Meaning |
|---|---|
| `NAMESPACE_MISMATCH` | the id's first segment is not the profile namespace |
| `DUPLICATE_ID` | the same id is defined in more than one file (reported for every file; context serves the first file by path) |
| `OWNER_UNKNOWN` | the owner, or an `override_owners` entry, is not in `owners.toml` |
| `OWNER_NOT_AUTHORIZED` | the owner has no authority over the record's scope |
| `UNKNOWN_REPO` / `UNKNOWN_MODULE` / `UNKNOWN_FEATURE` / `UNKNOWN_CONCEPT` | a registry id in scope, selectors, applicability, anchors, contract parties or consumers, the `repo:` qualifier of a `selectors.paths` or `verify` probe glob (`paths`, `from`, `to`; the text before the first `:` when it has no `/`, as written, so `Mobile:` is unknown), or the feature field does not exist |
| `SCOPE_UNSATISFIABLE` | no task can match every scope dimension |
| `CONTRACT_PARTY_OUT_OF_SCOPE` | a party's repo is outside the contract's scope, so the contract would not reach that party |
| `CONTRACT_PARTY_MODULE_MISMATCH` | a party lists a module of another repo |
| `DANGLING_LINK` | a link or `affects` target does not exist |
| `REQUIRES_CYCLE` / `SUPERSEDES_CYCLE` | cycle in `requires` / `supersedes` |
| `REQUIRES_NOT_ACCEPTED` | an accepted record requires a draft or superseded record |
| `REQUIRES_DEPRECATED` (warning) | an accepted record requires a deprecated record |
| `SUPERSEDED_STATUS` | a record named in `supersedes` does not have status `superseded` |
| `SUPERSEDED_ORPHAN` | a `superseded` record is not named by any `supersedes` |
| `OVERRIDE_TARGET_UNKNOWN`, `OVERRIDE_FORBIDDEN`, `OVERRIDE_TYPE_MISMATCH`, `OVERRIDE_WEAKENS`, `OVERRIDE_SCOPE_EXCEEDS`, `OVERRIDE_NOT_AUTHORIZED`, `OVERRIDE_AMBIGUOUS` | override rules 1–6 and ambiguity |
| `ID_REMOVED` | (`--base`) an id present at the base revision no longer exists |
| `ID_KIND_CHANGED` | (`--base`) an id changed its kind since the base revision |

### Registries (`model/registry.rs`, `corpus.rs`)

| Code | Meaning |
|---|---|
| `REGISTRY_PARSE` | a registry file is not UTF-8 or fails the strict TOML parse |
| `REGISTRY_ID_INVALID` | a registry id does not match the registry id pattern or exceeds 64 bytes |
| `REGISTRY_DUPLICATE_ID` | an id repeats within one registry file |
| `REGISTRY_UNKNOWN_REPO` | an owner, module, feature or `repo:` glob qualifier (as written, so `Mobile:` is unknown) names an unknown repo |
| `REGISTRY_UNKNOWN_FEATURE` | a module lists an unknown feature |
| `REGISTRY_GLOB_INVALID` | a module, feature, concept or change-type glob is invalid |
| `REGISTRY_PATH_INVALID` | a repo `version_file` is not a safe relative path |
| `REGISTRY_ALIAS_INVALID` | a concept alias, or a change-type alias or symbol, normalizes to nothing |

Problems in `project.toml` are not diagnostics: they stop every command with
`CONFIG_INVALID` (exit 11), or `UNSUPPORTED_SCHEMA_VERSION` (exit 13) for a `schema` other
than `1` or `2`.

### Schema-2 fields (`parse.rs`, `parse/evolution.rs`, `validate.rs`, `model/registry.rs`, `freshness.rs`)

The fields are described in [Schema 2 additions](#schema-2-additions). Their lists, local ids
and texts also use the generic `LIST_TOO_LONG`, `LIST_DUPLICATE`, `LOCAL_ID_INVALID`,
`LOCAL_ID_DUPLICATE`, `TEXT_EMPTY` and `TEXT_TOO_LONG` codes above.

| Code | Meaning |
|---|---|
| `SCHEMA_FIELD_UNAVAILABLE` | a `schema = 1` record uses a schema-2 field (for example `introduced`, `delivery`, `scope.change_types`, `verify`, `anchors.stamp` or the `diagnose` intent); run `kb migrate` |
| `TEMPORAL_INVALID` | `introduced`/`retired` is neither an ISO calendar date nor a 7–64 hex commit id, or `verified_at`/`review_by` is not an ISO calendar date |
| `TEMPORAL_ORDER` | a date `retired` is not after `introduced` (the upper bound is exclusive) |
| `TEMPORAL_KIND_MISMATCH` | `introduced` and `retired` mix a date and a commit id |
| `FRESHNESS_ORDER` | `review_by` precedes `verified_at` |
| `CHANGE_TYPE_INVALID` | a `scope.change_types` entry, or an id in `registry/change-types.toml`, is not a valid local id |
| `UNKNOWN_CHANGE_TYPE` | a `scope.change_types` entry is not in `registry/change-types.toml` |
| `DELIVERY_SCOPE_INVALID` | `delivery = "always"` without unconditional product scope (`product = true`, no `change_types`, no `applicability`) |
| `ANCHOR_STAMP_INVALID` | an `anchors.stamp` lacks a path (and a repo, except on `doc` anchors), a commit id, a nonzero inclusive line range or a lowercase hex SHA-256 |
| `ANCHOR_STAMP_COMMIT_MISMATCH` | the anchor's `commit` and its stamp's `commit` disagree |
| `TRANSITION_STATE_UNKNOWN` | a feature transition's `from` or `to` is not a declared state |
| `CONSUMER_INVALID` / `CONSUMER_DUPLICATE` | a contract consumer lacks a registry repo id or a safe repo-relative path / repeats the same repo, path and symbol |
| `TERM_DUPLICATE` | two glossary `terms` normalize to the same term |
| `VERIFY_INVALID` | a `forbidden-import` probe has empty `from` or `to`, or a `naming`/`banned-api` probe has empty `paths` |
| `VERIFY_GLOB_INVALID` | a probe glob is unsafe or not valid glob syntax |
| `VERIFY_REGEX_INVALID` | a probe `pattern` is not a valid regex or exceeds the 1 MiB compiled-size limit |
| `FRESHNESS_DATE_UNKNOWN` (warning) | no reference date: no `--on` and no commit date to read; freshness is not evaluated |
| `REVIEW_OVERDUE` (warning) | `review_by` is before the reference date |
| `VERIFIED_AT_FUTURE` (warning) | `verified_at` is after the reference date |
| `KNOWLEDGE_STALE` (warning) | (`--stale <days>`) `verified_at` is older than the allowed age |
| `VERIFIED_AT_MISSING` (warning) | (`--stale <days>`) the record has no `verified_at` |

The five freshness warnings concern accepted records only, and only those with
`verified_at` or `review_by` unless `--stale` is given. The reference date is `--on`, else
the commit date of the host `HEAD` (outside a host, of the validated KB revision).
`kb context` reports the same warnings for the records it delivers.

### Routing fixtures, templates and informational notes (`context/routing.rs`, `validate.rs`, `cli/cmd_validate.rs`)

| Code | Meaning |
|---|---|
| `ROUTING_TEST_INVALID` | a routing fixture file or case does not parse or is inconsistent |
| `ROUTING_CASE_EMPTY` (warning) | a routing case has no expectations |
| `NON_ROUTING_FILE` (warning) | a non-`.toml` file under `routing-tests/` is ignored |
| `TEMPLATES_MISSING` | (`--templates`) `core/templates` or its `records/` directory is missing |
| `TEMPLATE_KIND_MISSING` | (`--templates`) no valid record template of some kind |
| `TEMPLATE_INVALID` | (`--templates`) a project template does not parse as its destination type |
| `TEMPLATE_EXAMPLE_EMPTY` (warning) | (`--templates`) an example directory has no `project/` |
| `PROJECT_NOT_INITIALIZED` (info) | (`--templates` on an uninitialized checkout) only templates were validated |
| `BASE_CHECK_SKIPPED` (info) | `--base` was given but there are no project records |
| `BASE_NOT_INITIALIZED` (info) | the project did not exist at the `--base` revision |

Diagnostics that appear only in context or reading commands (for example `PATH_SCOPE_UNKNOWN`,
`SETTING_CONFLICT`, `PROPOSAL_*`, `INDEX_RECOVERED`) are described in
[context.md](context.md#completeness) and [snapshots-and-trust.md](snapshots-and-trust.md).
