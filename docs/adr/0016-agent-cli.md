# ADR 0016: kb as the single CLI between a coding agent, the code, the docs and the code tools

Status: accepted for engine 0.1.x (skill protocol 3)

## Context

Until now kb was a typed knowledge engine whose centre was task-scoped context delivery.
Code facts reached an agent only as an optional side input (`context --with-code`,
`impact --deep`, coverage fan-in), through a provider named per invocation on the command
line. Agents therefore still chose for themselves between CodeGraph, `grep`, file search
and other tools, spent tokens on unstructured searches, and learned the architectural
rules only if they asked for context at the right moment.

The target is one CLI through which an agent reaches the codebase, the project
documentation and the code-analysis tools: skills call kb; kb routes each request to a
backend chosen by code and configuration; every answer is bounded, structured and names
its source; knowledge is compact documents versioned in Git; skills cover changing code,
review, documentation and querying kb.

## Decision

### Command surface

The engine keeps its knowledge commands and adds an agent surface in front of them:

| responsibility | command | backend |
|---|---|---|
| locate a symbol's definition | `kb sym <name>` | route `sym` |
| find the code that calls a symbol | `kb callers <symbol>` | route `callers` |
| what a change to a symbol affects | `kb impact <symbol>` | route `impact`, then the knowledge of the affected files |
| feature documentation of a path or symbol | `kb doc <path\|symbol>` | knowledge (symbols located through route `sym`) |
| search the repository | `kb find "<text>"` | route `find` |
| run the checks for changed files | `kb check [files] [--base REV]` | registry command tools, knowledge validation, declared probes |
| a project capability (any tool connected by configuration) | `kb run <capability> [args]` | route `<capability>` (`exec` tools) |
| status of the connected tools | `kb tools` | registry |
| check the environment | `kb doctor` | adds `tools` and `code-index` checks |

`impact` keeps one name for one job, "what does this change affect": with a symbol it
relates the symbol's dependents to the features, invariants and contracts that govern the
affected files; with `--base`/`--head`/`--working-tree` it keeps relating a diff to
affected knowledge and coverage, including the `--check` gate CI already uses. `find`
searches the host repository; `search` stays the ranked search of knowledge records.
`context`, `outline`, `show`, `verify` and the authoring, trust, evaluation, migration and
update commands stay as they are: they are the knowledge layer the agent surface stands
on, and their contracts are unchanged.

### Layers: skills → routing → registry → adapters → output

* **Tool registry**: `<profile>/tools.toml` (`project/tools.toml`, or
  `core/maintainer-knowledge/tools.toml`), schema 1, `core/schemas/tools.v1.schema.json`.
  It declares tools (`id`, `adapter`, `program`, adapter options) and an ordered `routes`
  list per capability. Without the file the built-in defaults apply (codegraph → rg →
  git grep). It is read from the KB **working tree**, the checkout that also supplies the
  engine binary, never from a fetched snapshot.
* **Routing** is code plus configuration: the route order, filtered by availability on
  this machine; the first tool that finds the requested symbol answers; a tool that is
  missing, fails or does not know the symbol hands over to the next and the hand-over is
  reported as a note. `--tool <id>` forces one tool. No model decision is involved.
* **Adapters** are engine code: `codegraph` (live index), `code-provider` (any `kb.code.v1`
  provider, ADR 0011), `ripgrep`, `git-grep` (built in; also the lexical fallback for
  symbol requests), `command` (checks) and `exec`, which connects **any** command-line tool
  by configuration: an argv template with placeholders, an optional `prepare` step (for an
  index), the exit codes that mean success, and an output mapping (`lines`, `grep`, or JSON
  with an item path and field mapping, or the native item protocol). An `exec` tool answers
  a built-in request from its route or declares **project capabilities** that agents call
  with `kb run <capability>` and discover with `kb tools`. Connecting a tool is therefore
  always a registry entry; only a tool that needs engine logic of its own (CodeGraph's
  sync-and-status handling, validated `kb.code.v1` facts) has a dedicated adapter. Skills
  name kb commands only, so no case changes a skill.
* **Output**: every agent command returns one answer (`kb.answer.v1` inside the unchanged
  `kb.cli.v1` envelope): sources (tool id, adapter, version, state of what it read),
  items (kind, name, path, line, end line, detail, confidence, source), notes, follow-ups,
  total and a truncation flag. Text is at most 30 lines (`--lines`, minimum 5, capped at
  200) of at most 200 characters; later, numbered source lines may follow the items
  outside that bound ([agent-cli.md](../agent-cli.md#one-call-instead-of-two)). A line bound fits what agents read; the existing byte and token budgets stay
  for `context`, whose units are never truncated.

The registry is a plain configuration file for engine-owned adapters. It is not a plugin
system: kb loads no code, discovers nothing and installs nothing.

### Latency against commit-bound correctness

Interactive lookups use the host's live CodeGraph index, brought up to date with the
tool's incremental `sync` before every query (`sync = "auto"`, about 0.2 s on a mid-size
repository) and reported as `index synced, complete` or with its pending changes. kb never
creates a tool index; a missing index makes that tool unavailable and routing falls back.
The commit-bound path stays for what needs historical correctness: `context --with-code`,
`impact --deep`, coverage fan-in and the `code-provider` adapter, whose answers name the
commit they describe. Knowledge reads keep the freshness rules of ADR 0004: `doc` and the
knowledge part of `impact` fetch the approved ref unless `--offline` is given, and the
answer's knowledge source says whether freshness was verified.

### Checks

A check is (1) a `command` tool whose `paths` globs match a changed file, (2) validation
of the profile's knowledge when knowledge files changed, (3) with `--base` and a host
repository, the static probes records declare (`kb verify`); without a host they are skipped
with a note. kb runs configured commands: argv without a shell,
in the code root, with the tool's deadline and output limits, `{files}` expanded to the
matched files. The rule of ADR 0015 holds unchanged: nothing a record supplies is ever
executed. Commands come only from the registry of the KB checkout that supplies the engine,
which is the same trust root as the `kb` binary the user already runs; a registry change
is reviewed like engine code. `--plan` lists what would run. Any failure is
`CHECKS_FAILED` (46) with the full answer printed.

### Feature documentation

The typed records serve as the feature documents; no second format coexists with them.
One feature = one registry feature (`paths`) and one compact `feature` record: `summary`
is the purpose, `source` anchors with a `symbol` are the entry points (and gain drift
detection from `anchors check`), `boundaries` are the known limits and nuances, behaviors
and scenarios stay descriptive. A feature record has at most 60 lines (`FEATURE_TOO_LONG`
warning; `validate --strict` fails it). Invariants stay separate `invariant` records,
because context assembly must deliver them as mandatory obligations even when no feature
document is retrieved, and one invariant often spans several features. `kb doc` composes
the feature, its purpose, the invariant statements and contracts selected by the scope
rules of `context --intent implement`, the entry points, limits and gaps into one bounded
answer. For a symbol it first locates the definition through route `sym`. No document
schema change is needed (document schema stays 2).

### Skills

The generated bundle becomes a skill set: `kb` (querying kb: commands, answers, recovery;
the references stay with it), `kb-change` (the short universal change procedure of the
reference workflow: locate, `doc`, `impact`/`callers`, `context` before the first edit,
edit, `check`, documentation in the same change), `kb-review` and `kb-document`. All four
are installed side by side in the harness's skill directory; mixed Claude installs get one
`.claude/commands/<skill>.md` alias per skill. The always-on managed block points at the
set. Skill protocol 3.

### Where kb and the knowledge live

The upstream/downstream/submodule model stays (ADR 0001). A product with one repository
mounts its downstream at `.kb` inside that repository, which is the in-repository start in
effect; a product with several repositories keeps one KB for all of them, and multi-
repository support and CI knowledge checks, which the reference leaves out of its first
version, stay because they already work. Code and documentation travel together as one
host change that carries the code, the KB pin bump to the reviewed knowledge commit and
the `kb-impact` acknowledgement that the CI gate checks.

### Backends shipped connected

CodeGraph (live adapter, the reference backend), ripgrep and the built-in git grep are in
the defaults. ast-index stays connected through the `kb.code.v1` reference adapter
(`code-provider`); its live CLI needs no dedicated adapter: it connects as an `exec` entry
(its symbol search needs an index kb does not create, its caller search is lexical, which
the built-in git grep already covers). The maintainer registry dogfoods `exec` with
CodeGraph's `callees` and `affected` commands as project capabilities.

## Consequences

* An agent asks kb, not a tool. The same commands and the same `tools.toml` give the same
  routing on every machine; `kb tools` and `kb doctor` show what differs (installed tools,
  index state). Identical configuration does not make model decisions identical.
* Static answers remain navigation evidence: CodeGraph's graph is not runtime
  reachability, lexical matches are labelled, and the skills tell agents to confirm
  decisive facts in the source.
* `kb check`, `kb run` and every routed tool execute configured programs. That widens what
  the engine does at runtime;
  the registry is therefore trusted configuration, `SECURITY.md` says so, and a malicious
  registry is equivalent to a malicious engine checkout.
* README non-goals change: kb still is not an MCP server, daemon, web UI, telemetry
  uploader or plugin system and makes no LLM calls; it now does route to external code
  tools and run configured checks.
* Versions: skill protocol 3; CLI protocol stays `kb.cli.v1` (new commands, the answer
  result and the error codes `CHECKS_FAILED` 46 and `TOOL_UNAVAILABLE` 65 are additive);
  document schema 2 and index schema 2 are unchanged; new `tools.toml` schema 1.
