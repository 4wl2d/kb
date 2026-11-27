# Maintainer guide

For people and agents changing **the kb engine** in the upstream repository. Start with
[AGENTS.md](../AGENTS.md) (rules), [CONTRIBUTING.md](../CONTRIBUTING.md) (workflow),
[architecture.md](architecture.md) (the normative contract) and the [ADRs](adr/). Downstream
operation is covered in [downstream.md](downstream.md).

## Development setup

| requirement | source of truth | notes |
|---|---|---|
| Rust `1.98.1` with `rustfmt`, `clippy` | `rust-toolchain.toml`, `core/release.toml` `rust_toolchain` | rustup selects it automatically; without rustup, the `rustc` on `PATH` must report exactly this version or `kb engine bootstrap` fails with `TOOLCHAIN_MISMATCH` |
| Git ≥ `2.38.0` | `core/release.toml` `min_git` | `kb update check` predicts conflicts with `git merge-tree --write-tree`; `kb doctor` checks the version |
| `shellcheck` | CI | lints `install.sh`, the deprecated `kbw` forwarder and the shipped CI and hook shell templates |
| macOS SDK headers | — | the bundled SQLite is compiled from source; if headers are not found, `export SDKROOT="$(xcrun --show-sdk-path)"` (`kb engine bootstrap` and `package` do this for their builds) |

```sh
cargo build --workspace --locked      # the first build downloads crates; tests never use the network
cargo test --workspace --locked
cargo run -q -p kb -- engine info   # a fresh build serves this working tree in-process
```

Every build compiles in the engine fingerprint of the sources it was built from
(`core/cli/build.rs`; `kb version`: `build <fp>`), and engine selection runs a binary for a
checkout only when that fingerprint matches. Ways to run the engine while developing:

* `cargo run -q -p kb -- <command>` or `target/debug/kb <command>` after a build: the binary
  matches the working tree, so it serves this checkout in-process.
* An installed `kb` (another version, or built before your edits) needs a runtime for the
  current fingerprint. Any edit of a build input (`Cargo.toml`, `Cargo.lock`,
  `rust-toolchain.toml`, `core/release.toml`, `core/cli/Cargo.toml`, `core/cli/build.rs`,
  `core/cli/src/**`) changes it, so the next call fails with `RUNTIME_NOT_BOOTSTRAPPED` until
  you run `kb engine bootstrap` (or set `KB_AUTO_BOOTSTRAP=1`); each bootstrap adds a runtime
  to the store (`kb engine info` shows where; delete old ones freely). `KB_CARGO_TARGET_DIR`
  points source bootstraps at a shared target directory (default `<root>/.cache/cargo-target`;
  a relative value is resolved against the current directory); pointing it at the workspace
  `target/` lets bootstraps reuse your builds.

Code layout rules (from AGENTS.md): pure logic (`model`, `parse`, `validate`, `scope`,
`normalize`, `glob`, `context`) has no Git or filesystem side effects; adapters live in
`git`, `host`, `snapshot`, `overlay`, `source`, `index`, `integrate`, `update`. No Python
anywhere; shell only for `kb` and CI glue.

## Test suites

`cargo test --workspace --locked` runs unit tests inside `core/cli/src` (`#[cfg(test)]`
modules) and the integration suites in `core/cli/tests/`:

| suite | covers |
|---|---|
| `foundation.rs` | sanity of the shared fixtures and foundation loading |
| `validate_rules.rs` | cross-record validation rules (architecture §3–§4), one test per rule family, on temporary corpora, including ambiguous overrides with incomparable or equivalent scopes and front-matter errors that point at file lines |
| `schema_conformance.rs` | generated JSON Schemas vs. the strict parser (including property tests with `proptest`), drift against `core/schemas/`, real CLI output and shipped configuration against the schemas |
| `context_golden.rs` | context assembly, routing, search and show over `core/tests/fixtures/context` (directory paths, version checks of required records, deprecated dependencies, most specific overrides, budgets, receipts); golden text in `core/tests/fixtures/context/golden/` |
| `index_store.rs` | SQLite index: incremental reuse, snapshot isolation, proposals, concurrency, interrupted builds, corruption recovery (at open, build and query time) versus in-place rebuilds of outdated indexes (including a missing metadata key), content-id verification, duplicate ids, FTS safety, garbage collection |
| `snapshot_git.rs` | freshness, snapshot selection, frozen working-tree snapshots, engine compatibility of snapshots, proposal overlays, host detection and `kb sync` against real local Git repositories |
| `impact_analysis.rs` | impact analysis against temporary Git repositories and corpora |
| `init_integrate.rs` | `kb init` and `kb integrate` through the executable, plus consistency of shipped templates, skills and the synthetic example |
| `migrate_update.rs` | `kb migrate` on the synthetic legacy fixtures and `kb update` scenarios (check, prepare, divergence, abandon) with synthetic upstream/downstream repositories; the engine steps run through an injected test runner, or through the production runner up to a fake `cargo`, never a real build |
| `cli_commands.rs` | `context`, `search`, `show`, `index`, `validate`, `impact`, `doctor` end to end: KB pushed to a local bare origin, host mounting it as a submodule; also usage and protocol errors in JSON mode, the snapshot line of text output, absolute `--path` values through a symlinked host directory, the `doctor` checks `source`, `host-pin` and `snapshot-engine`, and that reading commands leave the checkouts untouched |
| `engine_selection.rs` | the `kb` command: in-process service of a matching checkout found through a `.kb` mount, `RUNTIME_NOT_BOOTSTRAPPED` with JSON and text output, refusal of a runtime that reports another build, `engine install` verification (digest, `BUILD-INFO`, names, tampered `SHA256SUMS`) and activation, `self update` (check, tampered digest, forced reinstall by rename), and on Unix runtimes run in place (store and kbw-era locations), source bootstrap with a fake toolchain (pinned toolchain, failures and concurrent edits leave runtimes intact, `KB_AUTO_BOOTSTRAP`), the `kbw` forwarder (flag mapping, loop, missing kb) and `install.sh` |
| `e2e_workflow.rs` | the specification's end-to-end scenario (unix only): synthetic upstream → downstream init → multi-repository context → proposal → approved-origin update → pin refresh → upstream-format update, through `kb` |
| `update_production.rs` | production runner in an update worktree whose engine differs from the running kb (a real source bootstrap): synthetic 0 → 1 → 2 migration to `v2-expected`, validation and unchanged main checkout |
| `schema_evolution.rs`, `retrieval_upgrade.rs` | schema-2 strictness, compatibility, tracked identifiers, change categories and historical slices |
| `authoring_workflow.rs`, `knowledge_ci.rs` | validated drafts/capture and offline merged-change/CI glue |
| `code_provider.rs`, `process_limits.rs` | immutable provider evidence, deleted symbols, uncertainty, bounded output and deadlines; a non-UTF-8 code unit is omitted with a limitation, a pinned response's `similar` candidates are not replayed into context, and validation reads files with a constant number of Git processes (counted through a `PATH` Git shim) |
| `delivery_efficiency.rs` | verified core/delta (units a receipt held only as `core` references are never reused through `--since-receipt`; they are delivered in full unless current core proof covers them), one full skill, terse/outline and local usage |
| `trust_workflow.rs`, `verify_workflow.rs` | stamps/drift/ledger/freshness and probes over real staged/committed inputs |
| `evaluation.rs` | labeled routing metrics, chronological history, missingness and determinism |
| `agent_cli.rs` | the agent commands (`sym`, `callers`, `impact <symbol>`, `doc`, `find`, `check`, `tools`): routing through the tool registry, fallbacks with notes, `NOT_FOUND` with the answer printed, forced and invalid routes, bounded and sorted answers, checks run without a shell (`--plan` runs nothing, `CHECKS_FAILED`), changed-knowledge validation and the knowledge part of `doc` and `impact`; a labeled shell stand-in replaces CodeGraph |

The optional provider crate has synthetic native-output tests and an end-to-end deadline
test (`core/providers/tests/deadline.rs`): it drives the real `kb-code-provider` binary with
a synthetic native tool and checks that the engine's deadline ends that tool and removes the
provider's frozen checkout. The replay crate adds pure accounting/statistics tests and a
shallow-Git isolation test. Run its OS-dependent fixtures explicitly with
`cargo test -p kb-eval --locked --test replay_workflow -- --ignored`; CI runs them last, on
Seatbelt/bubblewrap. The workflow fixture routes the synthetic coder and judge through the
egress proxy (they send it only a `CONNECT` to an unapproved host); on macOS a second
ignored test (`seatbelt_profile_loads_and_admits_only_the_proxy_port`) loads the generated
Seatbelt profile. They call no real model or public network and must not be mistaken for
held-out quality acceptance.

Shared helpers are in `core/cli/tests/common/`. Rules for every test:

* no network: remotes are local bare repositories addressed by path;
* Git runs with an isolated `HOME`, `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1`;
* never assert absolute timings;
* fixtures are synthetic and labeled (`core/tests/fixtures/`, `core/migrations/fixtures/`).

Notes:

* `e2e_workflow.rs` and `update_production.rs` build a real engine with the pinned toolchain
  and `CARGO_NET_OFFLINE=true`, so they need a warm Cargo registry cache (any earlier
  `cargo build --locked` provides it). They share compiled dependencies through one directory
  under Cargo's test tmpdir (`kb-cargo-target`). Each performs one release build of the `kb`
  crate (their synthetic upstreams carry a marker comment, so the kb under test is not their
  engine); the e2e update worktree reuses the downstream's runtime from the store, and
  `update_production.rs` covers a real schema migration by the bootstrapped target engine.
* CI runs `shellcheck install.sh kbw core/templates/ci/*.sh core/templates/ci/hooks/commit-msg`,
  the installers and `kb self update` against locally packaged archives on Linux, macOS and
  Windows.
* Regenerate golden context output only intentionally, then review the diff:
  `KB_UPDATE_GOLDEN=1 cargo test -p kb --test context_golden`.
* One suite: `cargo test -p kb --test cli_commands`.

## Required checks

Run before finishing any change (the same list is in AGENTS.md):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
shellcheck install.sh kbw core/templates/ci/*.sh core/templates/ci/hooks/commit-msg
kb schema --check
kb validate --profile maintainer --templates
```

`kb schema --check` fails with `DRIFT_DETECTED` (42) when `core/schemas/` differs from the
model. `kb validate --profile maintainer --templates` validates the maintainer knowledge
with its routing fixtures and every shipped template and example; on a clean upstream (no
`project/project.toml`) it reports the missing project as info and exits 0.

The same checks are configured as `command` tools in `core/maintainer-knowledge/tools.toml`,
each triggered by the files it covers. `kb --profile maintainer check` runs those whose
globs match the work tree's changes against `HEAD`, and
`kb --profile maintainer check --base main` those matching the changes against `main`;
changed maintainer knowledge is validated too, while declared probes need a host repository
and are skipped here. `--plan` lists what would run without running it. The selection
follows the changed files, so the full list above still applies before finishing.

## Engine and project ownership

* `core/release.toml` `engine_paths` lists what the engine owns: `kb`, the Cargo files,
  `rust-toolchain.toml`, `core`, `docs`, the top-level Markdown files, `LICENSE`,
  `.gitignore` and the two upstream workflows. `project_paths = ["project"]` is owned by each
  downstream. The root `SECURITY.md` covers the engine only; downstreams publish their own
  policy at `.github/SECURITY.md` from `core/templates/security/SECURITY.md.tmpl`, and the
  inherited workflows are switched off with repository variables, not edits (`KB_ENGINE_CI`,
  `KB_RELEASE`).
* The upstream never contains real project data; `project/` holds only a README. Examples
  (`core/templates/examples/`, test fixtures, `core/migrations/fixtures/`) are synthetic and
  labeled.
* Engine changes are made upstream and reach downstreams through `kb update prepare`, which
  merges an upstream ref in an isolated worktree on a `kb-update/*` branch. `kb update
  divergence` compares a downstream's engine paths (including uncommitted edits) with the
  base in `project/upstream.toml` and fails with `ENGINE_DIVERGED` unless a difference is
  declared as `[[engine_patches]]` with a reason.
* Process conventions (branching models, approvals) are not hard-coded in the engine.

## Versions

All versions live in `core/release.toml` (read line by line by `kb` and strictly by the
engine) and are compiled into the binary from `core/cli/src/versions.rs`.

| version | kind | bump when | effect of a mismatch |
|---|---|---|---|
| `engine_version` | semver; equals `core/cli/Cargo.toml` `version` | every release | snapshot → `UPDATE_REQUIRED`; runtime vs checkout → `RUNTIME_INCOMPATIBLE`; release tag must be `v<engine_version>` |
| `document_schema` | integer (currently `2`, reads `1`/`2`) | format changes of records, registries or `project.toml` (with a migration) | unreadable file schema → `UNSUPPORTED_SCHEMA_VERSION`; snapshot contract mismatch → `UPDATE_REQUIRED` |
| `protocol` | integer (`kb.cli.v<n>`) | an envelope field is removed or changes meaning; codes and exit codes stay stable within a version | snapshot → `UPDATE_REQUIRED` |
| `index_schema` | integer | incompatible change of the SQLite layout | existing index rebuilt in place (`INDEX_REBUILT`); snapshot → `UPDATE_REQUIRED` |
| `LAYOUT` | integer, code only (`core/cli/src/index/schema.rs`, recorded as meta key `layout`) | table changes within one `index_schema` (derived data only) | existing index rebuilt in place (`INDEX_REBUILT`); adding a new meta key has the same effect on older indexes (`<key> missing`) |
| `skill_protocol` | integer | generated skills change the calls or interpretation agents rely on (not for wording) | `--skill-protocol` → `SKILL_OUTDATED` |
| `manifest` | integer (`1`) | format of `core/release.toml` itself | `RUNTIME_INCOMPATIBLE` |
| `PARSER_VERSION` | integer, code only (currently `6`) | parsing output for identical bytes changes, including diagnostic messages | cached parses are invalidated: the index is rebuilt in place (`INDEX_REBUILT`) |
| `rust_toolchain` | toolchain | toolchain upgrade (together with `rust-toolchain.toml`) | `TOOLCHAIN_MISMATCH` |

Unit tests in `versions.rs` fail when `core/release.toml` and the compiled constants disagree
or when `rust_toolchain` differs from `rust-toolchain.toml`. Changing `core/release.toml` or
Cargo files changes the engine fingerprint. Contract versions are independent integers;
record every bump in `CHANGELOG.md`, and add an ADR for significant decisions.

## Adding a document schema migration

The full procedure and the existing steps (`v0-to-v1` from the synthetic legacy schema 0,
see [ADR 0009](adr/0009-synthetic-legacy-schema.md), and `v1-to-v2`) are in
[core/migrations/README.md](../core/migrations/README.md). In short:

1. Agree on the new schema in an ADR.
2. Bump `DOCUMENT_SCHEMA` in `core/cli/src/versions.rs` and `document_schema` in
   `core/release.toml`; add the previous version to `migrates_from`.
3. Register a `Migration { from, to, name, description, record, config, registry }` in
   `MIGRATIONS` (`core/cli/src/migrate.rs`) with pure text transforms: `toml_edit` for TOML,
   bodies byte-identical, fail on unexpected shapes instead of dropping data.
4. Add `core/migrations/fixtures/v<from>/` and `v<to>-expected/` and extend
   `core/cli/tests/migrate_update.rs` (byte-for-byte output, clean load, idempotent re-apply).
5. Regenerate schemas, update templates and the synthetic example, document the step in the
   migrations README, and add a changelog entry.

`kb migrate` is dry-run by default and writes only after every file has been transformed and
verified (`MIGRATION_FAILED` otherwise, nothing written); `kb update prepare` runs it on the
update branch.

## Regenerating schemas

The files in `core/schemas/` are generated from the Rust model (`core/cli/src/model/`,
`versions.rs`, and the envelope in `core/cli/src/schema_export.rs`); see
[core/schemas/README.md](../core/schemas/README.md). After changing model types:

```sh
kb engine bootstrap            # the model change altered the fingerprint
kb schema --write             # regenerate; removes stale *.schema.json files
kb schema --check             # DRIFT_DETECTED (42) if anything still differs
```

Without a runtime, the debug build does the same: `target/debug/kb --root . schema --write`.
`schema_conformance.rs` also fails on drift. Review the schema diff like code.

## Adding a tool adapter

The agent commands reach code tools through the tool registry and engine-owned adapters
([ADR 0016](adr/0016-agent-cli.md), [agent-cli.md](agent-cli.md#connecting-a-new-tool)).
Any command-line tool connects as an `exec` entry (argv template, `prepare` step, output
mapping; [agent-cli.md](agent-cli.md#connecting-any-tool-exec)), another tool of a supported
kind is a registry entry, and commit-bound facts can come from a `kb.code.v1` provider
connected with `adapter = "code-provider"`; none of these changes the engine. A dedicated
adapter is for a tool that needs engine logic of its own (an index protocol with status and
freshness handling, like `codegraph.rs`) and is an engine change:

1. Add one module in `core/cli/src/tools/` next to `codegraph.rs`, `provider.rs`,
   `grep.rs`, `exec.rs` and `command.rs`. It turns a `CodeQuery` into an `Outcome`: items with path,
   line, confidence and a `Source` that names the tool, its version and the state of what
   it read. Start the program through `run_tool`, which applies the deadline, the output
   limits and the shared environment (telemetry, update checks and daemons off). The
   adapter never creates a tool index or installs anything; a missing index makes the tool
   unavailable so routing falls back.
2. Add an `AdapterKind` variant in `core/cli/src/model/tools.rs` with its name,
   capabilities, whether it needs a `program`, and its default rank; dispatch it in
   `answer_code` (`core/cli/src/tools/mod.rs`). Keep validation of the new fields in the
   pure model.
3. Regenerate the schemas (`kb schema --write`; `tools.v1.schema.json` changes) and
   document the adapter in [agent-cli.md](agent-cli.md#tool-registry).
4. Test it with a synthetic, labeled stand-in executable as `core/cli/tests/agent_cli.rs`
   does for CodeGraph: routing, fallback notes, not-found answers and bounded output,
   without an installed tool or network access.

Skills name kb commands only, so an additive adapter changes no skill and needs no skill
protocol bump.

## Maintainer knowledge profile

`core/maintainer-knowledge/` is typed knowledge about kb itself (namespace `kb`): profile
config `profile.toml`, the tool registry `tools.toml` (code tools and the checks above),
registries, records (policies, invariants, contracts, decisions,
procedures such as `kb.procedure.add-migration` and `kb.procedure.cut-release`, a reference
and a gap) and routing fixtures. It is opt-in and never part of a project's context.

```sh
kb validate --profile maintainer
kb context --profile maintainer --snapshot working-tree --offline \
  --intent implement --repo kb --path core/cli/src/index/mod.rs
```

The context call exits 30 (`partial`) by design: working-tree knowledge with unverified
freshness is never `complete`; the mandatory obligations are still delivered. When you change
a contract or process these records describe, update them in the same change.

## Benchmarks

The `kb-bench` crate (`core/benchmarks/`) generates deterministic synthetic corpora and
measures the engine. Timings are reported, never asserted; results belong in
[benchmarks.md](benchmarks.md) together with the environment it prints.

```sh
cargo run -p kb-bench --release --locked -- gen --records 1000 --out <empty-dir> [--seed 42]
cargo run -p kb-bench --release --locked -- verify --dir <corpus-dir>
cargo build --release --locked -p kb
cargo run -p kb-bench --release --locked -- run [--sizes 1000,10000] [--queries 50] \
  [--workdir <empty-dir>] [--kb target/release/kb] [--json <file>] [--skip-cli]
cargo run -p kb-bench --release --locked -- smoke
```

* `gen` refuses a non-empty output directory; `verify` validates a corpus and fails on any
  error diagnostic.
* `run` measures in-process indexing (full, warm, incremental), warm queries, response size,
  and, unless `--skip-cli`, the real executable (startup, first sync, first index, warm
  offline context, online context against a local file remote, peak RSS where available).
  Without `--skip-cli` it needs `target/release/kb` or `--kb <path>`; `--workdir` must be
  empty or absent (default: a new directory under the system temp dir).
* `smoke` runs a 300-record corpus with sanity checks only; CI runs it.

## Releases

No release has been published yet. The release workflow
(`.github/workflows/release.yml`) produces one when an owner pushes a tag.

Prerequisites (owner or administrator actions, not guaranteed by the file):

* GitHub Actions is enabled for the repository;
* repository or organization policy allows `GITHUB_TOKEN` to be granted `contents: write`,
  which only the final `publish` job requests (all other jobs use `contents: read`);
* optionally, a ruleset protecting `v*` tags limits who can trigger a release.

Steps:

1. Through review: set `engine_version` in `core/release.toml` and `version` in
   `core/cli/Cargo.toml` to the new version, update `Cargo.lock` (a build without `--locked`
   rewrites the `kb` entry; CI builds with `--locked`), and add the `CHANGELOG.md` entry.
2. Wait for green CI on that commit.
3. The owner tags and pushes: `git tag -s v<engine_version> <commit>` and
   `git push origin v<engine_version>`.
4. The workflow runs `verify` (the tag must equal `v` + `engine_version`, and the crate
   version must match), `build` on `macos-15` (`aarch64-apple-darwin`), `ubuntu-latest`
   (`x86_64-unknown-linux-musl`), `ubuntu-24.04-arm` (`aarch64-unknown-linux-musl`) and
   `windows-latest` (`x86_64-pc-windows-msvc`) with `kb engine package dist --target <triple>`,
   verifies each archive with its installer (`install.sh`, `install.ps1`), `kb self update`
   and `kb engine install` against a local copy of the release layout, then `publish` writes
   `SHA256SUMS` and creates
   the GitHub release with `kb-<version>-<target>.tar.gz`, their `.sha256` files and
   `SHA256SUMS`. `workflow_dispatch` with an existing tag rebuilds it; it never creates tags.
5. Check the result as in `kb.procedure.cut-release`: install it with the installer on a clean
   machine, then in a clean checkout of the same tag:

   ```sh
   kb engine info        # the installed kb serves the checkout itself
   kb self update --check
   ```

As an engine runtime (`kb engine install`), an archive activates only for a checkout whose
engine fingerprint equals its `BUILD-INFO` (same engine inputs, same target) and whose binary
reports that fingerprint in its smoke test (`kb --json version`). The Linux binaries are
static (musl). Other platforms install from source (`cargo install --locked --git ...`).
`kb engine package <out-dir> [--target <triple>]` produces the same archive and `.sha256`
locally; it always builds the checkout's engine (incrementally, in `KB_CARGO_TARGET_DIR`).

Every downstream inherits `release.yml`. A downstream that does not publish runtime archives
sets the repository variable `KB_RELEASE` to `disabled`: `verify`, `build` and `publish` are
then skipped and a `v*` tag publishes nothing (see
[downstream.md](downstream.md#11-what-administrators-must-configure)).

## Continuous integration

`.github/workflows/upstream-ci.yml` runs on pull requests (once, on the merge commit), on
pushes to `main` and on demand (`workflow_dispatch`), with `contents: read` and no secrets,
on `ubuntu-latest` (x86_64 Linux), `macos-15` (arm64 macOS) and `windows-latest` (x86_64
Windows, glue steps in Git Bash, kb itself also from PowerShell and cmd). A newer push to a pull
request cancels its running checks; runs on `main` always finish. After a checkout without persisted credentials it runs:

1. install the pinned toolchain (`rustup toolchain install`);
2. `cargo fmt --all --check`;
3. `cargo clippy --workspace --all-targets --locked -- -D warnings`;
4. `cargo test --workspace --locked`;
5. `shellcheck install.sh kbw core/templates/ci/*.sh core/templates/ci/hooks/commit-msg`
   (Linux only);
6. `cargo build --release --locked -p kb` with `target/release` put on `PATH`, then
   `kb engine info` and `kb version` (the built kb serves this checkout itself; on Windows
   also from cmd);
7. `kb schema --check`;
8. `kb validate --profile maintainer --templates`;
9. Tier A routing on both corpora: `kb eval routing --profile maintainer --json` and
   `kb eval routing --example synthetic-multirepo --json`;
10. `kb engine package` into a local copy of the release layout, then the installer
    (`install.sh`, or `install.ps1` on Windows), `kb self update --check` and `--force`, and
    `kb engine install` against it (`KB_RELEASES_URL`, `KB_RUNTIME_DIR`);
11. `cargo run -p kb-bench --release --locked -- smoke`;
12. install the Linux replay isolation backend (Linux only): bubblewrap, probed with
    `bwrap --unshare-all`. When Ubuntu's AppArmor restriction
    (`kernel.apparmor_restrict_unprivileged_userns = 1`) blocks the probe, a binary-scoped
    AppArmor profile (`kb-eval-bwrap`) grants `userns` to `/usr/bin/bwrap` alone; the
    host-wide setting is never lowered, and a backend that still cannot create its
    namespaces fails the step;
13. the replay isolation smoke `cargo test -p kb-eval --locked --test replay_workflow -- --ignored`
    (Seatbelt on macOS, bubblewrap on Linux; not on Windows, which has no isolation backend;
    synthetic, offline, no model credentials).

The replay steps run last, so a runner that cannot sandbox does not skip the checks before
them.

The `ci` job passes only when every platform job passed; it is the one check to require for
merging (the `main-protection` ruleset of the upstream requires it), so a runner image can be
replaced without changing the required checks. A platform job that GitHub could not start
(for example "The job was not acquired by Runner of type hosted", which macOS arm64 capacity
limits can cause) fails `ci` as well; re-run the failed jobs once capacity returns.

Making these checks required for merging is a repository setting. Downstream forks inherit
this workflow; setting the repository variable `KB_ENGINE_CI` to `disabled` skips its job
(`check`) without editing the engine-owned file. With `KB_CARGO_TARGET_DIR` pointing at the
workspace `target/`, `kb engine package` reuses the release build of step 6.

CI and merge request templates for downstream KBs and host repositories live in
`core/templates/ci/` and `core/templates/mr/` (see [downstream.md](downstream.md)).

## Dependencies and licenses

kb is licensed under Apache-2.0 ([LICENSE](../LICENSE)). The runtime dependency list with
licenses, and the commands that regenerate it from `Cargo.lock`, are in
[dependencies.md](dependencies.md);
update it whenever `Cargo.lock` changes. Development-only dependencies (`tempfile`,
`proptest`, `jsonschema`) are not shipped in release archives.
