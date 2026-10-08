# kb documentation

All documentation of the kb engine. When a guide and the code disagree,
[architecture.md](architecture.md) and the code win and the guide is a bug.

## By task

| I want to… | read |
|---|---|
| turn a fresh downstream fork into a product knowledge base | [BOOTSTRAP.md](../BOOTSTRAP.md), [prompts/adaptation.md](prompts/adaptation.md) |
| run a downstream KB: mount it in hosts, CI, merge requests, upgrades | [downstream.md](downstream.md) |
| write or review knowledge records | [format.md](format.md) |
| understand what `kb context` returns and why | [context.md](context.md) |
| understand freshness, snapshot selection, caching and trust | [snapshots-and-trust.md](snapshots-and-trust.md) |
| call kb from a program or agent (`--json`) | [protocol.md](protocol.md) |
| fix an error | [troubleshooting.md](troubleshooting.md) |
| keep knowledge current during everyday coding work | [prompts/maintenance.md](prompts/maintenance.md) |
| produce domain drafts and check provenance, freshness or declared rules | [knowledge-lifecycle.md](knowledge-lifecycle.md) |
| measure retrieval or compare agent outcomes | [evaluation.md](evaluation.md), [replay kit](../core/eval/README.md) |
| attach optional static code facts | [reference providers](../core/providers/README.md) |
| change the engine, test, release | [maintainer-guide.md](maintainer-guide.md), [AGENTS.md](../AGENTS.md), [CONTRIBUTING.md](../CONTRIBUTING.md) |

## Reference

| document | content |
|---|---|
| [architecture.md](architecture.md) | normative engineering contract: layout, record format, registries, context assembly, freshness and snapshots, index, CLI, launcher, integrations |
| [adr/](adr/) | architecture decision records (below) |
| [format.md](format.md) | record schemas 1/2, the eight kinds, domain fields, scope, selectors, links, policies and overrides, registries, diagnostic catalogue |
| [context.md](context.md) | context assembly: scope resolution, mandatory selection, `requires` closure, ranking, ambiguity, proposals, budgets, completeness, receipts, explain, output formats, `search` and `show` |
| [snapshots-and-trust.md](snapshots-and-trust.md) | per-call freshness, `--snapshot` selection, host detection, engine compatibility, mirror and cache layout, proposal overlay, index behavior, offline work, trust model |
| [downstream.md](downstream.md) | downstream setup, submodule binding, host integration, CI/MR templates, linked merge requests, sync versus upgrade, coordinated upgrades, recovery, administrator settings |
| [protocol.md](protocol.md) | machine protocol `kb.cli.v1`: output channels, envelope, per-command results, error and exit codes, `meta` diagnostics, launcher codes, versioning |
| [troubleshooting.md](troubleshooting.md) | symptom → cause → fix for every error users hit, offline recovery, cache contents, bug reports |
| [maintainer-guide.md](maintainer-guide.md) | development setup, test suites, required checks, ownership, versions, migrations, schema regeneration, maintainer knowledge, benchmarks, releases, CI |
| [prompts/adaptation.md](prompts/adaptation.md) | ready-to-paste AI adaptation prompt for a downstream fork |
| [prompts/maintenance.md](prompts/maintenance.md) | ready-to-paste prompt for ongoing work in host repositories |
| [dependencies.md](dependencies.md) | runtime dependencies and their licenses, and how to regenerate the list |
| [benchmarks.md](benchmarks.md) | benchmark report: environment, method and measured results of `kb-bench` |
| [verification.md](verification.md) | verification report: the exact commands run and their actual results |
| [knowledge-lifecycle.md](knowledge-lifecycle.md) | coverage, change work orders, drafts, stamps, drift, ledger, probes and local usage |
| [evaluation.md](evaluation.md) | Tier A routing/history and Tier B acceptance boundaries |
| [optional-extensions.md](optional-extensions.md) | evidence gates for MCP, agent hooks, embeddings and other optional work |
| [upstream-upgrade.md](upstream-upgrade.md) | implementation checkpoints and outstanding validation/empirical/release gates |

## Architecture decision records

| ADR | decision |
|---|---|
| [0001](adr/0001-distribution-model.md) | one upstream, one downstream fork per product |
| [0002](adr/0002-record-format.md) | Markdown with strict TOML front matter as the only authoring format |
| [0003](adr/0003-applicability-vs-relevance.md) | separate mandatory applicability (scope) from relevance (selectors) |
| [0004](adr/0004-freshness-and-snapshots.md) | per-call freshness, explicit selection, isolated mirror |
| [0005](adr/0005-index.md) | content-addressed SQLite FTS5 index as a rebuildable cache |
| [0006](adr/0006-launcher-and-runtime.md) | POSIX launcher with fingerprinted runtimes |
| [0007](adr/0007-budgets-and-receipts.md) | whole-unit packing with byte and estimated-token budgets |
| [0008](adr/0008-integrations.md) | generated skills and managed instruction blocks |
| [0009](adr/0009-synthetic-legacy-schema.md) | synthetic legacy schema 0 exercises the migration machinery |
| [0010](adr/0010-schema-2.md) | additive schema 2 and lossless adjacent migration (proposed) |
| [0011](adr/0011-code-provider.md) | separate commit-bound static provider protocol (proposed) |
| [0012](adr/0012-temporal-context.md) | explicit temporal slices and frozen replay inputs (proposed) |
| [0013](adr/0013-delivery-reuse.md) | verified always-on core, receipt deltas and one full skill (proposed) |
| [0014](adr/0014-knowledge-production.md) | deterministic work orders, draft production and evidence ladder (proposed) |
| [0015](adr/0015-declarative-verification.md) | bounded probes, source evidence and freshness limits (proposed) |

## Next to the code

| file | content |
|---|---|
| [../SECURITY.md](../SECURITY.md) | vulnerability reporting for the engine and threat model summary (a downstream publishes its own policy at `.github/SECURITY.md`) |
| [../CHANGELOG.md](../CHANGELOG.md) | release notes (no release published yet) |
| [../core/schemas/README.md](../core/schemas/README.md) | generated JSON Schemas and the rules only the runtime enforces |
| [../core/migrations/README.md](../core/migrations/README.md) | `kb migrate`, the synthetic legacy schema 0 and how to add a migration |
| [../core/templates/README.md](../core/templates/README.md) | project, record, CI, merge request, ownership and security templates |
| [../core/templates/examples/synthetic-multirepo/README.md](../core/templates/examples/synthetic-multirepo/README.md) | the synthetic multi-repository example and its [expected queries](../core/templates/examples/synthetic-multirepo/expected-queries.md) |
| [../core/skills/README.md](../core/skills/README.md) | canonical skill sources rendered by `kb integrate --generate` |
| [../core/maintainer-knowledge/README.md](../core/maintainer-knowledge/README.md) | opt-in knowledge about kb itself (`--profile maintainer`) |
| [../core/tests/fixtures/context/README.md](../core/tests/fixtures/context/README.md) | synthetic fixture of the context golden tests |
