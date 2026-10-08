# kb

Typed, verifiable engineering knowledge for coding agents and people.

kb is a Rust CLI, run through the project launcher `./kbw`, that assembles
**task-scoped, freshness-checked context** from a knowledge base kept in Git: the policies,
invariants, contracts and known gaps that apply to the code you are about to change, their
required dependencies, and a small ranked set of background records. It returns the
applicable knowledge itself, with provenance and a completeness status, not a list of
files to read.

## What kb is and is not

kb is:

* **One strict authoring format.** Markdown files with TOML front matter between `+++`
  lines, eight record kinds (policy, feature, invariant, contract, decision, procedure,
  reference, gap), atomic typed obligations with conditions and exceptions, and project
  registries of repositories, modules, features and multilingual concepts.
* **Deterministic context assembly.** Obligations are selected by scope, independently of
  full-text ranking; `requires` dependencies are followed across repositories; ranking
  breaks ties by id; whole units are packed into a byte or estimated-token budget. Every
  answer says whether it is `complete`, `partial`, `conflict` or `incomplete` and carries a
  receipt.
* **Fresh by construction.** Every `context`, `show`, `search` and `impact` call fetches the
  configured approved ref into an isolated mirror. There is no TTL and no silent offline
  fallback; `--offline` is explicit and reported as unverified.
* **Local and reproducible.** An embedded SQLite FTS5 index under `.cache/` is a rebuildable
  cache; the source of truth is Git-tracked text. Each checkout runs its own engine through
  `kbw`.
* **Evidence-backed domain knowledge.** Schema 2 adds subsystem states/scenarios, contract
  consumers, glossary, validity and source stamps; authoring commands prepare drafts and
  keep acceptance in human review.
* **Agent integration by text.** One generated skill and native instruction targets serve
  Claude Code, Codex, Cursor, Grok Build, GitHub Copilot and Junie. Verified core/receipt
  reuse and terse output reduce repeated delivery without dropping obligations.

kb is not:

* an MCP server, daemon, web UI, telemetry uploader or plugin system;
* an AI: no LLM calls at runtime, no embeddings, no vector database. The CLI creates and
  validates structure and serves context; deciding what the knowledge is remains work for
  people or an external coding agent ([BOOTSTRAP.md](BOOTSTRAP.md));
* a proof of truth: a valid schema does not make a record true, a receipt proves delivery
  rather than understanding or compliance, and typing does not remove natural-language
  prompt injection. Trust comes from review into the approved ref ([SECURITY.md](SECURITY.md));
* a global tool: there is no `kb` on `PATH` to install or keep in sync.

Optional [code adapters](core/providers/README.md) supply commit-bound static facts outside
the engine, and the separate [replay kit](core/eval/README.md) runs explicitly authorized
evaluations. Local delivery observations contain ids/scopes/costs, not task text, and are
never uploaded by the engine. These tools and passing tests do not prove a quality gain;
held-out acceptance and release gates are tracked in [the upgrade record](docs/upstream-upgrade.md).

## Distribution model

* This repository is the **upstream**: engine, schemas, migrations, templates, skills,
  adaptation instructions and release machinery. It contains no project data; `project/`
  holds only a README.
* Each product creates **one downstream** fork or private copy that keeps the upstream
  history and adds its knowledge in `project/`. One downstream can describe many
  repositories (mobile, backend, libraries, shared contracts).
* Host repositories mount the downstream as a Git submodule (conventionally `.kb`) or use a
  separate checkout, and run `.kb/kbw`.
* Engine changes come from upstream through a reviewed `kbw update`; project adaptation
  happens in `project/`. `kbw update divergence` reports unintended engine edits. This
  README is itself engine-owned: in a downstream, the product's own README is
  `project/README.md`.

Details: [docs/downstream.md](docs/downstream.md) and [ADR 0001](docs/adr/0001-distribution-model.md).

## Repository layout

| path | content |
|---|---|
| `kbw` | POSIX sh launcher: runtime selection, explicit source bootstrap, verified artifact install |
| `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` | workspace and pinned toolchain |
| `core/cli/` | crate `kb`: library and `kb` executable |
| `core/schemas/` | generated JSON Schemas (drift-checked) |
| `core/migrations/` | migration documentation and synthetic legacy fixtures |
| `core/templates/` | project skeleton, record templates, CI and MR templates, synthetic example |
| `core/skills/` | canonical skill instructions and references |
| `core/maintainer-knowledge/` | opt-in knowledge about kb itself (`--profile maintainer`) |
| `core/tests/`, `core/benchmarks/` | shared test fixtures; corpus generator and measurements |
| `core/providers/`, `core/eval/` | optional static code adapters; isolated replay and paired analysis |
| `core/release.toml` | bootstrap and compatibility manifest (versions, toolchain, engine-owned paths) |
| `project/` | project-owned knowledge (created by `kbw init` in a downstream) |
| `docs/` | user guides, the [architecture](docs/architecture.md) contract, [ADRs](docs/adr/) and [prompts](docs/prompts/); index: [docs/README.md](docs/README.md) |
| `.cache/` | generated runtimes, Git mirror, SQLite index; git-ignored |

## Quickstart from a clean checkout

Prerequisites: Git ≥ 2.38, rustup (it installs the toolchain pinned in
`rust-toolchain.toml`), a POSIX shell, and `sha256sum` or `shasum`.

### 1. Bootstrap and check the upstream

```sh
./kbw --kbw-bootstrap                               # explicit: builds this checkout's runtime
./kbw version
./kbw validate --profile maintainer --templates     # the checks a clean upstream must pass
./kbw context --intent explain --task "anything"    # no project yet: exit 10
```

`--kbw-bootstrap` runs `cargo build --release --locked -p kb` with the pinned toolchain and
activates `.cache/runtime/<fingerprint>/kb` by an atomic rename. The fingerprint covers the
engine build inputs, so editing the engine requires a new bootstrap while knowledge commits
do not. Normal commands never build: without a runtime they fail with
`kbw: error[KBW_RUNTIME_NOT_BOOTSTRAPPED]: ...` and exit status 50 (CI may opt in to
implicit builds with `KBW_AUTO_BOOTSTRAP=1`). The fingerprint is compiled into the binary:
`./kbw version` ends with `build <fingerprint>` and `./kbw --json version` reports it as
`build_fingerprint`.

Historical schema-1 baseline excerpt (current counts and receipts change with the corpus):

```text
$ ./kbw validate --profile maintainer --templates
validate maintainer: records: 14, files: 14, errors: 0, warnings: 0
templates: checked (shipped templates and examples): 0 error(s), 0 warning(s)
routing tests: 3 passed, 0 failed
ok   core/maintainer-knowledge/routing-tests/core.toml: changing the index pulls determinism and completeness obligations [complete]
ok   core/maintainer-knowledge/routing-tests/core.toml: snapshot work pulls the read-only invariant [complete]
ok   core/maintainer-knowledge/routing-tests/core.toml: skill template changes include the protocol contract and the known gap [complete]

$ ./kbw context --intent explain --task "anything"
error[PROJECT_NOT_INITIALIZED]: project is not initialized: `project/project.toml` does not exist
  hint: run `kbw init --name <name> --namespace <ns>` (dry-run) and then with `--apply`
```

### 2. Try the synthetic example in a scratch clone

The example is an invented multi-repository product (`mobile`, `backend`,
`shared-contracts`; namespace `example`). Use a scratch clone so that this checkout stays
clean. Everything below is local; the "approved origin" is a bare repository next to it.

```sh
git clone . ../kb-demo && cd ../kb-demo
./kbw --kbw-bootstrap         # each checkout has its own runtime; optionally reuse compiled
                              # dependencies: KBW_CARGO_TARGET_DIR="$OLDPWD/.cache/cargo-target"
./kbw init --example synthetic-multirepo            # dry-run: prints the plan
./kbw init --example synthetic-multirepo --apply    # writes project/ and the CI workflow
./kbw validate                                      # records, links, policies, 7 routing tests
git add project .github/workflows/kb-knowledge.yml
git commit -m "Synthetic example"
git remote rename origin upstream
git clone --bare . ../kb-demo-origin.git            # the local approved source
git remote add origin "$(cd .. && pwd)/kb-demo-origin.git"
./kbw context --intent implement --task "Retry the token refresh after a network error" \
  --repo mobile --path mobile:app/auth/TokenRefresher.kt
```

The example's `project/project.toml` allows the `file` transport for exactly this purpose;
real projects normally allow only `https` and `ssh`. Excerpt of the result (the progress
lines go to stderr; paths shortened):

```text
kb: checking refs/heads/main on origin (<...>/kb-demo-origin.git)
kb: snapshot 0722edbb3d33 (latest, freshness=verified) (key 649311a9dd022c3e)
kb: index: built snapshot 649311a9dd022c3e (21 record files, 21 parsed, 0 reused, 0 proposals, 0 errors, 0 warnings)
kb: context: complete (5 mandatory, 3 supplementary units)
# kb context intent=implement engine=0.1.0 skill_protocol=1 protocol=kb.cli.v1
snapshot: 0722edbb3d33 (latest, freshness=verified); approved=yes; ref=origin refs/heads/main; source=<...>/kb-demo-origin.git; latest=0722edbb3d33; key=649311a9dd02
host: repo=unknown; head=unknown; versions=unknown
scope: repos=mobile; modules=mobile.auth; features=login; concepts=auth-token (alias `token refresh`)
path: mobile:app/auth/TokenRefresher.kt -> modules mobile.auth
status: COMPLETE
effective settings:
- example.common.code-review#min-reviewers = 1 (base)
- example.common.code-review#require-green-ci = true (base)
### mandatory example.common.code-review (policy, accepted): Code review and knowledge impact for every change
...
### mandatory example.contract.token-refresh (contract, accepted): Token refresh API between mobile and backend
why: applies (modules); also required by example.mobile.token-storage; source: project/knowledge/contracts/token-refresh.md
scope: modules=mobile.auth,backend.api
party provider: repo backend (backend.api); role: Issues and rotates tokens
party consumer: repo mobile (mobile.auth); role: Refreshes tokens before they expire
interface: POST /v2/auth/refresh (synthetic)
- provider MUST [rotate-on-refresh] Issue a new refresh token on every successful refresh and invalidate the previous one.
- provider SHOULD [grace-window] Accept the previous refresh token for 30 seconds after rotation.
  if: the previous token was issued to the same device
- consumer MUST [replace-atomically] Replace the stored refresh token only after the refresh response was fully received.
  except [logout-in-flight]: If the user logged out while the request was in flight, discard the response instead.
requires: example.contract.error-envelope
### supplementary example.feature.login (feature, accepted): Login: sign-in and silent session refresh
...
-- receipt sha256:d0d1cb91...; included: mandatory 5, dependencies 0, proposals 0, supplementary 3, sections 0; excluded by budget: none; other exclusions: 5 (--explain); budget: 1767/8000 tokens-est (estimate, not a tokenizer count), measured as compact
-- a receipt proves delivery, not understanding or compliance; re-request context after compaction, a new session, a hand-off, or a scope or snapshot change
```

The backend contract reaches this mobile task through scope and `requires`, while
backend-only records stay out. The same request in machine form (`--json` prints exactly
one `kb.cli.v1` envelope on stdout):

```text
$ ./kbw --json context --intent implement --task "Retry the token refresh after a network error" \
    --repo mobile --path mobile:app/auth/TokenRefresher.kt 2>/dev/null \
  | jq '{ok, completeness: .result.completeness, freshness: .result.snapshot.freshness,
         mandatory: [.result.units[] | select(.tier == "mandatory") | .id], receipt: .result.receipt.id}'
{
  "ok": true,
  "completeness": "complete",
  "freshness": "verified",
  "mandatory": [
    "example.common.code-review",
    "example.mobile.token-storage",
    "example.mobile.single-refresh",
    "example.contract.error-envelope",
    "example.contract.token-refresh"
  ],
  "receipt": "sha256:8af47958..."
}
```

Before the commit and the bare origin exist, the same knowledge can be read from the working
tree with `--offline --snapshot working-tree`; that answer is `partial` and exits with 30,
because the working tree is not approved and freshness is unverified. More requests with
their expected ids: `core/templates/examples/synthetic-multirepo/expected-queries.md`.

To go further: mount the demo KB into a host repository and install the agent integration
([docs/downstream.md](docs/downstream.md)), or start a real downstream with
[BOOTSTRAP.md](BOOTSTRAP.md).

## CLI walkthrough

Every command runs as `./kbw <command>` (or `<kb_path>/kbw` from a host repository). Commands
that change files are dry-runs unless given `--apply`.

| command | what it does | example |
|---|---|---|
| `init` | Create schema-2 project/domain scaffolding, GitHub/GitLab KB CI and review templates, or the synthetic example; preserve existing files | `./kbw init --name "<Product>" --namespace <ns> --apply` |
| `doctor` | Check environment, configuration, runtime, approved source, host binding and pin, engine compatibility of the approved tip and the pin, index, skill, bundle, engine divergence (`--online` also fetches; not with `--offline`) | `.kb/kbw doctor --online` |
| `validate` | Records, links, policies, registries and routing fixtures (`--strict`, `--base <rev>`, `--no-routing`, `--templates`) | `./kbw validate --base origin/main` |
| `index` | Build or update the derived index of the selected snapshot without contacting the remote (`--rebuild`, `--gc`) | `./kbw index --rebuild` |
| `context` | Task-scoped context: `--intent` (required), `--task`, repeatable `--repo`, `--path`, `--module`, `--feature`, `--concept`, `--host-version`; `--budget`, `--budget-unit tokens-est\|bytes`, `--sections none\|mandatory\|all`, `--max-supplementary`, `--include-proposals`, `--explain` | `.kb/kbw context --intent implement --task "..." --path src/x.kt` |
| `context` additions | Path-free `diagnose`, `--changed --base`, `--change-type`, historical `--as-of`, explicit receipt/core reuse, `--with-code` | `.kb/kbw context --intent review --changed --base origin/main --format terse` |
| `outline` | Bounded inventory; request context before editing | `.kb/kbw outline --intent diagnose --task "..."` |
| `coverage`, `propose`, `capture` | Prioritize domain gaps, prepare work orders and validate/apply drafts; never accept them | `.kb/kbw propose begin --from-change BASE..HEAD --json` |
| `anchors`, `drift`, `ledger` | Stamp/check Git evidence, owner review queues, freshness and seeded audits | `.kb/kbw anchors check --strict --json` |
| `verify` | Declared regex/glob/static-import probes; unknown evidence remains unknown | `.kb/kbw verify --diff BASE --head HEAD --json` |
| `usage report` | Private delivery metadata joined to a task's final diff | `.kb/kbw usage report --diff BASE --receipt ID --json` |
| `eval routing`, `eval history` | Ordered/budgeted routing metrics and labeled historical coverage | `./kbw eval routing --example synthetic-multirepo --json` |
| `search` | Ranked search; not a substitute for `context` (`--kind`, `--limit`, `--include-proposals`); like `show` and `impact`, its text output starts with a snapshot provenance line (revision, selection, freshness, approval, approved tip, pin) | `./kbw search "идемпотентность"` |
| `show` | A record, or one section (`--section` or `id#section`); `--raw` prints only the file bytes (the provenance line then goes to stderr when the content is unverified or not approved) | `./kbw show example.contract.payment-intent` |
| `sync` | Fetch the approved ref into the isolated mirror; report the tip, the local checkout and the host pin; changes nothing else | `.kb/kbw sync` |
| `impact` | Relate a host diff (`--base`, `--head`, `--working-tree`) to affected knowledge and unknown coverage; `--check --statement <file>` verifies the `kb-impact` block | `.kb/kbw impact --base origin/main --working-tree` |
| `integrate` | `--generate`: render the skill bundle in the KB; without it: install the bundle into the host; `--check`, `--apply`, `--force` | `.kb/kbw integrate --apply` |
| `migrate` | Show (unified diff) or `--apply` supported document schema migrations (`--to <n>`) | `./kbw migrate` |
| `update check` | Compatibility and merge prediction for an upstream ref; writes nothing to the repository | `./kbw update check --upstream <url> --ref <tag>` |
| `update prepare` | Reviewable `kb-update/<ref>` branch in an isolated worktree: merge, bootstrap, migrate, integrate, validate; never pushes | `./kbw update prepare --upstream <url> --ref <tag>` |
| `update divergence` | Engine-owned paths that differ from the recorded upstream base | `./kbw update divergence` |
| `update abandon` | Show (dry-run) or, with `--apply`, remove a `kb-update/*` branch and its worktree; `--apply --force` also discards uncommitted files there | `./kbw update abandon kb-update/<ref> --apply` |
| `schema` | Check (`--check`) or write (`--write`) the generated JSON Schemas | `./kbw schema --check` |
| `version` | Engine and contract versions and the build fingerprint | `./kbw version` |

Global options: `--root`, `--config`, `--profile project|maintainer`,
`--format compact|terse|human|json` (`--json`), `--offline`, `--snapshot
auto|latest|pinned|working-tree|<revision>`, `--host`, `--quiet`, `--skill-protocol <n>`.
Unknown arguments are rejected.

Output: compact text for agents (default), `--format human`, or `--json`. In JSON mode
stdout carries only the protocol envelope `{protocol, command, ok, result, error, meta}`;
progress and diagnostics go to stderr. Errors have stable symbolic codes and grouped exit
codes:

| exit | group | examples |
|---|---|---|
| 0 | success (`context`: completeness `complete`) | |
| 1, 2 | internal error, usage | `USAGE` |
| 10–19 | project and configuration | `PROJECT_NOT_INITIALIZED` 10, `UNKNOWN_SCOPE` 15 |
| 20–29 | freshness and snapshots | `FRESHNESS_UNVERIFIED` 20, `UPDATE_REQUIRED` 21, `SKILL_OUTDATED` 23 |
| 30–39 | context | `CONTEXT_INCOMPLETE` 30 (full result printed), `CONTEXT_BUDGET_EXCEEDED` 31 |
| 40–49 | checks | `VALIDATION_FAILED` 40, `ROUTING_TESTS_FAILED` 41, `DRIFT_DETECTED` 42, `IMPACT_UNACKNOWLEDGED` 44 |
| 50–59 | runtime and update | `RUNTIME_INCOMPATIBLE` 50, `UPDATE_CONFLICT` 51 |
| 60–69 | environment | `GIT_ERROR` 60, `INDEX_ERROR` 61, `UNSAFE_PATH` 63 |

The full list and the JSON envelope are in [docs/protocol.md](docs/protocol.md); what to do
for each code is in [docs/troubleshooting.md](docs/troubleshooting.md).

Launcher flags (recognized only as the first argument; errors are `kbw: error[CODE]: ...`
on stderr with exit 50, or 2 for usage):

| flag | action |
|---|---|
| `--kbw-help` | launcher help |
| `--kbw-bootstrap` | build and activate the runtime from source (pinned toolchain, `Cargo.lock`) |
| `--kbw-runtime-info` | fingerprint, runtime path, status, source, engine version, target |
| `--kbw-fingerprint` | engine fingerprint of this checkout |
| `--kbw-install-artifact <archive-or-https-url> --sha256 <hex>` (or `--sha256-file <SHA256SUMS>`) | verify the digest before extraction, check `BUILD-INFO` against this engine, smoke-test and activate atomically |
| `--kbw-package <out-dir>` | write `kb-<version>-<target>.tar.gz` and its `.sha256` |

The release workflow (`.github/workflows/release.yml`) builds archives for macOS arm64 and
Linux x86_64 when an owner pushes a `v<engine_version>` tag. No release has been published
yet; the source bootstrap works without one. Downstream forks inherit this workflow and the
upstream CI (`upstream-ci.yml`); they turn them off without editing them by setting the
repository variables `KB_RELEASE=disabled` and `KB_ENGINE_CI=disabled`
([docs/downstream.md](docs/downstream.md#11-what-administrators-must-configure)).

## Documentation

Start at the documentation index, [docs/README.md](docs/README.md). The main entry points:

| document | for |
|---|---|
| [BOOTSTRAP.md](BOOTSTRAP.md) | turning a fresh downstream into a project KB; includes the AI adaptation prompt |
| [docs/downstream.md](docs/downstream.md) | forking, mounting, host integration, CI/MR workflow, upgrades, offline recovery, administrator settings |
| [docs/prompts/adaptation.md](docs/prompts/adaptation.md), [docs/prompts/maintenance.md](docs/prompts/maintenance.md) | ready-to-paste agent prompts |
| [docs/format.md](docs/format.md) | writing records: kinds, scope, selectors, links, policies, registries, diagnostics |
| [docs/context.md](docs/context.md) | what `context` returns and why: selection, ranking, budgets, completeness, receipts |
| [docs/knowledge-lifecycle.md](docs/knowledge-lifecycle.md) | domain drafts, provenance, drift, freshness, declarative checks and local usage |
| [docs/evaluation.md](docs/evaluation.md), [core/eval/README.md](core/eval/README.md) | routing/history evidence and the optional isolated replay kit |
| [docs/snapshots-and-trust.md](docs/snapshots-and-trust.md) | freshness, snapshot selection, host detection, caches, trust model |
| [docs/protocol.md](docs/protocol.md), [docs/troubleshooting.md](docs/troubleshooting.md) | JSON protocol and error codes; what to do for each error |
| [docs/architecture.md](docs/architecture.md), [docs/adr/](docs/adr/) | the normative engineering contract and design decisions |
| [docs/maintainer-guide.md](docs/maintainer-guide.md), [AGENTS.md](AGENTS.md), [CONTRIBUTING.md](CONTRIBUTING.md) | changing, testing and releasing the engine |
| [SECURITY.md](SECURITY.md), [CHANGELOG.md](CHANGELOG.md), [docs/dependencies.md](docs/dependencies.md) | security model, changes, dependencies and licenses |
| [core/templates/README.md](core/templates/README.md), [core/skills/README.md](core/skills/README.md), [core/schemas/README.md](core/schemas/README.md) | templates, skills, schemas |

## License

Copyright 2026 kb contributors.

Licensed under the Apache License, Version 2.0; see [LICENSE](LICENSE).
Third-party dependency licenses are listed in [docs/dependencies.md](docs/dependencies.md).
