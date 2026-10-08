# Troubleshooting

Symptom → cause → fix for the errors people and agents actually hit. Every failure prints a
stable code and a hint; read the hint first. In JSON mode the same information is in
`error.code`, `error.details` and `error.hint` (see [protocol.md](protocol.md) for the
envelope and the complete code table). Commands below use `./kbw` from the KB checkout; in a
host repository use the mounted launcher, for example `.kb/kbw`.

Never "fix" an error by silently switching to `--offline`, another `--snapshot` or a smaller
scope: each of those changes what the answer means, so say so when you do it.

| code / symptom | entry |
|---|---|
| `PROJECT_NOT_INITIALIZED` (10) | [project not initialized](#project_not_initialized) |
| `UNKNOWN_SCOPE` (15), `UNSAFE_PATH` (63), `INVALID_INPUT` (64) | [bad input](#bad-input-unknown_scope-unsafe_path-invalid_input) |
| `FRESHNESS_UNVERIFIED` (20) | [freshness](#freshness_unverified) |
| `UPDATE_REQUIRED` (21) | [update required](#update_required) |
| `SNAPSHOT_NOT_FOUND` (22) | [snapshot not found](#snapshot_not_found) |
| `SKILL_OUTDATED` (23) | [skill outdated](#skill_outdated) |
| `CONTEXT_INCOMPLETE` (30) | [context incomplete](#context_incomplete) |
| `CONTEXT_BUDGET_EXCEEDED` (31) | [budget](#context_budget_exceeded) |
| `VALIDATION_FAILED` (40), `ROUTING_TESTS_FAILED` (41) | [validation](#validation_failed) (and [doctor checks](#doctor-checks)), [routing](#routing_tests_failed) |
| `DRIFT_DETECTED` (42), `CONFLICT` (45) | [drift](#drift_detected), [conflict](#conflict) |
| `ENGINE_DIVERGED` (43) | [engine divergence](#engine_diverged) |
| `IMPACT_UNACKNOWLEDGED` (44) | [impact](#impact_unacknowledged) |
| the host commit-msg hook rejects commits or never runs | [commit-msg hook](#commit-msg-hook) |
| `UNSUPPORTED_SCHEMA_VERSION` (13), `MIGRATION_FAILED` (52) | [schema versions](#unsupported_schema_version-and-migration_failed) |
| `RUNTIME_INCOMPATIBLE` (50) | [runtime mismatch](#runtime_incompatible) |
| `UPDATE_CONFLICT` (51), `UPDATE_FAILED` (53) | [upstream update](#update_conflict-and-update_failed) |
| `INDEX_ERROR` (61), `INDEX_RECOVERED`, `IO_ERROR` (62) | [index](#index_error-and-index-recovery) |
| `kbw: error[KBW_...]` (exit 50) | [launcher](#launcher-errors) |
| bundled SQLite fails to compile on macOS | [SDKROOT](#macos-bundled-sqlite-build-cannot-find-system-headers) |
| `kb-eval` replay: `isolation canary failed` on Linux | [replay isolation](#kb-eval-isolation-canary-failed-on-linux) |
| no network | [offline recovery](#offline-recovery) |

## PROJECT_NOT_INITIALIZED

* **Symptom**: `error[PROJECT_NOT_INITIALIZED]: project is not initialized: project/project.toml does not exist` (exit 10) from `context`, `search`, `validate`, …
* **Cause**: the checkout is a clean upstream or a downstream that was never initialized.
  `--profile maintainer` uses `core/maintainer-knowledge/` instead and does not need it.
* **Fix**: initializing is a maintainer decision. Preview, then write:

  ```sh
  ./kbw init --name "<Product>" --namespace <ns>            # dry-run: lists every file
  ./kbw init --name "<Product>" --namespace <ns> --apply
  ./kbw validate
  ```

  Review and commit `project/` (see [downstream.md](downstream.md)). `init` on an
  initialized KB fails with `ALREADY_INITIALIZED` (14): edit `project/` instead. On a clean
  upstream, `./kbw validate --templates` still succeeds and reports the missing project as an
  info diagnostic.

## Bad input (UNKNOWN_SCOPE, UNSAFE_PATH, INVALID_INPUT)

* `UNKNOWN_SCOPE` (15): an explicit `--repo`, `--module`, `--feature` or `--concept` id is not
  in `project/registry/*.toml`; `details` lists them per dimension. Ids are case-sensitive.
* `UNSAFE_PATH` (63): a path is absolute outside the host, contains `..`, escapes its root,
  or goes through a symlink (for example `--path ../../etc/passwd`, `--config ../x.toml`).
  Use repository-relative paths; symlinks are refused by design. An absolute `--path` that
  names the host through a symlinked prefix (macOS `/tmp` or `/var`, a symlinked checkout) is
  accepted and made host-relative: kb resolves the longest existing ancestor directory and
  keeps the last component as given; only paths that then lie outside the host are refused.
* `INVALID_INPUT` (64): a malformed value, such as an unknown `--kind`, a revision with
  spaces, `--path .`, an invalid `kb-impact` block without `--check`, `update prepare
  --branch` outside `kb-update/`, `integrate` run where no host repository can be found
  (run it inside the host or pass `--host <dir>`), or `doctor --online` together with
  `--offline` (nothing is fetched; drop one of them).
* `capture` with `--anchor` or `--test-anchor REPO:PATH@REV` fails with `anchor revision <X>
  is missing` (64): the text after the last `@` is not a commit in that repository's
  checkout. A path that itself contains `@` (`icon@2x.png`, `@types/...`) must exist at
  `HEAD` or end with an explicit `@REV`, for example
  `--anchor mobile:assets/icon@2x.png@HEAD`.
* `context` no longer fails with `INVALID_INPUT` when a task names too many files or the
  host patch exceeds a Git adapter bound (8 MiB, the 30 s Git deadline or 1 MiB of Git
  diagnostics); it reports discovery issues instead (see
  [context incomplete](#context_incomplete)). `propose begin` still fails closed when the
  range's patch exceeds one of those bounds, because the patch is the work order's primary
  input: narrow the range.

## FRESHNESS_UNVERIFIED

* **Symptom**: `error[FRESHNESS_UNVERIFIED]: cannot verify refs/heads/main on remote origin (...)` (exit 20) from `context`, `search`, `show`, `impact` or `sync`. No result is printed.
* **Cause**: every reading call fetches `source.approved_ref` from `source.remote` into an
  isolated mirror, and that fetch failed: no network or VPN, missing credentials, a remote
  URL that is wrong or not configured in the KB checkout (or a KB root that is not a Git
  checkout), a transport not listed in
  `source.allowed_protocols`, or an approved ref that does not exist. kb runs Git with
  `GIT_TERMINAL_PROMPT=0`, so credentials must come from a credential helper or SSH agent.
  There is no TTL and no automatic offline fallback.
* **Fix**:
  1. Read `details` (`remote`, `source` with credentials redacted, `approved_ref`) and check
     access with `git -C <kb> ls-remote <remote> <approved_ref>`.
  2. Check `allowed_protocols` in `project/project.toml` (known values: `https`, `ssh`,
     `git`, `file`, `http`).
  3. If the user accepts unverified knowledge, re-run with `--offline` and report
     `freshness = unverified` (see [offline recovery](#offline-recovery)).
  4. `./kbw doctor --online` checks the same fetch and reports the other checks too.

## UPDATE_REQUIRED

Two different causes share this code; `details` tells them apart.

**The host pin differs from the approved tip** (`details.pin`, `details.latest_approved`):

* **Symptom**: `error[UPDATE_REQUIRED]: the host pins KB revision b76e8729980c but the latest approved revision is 8b74dcf3fcfd` (exit 21), with the default `--snapshot auto`.
* **Cause**: knowledge was merged into the approved ref after the host's KB submodule pointer
  (or `.kbw.toml pin`) was last updated. This is the normal state between a knowledge merge
  and the host pin update. A pin that is ahead of or diverged from the approved tip (KB
  commits that are not merged) gives the same error. `--snapshot auto` fails this way only
  when the host has no `.kbw.toml selection`. `./kbw doctor` shows the relation as the
  `host-pin` check and says what `--snapshot auto` does under the host's `selection`.
* **Fix**, choose explicitly:
  * `--snapshot pinned`: the knowledge the host code was written against (still verifies
    freshness and reports the approved tip). The shipped host CI templates use this.
  * `--snapshot latest`: the approved tip, e.g. when implementing new work.
  * Update the pin through review: `git -C .kb fetch`, `git -C .kb checkout --detach <latest_approved>`,
    then commit the new gitlink in the host. `kbw sync` shows `host.status` (`current`,
    `behind`, `ahead`, `diverged`) without changing anything.
  * A host can set a default in `.kbw.toml` (`selection = "pinned"` or `"latest"`).

**The snapshot needs another engine** (`details.revision`, `details.mismatches[]` of
`{field, snapshot, runtime}`):

* **Cause**: the selected revision's `core/release.toml` has a different `engine_version`,
  `document_schema`, `index_schema` or `protocol` than the running runtime.
* **Fix**: update the KB checkout to that revision through review, then bootstrap its engine
  explicitly (`./kbw --kbw-bootstrap` or `./kbw --kbw-install-artifact ...`), or select
  `--snapshot pinned`. `./kbw doctor` reports this ahead of time as the `snapshot-engine`
  check (`fail` when the revision `--snapshot auto` selects needs another engine, `warn` for
  the other of approved tip and host pin).

## SNAPSHOT_NOT_FOUND

* **Symptom** (exit 22): `revision <rev> does not resolve to a commit in the KB checkout or mirror`, `no approved revision is known (offline, and nothing was fetched before)`, or `the host has no KB pin` / `no host repository was detected, so there is no KB pin`.
* **Fix**: fetch the revision in the KB checkout; run once online (`./kbw sync`); run
  `--snapshot pinned` from inside the host (or pass `--host <dir>`); or choose another
  `--snapshot`.

## SKILL_OUTDATED

* **Symptom**: `error[SKILL_OUTDATED]: the caller's skill targets skill protocol 99, this engine serves 2` (exit 23), `details = {"caller": 99, "engine": 2}`.
* **Cause**: generated skills pass `--skill-protocol <n>`; the KB's engine changed the skill
  protocol since the agent loaded its instructions.
* **Fix**: re-read the installed `SKILL.md` (or start a new session so the harness reloads
  it). If the installed skill itself is old, regenerate it: `./kbw integrate --generate
  --apply` in the KB, then `<kb_path>/kbw integrate --apply` in the host, and commit through
  review.

## CONTEXT_INCOMPLETE

* **Symptom**: `context` prints the full result and exits 30; `error.message` is
  `context is <status>: <reasons>` and `error.details.reasons[]` lists the codes below.
* **Rule**: use what was delivered, but never call it complete; state which reasons apply.

| reason | fix |
|---|---|
| `FRESHNESS_UNVERIFIED` | you ran `--offline`; re-run online when possible (then exit 0 if nothing else is missing) |
| `WORKING_TREE` | `--snapshot working-tree` is for authoring; use an approved snapshot for final answers |
| `NOT_APPROVED` | the selected revision is not reachable from the approved tip; merge it through review or select `latest`/`pinned` |
| `APPROVAL_UNKNOWN` | no approved tip is known to compare with; run once online (`./kbw sync`) |
| `REPO_UNKNOWN` | pass `--repo <id>`, or make the host identifiable: a registry `remotes` entry matching the host's remote URL, or `repo = "<id>"` in the host's `.kbw.toml` |
| `AS_OF_BOUND_UNRESOLVED` | `--as-of` withheld accepted records scoped to other repositories, because their commit bounds cannot be resolved in this host (`--explain` lists the temporal exclusions); replay from the repository those records name, or give cross-repository knowledge date bounds in the KB through review |
| `UNDETERMINED_OBLIGATIONS` | name what you change: `--path <file>` (a directory that maps to no module gives the warning `PATH_SCOPE_UNKNOWN`), `--module`, `--feature`, or `--changed`; filenames and symbols named in `--task` (reported in `scope.inferred_paths`) never establish module scope on their own; `undetermined[]` lists the obligations that may apply |
| `DEPENDENCY_VERSION_UNDETERMINED` | pass `--host-version <repo>=<x.y.z>` for the repo named in the message (only the identified host repo's version is read automatically, from its registry `version_file`) |
| `SETTING_CONFLICT` | the most specific applicable overrides of one setting (none strictly more specific than another) disagree; narrow the scope, and fix the overrides in the KB through review (`validate` reports them as `OVERRIDE_AMBIGUOUS`) |
| `INCOMPATIBLE_DEPENDENCY` | a required record does not apply to the host version; check `--host-version`, then fix `applicability` or `requires` in the KB |
| `REQUIRES_MISSING`, `REQUIRES_NOT_ACCEPTED` | a required record is missing, `draft` or `superseded`: the knowledge base is defective; report it; maintainers fix it and `validate` catches it. A `deprecated` required record is delivered with label `deprecated` and the warning `REQUIRES_DEPRECATED` in `issues` and does not lower completeness |
| `SNAPSHOT_INVALID` | the snapshot has validation errors; run `./kbw validate` (add `--snapshot <rev>` to check that revision) and fix them through review |

Discovery from `--task` and from the host diff reports these entries in `issues`; they do
not change the status by themselves and replace the former `INVALID_INPUT` errors for a
task that names too many files and for a host patch over a Git adapter bound:

| issue | meaning | fix |
|---|---|---|
| `IDENTIFIER_AMBIGUOUS` (info) | a filename or symbol named in the task matches more than 8 files and supplies no candidate paths | name the file with `--path` |
| `INFERRED_PATHS_TRUNCATED` (warning) | the task named more than 64 files (the message gives the count); only the first 64 by path are listed and ranked, and all of them still add their modules and features to the scope | name the files with `--path` |
| `CHANGED_TEXT_SKIPPED` (info) | the host patch exceeded a Git adapter bound (8 MiB, the 30 s Git deadline or 1 MiB of Git diagnostics; the message names it); changed-text change-type hints were skipped | pass `--change-type` for explicit categories |
| `TRACKED_NAMES_SKIPPED` (info) | tracked filenames that are not UTF-8 or not safe relative paths were not used for discovery | name what you change with `--path` or `--module` |

## CONTEXT_BUDGET_EXCEEDED

* **Symptom**: `error[CONTEXT_BUDGET_EXCEEDED]: the header and mandatory knowledge need 3584 tokens-est but the budget is 100` (exit 31), `result = null`.
* **Cause**: mandatory units are never truncated; the header plus the mandatory tier does not
  fit `--budget` (default from `[context] default_budget`).
* **Fix**: re-run with `--budget` at least `details.required` in `details.unit`, or narrow
  the task scope. Do not proceed with partial obligations.

## VALIDATION_FAILED

* **Symptom**: `validate` prints its report and exits 40 (`validation failed: N error(s), M warning(s)`); or `doctor` exits 40 with `doctor: N check(s) failed: <ids>`.
* **Cause**: record, link, registry, override or routing-file errors (warnings too with
  `--strict`); with `--base <rev>`, removed ids (`ID_REMOVED`) or reused ids
  (`ID_KIND_CHANGED`). For `doctor`, a check has `status = fail` (for example `profile` on an
  uninitialized KB, `source` when the remote is not configured, or `snapshot-engine` when the
  revision `--snapshot auto` selects needs another engine).
* **Fix**: read `result.validation.diagnostics[]` (`code`, `path`, `record`, `message`),
  fix the files and re-run. `validate` checks the KB working tree by default; `--snapshot
  <sel>` checks a snapshot instead. For `doctor`, read `details` of the failing check.
  Validation errors take precedence over routing failures in the exit code.

### Doctor checks

`./kbw doctor` runs these checks in this order and prints `ok`, `warn`, `fail` or `skip` for
each; any `fail` makes it exit 40. It only reads the checkout and the host (plus kb's own
caches) and contacts the remote only with `--online`; `--online` together with `--offline`
is `INVALID_INPUT` (64) and fetches nothing.

| check | reports | when it is not `ok` |
|---|---|---|
| `git` | Git version against `min_git` | install a newer Git |
| `kb-root` | KB root and `core/release.toml` versions | — |
| `runtime` | the runtime `kbw` selected (`BUILD-INFO`) | run through `./kbw`, not a copied binary |
| `profile` | the profile config (`fail` before `init`) | [initialize](#project_not_initialized) or fix `project/project.toml` |
| `knowledge` | working-tree validation counts | `./kbw validate` |
| `source` | the remote and the approved tip: `approved tip <rev>` when this run fetched it (`--online`), else `last known approved tip <rev>`; local `HEAD` ahead/behind | add the remote; `./kbw sync` when the tip was never fetched |
| `freshness` | only with `--online`: the fetch of the approved ref | [FRESHNESS_UNVERIFIED](#freshness_unverified) |
| `host`, `host-repo` | host detection and its registry repo | run from the host or pass `--host`; add `remotes` or `.kbw.toml repo` |
| `host-pin` | the pin against the (last known) approved tip and what `--snapshot auto` then does: with `.kbw.toml selection = "pinned"` it reads the pin, with `"latest"` the approved tip, without a selection (or with `"auto"`) it returns `UPDATE_REQUIRED` (pin behind, ahead or diverged) | update the pin through review, pass `--snapshot pinned` (or `latest`, suggested only when this engine can read the approved tip), or set `selection` |
| `snapshot-engine` | `core/release.toml` of the approved tip and of the host pin, read from the mirror, against this engine | `fail`: the revision `--snapshot auto` selects needs another engine, so every reading command returns `UPDATE_REQUIRED` ([fix](#update_required)); `warn`: only the other revision does; `skip`: no tip or pin is known yet (`./kbw sync`) |
| `index` | cache statistics and `PRAGMA quick_check` | `warn` after a rebuild or recovery; `fail`: `./kbw index --rebuild` |
| `skill`, `bundle` | installed skill protocol in the host; generated bundle drift | [DRIFT_DETECTED](#drift_detected) |
| `engine` | engine-owned paths against the recorded upstream base | [ENGINE_DIVERGED](#engine_diverged) |

## ROUTING_TESTS_FAILED

* **Symptom**: `validate` exits 41 with `N routing case(s) failed`; `details.failed[]` names `<file>#<case>`.
* **Cause**: a golden case in `project/routing-tests/*.toml` no longer holds (an expected
  mandatory id is missing, a forbidden id appears, or the status differs). Case statuses
  ignore snapshot provenance (freshness, working tree).
* **Fix**: compare `result.routing.cases[].failures` with `mandatory` and `included`; fix
  the records (scope, links) or the case in the same reviewed change. `--no-routing` skips
  the fixtures for a quick check only.

## DRIFT_DETECTED

| where | meaning | fix |
|---|---|---|
| `./kbw schema --check` | `core/schemas/*.schema.json` differ from the Rust model | maintainers: `./kbw schema --write`, review, commit |
| `./kbw integrate --generate --check` (KB) | `project/skill-config/generated/` differs from `core/skills` + `project/skill-config/skill.toml` | `./kbw integrate --generate --apply` in the KB, commit through review |
| `<kb_path>/kbw integrate --check` (host) | installed skill files or managed blocks differ from the committed bundle | `<kb_path>/kbw integrate --apply` in the host, commit |

`doctor` reports the same states as warnings (`bundle`, `skill`).

## CONFLICT

* **Symptom**: `integrate --apply` exits 45: `1 generated file(s) or block(s) were modified outside kb; nothing was written`, `details.conflicts[]` = `{path, block, reason}`.
* **Cause**: someone edited a generated file (for example `.claude/skills/kb/SKILL.md`) or
  the text between `<!-- kb:begin <name> -->` and `<!-- kb:end <name> -->` after kb installed
  it (tracked in `.kbw/integration.lock`). Text outside managed blocks is never touched.
* **Fix**: move the local edits into the KB (`notes` in `project/skill-config/skill.toml`),
  regenerate, then `integrate --apply`. Only when the edits may be discarded, `integrate
  --apply --force`. Agents must not pass `--force` on their own.

`propose submit` exits 45 when the submission would duplicate or overwrite knowledge;
nothing is written:

* **The id already exists elsewhere**: at several local paths, or at one local path while
  the selected snapshot has it at a different path. Submission never creates a second
  copy; resolve the duplicate through review first.
* **`<path> is committed with text that differs from the approved record in the selected snapshot`**:
  usually a draft that revises an already approved record on an unmerged KB proposal
  branch. Submit from that branch with `--snapshot working-tree --offline`, so the branch's
  own records are the base (see [knowledge-lifecycle.md](knowledge-lifecycle.md#find-the-gaps-and-prepare-a-draft));
  otherwise update the KB checkout to the selected snapshot.
* **`the KB checkout does not contain the approved record <path> from the selected snapshot`**:
  the checkout is clean but behind the snapshot, for example a proposal branch created
  before that record was accepted. Update the KB checkout to the selected snapshot, then
  submit again.
* **`<path> has uncommitted changes`**: edit and validate that draft file directly.
* **`<path> has local changes; submission will not overwrite them`**: the destination
  differs from HEAD in a way Git has not recorded, for example the record file was deleted
  from the working tree while HEAD still has it, or the KB checkout has no commit yet. It
  also appears when the default draft path already holds a file that is not this record.
  Restore or commit that change (or move the file) first.
* **`identical knowledge already exists under another id`**: revise the existing record
  (`details.duplicates`).
* **`<path> changed after validation`** (on `--apply`): run the submission again.

## ENGINE_DIVERGED

* **Symptom**: `update divergence` exits 43: `N engine-owned path(s) diverge from upstream <rev>`, `details.diverged[]` = `{path, status}`; `doctor` shows `engine` as a warning.
* **Cause**: a path listed in `core/release.toml` `engine_paths` differs (committed or not)
  from the upstream base recorded in `project/upstream.toml` `revision`.
* **Fix**: revert the change and make it upstream, then take it with `kb update prepare`; or,
  for an intentional downstream patch, declare it in `project/upstream.toml`:

  ```toml
  [[engine_patches]]
  path = "core/cli/src/some_file.rs"
  reason = "why this downstream must differ from upstream"
  ```

## IMPACT_UNACKNOWLEDGED

* **Symptom**: `impact --check` exits 44: `8 affected record(s) and 1 file(s) with unknown coverage are not acknowledged: ...`; the result still lists `affected[]` and `unknown_coverage[]`.
* **Cause**: the diff touches knowledge-linked or unknown-coverage files and neither updates
  the KB pointer nor carries a valid acknowledgement. Unknown coverage means no record is
  linked to a file; it does not mean documentation is unnecessary.
* **Fix**: add exactly one block to the merge request description (templates in
  `core/templates/mr/`) and pass the description file with `--statement <file>`:

  ```text
  <!-- kb-impact:v1
  kb_change = "none"            # none | linked | included
  reason = "Pure refactoring; no behavior or contract changes."
  # kb_revision = "<KB commit>" # linked: kb_revision and/or change_id
  # change_id = "<shared id>"
  -->
  ```

  `reason` must be non-empty; `linked` needs `kb_revision` (7–64 hex characters) and/or
  `change_id`; unknown keys, empty optional values and a second block are rejected;
  `none` fails when the diff updates the KB pointer. A malformed block is a hard error:
  `IMPACT_UNACKNOWLEDGED` with `--check`, `INVALID_INPUT` without. kb checks presence and
  shape only; reviewers judge the reasoning. On GitLab, editing the description does not
  start a pipeline; re-run the merge request pipeline after fixing the block.

## commit-msg hook

The optional host hook `core/templates/ci/hooks/commit-msg` (see
[downstream.md](downstream.md#6-ci-and-merge-request-templates)) runs `verify --only
commit-message --only branch-name` on the index Git is committing and the pending message,
and exits with verify's code.

* **Which KB revision it reads**: `KB_SNAPSHOT`, when set, is passed as-is. Otherwise, if
  `.kbw.toml` at the host root declares a `selection` key, the hook passes `auto` (the
  engine honors that selection). Otherwise, if the host pins the KB (`HEAD` has a gitlink,
  mode `160000`, at the KB path, or `.kbw.toml` declares `pin`), the hook passes `pinned`.
  Otherwise it passes `auto` (the approved tip). `KB_OFFLINE=1` adds `--offline`.
* **`FRESHNESS_UNVERIFIED` (20) while offline**: set `KB_OFFLINE=1`; host CI still verifies
  freshness.
* **`UPDATE_REQUIRED` (21), `SNAPSHOT_NOT_FOUND` (22)**: the same causes as for any reading
  command ([update required](#update_required), [snapshot not found](#snapshot_not_found)).
  Without `KB_SNAPSHOT` or a `.kbw.toml` `selection`, the rule above never asks for a pin
  the host lacks, nor for `auto` while a pin exists.
* **A message rejected although CI accepts it**: Git runs the hook before it cleans the
  message up, so the hook cleans it as Git will (scissors cut, comments stripped when Git
  strips them). When `GIT_EDITOR` is exactly `:` the hook cannot tell whether Git will strip
  comments, so it checks both the whitespace-cleaned and the comment-stripped message and
  rejects only if both fail (CI on the committed message stays authoritative). A message
  of only comments is checked whitespace-cleaned, because Git aborts an empty message, so
  it is rejected when the comment lines fail the pattern. The `git commit --cleanup=<mode>`
  and `--allow-empty-message` flags are invisible to hooks. To skip the editor, use
  `GIT_EDITOR=true`, not `:`.
* **The hook never runs**: it usually lacks the executable bit. The template is mode
  `100755`; keep that mode when installing it (`chmod +x`), in the directory Git reads hooks
  from (`git config core.hooksPath`, default `.git/hooks`).

## UNSUPPORTED_SCHEMA_VERSION and MIGRATION_FAILED

* `UNSUPPORTED_SCHEMA_VERSION` (13): a config, registry or record uses a document schema this
  engine does not serve, `migrate --to` names an unsupported target, or `update check`
  finds project schemas the upstream cannot migrate. Run `./kbw migrate` (dry-run with a
  diff) and `./kbw migrate --apply`; for upstream updates, go through an intermediate
  release whose `migrates_from` covers the project schema. See
  [core/migrations/README.md](../core/migrations/README.md).
* `MIGRATION_FAILED` (52): a transform or the verification of its output failed; nothing was
  written. The message lists every failing file; fix them or report the migration bug.

## RUNTIME_INCOMPATIBLE

* **Symptom** (exit 50, with a JSON envelope): `the running kb binary does not match the local engine manifest`, `details.mismatches[]` = `{field, manifest, runtime}`.
* **Cause**: a `kb` binary other than the runtime built for this checkout was used (a stale
  copy, another checkout's build, a `kb` on `PATH`).
* **Fix**: always run through the checkout's launcher (`./kbw` or `<kb_path>/kbw`); if the
  runtime is missing it says so ([KBW_RUNTIME_NOT_BOOTSTRAPPED](#kbw_runtime_not_bootstrapped)).

## UPDATE_CONFLICT and UPDATE_FAILED

* `update check` exits 51 when merging the upstream ref would conflict
  (`details.conflicts`); nothing is written.
* `update prepare` exits 51 when the merge in its worktree conflicted, or 53 when a step
  (`bootstrap`, `migrate`, `integrate`, `validate`, or the merge itself) failed. `details`
  carries `branch`, `worktree` (under `.cache/update/`), the failing `step`, `command`,
  `exit_code` and the tail of the `output` (for conflicts: `conflicts` and `next_steps`).
  The worktree is kept for inspection and the main checkout is not modified.
* **Fix**: resolve in the worktree (kb never resolves semantic conflicts), commit, run
  `./kbw migrate --apply`, `./kbw integrate --generate --apply` and `./kbw validate` there,
  and set `revision`/`ref` in `project/upstream.toml`; or discard the attempt:
  `./kbw update abandon <branch>` is a dry-run that shows the commit, the worktree, how many
  files in it are uncommitted (`dirty`) and how many commits are not in `HEAD`;
  `./kbw update abandon <branch> --apply` removes the worktree and deletes the branch. A
  worktree with uncommitted, untracked or conflicted files (for example right after an
  `UPDATE_CONFLICT`) is refused with `CONFLICT` (45) unless you add `--force`
  (`--apply --force`). Only `kb-update/*` branches with a worktree under `.cache/update/` are
  removed; the printed commit lets you recreate the branch (`git branch <name> <commit>`).

## INDEX_ERROR and index recovery

The SQLite index under `.cache/index/<profile>.sqlite` is derived data; Git-tracked text is
the source of truth.

* **Automatic rebuild**: an index whose metadata differs or lacks a key (written by another
  engine, index schema, table layout or parser version, or before a metadata key existed) is
  rebuilt in place and reported as info `INDEX_REBUILT`; no file is moved aside.
* **Automatic recovery**: a damaged file (corrupt or not a database, a database without kb
  metadata, unreadable metadata, a missing table) or damage found at query time is moved
  aside to `<profile>.sqlite.corrupt-<pid>-<nanos>`, the snapshot is rebuilt and the query is
  retried once. It is reported as `kb: warning[INDEX_RECOVERED]` on stderr and in
  `meta.diagnostics`. The moved-aside files may be deleted after inspection.
* **`INDEX_ERROR` (61)**: recovery did not help, or the database is locked longer than the
  30 s busy timeout, or the cache directory is not writable. Retry once; then
  `./kbw index --rebuild`; check permissions of `.cache/`. Deleting `.cache/index/` is safe.
* **`IO_ERROR` (62) "`<path>` changed while it was being read"**: a working-tree file changed
  during the call; nothing was stored. Run the command again.

## Launcher errors

The launcher prints `kbw: error[<CODE>]: <message>` on stderr, exits 50 (2 for `KBW_USAGE`)
and prints nothing on stdout, even with `--json`. `./kbw --kbw-runtime-info` shows the
fingerprint, runtime path, status (`active`/`missing`), source (`source-build`/`artifact`),
engine version and target.

### KBW_RUNTIME_NOT_BOOTSTRAPPED

* **Symptom**: `kbw: error[KBW_RUNTIME_NOT_BOOTSTRAPPED]: no runtime for engine 0.1.0 (fingerprint <fp>) in this checkout; bootstrap it explicitly ...`
* **Cause**: no `.cache/runtime/<fingerprint>/kb` exists for this checkout's engine inputs:
  first use, an edit of an engine build input, or a KB checkout moved to another engine
  revision (for example a submodule update). Normal commands never build or install an
  engine implicitly.
* **Fix** (an explicit decision, not something an agent does unasked):
  * `./kbw --kbw-bootstrap`: builds from source with the pinned toolchain
    (`cargo build --release --locked -p kb`), smoke-tests and activates it;
  * or install a verified release archive: `./kbw --kbw-install-artifact <archive-path|https-url> --sha256 <hex>`
    (or `--sha256-file <SHA256SUMS>`);
  * CI may set `KBW_AUTO_BOOTSTRAP=1` to allow implicit builds.

### KBW_TOOLCHAIN_MISMATCH

* **Symptom**: `rustc <x> is not the pinned toolchain 1.98.1 (core/release.toml rust_toolchain) ...`
* **Cause**: the `rustc` selected in the KB root is not `rust_toolchain` from
  `core/release.toml` (which equals `rust-toolchain.toml`).
* **Fix**: install the pinned toolchain (rustup reads `rust-toolchain.toml` automatically), or
  install a release archive instead. `KBW_ALLOW_TOOLCHAIN_MISMATCH=1` builds anyway and prints
  `kbw: warning[KBW_TOOLCHAIN_MISMATCH]`; such a runtime is not the reproducible build.

### KBW_ARTIFACT_INVALID

* **Symptom**: `kbw: error[KBW_ARTIFACT_INVALID]: ...; the active runtime is unchanged`.
* **Causes** (from the message): `sha256 mismatch ... (nothing was extracted)`; a checksum file
  without exactly one entry for the archive name; an unsafe entry (absolute, `..`, link,
  device) or entries outside `kb-<version>-<target>/`; a missing `kb` or `BUILD-INFO`;
  `BUILD-INFO engine_version/target/fingerprint is '...' but this checkout needs '...'` (the
  archive was built from other engine inputs or for another platform); a failed smoke test;
  an `http://` or other non-https URL; `curl` missing for downloads.
* **Fix**: get the archive and digest for exactly this engine version and target from the
  same release; never bypass digest verification. If no matching archive exists (for
  example a downstream with engine patches, whose fingerprint differs), bootstrap from
  source.

Other launcher codes: `KBW_BOOTSTRAP_FAILED` (cargo missing or failed, inputs changed during
the build, smoke test failed: the built binary must report this engine version and build
fingerprint through `kb --json version`; an earlier runtime is intact; do not retry in a loop),
`KBW_PACKAGE_FAILED`, `KBW_MANIFEST_INVALID` (`core/release.toml` missing or malformed; is
this a kb checkout?), `KBW_USAGE`.

## macOS: bundled SQLite build cannot find system headers

`rusqlite` is built with the `bundled` feature, which compiles SQLite from source. When the C
compiler cannot find the macOS SDK headers, export the SDK path before running Cargo
yourself:

```sh
export SDKROOT="$(xcrun --show-sdk-path)"
cargo test --workspace --locked
```

`./kbw --kbw-bootstrap` (and `--kbw-package`) sets `SDKROOT` this way automatically on macOS
when it is unset and `xcrun` is available.

## kb-eval: isolation canary failed on Linux

* **Symptom**: a `kb-eval` stage, or the upstream CI replay isolation smoke, fails with
  `isolation canary failed; no agent was started`, and the probe stderr it quotes contains
  `bwrap: setting up uid map: Permission denied` or `loopback: Failed RTM_NEWADDR`.
* **Cause**: Ubuntu's AppArmor restriction on unprivileged user namespaces
  (`kernel.apparmor_restrict_unprivileged_userns = 1`, for example on Ubuntu 24.04) stops
  bubblewrap from setting up its namespaces. The replay kit fails closed; it never runs a
  stage unsandboxed.
* **Fix** (an administrator action): grant `userns` to bubblewrap alone with a binary-scoped
  AppArmor profile, as the upstream CI does, then check that the probe succeeds:

  ```sh
  printf '%s\n' 'abi <abi/4.0>,' 'include <tunables/global>' \
    'profile kb-eval-bwrap /usr/bin/bwrap flags=(unconfined) {' '  userns,' '}' \
    | sudo tee /etc/apparmor.d/kb-eval-bwrap >/dev/null
  sudo apparmor_parser -r /etc/apparmor.d/kb-eval-bwrap
  bwrap --unshare-all --die-with-parent --ro-bind / / /bin/true
  ```

  Never lower the host-wide sysctl to make a run pass. See
  [core/eval/README.md](../core/eval/README.md#isolation-and-execution).

## Offline recovery

What works without network access, and what it means:

| need | command | result |
|---|---|---|
| context from the last verified state | `--offline` (any reading command) | uses the approved revision last fetched into the mirror (`.cache/git/<hash>.git`, ref `refs/kb/approved`), else the KB checkout's remote-tracking ref; `freshness = unverified`, so `context` is at best `partial` (exit 30, full result printed) |
| the knowledge the host pins | `--offline --snapshot pinned` | the host's gitlink or `.kbw.toml pin`; still `freshness = unverified` |
| authoring before a push | `--offline --snapshot working-tree` | the KB working tree; `WORKING_TREE` and `FRESHNESS_UNVERIFIED` reasons |
| validation | `./kbw validate` | reads the working tree; never fetches |
| index maintenance | `./kbw index` | never contacts the remote |
| diagnosis | `./kbw doctor` | fetches only with `--online` |
| runtime | `./kbw --kbw-install-artifact <local-archive> --sha256 <hex>` | works offline; a source bootstrap needs the crates in the Cargo cache (build once online, or set `CARGO_NET_OFFLINE=true` with a warm cache) |

* Prime the mirror while online with `./kbw sync` (or any reading command). When neither the
  mirror nor the KB checkout's remote-tracking ref knows the approved tip, `--offline` with
  `latest` fails with `SNAPSHOT_NOT_FOUND`.
* Report unverified freshness to the user; it is never upgraded to `complete`.
* The mirror is keyed by the source identity and the approved ref: after changing the remote
  URL the next online call creates a new mirror.
* Operational detail: [snapshots-and-trust.md](snapshots-and-trust.md#offline-work-and-recovery)
  and [downstream.md](downstream.md#10-offline-and-failure-recovery).

Cache contents (under `$KB_CACHE_DIR`, default `<kb>/.cache/`; the launcher's `runtime/` and
`cargo-target/` are always under `<kb>/.cache/`; all git-ignored, never part of review):

| path | content | removal |
|---|---|---|
| `runtime/<fingerprint>/` (`kb`, `BUILD-INFO`), `runtime/stamp` | active runtimes; warm-path stamp | requires a new bootstrap or install; the stamp only costs one re-hash |
| `git/<16 hex>.git` | isolated mirrors of the approved ref | the offline fallback is lost until the next online call |
| `index/<profile>.sqlite` (+ `-wal`, `-shm`, `.lock`, `.corrupt-*`) | derived index | rebuilt on the next read |
| `wt-stat-<profile>.json` | working-tree stat cache | harmless |
| `update/<branch with / replaced>/` (e.g. `update/kb-update-v0.1.1/`) | `update prepare` worktrees | use `./kbw update abandon <branch> --apply` instead (a dry-run without `--apply`) |
| `cargo-target/` | default Cargo target dir of source bootstraps (override with `KBW_CARGO_TARGET_DIR`; a relative value is resolved against the directory `kbw` is run from) | the next bootstrap rebuilds all dependencies |

## Reporting bugs

Security issues: follow [SECURITY.md](../SECURITY.md) (private reporting, not a public
issue). For other bugs, open an issue in the tracker of the upstream repository your KB was
forked from and include:

1. `./kbw --kbw-runtime-info` and `./kbw --json version` (engine, contract versions, build
   fingerprint, target);
2. `git --version`, the OS and, for build problems, `rustc --version`;
3. the exact command and its complete output: the stdout envelope from `--json` plus stderr
   (the `kb:` progress and diagnostic lines matter);
4. `./kbw --json doctor` from the same directory;
5. for context or routing problems, a minimal synthetic reproduction: the synthetic example
   (`./kbw init --example synthetic-multirepo --apply` in a scratch checkout) or
   `kb-bench gen` output is ideal.

Before sharing, remove product knowledge and internal URLs. kb redacts credentials in Git
error text, but record content, paths and remote names from a downstream are your data.
`INTERNAL` (exit 1) is always a bug.
