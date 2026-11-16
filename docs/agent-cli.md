# The agent CLI: code, knowledge and tools through one command

kb is the one CLI through which a coding agent (Claude Code, Codex, Cursor or any other)
reaches the codebase, the project knowledge and the code-analysis tools. The agent does not
choose between CodeGraph, `grep`, file search and other tools: it calls kb, kb routes the
request to a backend chosen by code and configuration, and the answer comes back bounded,
structured and labelled with its source. Design decision: [ADR 0016](adr/0016-agent-cli.md).

```text
coding agent
    |  skills (kb, kb-change, kb-review, kb-document) call the CLI
    v
kb:  routing · tool registry · adapters · bounded answers
    |- codegraph      symbols, callers, impact (live index)
    |- knowledge      features, invariants, contracts, gaps (typed records in Git)
    |- code-provider  any kb.code.v1 provider, commit-bound (e.g. ast-index)
    |- exec           any other command-line tool, by configuration (kb run <capability>)
    |- ripgrep / git grep   text search; lexical fallback
    `- command        the checks configured for changed files
```

## Commands

Run them as `<kb_path>/kbw <command>` (`./kbw` in the KB checkout itself). Paths are
relative to the host repository root; `repo:path` names a file of another repository the KB
describes.

| command | answers | backend |
|---|---|---|
| `kb sym <name> [--file TEXT] [--kind KIND]` | where a symbol is defined | route `sym` |
| `kb callers <symbol> [--file TEXT]` | the code that calls or references it | route `callers` |
| `kb impact <symbol> [--depth 1..4] [--file TEXT]` | the code that depends on it, then the features, invariants and contracts of the affected files | route `impact` + knowledge |
| `kb doc <path\|symbol>` | the feature documentation: feature, purpose, invariant statements, contracts, entry points, limits, gaps | knowledge (a symbol is located through route `sym` first) |
| `kb find "<text>" [--regex] [--glob GLOB]...` | matching lines in the repository, sorted, at most 20 per file | route `find` |
| `kb check [FILE...] [--base REV] [--plan]` | the result of the checks for the changed files | registry command tools, knowledge validation, declared probes |
| `kb run <capability> [ARG...]` | a project capability: any tool connected as an `exec` entry | route `<capability>` |
| `kb tools` | the routes, the tools and whether each is available here | registry |
| `kb doctor` | the environment, including `tools` and `code-index` | all |

Common options: `--lines N` (5..200, default 30), `--tool ID` (answer with one registry
tool instead of the route; `sym`, `callers`, `impact <symbol>`, `doc`, `find`), and the
global options (`--json`, `--host`, `--profile`, `--offline`, `--snapshot`).

`kb impact` keeps its diff mode: `kb impact --base <target> [--head REV | --working-tree]`
relates a host diff to affected knowledge records and unknown coverage, and
`--check --statement <file>` is the merge-request gate (see [downstream.md](downstream.md)).
A symbol and the diff options are mutually exclusive.

The knowledge commands stay available for what the agent surface does not cover:
`context` (the complete mandatory obligations with a receipt), `outline`, `show`, `search`
(ranked search of records), `verify`, and the authoring, trust and maintenance commands.

## Answers

Every agent command returns the same structure.

```text
$ .kb/kbw impact refresh
kb impact refresh · codegraph 1.6.2 (index synced, complete); knowledge 4aa0f99cd4c2 (verified, complete)
feature  login  Sign-in and session refresh [example.feature.login]  <knowledge>
invariant  example.mobile.single-refresh  At most one refresh in flight  <knowledge>
contract  example.contract.token-refresh  Token refresh API between mobile and backend (3 obligation(s))  <knowledge>
affected  signIn  app/auth/Login.kt:2  method  <codegraph>
note: 1 policy also apply: example.common.code-review
note: 1 dependent symbol(s) in 2 file(s); static analysis, not runtime reachability
next: kb callers refresh · kb doc app/auth/TokenRefresher.kt
-- 4 of 4
```

(Synthetic illustration; ids, revisions and counts depend on the corpus and the code.)

* **Header**: the command, the subject and every source with its version and the state of
  what it read: `index synced, complete` (the live index was brought up to date before the
  query), `index as is, N file change(s) not indexed` (`sync = "never"`), `commit <sha12>`
  (commit-bound; uncommitted edits are not seen), `working tree, lexical` (a text match),
  or the knowledge revision with `verified`/`unverified` freshness and the completeness.
* **Items**: `kind  name  path:line[-end]  detail  [confidence]  <source>`. Kinds:
  `definition`, `similar`, `caller`, `affected`, `match`, `feature`, `purpose`,
  `invariant`, `contract`, `entry`, `limit`, `gap`, `decision`, `check`, `output`, `route`,
  `tool`. Confidence `resolved`, `possible` or `lexical`. The source tag appears when an
  answer has more than one source.
* **Notes** (at most 3): fallbacks, limitations, policies that also apply, incomplete
  knowledge. **next** (at most 3): the kb command that usually follows.
* **Footer**: shown/total; `(truncated; --lines widens the bound)` when items were cut.
  Items are ordered by priority (for `doc`: feature, purpose, invariants, contracts, entry
  points, limits, gaps), so a cut keeps the binding facts first.

Text output never exceeds `--lines` lines of 200 characters. `--json` returns the same
answer inside the `kb.cli.v1` envelope:

```json
{"answer": "kb.answer.v1", "command": "sym", "subject": "refresh",
 "sources": [{"id": "codegraph", "adapter": "codegraph", "version": "codegraph 1.6.2", "state": "index synced, complete"}],
 "items": [{"kind": "definition", "name": "TokenRefresher::refresh", "path": "app/auth/TokenRefresher.kt",
            "line": 42, "end_line": 88, "detail": "method (): Token", "source": "codegraph"}],
 "shown": 1, "total": 1, "truncated": false, "notes": [], "next": ["kb callers refresh"]}
```

Exit codes: 0 success; 16 `NOT_FOUND` (no definition, symbol or match; the answer is still
printed); 46 `CHECKS_FAILED`; 65 `TOOL_UNAVAILABLE` (no connected tool could answer;
`details.attempts` says why); 11 `CONFIG_INVALID` for an invalid `tools.toml`; the usual
knowledge codes for `doc` and the knowledge part of `impact`. Full table:
[protocol.md](protocol.md).

## Tool registry

The registry is `<profile>/tools.toml`: `project/tools.toml` in a downstream (created by
`kbw init`), `core/maintainer-knowledge/tools.toml` for `--profile maintainer`. It is
trusted configuration of the KB checkout that supplies the engine and is reviewed like
code. Schema: [core/schemas/tools.v1.schema.json](../core/schemas/tools.v1.schema.json).

```toml
schema = 1

[[tool]]
id = "codegraph"
adapter = "codegraph"
program = "codegraph"        # bare name: PATH; {kb}/...: KB root; other relative paths: host root
sync = "auto"                # codegraph only: incremental sync before every query (default)

[[tool]]
id = "rg"
adapter = "ripgrep"
program = "rg"

[[tool]]
id = "git-grep"
adapter = "git-grep"         # built in, no program

[[tool]]
id = "ast-index"
adapter = "code-provider"    # any kb.code.v1 provider
program = "{kb}/target/release/kb-code-provider"
args = ["--backend", "ast-index"]
timeout_seconds = 300

[[tool]]
id = "android-lint"
adapter = "command"
program = "./gradlew"
run = ["lint"]               # argv after program; "{files}" expands to the matched files
paths = ["mobile:app/**/*.kt"]   # globs, optionally repo-qualified, that trigger the check
description = "Android lint"
timeout_seconds = 900

[routes]                     # ordered fallback per capability
sym = ["codegraph", "git-grep"]
callers = ["codegraph", "git-grep"]
impact = ["codegraph", "git-grep"]
find = ["rg", "git-grep"]
```

| field | applies to | meaning |
|---|---|---|
| `id` | all | lowercase kebab-case, unique; used by `routes` and `--tool` |
| `adapter` | all | `codegraph`, `code-provider`, `exec`, `ripgrep`, `git-grep`, `command` |
| `program` | all but `git-grep` | executable; a bare name is looked up on PATH, `{kb}/` and `{host}/` prefixes resolve against the KB and host roots, other relative paths against the host root |
| `args` | `code-provider`, `exec` | `code-provider`: arguments before the provider protocol; `exec`: the argv template (placeholders below) |
| `env` | all but `git-grep` | environment variables for the tool, on top of kb's (telemetry, update checks and daemons off) |
| `sync` | `codegraph` | `auto` (default) or `never` |
| `capabilities` | `exec` | requests it answers: `sym`, `callers`, `impact`, `find`, or project capabilities (kebab-case) called with `kb run` |
| `output` | `exec` | `lines` (default: one item per line), `grep` (`path:line[:col]:text`), `json` |
| `items` | `exec`, json | path of the item array: JSON pointer segments, `*` flattens arrays (`/definitions/*/callers`); default: the document if it is an array, else `/items` |
| `fields` | `exec`, json | item field ← element key or JSON pointer: `{ name = "symbol", path = "filePath", line = "/range/start" }`; unmapped fields use their own names (`kind`, `name`, `path`, `line`, `end_line`, `detail`, `confidence`) |
| `strings` | `exec`, json | the field a string element fills (default `detail`; `path` for a list of files) |
| `kind`, `confidence` | `exec` | item kind (wins over an element's `kind` unless `fields.kind` maps it) and a confidence label for every item |
| `ok_exit` | `exec` | exit codes meaning success (default `[0]`; `[0, 1]` for grep-like tools where 1 is "nothing found") |
| `prepare` | `exec` | argv of a step run before every request with the same program, e.g. `["sync", "{root}"]` for an index |
| `version_args` | `exec` | argv that prints the version for `kb tools` (default `["--version"]`; `[]` only checks that the program exists) |
| `run`, `paths` | `command` | argv after `program` (`{files}` expands to the matched files) and the globs that trigger the check |
| `description` | `command`, `exec` | one-line purpose shown by `kb tools`, `kb check` and `kb run` |
| `timeout_seconds` | all | 1..3600 (`code-provider`: at most 600, the `kb.code.v1` limit); defaults 60 (lookups), 300 (`code-provider`), 900 (`command`) |

Capabilities per adapter: `codegraph` and `code-provider` answer `sym`, `callers`,
`impact`; `ripgrep` answers `find`; `git-grep` answers all four (lexically for symbols);
`exec` answers what it declares; `command` is a check. `routes` maps any request — built in
or a project capability — to an ordered list of tools that can answer it. A request
without a route uses every declared tool that can answer it, structural adapters first,
then declaration order. Without `tools.toml` the defaults are `codegraph`, `rg` and
`git-grep` with the routes above. `kb validate` and `kb validate --templates` report an
invalid registry as `TOOLS_INVALID`.

### Connecting any tool (`exec`)

Every command-line tool can be connected by configuration alone. The `exec` adapter runs
`program` with the `args` template (no shell, in the host root, under the deadline and
output limits) and turns stdout into answer items:

```toml
# Test files affected by changed files, as a project capability: kb run affected-tests <files>
[[tool]]
id = "affected-tests"
adapter = "exec"
program = "codegraph"
prepare = ["sync", "{root}"]            # bring the index up to date first
args = ["affected", "--path", "{root}", "--json", "{args}"]
capabilities = ["affected-tests"]
description = "test files affected by changed source files"
output = "json"
items = "/affectedTests"                # ["core/cli/tests/a.rs", ...]
strings = "path"
kind = "test"

# Functions a symbol calls: kb run callees <symbol>
[[tool]]
id = "callees"
adapter = "exec"
program = "codegraph"
prepare = ["sync", "{root}"]
args = ["callees", "{subject}", "--path", "{root}", "--json"]
capabilities = ["callees"]
output = "json"
items = "/definitions/*/callees"
fields = { path = "filePath", line = "startLine", detail = "kind" }
kind = "callee"

# A live symbol index answering a built-in request (here before the lexical fallback).
[[tool]]
id = "ast-index-symbols"
adapter = "exec"
program = "ast-index"
args = ["symbol", "{subject}", "--in-file={file}", "--format", "json"]
capabilities = ["sym"]
output = "json"
version_args = ["version"]

# Any grep-format tool: compilers, linters, semgrep --emacs, ...
[[tool]]
id = "todo"
adapter = "exec"
program = "rg"
args = ["--line-number", "--no-heading", "TODO|FIXME", "{args}"]
capabilities = ["todo"]
output = "grep"
ok_exit = [0, 1]

[routes]
sym = ["codegraph", "ast-index-symbols", "git-grep"]
```

Placeholders in `args` and `prepare`: `{subject}` (the symbol, text or joined `kb run`
arguments), `{args}` (the `kb run` arguments, one argv element each), `{file}` and
`{kind}` (`--file`/`--kind`; an argument whose value is absent is dropped, so write
`--in-file={file}`), `{depth}`, `{limit}`, `{globs}` (`find --glob` values, one element
each), `{root}` (the code root) and `{kb}` (the KB root). `{args}` and `{globs}` must be
whole arguments. A template that uses neither `{subject}` nor `{args}` takes no input, and
kb refuses to call it with input rather than drop it silently.

A tool may also print the native item protocol, which `output = "json"` reads without
any mapping: `{"items": [{"kind": "...", "name": "...", "path": "...", "line": 1,
"detail": "..."}], "found": true, "state": "index fresh", "notes": ["..."]}` (all members
optional except `items`). Plain text printed instead of the configured JSON (a "not found"
message) is an empty answer with that text as a note; exit codes outside `ok_exit` are
tool failures, and routing falls back to the next tool.

**Project capabilities.** A capability an `exec` tool declares that is not a built-in
request becomes a project command: `kb run <capability> [args...]`, routed like the
built-in requests (route order, availability, fallback) and answered in the same bounded
format. `kb tools` lists every project capability with its description and how to call
it, so agents discover new tools without any skill change. kb options may come anywhere;
put `--` before tool arguments that start with `-`.

### Connecting a new tool

* **Any command-line tool**: an `exec` entry, as above — as a project capability, or in
  the route of `sym`, `callers`, `impact` or `find`. No engine change, no skill change.
* **Another tool of a supported kind** (a second CodeGraph build, a test runner for
  `kb check`): add a `[[tool]]` entry and put its id into a route or give it `paths`.
* **Commit-bound, validated code facts**: wrap the tool as a one-shot `kb.code.v1`
  provider ([core/providers/README.md](../core/providers/README.md)) and connect it with
  `adapter = "code-provider"`; the engine validates its facts against Git objects.
* **A new built-in adapter** (an index protocol that needs engine logic, such as
  CodeGraph's sync-and-status handling) is an engine change: one module in
  `core/cli/src/tools/`, one `AdapterKind` variant, tests with a synthetic stand-in.

### Routing, freshness and fallbacks

For `sym`, `callers` and `impact` kb tries the route in order. A tool that is not
installed, has no index for this code root, fails, or does not find the requested symbol
hands over to the next tool; each hand-over becomes a note (`codegraph failed: no CodeGraph
index in ...`). The first answer that finds the symbol wins; when none does, the most
useful partial answer is returned with `NOT_FOUND`. `find` uses the first available tool.

The codegraph adapter queries the host's own `.codegraph/` index. With `sync = "auto"` it
runs `codegraph sync <root>` before each query, so answers describe the current work tree.
kb never creates a tool index: create it once per checkout with
`codegraph init --yes <host root>` (keep `.codegraph/` out of Git, for example in
`.git/info/exclude`). `kb tools` and `kb doctor` report a missing or incomplete index.
kb starts every tool with telemetry, update checks and daemons disabled
(`CODEGRAPH_TELEMETRY=0`, `CODEGRAPH_NO_UPDATE_CHECK=1`, `CODEGRAPH_NO_DAEMON=1`,
`DO_NOT_TRACK=1`) and under a deadline and output limits.

The `code-provider` adapter is commit-bound: it materializes the host `HEAD` commit into a
temporary checkout and indexes it afresh (ADR 0011), which is slower and never sees
uncommitted edits, but cannot read a stale index. The knowledge part of `doc` and `impact`
follows the snapshot rules of [snapshots-and-trust.md](snapshots-and-trust.md): the approved
ref is fetched on every call unless `--offline` is given.

## Feature documentation

`kb doc` returns the documentation of the feature a path or symbol belongs to. It uses the
scope rules of `context --intent implement` for the path, so what it shows as binding is
exactly what context would deliver as mandatory:

| part of the answer | where it is written |
|---|---|
| `feature` | the registry feature (`project/registry/features.toml`, via `paths` or module `features`) |
| `purpose` | the feature record's `summary` |
| `invariant` (one line per statement) | `invariant` records whose scope covers the path or feature |
| `contract` | `contract` records of the path's modules |
| `entry` | `source` anchors of the feature record (`repo`, `path`, `symbol`, `note`) |
| `limit` | the feature record's `boundaries` |
| `gap` | `gap` records in scope |
| notes | policies that also apply, missing feature records, incomplete knowledge |

A feature record is one compact document per feature, at most 60 lines (`FEATURE_TOO_LONG`).
`doc` reads accepted knowledge of the selected snapshot; a draft becomes visible once its
change is accepted into the approved ref (inspect drafts with `context --include-proposals`).
The `kb-document` skill describes how to write one; [format.md](format.md) is the field
reference. `doc` is orientation; before the first edit the `kb-change` skill still requests
`context`, which adds required dependencies, effective settings, a completeness status and
a receipt.

## Checks

`kb check` runs, for the changed files:

1. every `command` tool whose `paths` match one of them, with `{files}` expanded to the
   matched files, without a shell, in the host root, under its deadline (stdout and stderr
   up to 16 MiB; on failure the last lines are shown);
2. validation of this profile's knowledge when knowledge files changed (inside the code
   root, in a mounted KB, or uncommitted in the KB checkout);
3. with `--base`, the static probes records declare ([knowledge-lifecycle.md](knowledge-lifecycle.md)),
   which never execute record text; probes need a host repository and are skipped with a
   note when kb runs in a KB checkout without one.

Changed files are the arguments, else the work tree's changes against `--base` (default
`HEAD`), deleted files excluded. `--plan` lists what would run and runs nothing. A failed
check, a missing check program, knowledge errors or a failed probe give `CHECKS_FAILED`.

kb executes only argv written in the registry of its own checkout. Knowledge records,
procedures, commit messages and tool output are never executed ([SECURITY.md](../SECURITY.md)).

## Skills

`kbw integrate` installs four skills side by side ([core/skills/README.md](../core/skills/README.md)):

| skill | use |
|---|---|
| `kb` | the commands, how to read answers, error recovery, references |
| `kb-change` | the universal change procedure below |
| `kb-review` | reviewing a change against obligations, invariants, callers and checks |
| `kb-document` | creating or updating feature documentation |

The change procedure:

1. locate the code: `kb sym`, `kb find`;
2. read the feature documentation: `kb doc <path|symbol>`;
3. analyse dependencies: `kb impact <symbol>`, `kb callers <symbol>`;
4. before the first edit: `kb context --intent implement --path <file> ...`, and apply every
   mandatory statement;
5. change the code, reading the source for details kb does not supply;
6. verify: `kb check --base <target>`;
7. update the documentation in the same change when behaviour changed (`kb-document`), and
   address `kb impact --base <target> --working-tree`;
8. report the receipt, the checks and unresolved gaps.
