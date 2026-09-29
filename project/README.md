# project/ (not initialized)

This directory holds the project-owned knowledge of a downstream knowledge base. In the
upstream kb repository it is intentionally empty: upstream ships the engine, templates and
skills, never project data. Until a downstream is initialized, project commands such as
`./kbw context` fail with `PROJECT_NOT_INITIALIZED` (exit code 10).

## Initialize a downstream

In your product's fork or private copy of this repository (keep the upstream history):

```sh
./kbw init --name "<Product name>" --namespace <ns>                # dry-run: shows the plan
./kbw init --name "<Product name>" --namespace <ns> --apply        # writes the files
```

Useful options: `--remote <name>` and `--approved-ref <ref>` (trust boundary, default
`origin` / `refs/heads/main`), `--kb-path <path>` (where hosts mount this KB, default `.kb`),
`--harness claude|codex|cursor` (repeatable, default all three), `--upstream-url <url>`
(recorded in `project/upstream.toml`).

`init --apply` creates `project/project.toml`, `project/registry/`, `project/knowledge/`,
`project/skill-config/` (including the rendered skill bundle), `project/routing-tests/`,
`project/upstream.toml` and `.github/workflows/kb-knowledge.yml`, and replaces this README
with the project README. Existing files are never overwritten. Then follow BOOTSTRAP.md to
turn the empty structure into the project's knowledge base.

## Try the synthetic example

`./kbw init --example synthetic-multirepo --apply` materializes an invented multi-repository
product (namespace `example`) for demonstration and testing. Use it only in a scratch clone;
see `core/templates/examples/synthetic-multirepo/README.md`.
