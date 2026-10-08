# Machine protocol (`kb.cli.v1`) and error codes

This is the reference for programs and agents that call kb: output channels, the JSON
envelope, what each command returns, every error and exit code, and how the protocol is
versioned. The normative contract is [architecture.md §8](architecture.md#8-cli); this page
describes engine `0.1.0`, CLI protocol `1`. Symptom-oriented fixes are in
[troubleshooting.md](troubleshooting.md).

## Output channels

| format | select with | stdout | stderr |
|---|---|---|---|
| `compact` (default) | nothing, or `--format compact` | dense text for agents | progress, diagnostics, errors |
| `terse` | `--format terse` | minimal tiered context; typed obligations retained, bodies deferred | same |
| `human` | `--format human` | verbose text for people | same |
| `json` | `--json` or `--format json` | exactly one `kb.cli.v1` document | same |

Rules that hold in every format:

* In JSON mode stdout carries **only** the protocol document: one pretty-printed JSON object
  followed by a newline. Nothing else is ever written to stdout, including for usage errors
  and `--help`.
* Progress and diagnostics go to stderr. Line shapes:
  * `kb: <progress>`: progress; suppressed by `--quiet` / `-q`;
  * `kb: <severity>[<CODE>] <path>: <message>`: a session diagnostic that is also copied into
    `meta.diagnostics` (see [Diagnostics in `meta`](#diagnostics-in-meta)); `warning` and
    `error` lines are printed even with `--quiet`, `info` lines are not;
  * `kb: snapshot: ...`: `show --raw` in a text format, when freshness is unverified or the
    revision is not approved (see [Snapshot line](#snapshot-line-in-text-output)); printed
    even with `--quiet`;
  * `error[<CODE>]: <message>` followed by optional `  details: <json>`, up to 50 diagnostic
    lines and `  hint: <text>`: a hard error (the command produced no result). It is printed
    in every format, including JSON mode, and regardless of `--quiet`. In text formats a
    failure that still carries a result (for example `CONTEXT_INCOMPLETE`) is printed the
    same way after the result; in JSON mode it is only in the envelope.
* The process exit code is `0` on success and otherwise equals `error.exit_code`.
* `--json` and `--format` together are a usage error.
* The launcher `kbw` reports its own failures before the engine runs, as text on stderr only
  (`kbw: error[KBW_...]: ...`), with an empty stdout even when `--json` was passed. See
  [Launcher error codes](#launcher-error-codes).

### Snapshot line in text output

In `compact` and `human` format, stdout of `show`, `search` and `impact` starts with one
provenance line, worded like the `context` header without `ref`, `source` and `key`:

```text
snapshot: <rev12|working-tree> (<latest|pinned|revision|working-tree>, freshness=<verified|unverified>); approved=<yes|no|unknown>[; latest=<tip12>][; pin=<pin12>][; overlay=<digest12> (<n> files)]
```

For example `snapshot: b76e8729980c (pinned, freshness=unverified); approved=yes;
latest=8b74dcf3fcfd; pin=b76e8729980c`. `show --raw` keeps stdout byte-exact (the file bytes
only); when freshness is unverified or `approved` is not `yes`, it prints the same line to
stderr as `kb: snapshot: ...`, even with `--quiet`. JSON output is unchanged: the provenance
is `result.snapshot`. `context` text keeps its own, longer header.

## Envelope

Every command prints this object in JSON mode (schema:
[`core/schemas/cli-envelope.v1.schema.json`](../core/schemas/cli-envelope.v1.schema.json)):

| field | type | meaning |
|---|---|---|
| `protocol` | string | always `"kb.cli.v1"` |
| `command` | string or null | subcommand name (`update` for all `update` subcommands); for usage errors the subcommand named in the arguments, or `null` when none is |
| `ok` | bool | `true` exactly when `error` is `null` |
| `result` | any or null | deterministic, command-specific result; `null` on hard errors |
| `error` | object or null | `code`, `exit_code`, `message`, and when present `details` (code-specific JSON), `hint` (string), `diagnostics` (array); absent members are omitted, not `null` |
| `meta` | object | non-deterministic data (timings, cache statistics, session diagnostics); never part of `result` and never counted in a context budget |

Keys are printed in sorted order; do not depend on key order, and ignore members you do not
know. `result` is deterministic for identical inputs: collections are sorted with id
tie-breaks and it holds no timings.

Failures come in two shapes:

* **Hard error**: `ok = false`, `result = null`, `error` set. The command could not produce a
  result (for example `CONTEXT_BUDGET_EXCEEDED`, `UPDATE_REQUIRED`, `NOT_FOUND`).
* **Result-carrying failure**: `ok = false`, `result` is the full result, `error` explains
  why the command still failed. Read both. These are `CONTEXT_INCOMPLETE` (`context`),
  `VALIDATION_FAILED` (`validate`, `doctor`), `ROUTING_TESTS_FAILED` (`validate`),
  `DRIFT_DETECTED` (`schema --check`, `integrate --check`), `CONFLICT` (`integrate --apply`),
  `IMPACT_UNACKNOWLEDGED` (`impact --check` with a well-formed or absent statement),
  `ENGINE_DIVERGED` (`update divergence`), `UPDATE_CONFLICT` and
  `UNSUPPORTED_SCHEMA_VERSION` (`update check`).

A hard error captured in a host repository of the synthetic example (`mobile`, KB mounted at
`.kb`) with `.kb/kbw --json context --intent implement --path app/auth/TokenRefresher.kt --budget 100`:

```json
{
  "command": "context",
  "error": {
    "code": "CONTEXT_BUDGET_EXCEEDED",
    "details": {
      "format": "json",
      "limit": 100,
      "required": 3584,
      "unit": "tokens-est"
    },
    "exit_code": 31,
    "hint": "re-run with --budget 3584 or more; mandatory knowledge is never truncated",
    "message": "the header and mandatory knowledge need 3584 tokens-est but the budget is 100"
  },
  "meta": {
    "elapsed_ms": 62
  },
  "ok": false,
  "protocol": "kb.cli.v1",
  "result": null
}
```

The same request with `--offline` instead of `--budget 100` prints the full context result
and exits 30 with:

```json
"error": {
  "code": "CONTEXT_INCOMPLETE",
  "details": {
    "completeness": "partial",
    "reasons": [
      {
        "code": "FRESHNESS_UNVERIFIED",
        "message": "the approved ref was not checked against the remote in this call",
        "status": "partial"
      }
    ],
    "receipt": "sha256:b085d8e3ff3d94701ba71a1513674f9637108e550e9df0cbe415c070ecf74309"
  },
  "exit_code": 30,
  "hint": "the result above lists what is missing; do not treat it as complete (pass --repo/--path/--host-version, sync, or fix validation errors)",
  "message": "context is partial: FRESHNESS_UNVERIFIED"
}
```

### Usage errors and help in JSON mode

Argument errors are detected before any command runs. In JSON mode (recognized from the raw
arguments `--json`, `--format json` or `--format=json`) stdout still gets one envelope with
`USAGE` (exit 2) and `meta = {}`; clap's own message goes to stderr. `command` is the first
argument that names a subcommand, otherwise `null`:

```json
{
  "command": null,
  "error": {
    "code": "USAGE",
    "exit_code": 2,
    "hint": "run `kbw <command> --help` for the accepted arguments",
    "message": "error: unexpected argument '--nosuch' found\n\nUsage: kb --json <COMMAND>\n\nFor more information, try '--help'."
  },
  "meta": {},
  "ok": false,
  "protocol": "kb.cli.v1",
  "result": null
}
```

`--help` and `--version` in JSON mode succeed with `result = {"help": "<text>"}`.

### `meta`

| member | when | content |
|---|---|---|
| `elapsed_ms` | every parsed command | wall-clock milliseconds (absent for usage errors) |
| `index` | commands that open the index (`context`, `search`, `show`, `impact`, `index`) and produce a result | build statistics: `snapshot_key`, `reused`, `files`, `parsed`, `reused_docs`, `proposals`, `errors`, `warnings`, `collected`, and `recovery` (`kind` = `corrupt`, `outdated` or `requested`, `reason`, and `moved_to` for `corrupt`) when the cache was replaced or rebuilt |
| `cache` | `kb index` with a result | `path` (absolute path of the SQLite file) and `bytes` (its size) |
| `diagnostics` | when a session noted something | array of diagnostics (`severity`, `code`, `message`, optional `path`, `record`, `line`) |

A hard error envelope carries only `elapsed_ms`; `index`, `cache` and `diagnostics` are
attached only when the command produced a result.

## Per-command results

Top-level `result` members of the current unreleased engine. Baseline examples above retain
their captured historical counts/hashes; additions below are defined by the current command
report types and integration tests. Members listed as "+ `snapshot`" carry provenance
(`profile`, `remote`, `source`, `approved_ref`, `selection`, `freshness`, `revision`,
`latest_approved`, `approved`, `pin`, `overlay`, `engine_version`, `key`, `content_digest`).

| command | `result` members | result-carrying failures; notes |
|---|---|---|
| `init` | `mode` (`dry-run`/`apply`), `plan` (`name`, `namespace`, `remote`, `approved_ref`, `kb_path`, `harnesses`, `example`, `upstream_url`, `upstream_revision`, `changes[]` of `{action, path, reason?}`), `summary` (count per action), `written` | hard: `ALREADY_INITIALIZED`; `USAGE` with `--profile maintainer` or `--config` |
| `doctor` | `profile`, `online`, `summary` (`ok`, `warn`, `fail`, `skip`), `checks[]` of `{id, status, message, hint?, details?}` | `VALIDATION_FAILED` when a check has `status = fail`; `--online` with `--offline` is a hard `INVALID_INPUT` (nothing is fetched). Check ids in order: `git`, `kb-root`, `runtime`, `profile`, `knowledge`, `source`, `freshness`, `host`, `host-repo`, `host-pin`, `snapshot-engine`, `index`, `skill`, `bundle`, `engine`. `snapshot-engine` details: `engine_version`, `auto` (the revision `--snapshot auto` selects, or `null`), `revisions[]` of `{role` (`latest`/`pinned`), `revision`, `status` (`ok`/`incompatible`/`unknown`), `message?`, `mismatches?}` |
| `validate` | `source`, `strict`, `templates`, `base`, `validation` (`profile`, `files`, `records`, `errors`, `warnings`, `diagnostics[]`), `routing` (`passed`, `failed`, `cases[]`, or `null` with `--no-routing`) | `VALIDATION_FAILED` takes precedence over `ROUTING_TESTS_FAILED`; with `--templates` the template diagnostics are included in the totals, and the text output adds a line saying templates were checked |
| `index` | `build` (as `meta.index`), `index` (`docs`, `index_schema`, `parser_version`, `snapshots[]` of `{key, created` (build counter, larger = newer)`, files, proposals, errors, warnings}`), `gc` (`{keep, removed}` with `--gc`, else `null`), `freshness_verified` (always `false`), `note` + `snapshot` | never contacts the remote; the absolute cache path and its size in bytes are in `meta.cache` (`path`, `bytes`), not in `result`; `build.recovery.moved_to` (after a corrupt-cache recovery) is still an absolute path |
| `context` | `protocol`, `engine_version`, `skill_protocol`, `intent`, `request`, `snapshot`, `freshness`, `host`, `scope`, `completeness`, `status_reasons[]`, `undetermined[]`, `effective_settings[]`, `ambiguities[]`, `issues[]`, `diagnostics` (`errors`, `warnings`, `items[]`), `notes[]`, `budget`, `units[]`, `receipt`; `explain` with `--explain` | `CONTEXT_INCOMPLETE` unless `completeness = complete`; hard: `CONTEXT_BUDGET_EXCEEDED`, `UNKNOWN_SCOPE`. Field semantics: [context.md](context.md) |
| `search` | `query`, `kinds`, `limit`, `total`, `hits[]` of `{id, kind, title, status, origin, path, score, matched}`, `context_assembly` (always `false`), `note` + `snapshot` | not a substitute for `context` |
| `show` | `id`, `kind`, `title`, `status`, `origin`, `path`, `note`, `proposals`, `successors`, and one of `record` + `sections` (default), `section` (`--section` or `id#section`), `raw` (`--raw`) + `snapshot` | hard: `NOT_FOUND` (record or section) |
| `sync` | `profile`, `remote`, `source`, `approved_ref`, `freshness`, `latest_approved`, `previous_approved`, `local` (KB checkout: `git`, `head`, `branch`, `ahead`, `behind`, `dirty`, `changes`), `host` (`root`, `pin`, `status`, `ahead`, `behind`, `linked_worktree`, or `null`), `checkout_unchanged` | never modifies the checkout or the host |
| `impact` | `repo`, `diff` (`base`, `head`, `merge_base`, `working_tree`), `files[]`, `affected[]`, `unknown_coverage[]`, `stale_anchors[]`, `summary`, `kb_change`, `kb_pointer`, `kb_submodule`, `notes[]`, `check` (verdict with `--check` or `--statement`, else `null`) + `snapshot` | `IMPACT_UNACKNOWLEDGED` with `--check`; hard: `USAGE` without `--base`/`--working-tree`, `NOT_FOUND` without a host; a malformed statement is a hard `IMPACT_UNACKNOWLEDGED` with `--check`, else `INVALID_INPUT` |
| `integrate` | `level` (`kb` with `--generate`, `host` otherwise), `mode` (`dry-run`/`check`/`apply`), `harnesses`, `kb_path`, `engine_version`, `skill_protocol`, `changes[]`, `summary`, `clean`; host level adds `host`, `lock`, `warnings` | `DRIFT_DETECTED` with `--check`, `CONFLICT` with `--apply`; hard `INVALID_INPUT` when no host is found |
| `migrate` | dry-run: `mode`, `config`, `current`, `target`, `oldest_supported`, `pending`, `migrations[]`, `files[]`; `--apply`: `mode`, `config`, `target`, `written[]`, `up_to_date`, `skipped[]` | hard: `UNSUPPORTED_SCHEMA_VERSION`, `MIGRATION_FAILED` |
| `update check` | `upstream` (`url`, `reference`, `commit`), `head`, `versions`, `schema`, `merge` (`clean`, `conflicts`, `merge_base`, `up_to_date`), `divergence`, `uncommitted`, `notes` | `UPDATE_CONFLICT`, `UNSUPPORTED_SCHEMA_VERSION`; `--offline` is `INVALID_INPUT` |
| `update prepare` | `branch`, `worktree`, `base`, `upstream`, `versions`, `schema`, `steps[]` (`bootstrap`, `migrate`, `integrate`, `validate`), `commits[]`, `changes`, `uncommitted`, `notes`, `next_steps` | hard: `UPDATE_CONFLICT`, `UPDATE_FAILED`, `INVALID_INPUT` (branch not under `kb-update/`, branch or worktree exists); `next_steps` and the hints name `kbw update abandon <branch> --apply` for discarding |
| `update divergence` | `revision`, `reference`, `url`, `engine_paths`, `diverged[]`, `patched[]`, `unused_patches[]` | `ENGINE_DIVERGED` |
| `update abandon` | `mode` (`dry-run`/`apply`), `branch`, `commit`, `worktree`, `dirty` (number of `git status` entries of the worktree, `null` when its directory is missing), `commits_not_in_head` | dry-run unless `--apply`; hard: `NOT_FOUND`, `INVALID_INPUT` (not a `kb-update/*` branch, worktree outside `.cache/update/` or locked), `CONFLICT` with `--apply` when the worktree has uncommitted, untracked or conflicted files (`details` `{branch, worktree, dirty}`; `--apply --force` discards them), `USAGE` for `--force` without `--apply` |
| `schema` | `dir`, `schemas[]`, `in_sync`, `drift[]`; with `--write`: `dir`, `schemas[]`, `changes[]` | `DRIFT_DETECTED` only with `--check` |
| `version` | `engine_version`, `document_schema`, `protocol`, `index_schema`, `skill_protocol`, `parser_version`, `build_fingerprint` | needs no KB root; `build_fingerprint` is `unknown` for builds made outside `kbw` |

Additive command contracts (CLI envelope/error meanings remain `kb.cli.v1`):

| command | result and interpretation | failure boundary |
|---|---|---|
| `outline` | `kb.outline.v1`: completeness/reasons, snapshot content digest, unit inventory, omitted optional count and budget; no delivery receipt | mandatory inventory over the requested/2000-token ceiling is `CONTEXT_BUDGET_EXCEEDED` |
| `coverage` | `coverage`, `history`, optional provider `code` + `snapshot` | unknown host/repo is invalid input; absent provider is explicit churn-only evidence |
| `propose begin` | `kb.work-order.v1`: repo, diff, patch, modules, impact, existing records, templates, review comments, test candidates, consumer candidates and provider metadata + `snapshot` | malformed export or mismatched host identity is rejected; a patch over 8 MiB, past the 30 s Git deadline or with over 1 MiB of Git diagnostics is `INVALID_INPUT` (never an omitted patch); exported merged/approval text is only a claim |
| `propose submit`, `capture` | `mode`, `written`, `draft` plan with id/path/status/diff/anchors/duplicates and optional consumer evidence + `snapshot` | only drafts; invalid evidence, exact duplicate or dirty destination prevents writing |
| `anchors stamp` | `mode`, `source=working-tree`, `written`, stamp `plan` | explicit ids only; no selected-snapshot mutation or automatic review dates |
| `anchors check` | `anchors`, `strict`, `unstamped` + `snapshot` | changed/missing evidence: `DRIFT_DETECTED`; unverifiable/required unstamped evidence: `CONTEXT_INCOMPLETE` |
| `drift` | owner-grouped `drift` + `snapshot` | `--check` reports drift/unverifiable evidence as failure |
| `ledger` | statement support/freshness and seeded draft audit in `ledger` + `snapshot` | `--check` fails stale/unverifiable accepted evidence; drafts are never accepted |
| `verify` | `verification`: input digest, mode/diff/branch, declared probes, concrete evidence, counts, explicit applicability and optional provider provenance + `snapshot` | blocking failure: `VALIDATION_FAILED`; blocking unknown evidence: `CONTEXT_INCOMPLETE` |
| `usage report` | `usage`, final `diff`, selected/missing receipts and selection description + `snapshot` | missing receipts or invalid preserved log lines: `CONTEXT_INCOMPLETE` |
| `eval routing` | normal profile uses validation/routing report; `--example` returns example, validation and routing | validation/routing errors retain 40/41; absent labels are unknown |
| `eval history` | `history` + `snapshot`: per-change delivery/labels/drift and temporal limitations | `--check` fails incomplete context or unavailable labels/temporal evidence |

Context additionally emits optional `code`, `delivery`, `freshness_reference` and
`pruned_change_types`; `request.as_of` records the resolved historical point. Normal CLI
receipts use `kb.receipt.v2` and per-unit `content_sha256`; reused units carry a `delivery`
reference. Verify these fields before omitting previously delivered text. Code units have
`kind=code`, `status=observed`, `origin=provider` and no accepted record identity. See
[context.md](context.md) for reuse, historical completeness and budget semantics.
`show --sections`, `impact --deep` and `integrate --probe` extend their existing commands;
the probe explicitly reports `runtime_load_verified=false` until an external harness run
is observed. `kb.code.v1` and the optional replay kit's `kb.eval.*.v1` are separate protocols,
not changes to the engine envelope.

## Error codes

Codes and exit codes come from `core/cli/src/error.rs` and are stable within a protocol
version. Groups: 1 internal, 2 usage, 10–19 project/config, 20–29 freshness/snapshot, 30–39
context, 40–49 checks, 50–59 runtime/update, 60–69 environment. Exit 12 is unused: the
project/config group has a gap there, and it is not reassigned (invalid records are reported
as `validate` diagnostics and as the context reason `SNAPSHOT_INVALID`, not as an error code).

| code | exit | group | meaning | typical action |
|---|---|---|---|---|
| `INTERNAL` | 1 | internal | engine bug or impossible state | [report a bug](troubleshooting.md#reporting-bugs) |
| `USAGE` | 2 | usage | unknown or invalid arguments, missing required option, conflicting flags | fix the command line; `kbw <command> --help` |
| `PROJECT_NOT_INITIALIZED` | 10 | project/config | `project/project.toml` does not exist | [initialize](troubleshooting.md#project_not_initialized) (maintainer decision) |
| `CONFIG_INVALID` | 11 | project/config | KB root not found, or a config, lock, bundle or remote setting is invalid | fix the file named in the message through review; run from the launcher or pass `--root` |
| `UNSUPPORTED_SCHEMA_VERSION` | 13 | project/config | a file, migration target or upstream uses a document schema this engine cannot read or migrate | `kbw migrate`, or update through an intermediate upstream release |
| `ALREADY_INITIALIZED` | 14 | project/config | `init` on an initialized KB | edit `project/` directly |
| `UNKNOWN_SCOPE` | 15 | project/config | an explicit repo, module, feature or concept id is not in the registry | check `project/registry/*.toml` (ids are case-sensitive) |
| `NOT_FOUND` | 16 | project/config | record, section, host repository or update worktree not found | check the id (`kbw search`); run from the host or pass `--host` |
| `FRESHNESS_UNVERIFIED` | 20 | freshness/snapshot | the approved ref could not be fetched in this call | [restore access or go offline explicitly](troubleshooting.md#freshness_unverified) |
| `UPDATE_REQUIRED` | 21 | freshness/snapshot | the host pin differs from the approved tip, or the selected snapshot needs another engine | [choose a snapshot or update the pin](troubleshooting.md#update_required) |
| `SNAPSHOT_NOT_FOUND` | 22 | freshness/snapshot | the requested revision, pin or approved tip is not known | fetch it, run once online, or select another snapshot |
| `SKILL_OUTDATED` | 23 | freshness/snapshot | `--skill-protocol` differs from the engine's skill protocol | [reload the skill](troubleshooting.md#skill_outdated) |
| `CONTEXT_INCOMPLETE` | 30 | context | completeness is `partial`, `conflict` or `incomplete`; full result printed | [see the reason codes](troubleshooting.md#context_incomplete) |
| `CONTEXT_BUDGET_EXCEEDED` | 31 | context | header plus mandatory units exceed `--budget` | re-run with `--budget` ≥ `details.required` |
| `VALIDATION_FAILED` | 40 | checks | `validate` found errors (or warnings with `--strict`), or a `doctor` check failed | fix the diagnostics and re-run |
| `ROUTING_TESTS_FAILED` | 41 | checks | a routing fixture case failed | fix records or the case in the same change |
| `DRIFT_DETECTED` | 42 | checks | generated files (skill bundle, host integration, JSON Schemas) differ from their source | regenerate and commit |
| `ENGINE_DIVERGED` | 43 | checks | engine-owned paths differ from the recorded upstream base | move changes upstream or declare `engine_patches` |
| `IMPACT_UNACKNOWLEDGED` | 44 | checks | affected knowledge or unknown coverage has no valid `kb-impact` acknowledgement | add or fix the block in the MR description |
| `CONFLICT` | 45 | checks | generated files or managed blocks were edited outside kb (`integrate --apply`), or an update worktree has uncommitted, untracked or conflicted files (`update abandon --apply`); nothing was written or removed | move the edits into the KB, or keep what matters and re-run with `--apply --force` after review |
| `RUNTIME_INCOMPATIBLE` | 50 | runtime/update | the running binary does not match the checkout's `core/release.toml` | run through `kbw`; re-bootstrap |
| `UPDATE_CONFLICT` | 51 | runtime/update | merging the upstream ref conflicts (predicted by `check`, left in the worktree by `prepare`) | resolve in the update worktree, or discard it with `kbw update abandon <branch> --apply --force` (the conflicted files count as uncommitted) |
| `MIGRATION_FAILED` | 52 | runtime/update | a migration transform or verification failed; nothing was written | report the listed files |
| `UPDATE_FAILED` | 53 | runtime/update | a step of `update prepare` failed; the worktree is kept | inspect `details.output`, fix there, or discard with `kbw update abandon <branch> --apply` (add `--force` for uncommitted changes) |
| `GIT_ERROR` | 60 | environment | a Git command failed or the repository state changed mid-command | read the (credential-redacted) message; retry |
| `INDEX_ERROR` | 61 | environment | the SQLite cache could not be opened or used | [retry, then rebuild](troubleshooting.md#index_error-and-index-recovery) |
| `IO_ERROR` | 62 | environment | a file could not be read or written, or changed while being read (retryable) | check the path; re-run |
| `UNSAFE_PATH` | 63 | environment | a path is absolute, escapes its root, contains `..` or is a symlink | use a relative path inside the repository |
| `INVALID_INPUT` | 64 | environment | malformed argument or file (revision, `--path .`, kind, statement block, branch name), or options that do not combine (`doctor --online` with `--offline`) | fix the input named in the message |

### Context status reasons

`CONTEXT_INCOMPLETE` lists why in `error.details.reasons[]` (`code`, `status`, `message`);
the result repeats them in `status_reasons[]` with a `provenance` flag. The overall
completeness is the worst status (`partial` < `conflict` < `incomplete`). Provenance reasons
describe how the snapshot was obtained, not the knowledge; routing fixtures ignore them.

| reason | status | provenance | cause |
|---|---|---|---|
| `FRESHNESS_UNVERIFIED` | partial | yes | `--offline`: the approved ref was not checked in this call |
| `WORKING_TREE` | partial | yes | `--snapshot working-tree` |
| `NOT_APPROVED` | partial | yes | the selected revision is not reachable from the approved tip |
| `APPROVAL_UNKNOWN` | partial | yes | whether the selected revision is approved could not be determined |
| `REPO_UNKNOWN` | partial | no | no `--repo` and the host repository was not identified |
| `DIAGNOSE_SCOPE_PROVISIONAL` | partial | no | path-free diagnosis has no explicit path/module/diff scope |
| `AS_OF_UNDATED` | partial | no | accepted records lack a verifiable introduction boundary for the requested slice |
| `AS_OF_BOUND_UNRESOLVED` | partial | no | accepted records scoped to other repositories have commit bounds that cannot be resolved in the host; they are withheld from the slice |
| `UNDETERMINED_OBLIGATIONS` | partial | no | obligations may apply but a scope dimension is unknown (see `undetermined[]`) |
| `DEPENDENCY_VERSION_UNDETERMINED` | partial | no | a `requires` target has a version constraint for a repo whose version is unknown |
| `SETTING_CONFLICT` | conflict | no | the most specific applicable overrides of one policy setting (none strictly more specific) disagree |
| `INCOMPATIBLE_DEPENDENCY` | conflict | no | a `requires` target is not applicable to the host version |
| `REQUIRES_MISSING` | incomplete | no | a `requires` target does not exist |
| `REQUIRES_NOT_ACCEPTED` | incomplete | no | a `requires` target is `draft` or `superseded` (a `deprecated` target is included with the warning `REQUIRES_DEPRECATED` in `issues[]`) |
| `SNAPSHOT_INVALID` | incomplete | no | the snapshot has validation errors |

Warnings about the request itself (for example `PATH_SCOPE_UNKNOWN`, `HOST_REPO_UNKNOWN`,
`PROPOSAL_INVALID`) and about the knowledge that do not change the status (`REQUIRES_DEPRECATED`,
ignored overrides) are in the context result's `issues[]`.

## Diagnostics in `meta`

Reading commands report what is noteworthy but not part of the deterministic result both on
stderr and in `meta.diagnostics`. Recovery is never silent.

| code | severity | when |
|---|---|---|
| `INDEX_RECOVERED` | warning | the index file was damaged: corrupt or not a database, a database without kb metadata (no `meta` table), unreadable metadata, a required table missing, WAL mode unavailable, or damage found at query time; it was moved aside to `<profile>.sqlite.corrupt-<pid>-<nanos>`, the snapshot was rebuilt and the query retried once. `meta.index.recovery` has `kind = corrupt` and `moved_to` |
| `INDEX_REBUILT` | info | a well-formed index whose metadata (`index_schema`, `layout`, `parser_version`, `engine_version`, `profile`) differs or lacks a key, for example one written by another engine or before a key existed, was rebuilt in place with no `.corrupt-*` file (`kind = outdated`, `reason` like `parser_version 1 -> 2` or `layout missing -> 2`), or `kb index --rebuild` was requested (`kind = requested`) |
| `PROPOSAL_*` and proposal parse diagnostics | warning/error | problems of the proposal overlay (`PROPOSAL_NOT_APPLIED`, `PROPOSAL_REPLACES_RECORD`, `PROPOSAL_UNREADABLE`); with `--include-proposals` also the parse diagnostics of each changed file (for example `FRONT_MATTER_INVALID`) |
| `HOST_BINDING_REPO_UNKNOWN` | warning | `.kbw.toml` names a `repo` the registry does not define; the host repo is identified by its remotes instead |
| `HOST_REPO_UNKNOWN` | warning | `impact`: the host repository is not identified in the registry; only repo-independent path selectors were evaluated |

Example (`kbw --json --offline show <id>` after the index file was overwritten with garbage;
the path is shortened):

```json
"meta": {
  "diagnostics": [
    {
      "code": "INDEX_RECOVERED",
      "message": "the index cache was unusable (unreadable database: file is not a database); the damaged file was moved to <kb>/.cache/index/project.sqlite.corrupt-25375-1790538939813748000",
      "severity": "warning"
    }
  ],
  "elapsed_ms": 117,
  "index": { "recovery": { "kind": "corrupt", "moved_to": "…", "reason": "unreadable database: file is not a database" }, "reused": false, "parsed": 21, "…": "…" }
}
```

Host binding: when the host has a `.kbw.toml` its `selection` (`auto`, `latest`, `pinned`)
is the default of `--snapshot auto`, and its `repo`/`pin` feed host identification; see
[snapshots-and-trust.md](snapshots-and-trust.md).

## Launcher error codes

`kbw` (POSIX sh) prints `kbw: error[<CODE>]: <message>` on stderr and exits before the engine
runs; stdout stays empty and no envelope is printed. Progress lines are `kbw: <text>`.

| code | exit | meaning |
|---|---|---|
| `KBW_USAGE` | 2 | invalid launcher flag or arguments (launcher flags are recognized only as the first argument) |
| `KBW_RUNTIME_NOT_BOOTSTRAPPED` | 50 | no runtime for this checkout's engine fingerprint; normal commands never build implicitly (unless `KBW_AUTO_BOOTSTRAP=1`) |
| `KBW_BOOTSTRAP_FAILED` | 50 | the source build failed, the engine inputs changed during the build, or the built binary failed the smoke test (`kb --json version` must report this `engine_version` and `build_fingerprint`); an earlier runtime is unchanged |
| `KBW_TOOLCHAIN_MISMATCH` | 50 | `rustc` is not the pinned `rust_toolchain` (a warning instead when `KBW_ALLOW_TOOLCHAIN_MISMATCH=1`) |
| `KBW_ARTIFACT_INVALID` | 50 | a release archive failed verification (digest, entries, `BUILD-INFO`, smoke test); the active runtime is unchanged |
| `KBW_PACKAGE_FAILED` | 50 | `--kbw-package` could not build or write the archive |
| `KBW_MANIFEST_INVALID` | 50 | `core/release.toml` is missing or malformed, or the build inputs cannot be hashed |

Exit 50 is shared with the engine's `RUNTIME_INCOMPATIBLE` and exit 2 with `USAGE`; tell them
apart by the empty stdout and the `kbw:` stderr prefix. Interrupts exit 129/130/143 (HUP, INT,
TERM). Environment read by the launcher: `KBW_AUTO_BOOTSTRAP`, `KBW_ALLOW_TOOLCHAIN_MISMATCH`,
`KBW_CARGO_TARGET_DIR` (Cargo target directory of source builds, default
`<root>/.cache/cargo-target`; a relative value is resolved against the directory `kbw` is run
from, not against the root); it exports `KB_ROOT` and `KBW_FINGERPRINT` to the engine and
passes the fingerprint to source builds as `KBW_BUILD_FINGERPRINT`. The engine itself also
honors `KB_CACHE_DIR` (default `<kb root>/.cache`).

## Versioning

| contract | where | identifies |
|---|---|---|
| CLI protocol | `protocol = 1` in `core/release.toml`; `PROTOCOL`/`PROTOCOL_ID` in `core/cli/src/versions.rs`; the envelope's `"kb.cli.v1"` | this envelope, the error codes and exit codes |
| envelope schema | `core/schemas/cli-envelope.v1.schema.json` (`$id` `kb:schema/cli-envelope/v1`, JSON Schema draft 2020-12) | the envelope and the `error` object; `result` is command-specific and not constrained by it |
| skill protocol | `skill_protocol = 2`; checked with `--skill-protocol <n>` | scoped diagnosis, verified reuse and evidence/draft workflow |
| engine | `engine_version` (semver) | the release |

* `kb version` reports all of them plus the build fingerprint (`kb 0.1.0 (document schema 2,
  protocol 1, index schema 2, skill protocol 2) build <fingerprint>`); `kbw --json version`
  adds `parser_version` and `build_fingerprint` (`unknown` for a build made outside `kbw`).
* The maintainer contract `kb.contract.cli-protocol` (in `core/maintainer-knowledge/`)
  requires increasing `protocol` when an envelope field is removed or changes meaning, and
  `skill_protocol` when generated instructions change the calls or their interpretation;
  error codes and their exit codes stay stable within a protocol version. Consumers should
  tolerate additional members.
* A snapshot whose `core/release.toml` has a different `engine_version`, `document_schema`,
  `index_schema` or `protocol` than the running engine fails with `UPDATE_REQUIRED`
  (`details.mismatches`); a runtime that differs from the checkout's own manifest (including
  `skill_protocol`) fails with `RUNTIME_INCOMPATIBLE`.
* The schema files are generated from the Rust model (`kbw schema --write`) and drift-checked
  by `kbw schema --check` and the test suite; see
  [maintainer-guide.md](maintainer-guide.md#regenerating-schemas).
