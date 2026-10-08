# Contributing

Thank you for improving kb. This repository is the **upstream**: it contains the engine,
schemas, migrations, templates, shared skills and release machinery. Product knowledge lives
in downstream forks under `project/` and never comes back here.

## Development setup

* Rust toolchain pinned in `rust-toolchain.toml` (rustup picks it up automatically).
* Git ≥ 2.38 (`git merge-tree --write-tree` is used by `kb update check`).
* `shellcheck` for the launcher and the CI/hook shell templates
  (`shellcheck kbw core/templates/ci/*.sh core/templates/ci/hooks/commit-msg`, as in
  `AGENTS.md`).

```sh
cargo test --workspace --locked
./kbw --kbw-bootstrap        # builds the release runtime used by the launcher
./kbw version
```

## Change workflow

1. Read `AGENTS.md`, `docs/architecture.md` and the relevant ADRs. Query the maintainer
   profile for obligations touching your paths.
2. Keep changes small and covered by tests that exercise real behavior (the executable,
   the launcher, real temporary Git repositories).
3. Update documentation, schemas (`./kbw schema --write`), templates and `CHANGELOG.md`
   in the same change.
4. Significant decisions get an ADR in `docs/adr/`.

## Versioning

`engine_version` follows semantic versioning. Contract versions (document schema, CLI
protocol, index schema, skill protocol) are independent integers in `core/release.toml`;
bump them only for incompatible changes, together with migrations where applicable.

## Releases

Releases are cut by an owner by pushing a `v<engine_version>` tag; see
`docs/maintainer-guide.md` and `.github/workflows/release.yml`. Downstream forks inherit the
release and CI workflows; they switch them off with the repository variables
`KB_RELEASE=disabled` and `KB_ENGINE_CI=disabled` instead of editing engine-owned files.
