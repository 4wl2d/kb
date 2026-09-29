# Maintainer guide

For people and agents changing **the kb engine** in the upstream repository. Start with
[AGENTS.md](../AGENTS.md) (rules), [CONTRIBUTING.md](../CONTRIBUTING.md) (workflow),
[architecture.md](architecture.md) (the normative contract) and the [ADRs](adr/). Downstream
operation is covered in [downstream.md](downstream.md).

## Development setup

| requirement | source of truth | notes |
|---|---|---|
| Rust `1.98.1` with `rustfmt`, `clippy` | `rust-toolchain.toml`, `core/release.toml` `rust_toolchain` | rustup selects it automatically; without rustup, the `rustc` on `PATH` must report exactly this version or `./kbw --kbw-bootstrap` fails with `KBW_TOOLCHAIN_MISMATCH` |
| Git ≥ `2.38.0` | `core/release.toml` `min_git` | `kb update check` predicts conflicts with `git merge-tree --write-tree`; `kb doctor` checks the version |
| `shellcheck` | CI | lints the POSIX launcher `kbw` |
| macOS SDK headers | — | the bundled SQLite is compiled from source; if headers are not found, `export SDKROOT="$(xcrun --show-sdk-path)"` (the launcher does this for its own builds) |

```sh
cargo build --workspace --locked      # the first build downloads crates; tests never use the network
cargo test --workspace --locked
./kbw --kbw-bootstrap                 # release runtime for the launcher (.cache/runtime/<fingerprint>/)
./kbw version
```

Two ways to run the engine while developing:

* `target/debug/kb --root . <command>` runs the debug build directly (`kb version` prints
  `build unknown` and `build_fingerprint` is `unknown`; `doctor` notes that it was not
  started through `kbw`).
* `./kbw <command>` runs the runtime of the current engine fingerprint. Any edit of a build
  input (`Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `core/release.toml`,
  `core/cli/Cargo.toml`, `core/cli/src/**`) changes the fingerprint, so the next call fails
  with `KBW_RUNTIME_NOT_BOOTSTRAPPED` until you run `./kbw --kbw-bootstrap` again (or set
  `KBW_AUTO_BOOTSTRAP=1`). The fingerprint is compiled into the runtime
  (`KBW_BUILD_FINGERPRINT`, a tracked Cargo input) and reported by `./kbw version`
  (`build <fp>`). `KBW_CARGO_TARGET_DIR` points source bootstraps at a shared target
  directory (default `<root>/.cache/cargo-target`; a relative value is resolved against the
  directory you run `kbw` from, not against the root). Sharing it with a plain
  `cargo build --release` costs one recompilation of the `kb` crate whenever the two
  alternate, because that build embeds `unknown` instead of the fingerprint.

Code layout rules (from AGENTS.md): pure logic (`model`, `parse`, `validate`, `scope`,
`normalize`, `glob`, `context`) has no Git or filesystem side effects; adapters live in
`git`, `host`, `snapshot`, `overlay`, `source`, `index`, `integrate`, `update`. No Python
anywhere; shell only for `kbw` and CI glue.

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
| `migrate_update.rs` | `kb migrate` on the synthetic legacy fixtures and `kb update` scenarios (check, prepare, divergence, abandon) with synthetic upstream/downstream repositories; the engine steps run through an injected test runner or a stub `kbw`, never a real build |
| `cli_commands.rs` | `context`, `search`, `show`, `index`, `validate`, `impact`, `doctor` end to end: KB pushed to a local bare origin, host mounting it as a submodule; also usage and protocol errors in JSON mode, the snapshot line of text output, absolute `--path` values through a symlinked host directory, the `doctor` checks `source`, `host-pin` and `snapshot-engine`, and that reading commands leave the checkouts untouched |
| `launcher.rs` | the real `kbw` script: fingerprint definition, warm path, invalidation, toolchain mismatch, the build fingerprint compiled into source builds, a relative `KBW_CARGO_TARGET_DIR`, packaging (including rebuilding a runtime that does not report its fingerprint), artifact installation, reinstalling with the runtime resolvable at every step, rejection of invalid archives with the active runtime kept (fake runtimes and a fake `cargo`), running under `dash`, one real source bootstrap, shellcheck |
| `e2e_workflow.rs` | the specification's end-to-end scenario (unix only): synthetic upstream → downstream init → multi-repository context → proposal → approved-origin update → pin refresh → upstream-format update, through `kbw` |
| `update_production.rs` | `kb update prepare` through the production path (unix only): the real `kbw` of the target engine bootstraps it inside the update worktree, migrates the synthetic legacy (schema 0) project to the `v1-expected` fixture byte for byte and validates it, leaving the main checkout untouched |

Shared helpers are in `core/cli/tests/common/`. Rules for every test:

* no network: remotes are local bare repositories addressed by path;
* Git runs with an isolated `HOME`, `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1`;
* never assert absolute timings;
* fixtures are synthetic and labeled (`core/tests/fixtures/`, `core/migrations/fixtures/`).

Notes:

* `launcher.rs` (real bootstrap), `e2e_workflow.rs` and `update_production.rs` build with the
  pinned toolchain and `CARGO_NET_OFFLINE=true`, so they need a warm Cargo registry cache
  (any earlier `cargo build --locked` provides it). They share compiled dependencies through
  one directory under Cargo's test tmpdir (`kbw-cargo-target`). The e2e test performs two
  release builds of the `kb` crate (the downstream runtime and the update worktree), so it is
  the slowest suite; `update_production.rs` performs one. The e2e "upstream-format update"
  leg has no format change; `update_production.rs` covers a real schema migration through the
  production launcher.
* `kbw_is_shellcheck_clean` prints `skipping` and passes when `shellcheck` is not installed;
  CI runs `shellcheck kbw` explicitly.
* Regenerate golden context output only intentionally, then review the diff:
  `KB_UPDATE_GOLDEN=1 cargo test -p kb --test context_golden`.
* One suite: `cargo test -p kb --test cli_commands`.

## Required checks

Run before finishing any change (the same list is in AGENTS.md):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
shellcheck kbw
./kbw schema --check
./kbw validate --profile maintainer --templates
```

`./kbw schema --check` fails with `DRIFT_DETECTED` (42) when `core/schemas/` differs from the
model. `./kbw validate --profile maintainer --templates` validates the maintainer knowledge
with its routing fixtures and every shipped template and example; on a clean upstream (no
`project/project.toml`) it reports the missing project as info and exits 0.

## Engine and project ownership

* `core/release.toml` `engine_paths` lists what the engine owns: `kbw`, the Cargo files,
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

All versions live in `core/release.toml` (read line by line by `kbw` and strictly by the
engine) and are compiled into the binary from `core/cli/src/versions.rs`.

| version | kind | bump when | effect of a mismatch |
|---|---|---|---|
| `engine_version` | semver; equals `core/cli/Cargo.toml` `version` | every release | snapshot → `UPDATE_REQUIRED`; runtime vs checkout → `RUNTIME_INCOMPATIBLE`; release tag must be `v<engine_version>` |
| `document_schema` | integer | incompatible change of records, registries or `project.toml` (with a migration) | files with another schema → `UNSUPPORTED_SCHEMA_VERSION`; snapshot → `UPDATE_REQUIRED` |
| `protocol` | integer (`kb.cli.v<n>`) | an envelope field is removed or changes meaning; codes and exit codes stay stable within a version | snapshot → `UPDATE_REQUIRED` |
| `index_schema` | integer | incompatible change of the SQLite layout | existing index rebuilt in place (`INDEX_REBUILT`); snapshot → `UPDATE_REQUIRED` |
| `LAYOUT` | integer, code only (`core/cli/src/index/schema.rs`, recorded as meta key `layout`) | table changes within one `index_schema` (derived data only) | existing index rebuilt in place (`INDEX_REBUILT`); adding a new meta key has the same effect on older indexes (`<key> missing`) |
| `skill_protocol` | integer | generated skills change the calls or interpretation agents rely on (not for wording) | `--skill-protocol` → `SKILL_OUTDATED` |
| `manifest` | integer (`1`) | format of `core/release.toml` itself | `RUNTIME_INCOMPATIBLE` |
| `PARSER_VERSION` | integer, code only (currently `2`) | parsing output for identical bytes changes, including diagnostic messages | cached parses are invalidated: the index is rebuilt in place (`INDEX_REBUILT`, for example `parser_version 1 -> 2`) |
| `rust_toolchain` | toolchain | toolchain upgrade (together with `rust-toolchain.toml`) | `KBW_TOOLCHAIN_MISMATCH` |

Unit tests in `versions.rs` fail when `core/release.toml` and the compiled constants disagree
or when `rust_toolchain` differs from `rust-toolchain.toml`. Changing `core/release.toml` or
Cargo files changes the engine fingerprint. Contract versions are independent integers;
record every bump in `CHANGELOG.md`, and add an ADR for significant decisions.

## Adding a document schema migration

The full procedure and the only existing step (`v0-to-v1` from the synthetic legacy schema
0, see [ADR 0009](adr/0009-synthetic-legacy-schema.md)) are in
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
./kbw --kbw-bootstrap            # the model change altered the fingerprint
./kbw schema --write             # regenerate; removes stale *.schema.json files
./kbw schema --check             # DRIFT_DETECTED (42) if anything still differs
```

Without a runtime, the debug build does the same: `target/debug/kb --root . schema --write`.
`schema_conformance.rs` also fails on drift. Review the schema diff like code.

## Maintainer knowledge profile

`core/maintainer-knowledge/` is typed knowledge about kb itself (namespace `kb`): profile
config `profile.toml`, registries, records (policies, invariants, contracts, decisions,
procedures such as `kb.procedure.add-migration` and `kb.procedure.cut-release`, a reference
and a gap) and routing fixtures. It is opt-in and never part of a project's context.

```sh
./kbw validate --profile maintainer
./kbw context --profile maintainer --snapshot working-tree --offline \
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
   version must match), `build` on `macos-14` (`aarch64-apple-darwin`) and `ubuntu-latest`
   (`x86_64-unknown-linux-gnu`) with `./kbw --kbw-package dist`, verifies each archive by
   installing it with `--kbw-install-artifact`, then `publish` writes `SHA256SUMS` and creates
   the GitHub release with `kb-<version>-<target>.tar.gz`, their `.sha256` files and
   `SHA256SUMS`. `workflow_dispatch` with an existing tag rebuilds it; it never creates tags.
5. Check the result as in `kb.procedure.cut-release`: download an archive, compare its digest
   with `SHA256SUMS`, and install it on a clean checkout of the same tag:

   ```sh
   ./kbw --kbw-install-artifact <archive-path-or-https-url> --sha256 <digest from SHA256SUMS>
   ./kbw --kbw-runtime-info
   ```

An archive activates only for a checkout whose engine fingerprint equals its `BUILD-INFO`
(same engine inputs, same target) and whose binary reports that fingerprint in its smoke test
(`kb --json version`). The Linux binary needs a glibc at least as new as the build runner's.
Other platforms are not built. `./kbw --kbw-package <out-dir>` produces the same archive and
`.sha256` locally; it packages the active source-built runtime, and builds from source instead
when the active runtime is an installed artifact or does not report the current fingerprint.

Every downstream inherits `release.yml`. A downstream that does not publish runtime archives
sets the repository variable `KB_RELEASE` to `disabled`: `verify`, `build` and `publish` are
then skipped and a `v*` tag publishes nothing (see
[downstream.md](downstream.md#11-what-administrators-must-configure)).

## Continuous integration

`.github/workflows/upstream-ci.yml` runs on every push and pull request with
`contents: read` and no secrets, on `ubuntu-latest` (x86_64 Linux) and `macos-14` (arm64
macOS):

1. install the pinned toolchain (`rustup toolchain install`);
2. `cargo fmt --all --check`;
3. `cargo clippy --workspace --all-targets --locked -- -D warnings`;
4. `cargo test --workspace --locked`;
5. `shellcheck kbw` (Linux only);
6. `./kbw --kbw-bootstrap`, `./kbw --kbw-runtime-info`, `./kbw version` (with
   `KBW_CARGO_TARGET_DIR` set to the workspace `target/`);
7. `./kbw schema --check`;
8. `./kbw validate --profile maintainer --templates`;
9. `cargo run -p kb-bench --release --locked -- smoke`.

Making these checks required for merging is a repository setting. Downstream forks inherit
this workflow; setting the repository variable `KB_ENGINE_CI` to `disabled` skips its job
(`check`) without editing the engine-owned file. With `KBW_CARGO_TARGET_DIR` pointing at the
workspace `target/`, the launcher's release build (which embeds the engine fingerprint) and
plain Cargo release builds such as the benchmark smoke run (which embed `unknown`) share one
directory, so the `kb` crate may be recompiled when they alternate; this costs time only.
`release.yml` builds with `./kbw --kbw-package`, so its archives carry the fingerprint.

CI and merge request templates for downstream KBs and host repositories live in
`core/templates/ci/` and `core/templates/mr/` (see [downstream.md](downstream.md)).

## Dependencies and licenses

kb is licensed under Apache-2.0 ([LICENSE](../LICENSE)). The runtime dependency list with
licenses, and the commands that regenerate it from `Cargo.lock`, are in
[dependencies.md](dependencies.md);
update it whenever `Cargo.lock` changes. Development-only dependencies (`tempfile`,
`proptest`, `jsonschema`) are not shipped in release archives.
