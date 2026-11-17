# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses semantic
versioning for `engine_version`; contract versions (document schema, CLI protocol, index
schema, skill protocol) are independent integers in `core/release.toml`.

## [0.1.0] - Unreleased

First complete version of the upstream repository. No release has been published yet;
the release workflow builds archives when an owner pushes a `v0.1.0` tag.

### Agent CLI (unreleased)

kb becomes the single CLI between a coding agent, the code, the project knowledge and the
code tools ([ADR 0016](docs/adr/0016-agent-cli.md), guide `docs/agent-cli.md`).

- New commands `sym`, `callers`, `doc`, `find`, `check` and `tools`; `impact <symbol>` adds
  a symbol mode next to the unchanged diff mode.
- Every agent command returns one bounded answer, `kb.answer.v1` inside the unchanged
  `kb.cli.v1` envelope: sources with tool, version and state, prioritized items, notes and
  follow-ups; text output is at most 30 lines (`--lines N`: minimum 5, values above 200
  are capped with a note) of at most 200 characters. With `--json` the envelope of an
  answer is written on one line; every other envelope stays pretty-printed.
- Tool registry `<profile>/tools.toml` (schema 1, `core/schemas/tools.v1.schema.json`) with
  engine-owned adapters `codegraph` (live index, incremental sync before each query; kb
  never creates the index), `code-provider` (any `kb.code.v1` provider, commit-bound),
  `ripgrep`, `git-grep` and `command`. Ordered routes per capability with reported
  fallbacks; built-in defaults codegraph → rg → git grep without a file; `--tool <id>`
  forces one tool. The registry is read from the KB working tree; it is not a plugin system.
- `kb check` runs the configured `command` tools whose globs match the changed files (argv,
  no shell, deadline and output limits), validates changed knowledge and, with `--base` in a
  host repository, evaluates declared probes; `--plan` runs nothing. Records are never
  executed.
- `kb doctor` adds the checks `tools` and `code-index`.
- The generic `exec` adapter connects any command-line tool by configuration: an argv
  template (`{subject}`, `{args}`, `{file}`, `{root}`, ...), an optional `prepare` step,
  `ok_exit`, `env`, and an output mapping (`lines`, `grep`, or JSON with an `items` path
  using `*` wildcards, `fields` and `strings`, or the native `{"items": [...]}` protocol).
  `exec` tools join built-in routes or declare project capabilities called with
  `kb run <capability> [args]`; `routes` accepts any request name. `kb tools` and
  `kb doctor` list project capabilities.
- `kb read <path> [FIRST [LAST]]` prints numbered source lines (40 by default, at most
  80); it marks the end of the file and clamps a continuation to it.
- Misses are final: `kb sym` answers a name that is used while no line of the checkout
  reads as its declaration with up to five of the files that use it (`reference` items),
  lists the lines that could declare it without a keyword as `possible` definitions, or
  says that it does not occur (as a whole word, as written; `kb find` also matches parts
  of words and, for a text in lower case, any letter case); `kb find` and `kb read` name
  the number of files they covered. A definition's follow-up reads its span as far as
  one excerpt reaches.
- `kb doc` prints by default the source location or a `kb read` command, the ids and
  titles of the records that apply to or cite the path, document anchors and
  repository-wide `guidance` ids; statements are read with `kb show <id>` or `kb context`,
  and `--format human` and `--json` keep the feature detail. The records that cite a path
  as a source, test or document are listed even when their scope does not cover it.
- `kb show` adds the declaring line of an anchored symbol and marks anchored files that
  are not in the checkout (`anchor_targets` in JSON).
- `kb outline --format terse` writes each inclusion reason and kind once with its ids.
- One call instead of two: `kb sym` attaches the lines of the single definition it found
  (up to one 80-line `read` window) or the head of each of up to four definitions,
  `kb find` the lines around up to four matches and `kb callers` the lines around up to
  four call sites (`excerpts[]` in JSON, at most 80 lines and 32 KiB per answer, outside
  the `--lines` bound; an import line is listed and not shown); `kb read` takes several
  `FILE FIRST LAST` ranges in one call (up to 12 ranges of at most 80 lines, 240 lines
  per call); an item's code text keeps 120 characters after a long path, and the text of
  a line that a text search matched is at most 500 characters, in JSON too.
- `kb find` answers likely slips instead of missing: globs that select no file are
  retried without regard to case and as directory names; a name or phrase of two or three
  words, each of at least three letters, that no line reads gets up to ten lines that
  hold its words; a miss inside one file lists that file's outline (`declaration` items):
  the headings of a Markdown file, the keyword-led declarations of any other. Its text
  output writes each path once, also for ranked identifier searches.
- `kb read` accepts `FIRST LAST` alone for a further range of the file named last.
- The `kb` skill's procedure for investigating without editing starts with `search`,
  cites the lines that `sym`, `callers` and `find` print, puts every nameable lookup into
  one shell call, reports record statements as written, says that accepted records apply
  where general guidance from a connected tool disagrees, and asks for claims that keep
  every condition, exception and case within the length the question allows.
- `kb search` lists, after the records, up to eight code files of the host checkout whose
  names read like the query (`files[]`, `files_total`), each with the line on which it
  declares the name it carries when it declares one, so a search in the words of a task
  names the code to open and where to start reading. Forms of one word match
  (`entry`/`entries`, `config`/`configuration`); words that merely begin alike do not
  (`list`/`listener`). The default `search` text lists `id [kind, status] title`; scores
  and match reasons are in `--format human` and `--json`.
- `kb show` gives an anchored document its length and the lines where it has the sections
  its anchor note names and mentions the code names of the record
  (`{state, lines, mentions[]}` in `anchor_targets`).
- More final misses: a declaration-shaped `kb find` text that misses is answered for the
  name (its declaration, a line that may declare it, its uses, or that it does not
  occur); `kb callers` and `kb impact` say when a symbol occurs nowhere. A name that is
  used without a recognisable declaration they answer from text with a note that says so
  (exit 0) when the route holds a text tool, as the default routes do, and with
  `reference` items (`NOT_FOUND`) only when none answers from text. `callers` and `sym`
  no longer answer `Type.member` with the same-named member of another type when no line
  of the checkout reads as a declaration of `Type`: `callers` keeps the lines that write
  the dotted name, a member declared with the dotted name (`fun Type.member`) is its
  definition, and a dotted name that no line writes while `member` occurs is
  answered `NOT_FOUND` with the lookups of `member` as follow-ups.
- `kb find` through ripgrep searches hidden files too (never `.git`); `kb read` opens any
  existing file given by its exact path, ignored or not.
- The process runner retries a spawn briefly while the executable is busy (`ETXTBSY`),
  which also removes a race in the provider deadline test on Linux.
- CI and release builds use the `macos-15` runner; GitHub no longer runs `macos-14`. Upstream
  CI runs once per pull request commit (pushes only on `main`, plus `workflow_dispatch`), and
  one `ci` job aggregates the platform checks as the single required check.
- The generated bundle is a skill set installed side by side: `kb`, `kb-change`,
  `kb-review` and `kb-document`, with one `.claude/commands/<skill>.md` alias per skill for
  mixed Claude installs, the template placeholder `{{skills_dir}}` and the always-on block
  "Code, knowledge and tools (kb)". Skill protocol 3.
- Error codes `CHECKS_FAILED` (46) and `TOOL_UNAVAILABLE` (65); agent commands use
  `NOT_FOUND` (16) for an unknown symbol or no match and still print the answer.
- Validation: warning `FEATURE_TOO_LONG` for feature records over 60 lines (a feature
  record is the compact per-feature document with entry-point anchors) and error
  `TOOLS_INVALID` for an invalid registry; the feature template shows an entry-point anchor.
- `kbw init` writes `project/tools.toml`; the synthetic example and the maintainer profile
  ship a registry with checks.

CLI protocol stays `kb.cli.v1` (version 1; the additions are additive), document schema
and index schema stay 2, the engine stays 0.1.0.

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
