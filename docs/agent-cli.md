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
    |- read           numbered source excerpts, with the repository path once
    |- codegraph      symbols, callers, impact (live index)
    |- knowledge      features, invariants, contracts, gaps (typed records in Git)
    |- code-provider  any kb.code.v1 provider, commit-bound (e.g. ast-index)
    |- exec           any other command-line tool, by configuration (kb run <capability>)
    |- ripgrep / git grep   text search; lexical fallback
    `- command        the checks configured for changed files
```

## Commands

Run them as `kb <command>` (`kb` in the KB checkout itself). Paths are
relative to the host repository root; `repo:path` names a file of another repository the KB
describes.

| command | answers | backend |
|---|---|---|
| `kb sym <name> [--file TEXT] [--kind KIND]` | where a symbol is defined, with the lines of a single definition or the head of each of a few; when no tool resolves it, the lines that could declare it (`possible`), else up to five of the files that use it | route `sym`, then the built-in `git grep` text scan |
| `kb callers <symbol> [--file TEXT]` | the code that calls or references it, with the lines around a few call sites; when no line of the checkout reads as a declaration of the symbol or of its type, its textual uses and a note that says so | route `callers` |
| `kb impact <symbol> [--depth 1..4] [--file TEXT]` | the code that depends on it, then the features, invariants and contracts of the affected files | route `impact` + knowledge |
| `kb doc <path\|symbol>` | the source location, the ids and titles of the records that apply to or cite it, documentation anchors and `guidance` ids | knowledge (a symbol is located through route `sym` first) |
| `kb read <path> [FIRST [LAST [[<path>] FIRST LAST]...]]` | numbered source lines with the full path (40 from FIRST by default, at most 80 per range), a continuation when the requested excerpt exceeds the window, and an end-of-file marker when it reaches the last line; several ranges in one call | host working tree |
| `kb find "<text>" [--regex] [--glob GLOB]...` | matching lines, at most 20 per file; a few matches come with the lines around them; a miss names the files it covered, a declaration-shaped miss is answered for the name, and a name or phrase of two or three words that no line reads is answered with the lines that hold its words. Bare Kotlin and Java identifier searches put code before imports and packages, and rotate through files within each group | route `find` |
| `kb check [FILE...] [--base REV] [--plan]` | the result of the checks for the changed files | registry command tools, knowledge validation, declared probes |
| `kb run <capability> [ARG...]` | a project capability: any tool connected as an `exec` entry | route `<capability>` |
| `kb tools` | the routes, the tools and whether each is available here | registry |
| `kb doctor` | the environment, including `tools` and `code-index` | all |

Common options: `--lines N` (minimum 5, default 30, values above 200 are capped with a
note), `--tool ID` (answer with one registry tool instead of the route; `sym`, `callers`,
`impact <symbol>`, `doc`, `find`), and the global options (`--json`, `--host`,
`--profile`, `--offline`, `--snapshot`).

`kb impact` keeps its diff mode: `kb impact --base <target> [--head REV | --working-tree]`
relates a host diff to affected knowledge records and unknown coverage, and
`--check --statement <file>` is the merge-request gate (see [downstream.md](downstream.md)).
A symbol and the diff options are mutually exclusive.

The knowledge commands stay available for what the agent surface does not cover:
`context` (the complete mandatory obligations with a receipt), `outline`, `show`, `search`
(ranked search of records and, in a host checkout, the code files named like the query),
`verify`, and the authoring, trust and maintenance commands.

## Answers

Routed lookups and `doc` return the common bounded answer. `read` returns a source excerpt
with its resolved path and line range.

```text
$ kb impact refresh
kb impact refresh · codegraph 1.6.2 (index synced, complete); knowledge 4aa0f99cd4c2 (verified, complete)
feature  login  Sign-in and session refresh [example.feature.login]  <knowledge>
invariant  example.mobile.single-refresh  At most one refresh in flight  <knowledge>
contract  example.contract.token-refresh  Token refresh API between mobile and backend (3 obligation(s))  <knowledge>
affected  app/auth/Login.kt:2  signIn  method  <codegraph>
note: 1 policy also apply: example.common.code-review
note: 1 dependent symbol(s) in 2 file(s); static analysis, not runtime reachability
next: kb callers refresh · kb doc app/auth/Login.kt
-- 4 of 4
```

(Synthetic illustration; ids, revisions and counts depend on the corpus and the code.)

* **Header**: the command, the subject and every source with its version and the state of
  what it read: `index synced, complete` (the live index was brought up to date before the
  query), `index as is, N file change(s) not indexed` (`sync = "never"`), `commit <sha12>`
  (commit-bound; uncommitted edits are not seen), `working tree, lexical` (a text match),
  or the knowledge revision with `verified`/`unverified` freshness and the completeness.
* **Items** use `kind  path:line[-end]  name  detail  [confidence]  <source>`. Code
  locations appear before metadata so the filename and line survive clipping. `find`
  writes each path once and groups the selected matches below it: files in ranked
  order for bare Kotlin and Java identifier searches, in path order otherwise. A
  continuation `match  :LINE  text` refers to the path above it. Kinds include
  `definition`, `similar`, `caller`, `affected`, `match`, `declaration` (a line of the
  outline that answers a miss inside one file), `feature`, `purpose`,
  `invariant`, `contract`, `entry`, `limit`, `gap`, `decision`, `guidance`, `check`,
  `output`, `route`, `tool`, `reference` (a use of a name that has no recognisable
  declaration in the checkout). Confidence `resolved`, `possible` or `lexical`. The
  source tag appears when an answer has more than one source.
* **Notes** (at most 3): fallbacks, limitations, policies that also apply, incomplete
  knowledge. **next** (at most 3): the kb command that usually follows.
* **Footer**: shown/total; `(truncated; --lines widens the bound)` when items were cut.
  The command keeps the highest-priority items that fit the bound.

Bounded answer text uses the effective `--lines` limit and at most 200 characters per
line. Overlong locations keep their filename suffix and line, a name next to code text is
clipped to 80 characters, and an item's code text keeps at least 120 characters after a
long path, so a matched line is not cut to a stub that has to be asked for again. `--json`
returns the complete selected fields in the `kb.cli.v1` envelope, which is written on one
line for an answer; the text of a line that a text search matched (`detail` of its `match`,
`reference`, `caller` and `definition` items) is at most 500 characters there too. The
`result` below is wrapped for reading, and the 50 lines of its excerpt are left out:

```json
{"answer": "kb.answer.v1", "command": "sym", "subject": "refresh",
 "sources": [{"id": "codegraph", "adapter": "codegraph", "version": "codegraph 1.6.2", "state": "index synced, complete"}],
 "items": [{"kind": "definition", "name": "TokenRefresher::refresh", "path": "app/auth/TokenRefresher.kt",
            "line": 42, "end_line": 88, "detail": "method (): Token", "source": "codegraph"}],
 "shown": 1, "total": 1, "truncated": false, "notes": [], "next": ["kb callers refresh"],
 "excerpts": [{"path": "app/auth/TokenRefresher.kt", "first_line": 39, "lines": ["..."]}]}
```

### One call instead of two

Every further call re-sends the conversation, so an answer carries what would otherwise be
the next request:

* `kb sym` attaches the single definition it found: three lines above it (annotations,
  the end of a doc comment) and its span (15 lines when the tool reports no end), up to
  one `kb read` window of 80 lines, numbered as `kb read` prints them. A longer definition
  names the `kb read` that continues it in `next:`. Two to four definitions of one name
  (overloads, an interface and its implementation) come with the head of each and share
  the 80 lines: three lines above plus at most 25, 23 or 17 lines of each definition for
  two, three or four of them. More are listed without lines.
* `kb find` with exactly one match attaches the lines around it: three above, twenty
  below. Two to four matches come with three lines above and eight below each; five or
  more are a list.
* `kb callers` with up to four callers attaches three lines above and eight below each
  call site.
* `kb search` lists, after the records, up to eight code files whose names read like the
  query (`code: 3 of 3 file(s) named like the query`): a file is listed when its name
  holds two words of the query (one, for a one-word query) or consists of query words
  only. Query words shorter than three characters are ignored. Words are split at case
  changes, digits and punctuation. Two words are the same word when they are equal, when
  one is the other with a known ending (`s`, `es`, `ed`, `ing`, `ment`, `er`, `or`,
  `ion`, `al`, `ly`, `able`, `ation` and their plurals; the shorter the word, the fewer
  endings count), when a word of six or more letters begins the longer one (`config`,
  `configuration`), or as `entry`/`entries` and `migrate`/`migration`. Words that merely
  begin alike are different (`list`/`listener`, `auth`/`author`, `store`/`story`). Two
  forms of one word in the query count as one word, and a word that few file names hold
  weighs more than a common one. A file that declares the name it carries is listed with
  that line (`app/auth/TokenStore.kt:6  class TokenStore {`), so the next read starts at
  the declaration and not at the imports.
* `kb show` gives the line of each anchored symbol and, for an anchored document, its
  length and where it speaks of the record (see [Feature documentation](#feature-documentation)).
* `kb read` takes further ranges once FIRST and LAST of the first one are given:
  `FILE FIRST LAST` groups, or `FIRST LAST` alone for another range of the file named last
  (`kb read Login.kt 10 40 90 120 TokenStore.kt 1 30`). One call reads up to 12 ranges
  (more are `INVALID_INPUT`) of at most 80 lines each and 240 lines in all. A range that
  fails is reported in its place without costing the others; ranges beyond the 240 lines
  are named in a closing `-- not read` line.

A match or call site that is an import line (`import`, `package`, `use`, `pub use`,
`using`, `#include`, `from x import y`) is listed and not shown, and an excerpt is left
out when its lines hold control characters or would take the excerpts of the answer
beyond 32 KiB; the item still names the line. Windows of one file that touch are one
excerpt; several excerpts are headed by their path, once per file, and a single excerpt
is headed by its path unless every shown item is in that file (so also when no item is
shown). Excerpt lines are clipped at 300 characters. All excerpts of an answer hold at
most 80 lines together and are outside the `--lines` bound. In JSON they are `excerpts[]`
of `path`, `first_line`, `lines[]` and `next_line`, which is present only when the 80
lines of the answer ended inside a located span.

### Misses are final

A lookup that finds nothing says what it looked for and in which files, and names the
lookup that is still worth making, if there is one:

* `kb sym` for a name no tool resolves scans the text of the checkout for the name as a
  whole word, as written, and answers with what it holds:
  * a line that could declare the name without a keyword is listed as a `definition` item
    with confidence `possible` (exit 0, note ``no tool resolved `<name>`; each line shown
    could be its declaration``), and a single one comes with its lines. The shapes,
    decided by the file extension: C family and Java `Type name(` where every word before
    the name is shaped like a type, a modifier or an annotation; Go `func (r T) name(`;
    JavaScript and TypeScript `name(...) {` or `name(...): T {` with the parameter list
    closed on the line and no arrow or `function`; Rust `static NAME`. An enum entry or
    any other shape is not recognised and gets the next answer;
  * uses without a recognisable declaration are listed as `reference` items, at most
    five, one per file, a code line rather than an import, with the note
    `` `<name>` is used in N file(s) and no line of the M file(s) of this checkout reads
    as its declaration ``, the error ``no declaration of `<name>` was found in this
    checkout`` and the hint ``it is declared in a dependency, in generated code or in a
    form the text scan does not recognise; the uses above are what the files `kb find`
    and `kb read` cover hold of it, so cite them``. When more files use the name,
    `next:` names `kb callers <name>`;
  * a name that occurs nowhere is reported as `` `<name>` does not occur in the N
    file(s) of this checkout ``, with at most three similar names and the hint ``check
    the spelling: the name was looked for as a whole word, as written, in the files
    `kb find` and `kb read` cover``;
  * a dotted name `Type.member` that no line writes while `member` occurs is not a final
    miss: `NOT_FOUND` with the message ``no line writes `Type.member` ``, the note ``no
    line of this checkout writes `Type.member`; `member` occurs in it``, the follow-ups
    `kb sym member` and `kb callers member` and the hint `a member is declared and often
    called without its type's name; look it up by its own name`. When `member` does not
    occur either, the answer is `does not occur`. A dotted name that is written, whose
    type is declared in the checkout and whose member has a declaration under its own
    name, is not final either: `NOT_FOUND` with the message ``no declaration is written
    as `Type.member` `` and the follow-up `kb sym member`. A line that declares the
    member with the dotted name (`fun Type.member(`) is its definition, whatever is
    known of the type. A `Type.member` answered from text is
    not answered with the same-named member of another type when no line of the checkout
    reads as a declaration of `Type`.

  The `reference` and `does not occur` answers add `submodules of this checkout were not
  scanned` as their second note when `.gitmodules` lists a submodule other than the
  mounted knowledge base. All four answers drop the hand-over note ``<tool> did not find
  `<name>` ``, the `possible` and `reference` answers also the note about similar
  symbols. A lookup narrowed by `--file` or `--kind`, or outside a Git work tree, keeps
  the plain miss: it settles nothing about the whole checkout.
* `kb find` reports `no match for "<text>" in the N file(s) of this checkout`, or in the
  files the globs select, or that the globs select no file. A text shaped like a
  declaration misses on any difference in keywords, so its miss is answered for the
  name: the lines that declare it when it is declared after all (exit 0, note ``no line
  reads "<text>"; `<name>` is declared as shown``, or `may be declared as shown` when
  only a keyword-less line could declare it), otherwise the `reference` or `does not
  occur` answer of `kb sym`. With `--glob` that note begins `no line of the files the
  glob(s) select reads`. A text is declaration-shaped when it is declaration keywords and
  a name that is written like code (an upper-case letter, a digit or an underscore in
  it) after a keyword that introduces a declaration (`sealed interface Name`), or that
  is followed by declaration punctuation (`(`, `<`, `:`, `{`, `=`, as in `fun name(`).
  `class action`, `open file` and `private key` are prose: they miss as text and may be
  answered with the lines that hold their words. Before it reports a miss, `find` tries
  what the question most likely meant, and says so in a note:
  * globs that select no file are read again without regard to letter case, then every
    glob without a `/` as "below the directories of that name" (`*tokenStore*` then
    selects the files below `app/tokenstore/`); note `the glob(s) select no file as
    written; searched without regard to case`, or `searched below the directories of
    that name`;
  * a text of letters, digits, underscores and spaces that splits into two or three
    words, each of at least three letters (`refreshDelay`, `rotation window`), and that
    no line reads is answered with up to ten lines that hold all of its words, in any
    order and case (exit 0, note `no line reads "<text>"; these lines hold its words
    (refresh, delay)`). A text with a shorter word, a digit or punctuation, a longer
    one, a regular expression and a declaration-shaped text are searched as written only;
  * a text that misses inside the single file the globs select is answered with the
    outline of that file as `declaration` items (exit 16, note `no line of the file
    reads "<text>"; its declarations follow`, and `the outline was made from the first
    20 matching lines of the file` when it is cut), so the next read can be aimed: in a
    Markdown file (`.md`, `.markdown`, `.mdx`) its headings, in any other file the lines
    that open a declaration with a keyword written in lower case (`fn`, `def`, `class`,
    `struct`, `enum`, `trait`, `interface`, `func`, `function`, `fun`, `object`,
    `protocol`, `module`, `impl`, `typealias`, `record`, after optional modifiers,
    `pub(crate)` included). Comments, Markdown prose, and constants and methods written
    without a keyword are not listed.
* `kb callers` and `kb impact <symbol>` give the `does not occur` answer of `kb sym` for a
  symbol that occurs nowhere. A name that is used without a recognisable declaration is
  answered from text when the route holds a text tool (the default routes end in
  `git-grep`): exit 0, with a note unless `--file` narrows the lookup. `callers` notes
  ``no line of this checkout reads as a declaration of `<X>`; these lines are textual
  uses of `<Y>` in it`` and suggests no follow-up; `impact` notes ``no line of this
  checkout reads as a declaration of `<X>`; the affected files hold textual uses of
  `<Z>` ``. `<X>` is the type before the dot, else the last segment of the name; `<Y>` is
  the dotted symbol when a type stands before the dot, else the last segment; `<Z>` is
  the last segment. The `NOT_FOUND` answer with `reference` items comes from `callers`
  and `impact` only when no tool of the route answers from text, and so does the `no line
  writes` answer of `impact`. Of a `Type.member` whose type has no recognisable
  declaration, `callers` keeps the lines that write the dotted name; when none does, it
  answers as `kb sym` does: `no line writes`, or `does not occur`. When the type is
  declared in the checkout, the lexical answer lists the whole-word uses of the member's
  name, as lexical `impact` does in either case.
* `kb read` reports ``no file matches `<name>` among the N file(s) of this checkout``, and
  `` `<path>` ends at line N `` for a range past the end. An excerpt that reaches the last
  line ends with `-- end of file (N lines)`; a continuation is clamped to the last line.

`kb find` through `ripgrep` searches every file that is not ignored, hidden files included
(never `.git`), and enters submodule work trees. The text scan behind the `sym` and
`callers` misses, `find` through `git-grep` and the name matching of `read` use Git's
listing of tracked and untracked files that are not ignored: hidden files included,
submodules not. `read` opens any existing file given by its exact path, ignored or not.
The scan reads up to 200 matching lines per file and looks for the name as a whole word,
as written; `find` also matches parts of words and ignores letter case for a text in
lower case, so it can show another spelling of a name that `does not occur`.

Exit codes: 0 success; 16 `NOT_FOUND` (no definition, symbol or match; the answer is still
printed); 46 `CHECKS_FAILED`; 65 `TOOL_UNAVAILABLE` (no connected tool could answer;
`details.attempts` says why); 11 `CONFIG_INVALID` for an invalid `tools.toml`; the usual
knowledge codes for `doc` and the knowledge part of `impact`. Full table:
[protocol.md](protocol.md).

## Tool registry

The registry is `<profile>/tools.toml`: `project/tools.toml` in a downstream (created by
`kb init`), `core/maintainer-knowledge/tools.toml` for `--profile maintainer`. It is
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

`git-grep` answers `sym` only with lines that read as a declaration of the name: a
definition keyword introduces it (`fn`, `def`, `class`, `struct`, `enum`, `trait`,
`interface`, `func`, `function`, `fun`, `type`, `object`, `protocol`, `module`, `impl`,
`val`, `var`, `let`, `const`, `typealias`, `record`). `fun Receiver.name(` declares `name`,
not the receiver; several declarations on one line are each seen; a name after `:`, after
`extends` or `implements`, or inside an unclosed `<` is not declared there. A typed use
such as `val x: Name` is a use and appears under `callers`.

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

By default (and with `--format terse`) `kb doc` leads with the source location or a
`kb read` command for an existing documentation file. It lists the ids and titles of the
records that apply to the path and their unique document anchors, ranking exact anchors
and path selectors ahead of module and feature matches. Records that cite the path as a
source, test or document are listed with the exact anchors even when their scope does not
cover the path, so a document leads to the records written from it. Repository-wide
guidance is listed by id (`guidance` items). Statements are not printed: read them with
`kb show <id>`, or with `kb context` for the full scoped obligations.

The `--format human` and `--json` views keep the feature detail selected by
`context --intent implement` for the path (purpose, invariant statements, contracts, entry
points, limits and gaps; see the table below) and add the citing records, marked
`(cites this path)`.

`kb show <id>` relates a record's anchors to the checked-out code: an anchored symbol gets
the line that declares it, else the first that names it, after the path (`source
mobile:app/auth/TokenStore.kt:6 symbol TokenStore`), and an anchored file that is absent
is marked `(not in this checkout)`. An anchored document that is present gets its length
and up to five places to read, printed after the anchor's note:
`doc mobile:docs/token-storage.md (§ Sign-out) [48 lines; § Sign-out at :31, TokenStore at :12]`.
A `§ Name` place exists only when the anchor's note names it: it is the heading that is
the name, else the first heading that begins with it as a whole word. Headings are
Markdown headings (one to six `#` and a space, at most three spaces of indentation)
outside fenced code, and an `and` inside a section name belongs to it. Then comes the
first whole-word mention of each code name the record writes in backticks (a name with a
case change or an underscore, at least five characters). All of these lines are lexical
locators for `kb read`, not resolved definitions; `--json` carries the same facts in
`anchor_targets`, parallel to the record's anchors: `{state: "found", line}`,
`{state: "found", lines, mentions[{text, line}]}`, `{state: "missing"}` or `null` (not
checked).

| part of the feature detail | where it is written |
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

`kb integrate` installs four skills side by side ([core/skills/README.md](../core/skills/README.md)):

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
8. summarize the outcome, relevant verification and unresolved gaps. Usage reports and
   token-savings comparisons are optional diagnostics to run on request.
