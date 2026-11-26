# kb

One CLI between a coding agent, the code, the project knowledge and the code tools.

kb is a Rust CLI, installed once and run as `kb` from any repository on macOS, Linux or
Windows, through which an AI coding agent (Claude Code or any other) reaches the codebase,
the project documentation and the code-analysis tools. The agent does not choose between CodeGraph, `grep`, file search and
other tools: it asks kb, kb routes the request to the backend its code and configuration
name, and the answer comes back as structured lines that name their source, at most 30 by
default, with the source lines of a few located places attached. The agent finds what it
needs quickly, knows the architectural constraints before it edits, and does not spend
tokens searching unstructured output.

```text
coding agent  --skills-->  kb: routing · tool registry · adapters · bounded answers
                              |- CodeGraph        symbols, callers, impact
                              |- knowledge        features, invariants, contracts, gaps
                              `- adapters         search, checks, kb.code.v1 providers
```

| the agent needs | command |
|---|---|
| a symbol's definition | `kb sym <name>` |
| the code that calls it | `kb callers <symbol>` |
| what changing it affects, in code and knowledge | `kb impact <symbol>` |
| where a path or symbol is and which records document it | `kb doc <path\|symbol>` |
| numbered lines of a source file | `kb read <path> [FIRST [LAST]]` |
| text in the repository | `kb find "<text>"` |
| the checks for changed files | `kb check [files] [--base <target>]` |
| every obligation before an edit, with a receipt | `kb context --intent implement --path <file>` |
| any other connected tool (project capability) | `kb run <capability> [args]` |
| the connected tools; the environment | `kb tools`; `kb doctor` |

Behind the agent surface sits a typed knowledge engine: it assembles **task-scoped,
freshness-checked context** from a knowledge base kept in Git — the policies, invariants,
contracts and known gaps that apply to the code about to change, their required
dependencies, and a small ranked set of background records — with provenance and a
completeness status. The agent CLI is described in [docs/agent-cli.md](docs/agent-cli.md)
and decided in [ADR 0016](docs/adr/0016-agent-cli.md).

## Install and update

`kb` is one native binary for macOS (Apple silicon), Linux (x86_64 and arm64, statically
linked) and Windows (x86_64, also on Windows on Arm under emulation). It runs from any
shell, including PowerShell and cmd, and one installation serves every KB checkout of the
user.

macOS and Linux:

```sh
curl -fsSLO https://raw.githubusercontent.com/4wl2d/kb/main/install.sh
less install.sh                      # read it before running it
sh install.sh                        # latest release into ~/.local/bin (--dir, --version, --force)
```

Windows (PowerShell 5.1 or 7):

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/4wl2d/kb/main/install.ps1 -OutFile install.ps1
Get-Content install.ps1              # read it before running it
powershell -ExecutionPolicy Bypass -File install.ps1   # into %LOCALAPPDATA%\Programs\kb\bin, added to the user PATH
```

The installers download `kb-<version>-<target>.tar.gz` and `SHA256SUMS` from the GitHub
release, verify the digest and the archive entries, check that the binary runs and refuse to
replace an existing `kb` that is not kb (a wrapper script, say) unless forced. From source,
on any platform with Rust: `cargo install --locked --git https://github.com/4wl2d/kb kb`
(or `cargo install --locked --path core/cli` in a checkout). Releases appear when the owner
tags one; until the first release, install from source.

Update with `kb self update` (the latest release; `--version <x.y.z>`, `--check`): it
verifies the release the same way and replaces the installed binary by rename.

Updating `kb` never changes which engine reads a checkout. In a KB checkout, or in a host
repository that mounts one at `.kb`, `kb` computes the fingerprint of the checkout's engine
sources. When the installed `kb` was built from exactly those sources it runs the command
itself; otherwise it runs that engine's runtime from a per-user store. A missing runtime is
an error that names the commands providing it: `kb engine install` (the release archive of
the checkout's engine version) or `kb engine bootstrap` (a source build with the pinned
toolchain). `kb engine info` shows which engine serves the checkout. Upgrading a downstream
to a newer upstream is a different operation (`kb update`). Design: [ADR 0017](docs/adr/0017-kb-command.md).

Coming from `kbw`: install `kb`, then run `kb ...` where you ran `./kbw ...` or
`.kb/kbw ...`; runtimes `kbw` already built are reused. `kbw` remains for one release as a
deprecated forwarder to `kb`, and a host binding `.kbw.toml` is still read (rename it to
`.kb.toml`). Details: [docs/downstream.md](docs/downstream.md#moving-from-kbw-to-kb).

## What kb is and is not

kb is:

* **One interface for agents.** `sym`, `callers`, `impact`, `doc`, `find`, `check`, `run`
  and `tools` return one common answer (`kb.answer.v1`): sources with tool version and
  index state, items with paths, lines, symbols and kinds, notes and follow-ups, bounded to
  30 lines by default (`--lines`), and the source lines of a few located places; `read`
  prints numbered lines of a file. Skills (`kb`, `kb-change`, `kb-review`, `kb-document`)
  call these commands; they never name a tool.
* **An umbrella for any tool, routed by code and configuration.** A tool registry
  (`project/tools.toml`) connects tools through engine adapters (CodeGraph's live index, any
  `kb.code.v1` provider, ripgrep, the built-in `git grep`, check commands) and through the
  generic `exec` adapter, which wraps **any** command-line tool with an argv template and
  an output mapping (lines, grep format, or JSON with field mapping). An `exec` tool joins a
  built-in route or adds a project capability (`kb run <capability>`). Missing tools,
  absent indexes and failures fall back to the next tool and are reported. A new tool is a
  registry entry; skills do not change.
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
  `kb`.
* **Evidence-backed domain knowledge.** Schema 2 adds subsystem states/scenarios, contract
  consumers, glossary, validity and source stamps; authoring commands prepare drafts and
  keep acceptance in human review.
* **Agent integration by text.** One generated skill and native instruction targets serve
  Claude Code, Codex, Cursor, Grok Build, GitHub Copilot and Junie. Verified core/receipt
  reuse and terse output reduce repeated delivery without dropping obligations.

kb is not:

* an MCP server, daemon, web UI, telemetry uploader or plugin system: the tool registry
  configures engine-owned adapters and loads no code. kb does start the external tools the
  registry names (CodeGraph, ripgrep, a `kb.code.v1` provider) and runs the check commands
  written there, argv only, never anything a knowledge record supplies;
* a code indexer: it never creates or owns a tool index; CodeGraph keeps its own;
* an AI: no LLM calls at runtime, no embeddings, no vector database. The CLI creates and
  validates structure and serves context; deciding what the knowledge is remains work for
  people or an external coding agent ([BOOTSTRAP.md](BOOTSTRAP.md));
* a proof of truth: a valid schema does not make a record true, a receipt proves delivery
  rather than understanding or compliance, and typing does not remove natural-language
  prompt injection. Trust comes from review into the approved ref ([SECURITY.md](SECURITY.md));
* a global tool: there is no `kb` on `PATH` to install or keep in sync.

The [reference code adapters](core/providers/README.md) supply commit-bound static facts
through the `kb.code.v1` protocol, and the separate [replay kit](core/eval/README.md) runs explicitly authorized
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
  separate checkout, and run the installed `kb`, which reads the checkout with the engine
  built from the checkout's own engine sources.
* Engine changes come from upstream through a reviewed `kb update`; project adaptation
  happens in `project/`. `kb update divergence` reports unintended engine edits. This
  README is itself engine-owned: in a downstream, the product's own README is
  `project/README.md`.

Details: [docs/downstream.md](docs/downstream.md) and [ADR 0001](docs/adr/0001-distribution-model.md).

## Repository layout

| path | content |
|---|---|
| `install.sh`, `install.ps1` | installers of the `kb` command from a GitHub release (macOS/Linux, Windows) |
| `kbw` | deprecated POSIX forwarder to `kb` (removed in a later release) |
| `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` | workspace and pinned toolchain |
| `core/cli/` | crate `kb`: library and `kb` executable (agent surface in `src/cli/`, adapters in `src/tools/`, answers in `src/answer.rs`, engine selection and runtimes in `src/launcher/`) |
| `core/schemas/` | generated JSON Schemas (drift-checked) |
| `core/migrations/` | migration documentation and synthetic legacy fixtures |
| `core/templates/` | project skeleton, record templates, CI and MR templates, synthetic example |
| `core/skills/` | canonical sources of the skill set `kb`, `kb-change`, `kb-review`, `kb-document` and the managed blocks |
| `core/maintainer-knowledge/` | opt-in knowledge about kb itself (`--profile maintainer`) |
| `core/tests/`, `core/benchmarks/` | shared test fixtures; corpus generator and measurements |
| `core/providers/`, `core/eval/` | `kb.code.v1` reference adapters (CodeGraph, ast-index), connected through the tool registry; isolated replay and paired analysis |
| `core/release.toml` | bootstrap and compatibility manifest (versions, toolchain, engine-owned paths) |
| `project/` | project-owned knowledge (created by `kb init` in a downstream) |
| `docs/` | user guides, the [architecture](docs/architecture.md) contract, [ADRs](docs/adr/) and [prompts](docs/prompts/); index: [docs/README.md](docs/README.md) |
| `.cache/` | Git mirror, SQLite index, engine fingerprint stamp, Cargo builds; git-ignored |

## Quickstart from a clean checkout

Prerequisites: Git ≥ 2.38 and rustup (it installs the toolchain pinned in
`rust-toolchain.toml`). Any shell works, including PowerShell and cmd on Windows.

### 1. Build kb from this checkout and check the upstream

```sh
cargo install --locked --path core/cli           # kb built from these engine sources
kb engine info                                   # which engine serves this checkout: this kb
kb version
kb validate --profile maintainer --templates     # the checks a clean upstream must pass
kb context --intent explain --task "anything"    # no project yet: exit 10
```

`kb` reads a checkout with the engine built from the checkout's own engine sources. The
engine fingerprint over the build inputs (`Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`,
`core/release.toml`, `core/cli/`) decides, and every build compiles its fingerprint in:
`kb version` ends with `build <fingerprint>`, `kb --json version` reports
`build_fingerprint`. A `kb` built from this checkout therefore serves it directly. Editing the
engine changes the fingerprint, knowledge commits do not; then the installed `kb` needs a
runtime for the edited sources, and `kb engine bootstrap` builds one (`cargo build --release
--locked -p kb`, pinned toolchain) into the per-user runtime store. While developing the
engine, `cargo run -q -p kb -- <args>` always matches the working tree. Normal commands never
build: without a runtime they fail with `RUNTIME_NOT_BOOTSTRAPPED` (exit 54; CI may opt in to
implicit builds with `KB_AUTO_BOOTSTRAP=1`).

Historical schema-1 baseline excerpt (current counts and receipts change with the corpus):

```text
$ kb validate --profile maintainer --templates
validate maintainer: records: 14, files: 14, errors: 0, warnings: 0
templates: checked (shipped templates and examples): 0 error(s), 0 warning(s)
routing tests: 3 passed, 0 failed
ok   core/maintainer-knowledge/routing-tests/core.toml: changing the index pulls determinism and completeness obligations [complete]
ok   core/maintainer-knowledge/routing-tests/core.toml: snapshot work pulls the read-only invariant [complete]
ok   core/maintainer-knowledge/routing-tests/core.toml: skill template changes include the protocol contract and the known gap [complete]

$ kb context --intent explain --task "anything"
error[PROJECT_NOT_INITIALIZED]: project is not initialized: `project/project.toml` does not exist
  hint: run `kb init --name <name> --namespace <ns>` (dry-run) and then with `--apply`
```

### 2. Try the synthetic example in a scratch clone

The example is an invented multi-repository product (`mobile`, `backend`,
`shared-contracts`; namespace `example`). Use a scratch clone so that this checkout stays
clean. Everything below is local; the "approved origin" is a bare repository next to it.

```sh
git clone . ../kb-demo && cd ../kb-demo            # same engine sources: the same kb serves it
kb init --example synthetic-multirepo            # dry-run: prints the plan
kb init --example synthetic-multirepo --apply    # writes project/, GitHub/GitLab KB CI and review templates
kb validate                                      # records, links, policies, 7 routing tests
git add project .github .gitlab
git commit -m "Synthetic example"
git remote rename origin upstream
git clone --bare . ../kb-demo-origin.git            # the local approved source
git remote add origin "$(cd .. && pwd)/kb-demo-origin.git"
kb context --intent implement --task "Retry the token refresh after a network error" \
  --repo mobile --path mobile:app/auth/TokenRefresher.kt
```

The example's `project/project.toml` allows the `file` transport for exactly this purpose;
real projects normally allow only `https` and `ssh`. Excerpt of the result (the progress
lines go to stderr; paths shortened; commit ids, keys and receipts differ per run):

```text
kb: checking refs/heads/main on origin (<...>/kb-demo-origin.git)
kb: snapshot 4aa0f99cd4c2 (latest, freshness=verified) (key 2bad1f6822381b84)
kb: index: built snapshot 2bad1f6822381b84 (21 record files, 21 parsed, 0 reused, 0 proposals, 0 errors, 0 warnings)
kb: context: complete (5 mandatory, 3 supplementary units)
# kb context intent=implement engine=0.1.0 skill_protocol=3 protocol=kb.cli.v1
snapshot: 4aa0f99cd4c2 (latest, freshness=verified); approved=yes; ref=origin refs/heads/main; source=<...>/kb-demo-origin.git; latest=4aa0f99cd4c2; key=2bad1f682238
host: repo=mobile (argument); head=unknown; versions=unknown
scope: repos=mobile; modules=mobile.auth; features=login; concepts=auth-token (alias `token refresh`)
change type hints: retry (positive evidence; unmentioned categories stay unresolved)
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
-- receipt sha256:91e9e17e...; included: mandatory 5, dependencies 0, proposals 0, supplementary 3, sections 0; excluded by budget: none; other exclusions: 5 (--explain); budget: 1815/8000 tokens-est (estimate, not a tokenizer count), measured as compact
-- a receipt proves delivery, not understanding or compliance; re-request context after compaction, a new session, a hand-off, or a scope or snapshot change
-- kb.receipt.v2 snapshot-content=6ce5466c...
```

The backend contract reaches this mobile task through scope and `requires`, while
backend-only records stay out. The same request in machine form (`--json` prints exactly
one `kb.cli.v1` envelope on stdout):

```text
$ kb --json context --intent implement --task "Retry the token refresh after a network error" \
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
  "receipt": "sha256:0bb5acb5..."
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

Every command runs as `kb <command>`, inside the KB checkout or anywhere in a host repository
that mounts it at `.kb` (elsewhere, `--root <dir>` or `KB_ROOT`). Commands that change files
are dry-runs unless given `--apply`.

| command | what it does | example |
|---|---|---|
| `sym` | Definition of a symbol through route `sym` (`--file`, `--kind`); bounded answer naming the tool and its index state | `kb sym TokenRefresher.refresh` |
| `callers` | Code that calls or references a symbol through route `callers` (`--file`) | `kb callers refresh` |
| `impact <symbol>` | Dependents of a symbol (route `impact`, `--depth 1..4`) and the features, invariants and contracts of the affected files | `kb impact refresh` |
| `doc` | Where a path or symbol is and what documents it: the source location or a `read` command, the ids and titles of the records that apply to or cite the path, document anchors and repository-wide `guidance` ids; statements are read with `show <id>` or `context`, and `--format human` and `--json` keep the feature detail (purpose, invariant statements, contracts, entry points, limits, gaps) | `kb doc app/auth/TokenRefresher.kt` |
| `read` | Numbered lines of a file under its path: 40 lines from `FIRST` by default, at most 80; further `[<path>] FIRST LAST` ranges follow `FIRST LAST` of the first one, at most 12 ranges and 240 lines per call | `kb read app/auth/Login.kt 1 40` |
| `find` | Repository text search through route `find` (`--regex`, `--glob`), at most 20 matches per file; hidden files are searched, ignored files and `.git` are not | `kb find "refresh token"` |
| `check` | Configured check commands for changed files, validation of changed knowledge, and with `--base` the declared probes; `--plan` runs nothing | `kb check --base origin/main` |
| `run` | A project capability: any command-line tool connected as an `exec` entry of `tools.toml`, routed and answered like the built-in requests | `kb run affected-tests src/a.kt` |
| `tools` | Routes, connected tools, versions, availability and the CodeGraph index state | `kb tools` |
| `init` | Create schema-2 project/domain scaffolding, GitHub/GitLab KB CI and review templates, or the synthetic example; preserve existing files | `kb init --name "<Product>" --namespace <ns> --apply` |
| `doctor` | Check environment, configuration, runtime, approved source, host binding and pin, engine compatibility of the approved tip and the pin, index, tool registry and routes, CodeGraph index, skill, bundle, engine divergence (`--online` also fetches; not with `--offline`) | `kb doctor --online` |
| `validate` | Records, links, policies, registries and routing fixtures (`--strict`, `--base <rev>`, `--no-routing`, `--templates`) | `kb validate --base origin/main` |
| `index` | Build or update the derived index of the selected snapshot without contacting the remote (`--rebuild`, `--gc`) | `kb index --rebuild` |
| `context` | Task-scoped context: `--intent` (required), `--task`, repeatable `--repo`, `--path`, `--module`, `--feature`, `--concept`, `--host-version`; `--budget`, `--budget-unit tokens-est\|bytes`, `--sections none\|mandatory\|all`, `--max-supplementary`, `--include-proposals`, `--explain` | `kb context --intent implement --task "..." --path src/x.kt` |
| `context` additions | Path-free `diagnose`, `--changed --base`, `--change-type`, historical `--as-of`, explicit receipt/core reuse, `--with-code` | `kb context --intent review --changed --base origin/main --format terse` |
| `outline` | Bounded inventory; request context before editing | `kb outline --intent diagnose --task "..."` |
| `coverage`, `propose`, `capture` | Prioritize domain gaps, prepare work orders and validate/apply drafts; never accept them | `kb propose begin --from-change BASE..HEAD --json` |
| `anchors`, `drift`, `ledger` | Stamp/check Git evidence, owner review queues, freshness and seeded audits | `kb anchors check --strict --json` |
| `verify` | Declared regex/glob/static-import probes; unknown evidence remains unknown | `kb verify --diff BASE --head HEAD --json` |
| `usage report` | Private delivery metadata joined to a task's final diff | `kb usage report --diff BASE --receipt ID --json` |
| `eval routing`, `eval history` | Ordered/budgeted routing metrics and labeled historical coverage | `kb eval routing --example synthetic-multirepo --json` |
| `search` | Ranked search; not a substitute for `context` (`--kind`, `--limit`, `--include-proposals`); like `show` and `impact`, its text output starts with a snapshot provenance line (revision, selection, freshness, approval, approved tip, pin) | `kb search "идемпотентность"` |
| `show` | A record, or one section (`--section` or `id#section`); `--raw` prints only the file bytes (the provenance line then goes to stderr when the content is unverified or not approved) | `kb show example.contract.payment-intent` |
| `sync` | Fetch the approved ref into the isolated mirror; report the tip, the local checkout and the host pin; changes nothing else | `kb sync` |
| `impact` (diff mode) | Relate a host diff (`--base`, `--head`, `--working-tree`) to affected knowledge and unknown coverage; `--check --statement <file>` verifies the `kb-impact` block | `kb impact --base origin/main --working-tree` |
| `integrate` | `--generate`: render the skill bundle in the KB; without it: install the bundle into the host; `--check`, `--apply`, `--force` | `kb integrate --apply` |
| `migrate` | Show (unified diff) or `--apply` supported document schema migrations (`--to <n>`) | `kb migrate` |
| `update check` | Compatibility and merge prediction for an upstream ref; writes nothing to the repository | `kb update check --upstream <url> --ref <tag>` |
| `update prepare` | Reviewable `kb-update/<ref>` branch in an isolated worktree: merge, bootstrap, migrate, integrate, validate; never pushes | `kb update prepare --upstream <url> --ref <tag>` |
| `update divergence` | Engine-owned paths that differ from the recorded upstream base | `kb update divergence` |
| `update abandon` | Show (dry-run) or, with `--apply`, remove a `kb-update/*` branch and its worktree; `--apply --force` also discards uncommitted files there | `kb update abandon kb-update/<ref> --apply` |
| `schema` | Check (`--check`) or write (`--write`) the generated JSON Schemas | `kb schema --check` |
| `version` | Engine and contract versions and the build fingerprint of the engine serving the checkout | `kb version` |
| `engine info` | The installed `kb`, the runtime store, the checkout found, its engine fingerprint and which runtime serves it | `kb engine info` |
| `engine bootstrap` | Make sure a runtime serves the checkout, building its engine from source (pinned toolchain, `Cargo.lock`) when none does; prints the runtime directory | `kb engine bootstrap` |
| `engine install` | Verify and activate a release archive of the checkout's engine: by default the release of its `engine_version` (digest from that release's `SHA256SUMS`), or `<archive-or-https-url> --sha256 <hex>` (or `--sha256-file <SHA256SUMS>`); checks entries, `BUILD-INFO` (version, target, fingerprint) and a smoke run | `kb engine install` |
| `engine package` | Build the checkout's engine and write `kb-<version>-<target>.tar.gz` and its `.sha256` (`--target <triple>`) | `kb engine package dist` |
| `engine fingerprint` | The engine fingerprint of the checkout | `kb engine fingerprint` |
| `self update` | Replace the installed `kb` with a release (default: the latest; `--version`, `--check`, `--force`), verified before the swap | `kb self update` |

Global options: `--root`, `--config`, `--profile project|maintainer`,
`--format compact|terse|human|json` (`--json`), `--offline`, `--snapshot
auto|latest|pinned|working-tree|<revision>`, `--host`, `--quiet`, `--skill-protocol <n>`.
Unknown arguments are rejected.

Output: compact text for agents (default), `--format human`, or `--json`. The agent commands
(`sym`, `callers`, `impact <symbol>`, `doc`, `find`, `check`, `run`, `tools`) print one bounded
answer in every text format and `kb.answer.v1` in JSON: at most 30 lines by default (`--lines`,
minimum 5, values above 200 are capped with a note), plus up to 80 numbered source lines when
`sym`, `find` or `callers` locates one to four places (`excerpts[]` in JSON). `read` prints
numbered source lines. In JSON mode stdout carries only the protocol envelope
`{protocol, command, ok, result, error, meta}`; progress and diagnostics go to stderr. Errors
have stable symbolic codes and grouped exit codes:

| exit | group | examples |
|---|---|---|
| 0 | success (`context`: completeness `complete`) | |
| 1, 2 | internal error, usage | `USAGE` |
| 10–19 | project and configuration | `PROJECT_NOT_INITIALIZED` 10, `UNKNOWN_SCOPE` 15 |
| 20–29 | freshness and snapshots | `FRESHNESS_UNVERIFIED` 20, `UPDATE_REQUIRED` 21, `SKILL_OUTDATED` 23 |
| 30–39 | context | `CONTEXT_INCOMPLETE` 30 (full result printed), `CONTEXT_BUDGET_EXCEEDED` 31 |
| 40–49 | checks | `VALIDATION_FAILED` 40, `ROUTING_TESTS_FAILED` 41, `DRIFT_DETECTED` 42, `IMPACT_UNACKNOWLEDGED` 44, `CHECKS_FAILED` 46 |
| 50–59 | runtime and update | `RUNTIME_INCOMPATIBLE` 50, `UPDATE_CONFLICT` 51, `RUNTIME_NOT_BOOTSTRAPPED` 54, `BOOTSTRAP_FAILED` 55, `ARTIFACT_INVALID` 57 |
| 60–69 | environment | `GIT_ERROR` 60, `INDEX_ERROR` 61, `UNSAFE_PATH` 63, `TOOL_UNAVAILABLE` 65, `DOWNLOAD_FAILED` 66 |

The full list and the JSON envelope are in [docs/protocol.md](docs/protocol.md); what to do
for each code is in [docs/troubleshooting.md](docs/troubleshooting.md).

`kb engine ...` and `kb self ...` are always handled by the installed `kb` itself, so they
also work for a checkout whose engine is missing, older or newer. Environment: `KB_ROOT`,
`KB_RUNTIME_DIR` (runtime store), `KB_CARGO_TARGET_DIR` (Cargo target directory of source
builds, default `<root>/.cache/cargo-target`), `KB_AUTO_BOOTSTRAP=1`,
`KB_ALLOW_TOOLCHAIN_MISMATCH=1`, `KB_RELEASES_URL` (release location; see
[docs/troubleshooting.md](docs/troubleshooting.md#runtime-and-installation)).

The release workflow (`.github/workflows/release.yml`) builds archives for
`aarch64-apple-darwin`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` and
`x86_64-pc-windows-msvc`, verifies each with the installers and `kb self update`, and
publishes them with `SHA256SUMS` when an owner pushes a `v<engine_version>` tag. No release
has been published yet; installing from source works without one. Downstream forks inherit this workflow and the
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
| [docs/agent-cli.md](docs/agent-cli.md) | the agent commands, answers, the tool registry, routing, checks and skills |
| [docs/protocol.md](docs/protocol.md), [docs/troubleshooting.md](docs/troubleshooting.md) | JSON protocol and error codes; what to do for each error |
| [docs/architecture.md](docs/architecture.md), [docs/adr/](docs/adr/) | the normative engineering contract and design decisions |
| [docs/maintainer-guide.md](docs/maintainer-guide.md), [AGENTS.md](AGENTS.md), [CONTRIBUTING.md](CONTRIBUTING.md) | changing, testing and releasing the engine |
| [SECURITY.md](SECURITY.md), [CHANGELOG.md](CHANGELOG.md), [docs/dependencies.md](docs/dependencies.md) | security model, changes, dependencies and licenses |
| [core/templates/README.md](core/templates/README.md), [core/skills/README.md](core/skills/README.md), [core/schemas/README.md](core/schemas/README.md) | templates, skills, schemas |

## License

Copyright 2026 kb contributors.

Licensed under the Apache License, Version 2.0; see [LICENSE](LICENSE).
Third-party dependency licenses are listed in [docs/dependencies.md](docs/dependencies.md).
