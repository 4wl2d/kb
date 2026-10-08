# Agent instructions for the kb upstream repository

These instructions are for coding agents and people changing **the kb engine** (this
repository). Agents working in a *host* repository that uses a downstream KB get their
instructions from the generated `kb` skill instead.

## Orientation

* Engineering contract: `docs/architecture.md`. Decisions: `docs/adr/`.
* Rust crate `kb` (library + executable): `core/cli/`. Benchmarks: `core/benchmarks/`.
* Launcher: `kbw` (POSIX sh). Manifest: `core/release.toml`.
* Opt-in knowledge about kb itself: `core/maintainer-knowledge/` — query it with
  `./kbw context --profile maintainer --snapshot working-tree --offline --intent implement --repo kb --path <file>`.

## Rules

1. Engine-owned paths are listed in `core/release.toml` (`engine_paths`). Never put real
   project data in this repository; examples must be synthetic and labeled.
2. Keep pure logic (`model`, `parse`, `validate`, `scope`, `normalize`, `glob`, `context`)
   free of Git/filesystem side effects; adapters live in `git`, `host`, `snapshot`,
   `overlay`, `source`, `index`, `integrate`, `update`.
3. Output determinism: sorted collections, id tie-breaks, no timings or other
   non-deterministic values in `result`; they belong in `meta` (as the index cache path and
   size of `kb index` do).
   In JSON mode stdout carries only the protocol envelope, also for usage errors.
4. Tests never use the network. Git in tests runs with an isolated HOME and
   `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1` (see `core/cli/tests/common`).
   Do not assert absolute timings.
5. Contract changes: bump the matching version in `core/release.toml` and
   `core/cli/src/versions.rs` (document schema, protocol, index schema, skill protocol) and
   add a migration when the document schema changes (`kb.procedure.add-migration`).
6. After changing model types, regenerate schemas: `./kbw schema --write`.
7. No Python in implementation, tests, generators or benchmarks; shell only for `kbw` and CI glue.

## Checks before finishing

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
shellcheck kbw core/templates/ci/*.sh core/templates/ci/hooks/commit-msg
./kbw schema --check
./kbw validate --profile maintainer --templates
```

On macOS, if the bundled SQLite build cannot find system headers, export
`SDKROOT="$(xcrun --show-sdk-path)"` (the launcher does this automatically).
