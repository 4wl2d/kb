# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses semantic
versioning for `engine_version`; contract versions (document schema, CLI protocol, index
schema, skill protocol) are independent integers in `core/release.toml`.

## [0.1.0] - Unreleased

First complete version of the upstream repository. No release has been published yet;
the release workflow builds archives when an owner pushes a `v0.1.0` tag.

### Knowledge upgrade (unreleased)

- Document schema 2 with lossless adjacent migration, structured subsystem/scenario and
  contract-consumer fields, glossary, temporal validity, freshness, stamps and probes.
  Schema-1 content remains readable; CLI protocol stays 1, index/skill protocols become 2.
- Coverage, change work orders, validated draft submission/capture, domain harvest and
  evidence ladder in adaptation/maintenance v2; optional CI accrual and review templates.
- Provisional path-free diagnosis, tracked identifier discovery, diff/change-category
  scope and `--as-of`; explicit provider facts, deep impact and consumer suggestions.
- Verified always-on core and receipt deltas, terse/outline output, one shared full skill
  and native targets for six harnesses with installation/load-probe boundaries.
- Anchor checks, owner-grouped drift, ledger audits, dated freshness, declarative
  verification, optional Git hook/CI checks and private local delivery observations.
- Tier A routing/history metrics and the separate Rust Tier B replay kit with frozen Git
  inputs, OS isolation, pinned native adapters, blinded judging, complete segment accounting
  and paired cluster/Holm/SESOI analysis.
- Pre-merge audit hardening: repo qualifiers of verify-probe, selector and registry globs
  must name registry repos as written (parser version 6); migration and stamping keep line
  endings; discovered identifiers and over-limit patches never narrow obligations or fail
  context; core-only receipt references are never reused; the commit-msg hook checks the
  index and cleaned message Git commits and selects its snapshot from the host layout;
  provider deadlines end nested commands; replay egress works under Seatbelt and the Linux
  CI smoke grants bubblewrap user namespaces through a binary-scoped AppArmor profile.

These changes are implementation work, not a tagged 0.2/0.3/0.4 release or a claim of
better agent quality. ADR review, complete validation and preregistered empirical/release
gates are recorded in `docs/upstream-upgrade.md`; optional extensions stay gated.

### Added

- `kb` engine (Rust library + executable) with commands `init`, `doctor`, `validate`,
  `index`, `context`, `search`, `show`, `sync`, `impact`, `integrate`, `migrate`,
  `update {check,prepare,divergence,abandon}`, `schema` and `version`.
- Strict typed knowledge format: Markdown with `+++` TOML front matter, eight record kinds
  (policy, feature, invariant, contract, decision, procedure, reference, gap), typed
  normative statements with conditions and exceptions, scope/selectors separation,
  `requires`/`rationale`/`related`/`supersedes` links, provenance anchors, version
  applicability, overridable policy settings with authority and weakening checks.
- Project registries (owners, repos, modules, features, concepts) with multilingual,
  normalized aliases; generated JSON Schemas in `core/schemas/` with conformance tests.
- Deterministic context assembly: mandatory obligations selected by scope independently of
  full-text ranking, transitive `requires` closure across repositories, integer ranking with
  id tie-breaks, whole-unit packing into byte or estimated-token budgets,
  `CONTEXT_BUDGET_EXCEEDED`, completeness statuses, ambiguity reporting, receipts and an
  explain mode; compact, human and versioned JSON (`kb.cli.v1`) output.
- Per-call freshness check of the approved ref into an isolated bare mirror, explicit
  snapshot selection (latest, pinned, revision, working tree), `UPDATE_REQUIRED` for lagging
  host pins and incompatible engines, proposal overlays for local changes, host detection
  including submodules and linked worktrees.
- Content-addressed SQLite FTS5 index with transactional incremental builds, corruption
  recovery and a working-tree stat cache.
- `kbw` POSIX launcher: fingerprinted runtimes, explicit source bootstrap with the pinned
  toolchain, digest-verified release artifact installation, release packaging.
- Canonical `kb` skill and generated integrations for Claude Code, Codex and Cursor with
  managed instruction blocks and conflict detection.
- Migration registry (synthetic legacy schema 0 → 1), reviewable upstream updates in
  isolated worktrees, engine divergence checks, impact analysis with unknown-coverage
  reporting and structured `kb-impact` acknowledgements.
- Templates for projects, all record kinds, GitHub/GitLab CI, merge requests, ownership and
  security; a synthetic multi-repository example; opt-in maintainer knowledge.
- GitHub Actions upstream CI and release workflows; benchmark crate `kb-bench`.
- Apache License 2.0 and repository metadata for [4wl2d/kb](https://github.com/4wl2d/kb).

### Fixed

- Use explicit launcher validation branches compatible with ShellCheck 0.9.0 on Ubuntu
  and newer versions, preserving artifact checks and usage errors.
