# Verification report

What was run to check this version, with the observed results. Only checks that were
actually executed are listed; platforms and services that were not exercised are named
explicitly under [Not verified](#not-verified).

Date: 2026-09-27. Machine: Apple M5 Max (arm64), macOS 26.6.2. Toolchain: rustc/cargo
1.98.1 (Homebrew; no rustup installed; equals the pin in `rust-toolchain.toml`), git 2.55.0,
ShellCheck from Homebrew. On this machine the bundled SQLite C build needs
`SDKROOT=$(xcrun --show-sdk-path)` for plain `cargo` commands; `kbw` sets it automatically
when it is unset (verified by running the launcher with `SDKROOT` unset).

## Static checks and tests (repository root)

| command | result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | exit 0, no warnings |
| `shellcheck kbw` | exit 0, no findings |
| `cargo build --release --locked --workspace` | exit 0 |
| `cargo test --workspace --locked` | exit 0: **316 passed, 0 failed, 0 ignored** |

Per suite (`cargo test --workspace --locked`):

| suite | passed | notes |
|---|---:|---|
| library unit tests (`src/lib.rs`) | 109 | model, parsing, normalization, globs, scope algebra, git runner, stat cache, index, context, integrate, migrate, update, CLI session |
| `tests/cli_commands.rs` | 16 | real `kb` binary against local bare remotes and a host with a KB submodule |
| `tests/context_golden.rs` | 36 | golden routing fixtures (19 cases), determinism, budgets, overrides, versions, directory paths, receipts |
| `tests/e2e_workflow.rs` | 1 | full scenario, about 55 s (below) |
| `tests/foundation.rs` | 3 | shared fixtures, clean upstream, maintainer profile |
| `tests/impact_analysis.rs` | 11 | diffs incl. renames/deletes/working tree, unknown coverage, statements |
| `tests/index_store.rs` | 26 | incremental builds, isolation, concurrency, interrupted builds, corruption recovery, FTS safety |
| `tests/init_integrate.rs` | 26 | init dry-run/apply, templates, example, skill bundle, host managed blocks, conflicts |
| `tests/launcher.rs` | 18 | real `kbw`: source bootstrap, warm path, invalidation, artifact install and tampering, about 33 s |
| `tests/migrate_update.rs` | 12 | synthetic legacy migration, update check/prepare/divergence/abandon |
| `tests/schema_conformance.rs` | 15 | schema drift, runtime-vs-schema verdicts, property tests, JSON envelopes |
| `tests/snapshot_git.rs` | 20 | freshness, selection, pins, overlays, worktrees, cache isolation, transport policy, redaction |
| `tests/update_production.rs` | 1 | `update prepare` with the production launcher runner on a real legacy-format downstream, about 25 s |
| `tests/validate_rules.rs` | 22 | every cross-record validation rule family |

The end-to-end test `e2e_workflow` runs the whole chain against the real executable and the
production launcher, offline with local bare repositories:

1. synthetic upstream;
2. downstream clone with `kbw --kbw-bootstrap` and `init --example`;
3. multi-repository context from mobile and backend hosts mounting the KB as a submodule;
4. a dirty proposal overlay;
5. a new approved commit giving `UPDATE_REQUIRED`, `--snapshot latest` and `--snapshot pinned`, and `sync`;
6. a host pin refresh through a normal commit;
7. `update check` and `update prepare` of a new upstream tag in an isolated worktree, with the main checkout unchanged.

## Launcher and CI commands (repository root)

These are the commands `.github/workflows/upstream-ci.yml` runs, executed locally with
`KBW_CARGO_TARGET_DIR=$PWD/target`:

| command | result |
|---|---|
| `./kbw version` (before bootstrapping) | exit 50, `KBW_RUNTIME_NOT_BOOTSTRAPPED` |
| `./kbw --kbw-bootstrap` | exit 0, source build activated under `.cache/runtime/<fingerprint>/` |
| `./kbw --kbw-runtime-info` | `status=active`, `source=source-build`, `target=aarch64-apple-darwin` |
| `./kbw version` | `kb 0.1.0 (document schema 1, protocol 1, index schema 1, skill protocol 1) build <fingerprint>` |
| `./kbw schema --check` | exit 0, 13 schemas match the model |
| `./kbw validate --profile maintainer --templates` | exit 0: 14 records, 0 errors, 0 warnings; templates checked; 3/3 routing tests |
| `./kbw context --intent explain --task anything` | exit 10, `PROJECT_NOT_INITIALIZED` (clean upstream) |
| `./kbw --json context --intent implment` | exit 2, one `kb.cli.v1` envelope with `USAGE` on stdout |
| `cargo run -p kb-bench --release --locked -- smoke` | exit 0, sanity checks passed |
| YAML parse of `.github/workflows/*.yml` and `core/templates/ci/**` (Ruby `YAML.load_file`) | all 6 files parse |

Release packaging, the step `release.yml` runs, was also exercised locally:
`./kbw --kbw-package <dir>` wrote `kb-0.1.0-aarch64-apple-darwin.tar.gz` and its `.sha256`.
The archive was then installed into a clean copy of the checkout with
`./kbw --kbw-install-artifact <archive> --sha256-file <archive>.sha256` (exit 0,
`source=artifact`, same build fingerprint). The same install with a wrong digest was refused
before extraction (exit 50, `KBW_ARTIFACT_INVALID`).

## Documented quickstart

The README quickstart was executed verbatim in a scratch copy of this tree, committed there
because `git clone .` needs history:

1. `./kbw --kbw-bootstrap`, `./kbw version`, `./kbw validate --profile maintainer --templates`
   (exit 0) and `./kbw context ...` (exit 10).
2. `git clone . ../kb-demo`, `./kbw --kbw-bootstrap`, `./kbw init --example
   synthetic-multirepo` (dry-run, then `--apply`), `./kbw validate` (21 records, 0 errors,
   7/7 routing tests), commit, `git clone --bare` as the approved origin.
3. `./kbw context --intent implement --task "Retry the token refresh after a network error"
   --repo mobile --path mobile:app/auth/TokenRefresher.kt`: exit 0, `COMPLETE`, freshness
   `verified`, mandatory ids `example.common.code-review`, `example.mobile.token-storage`,
   `example.mobile.single-refresh`, `example.contract.error-envelope`,
   `example.contract.token-refresh`. This matches the excerpt in the README; the `--json` +
   `jq` variant gives the same ids.

The documentation writers additionally ran every command shown in README, BOOTSTRAP and
`docs/*.md` against scratch KBs and host repositories, including host integration, proposals,
`UPDATE_REQUIRED`/pinned/latest, `impact --check` and `update check`.

## Benchmarks

See [benchmarks.md](benchmarks.md). Measured at 10 000 records on this machine: warm
in-process query p50 22 ms; warm `kb --offline context` through the real executable p50
53 ms, p95 57 ms.

## Reviews

Two adversarial review rounds were run over the code. Each finding was independently
verified before any fix. Round one confirmed 21 findings and refuted 3; round two confirmed 17
and refuted 3. All confirmed findings were fixed, each with a regression test that failed
before the fix.

## Not verified

* **Linux x86_64.** No Linux environment was available. GNU `find`/`tar`/`sha256sum` paths
  of `kbw`, the Linux release build, and `dash` as `/bin/sh` on Linux are covered only by the
  CI workflows, which have not run. `kbw` was tested under `/bin/dash` on macOS.
* **GitHub Actions and GitLab CI.** The workflows and templates were parsed and their
  commands run locally, but they never ran on GitHub or GitLab. No release has been
  published; `release.yml` needs an owner to push a `v0.1.0` tag and to allow
  `contents: write` for the publish job.
* **rustup-managed toolchains.** This machine has no rustup; the toolchain check was
  exercised with the Homebrew rustc (which matches the pin) and with fake `rustc`/`cargo` in
  tests.
* **Network transports.** Tests and checks use local `file` remotes only; `https`/`ssh`
  fetches were not exercised. Credential redaction was tested with a refused local
  connection.
* **Harness behavior.** Generated skills follow the documented formats of Claude Code, Codex
  and Cursor (sources in `core/skills/kb/references/harnesses.md.tmpl`), but no harness was
  run against them. Cursor's handling of two identical `kb` skills is undocumented and
  recorded as a known gap.
* **Windows** is not supported (POSIX launcher).
