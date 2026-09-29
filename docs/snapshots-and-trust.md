# Snapshots, freshness and trust

Every knowledge query answers from one immutable **snapshot** of the KB: a commit of the
approved source, a pinned commit, an explicit revision, or the local working tree. This guide
explains how kb checks that the approved source is current (freshness), which snapshot it
reads (selection), how it finds the host repository, what it caches, and what the result can
and cannot be trusted for.

Normative contract: [architecture.md §6–§7](architecture.md#6-freshness-and-snapshots),
[ADR 0004](adr/0004-freshness-and-snapshots.md) and [ADR 0005](adr/0005-index.md). How the
snapshot's content becomes task context is in [context.md](context.md); the record format is
in [format.md](format.md).

Examples were captured with scratch repositories: a KB with the synthetic example published
to a local bare `origin` (shown as `<kb-origin.git>`), and a host repository that mounts the
KB as a submodule at `.kb` and declares `repo = "mobile"` in `.kbw.toml`.

## Freshness: one remote check per call

`kb context`, `kb show`, `kb search` and `kb impact` fetch the configured approved ref on
**every call** before reading knowledge; `kb sync` does the same and only reports. There is no
TTL and no automatic offline fallback: if the fetch fails, the command fails with
`FRESHNESS_UNVERIFIED` (exit 20).

```text
error[FRESHNESS_UNVERIFIED]: cannot verify `refs/heads/main` on remote `origin` (https://***@git.example.invalid/example/kb.git): fatal: unable to access 'https://git.example.invalid/example/kb.git/': Could not resolve host: git.example.invalid
  hint: check network access, credentials and `source.allowed_protocols`; or pass --offline to use the last fetched approved revision (reported as freshness=unverified)
```

The fetch runs inside kb's isolated mirror (see [cache layout](#isolated-mirror-and-cache-layout)):

```text
git fetch --quiet --no-tags --no-recurse-submodules --no-write-fetch-head --end-of-options <url> +<approved_ref>:refs/kb/approved
```

with `-c protocol.allow=never` plus `-c protocol.<p>.allow=always` for each allowed protocol,
hooks disabled, automatic gc and maintenance disabled, `GIT_TERMINAL_PROMPT=0`, and argument
arrays (never a shell). A fetch that loses a ref lock race against another kb process in the
same mirror is retried briefly (at most three attempts).

What is fetched, and from where, comes only from the **local working tree**: `[source]` in
`project/project.toml` (`remote`, default `origin`; `approved_ref`, a fully qualified ref such
as `refs/heads/main`; `allowed_protocols`, default `["https", "ssh"]`) and the URL of that
remote in the KB checkout (`git config remote.<remote>.url`; a relative local path is resolved
against the KB root). A snapshot can never redirect where kb fetches from.

Requirements: the KB root must be the top level of its own Git checkout and the remote must be
configured there. Otherwise online calls fail with `FRESHNESS_UNVERIFIED`; offline, only
`--snapshot working-tree` works.

Commands that do not verify freshness say so: `kb index` never contacts the remote and prints
that freshness is not verified; `kb validate` reads the working tree without Git or network
access unless `--snapshot` names another snapshot.

### `--offline`

`--offline` skips the fetch and uses the last approved tip known locally: the mirror's
`refs/kb/approved` from the last successful fetch, else the KB checkout's remote-tracking ref
`refs/remotes/<remote>/<branch>` (only when `approved_ref` is `refs/heads/<branch>`). Every
result is labeled `freshness=unverified`, which makes context at best `partial`
(`FRESHNESS_UNVERIFIED` reason). If no tip is known, selecting `latest` fails with
`SNAPSHOT_NOT_FOUND` (exit 22). Offline, `approved` is judged against that last known tip.

Every answer states what it was read from. `context` prints its snapshot header; the text
output of `show`, `search` and `impact` starts with the same provenance in one line:

```text
snapshot: 8541b350d6b6 (pinned, freshness=unverified); approved=yes; latest=27dd755e5257; pin=8541b350d6b6
```

It names the revision read, the selection mode, the freshness, whether the revision is
approved, the approved tip known to this call and the host pin (both revisions when they
differ), plus the proposal overlay when there is one. `show --raw` keeps stdout to the exact
file bytes and prints this line to stderr (`kb: snapshot: ...`, even with `--quiet`) when
freshness is unverified or the revision is not approved. JSON results carry it as
`result.snapshot`.

## Selection

Freshness (was the remote checked in this call?) and selection (which snapshot is read?) are
separate. `--snapshot` chooses:

| `--snapshot` | Reads | `approved` |
|---|---|---|
| `auto` (default) | `.kbw.toml selection` when set; otherwise, when the host pins a KB revision that differs from the approved tip, fails with `UPDATE_REQUIRED`; otherwise `latest` | as below |
| `latest` | the approved tip | yes |
| `pinned` | the host pin: the KB submodule gitlink in the host `HEAD`, else `.kbw.toml pin`; the approved tip is still fetched and reported | yes if the pin is the tip or an ancestor of it, else no |
| `<revision>` | a commit that resolves in the KB checkout or the mirror | yes if reachable from the tip |
| `working-tree` | the KB working tree | never (`WORKING_TREE` reason) |

A revision that is not reachable from the approved tip gives `NOT_APPROVED`; with no known tip
the approval is `APPROVAL_UNKNOWN`. Both make context at best `partial`. A pin is never
reported as latest: the output shows the selection mode and both revisions.

With the approved tip one commit ahead of the host's submodule pin, the default `auto`
selection refuses to choose silently (exit 21):

```text
error[UPDATE_REQUIRED]: the host pins KB revision 8541b350d6b6 but the latest approved revision is 27dd755e5257
  details: {"latest_approved":"27dd755e525733d9087b0f3ab2b21e077802f9ff","pin":{"path":".kb","revision":"8541b350d6b6efb586079fa74f9c02e07aa40124","source":"submodule"}}
  hint: select --snapshot pinned to use the pinned knowledge (both revisions are reported), --snapshot latest to use the approved tip, or update the host pin through review
```

`--snapshot pinned` then reads the pinned knowledge and still verifies freshness:

```text
snapshot: 8541b350d6b6 (pinned, freshness=verified); approved=yes; ref=origin refs/heads/main; source=<kb-origin.git>; latest=27dd755e5257; pin=8541b350d6b6; key=3bc2e6131ef0
host: repo=mobile (binding-file); head=f1e239d3b197; versions=mobile=2.1.0
```

Choose explicitly: `pinned` when the host code must be checked against the knowledge it was
reviewed with (an older release branch, the shipped host CI templates, which run
`kbw impact --snapshot pinned --check`), `latest` when the newest approved knowledge should
apply. A host can make the choice permanent with `selection = "pinned"` or `"latest"` in
`.kbw.toml`; the generated skill passes an explicit `--snapshot` when the skill setting
`snapshot` is `latest` or `pinned` and nothing for `auto`.

`kb doctor` reports the pin relation as the `host-pin` check, worded for the host's
`.kbw.toml selection`: with `pinned`, `--snapshot auto` reads the pin; with `latest`, it reads
the approved tip (`last known approved tip` when the run did not fetch it); without a
selection it returns `UPDATE_REQUIRED` for a pin that is behind, ahead or diverged. Its hint
suggests `--snapshot latest` only when this engine can read the approved tip.

A pinned commit that is missing from the mirror is copied from the KB checkout, else fetched
from the remote by commit id (this works only when the server allows fetching by id). If it
cannot be found, or `pinned` is requested without a pin, the result is `SNAPSHOT_NOT_FOUND`.

## Host detection

The host is the repository the developer works in. kb identifies it from Git, never from
folder names:

1. `--host <dir>` wins (it must be inside a Git work tree, else `INVALID_INPUT`).
2. Otherwise the Git top level of the current directory.
3. If that is the KB checkout itself, the superproject when the KB is a submodule, else no
   host.

**Pin.** When the KB checkout lies inside the host work tree, the gitlink at its path in the
host `HEAD` tree is the pin (`source: submodule`). **Linked worktrees** are supported: the host
root is the linked worktree, and a KB checkout that is a submodule of another worktree of the
same repository (same common Git directory) is found at its path relative to that worktree;
the pin is read from the linked worktree's own `HEAD`:

```text
  host:           <scratch>/docs-host-wt (linked worktree)
  host pin:       8541b350d6b6 (submodule .kb): 1 commits behind approved
```

Without a submodule gitlink, `.kbw.toml pin` is used (`source: binding-file`).

**`.kbw.toml`** (optional, host root, strict TOML, at most 64 KiB, never read through a
symlink):

```toml
schema = 1
repo = "mobile"          # registry repo id of this host
pin = "<KB commit>"      # explicit pin for setups without a submodule
selection = "pinned"     # default for --snapshot auto: auto, latest or pinned
```

Invalid content is `CONFIG_INVALID` (exit 11), another `schema` is
`UNSUPPORTED_SCHEMA_VERSION` (exit 13).

**Host repo id.** `.kbw.toml repo` when the registry defines it; otherwise the first host
remote (sorted by name) whose URL matches a `remotes` entry in `registry/repos.toml` after
normalization (scheme, user info and a trailing `.git` or `/` removed, `host:path` scp syntax
rewritten to `host/path`, the host name lowercased). A `.kbw.toml repo` that the registry does
not define produces `HOST_BINDING_REPO_UNKNOWN` (in `meta.diagnostics`) and `HOST_REPO_UNKNOWN`
(in context `issues`). Without an identified repo and without `--repo`, context reports
`REPO_UNKNOWN` (`partial`).

**Host version.** The first line of the identified repo's `version_file` in the host (semver,
optional leading `v`). Missing, unsafe or unparsable files leave the version unknown. Only the
host repo's version is detected; pass `--host-version repo=x.y.z` for other repos.

## Engine compatibility

Before interpreting any knowledge of a snapshot, kb reads `core/release.toml` **at that
snapshot** and compares `engine_version`, `document_schema`, `protocol` and `index_schema`
with the running engine. A mismatch, or a missing or invalid manifest, is `UPDATE_REQUIRED`
(exit 21), and nothing of that snapshot is read:

```text
error[UPDATE_REQUIRED]: snapshot 99aad8466361f209c68b4f19340275d7c0adeb7f requires a different engine than the running runtime
  details: {"mismatches":[{"field":"engine_version","runtime":"0.1.0","snapshot":"0.2.0"}],"revision":"99aad8466361f209c68b4f19340275d7c0adeb7f"}
  hint: update the KB checkout to that revision through review, then bootstrap its engine explicitly with `./kbw --kbw-bootstrap` or `./kbw --kbw-install-artifact` (kbw never builds implicitly), or select --snapshot pinned
```

Reading commands never build, install or switch engines and never regenerate skills. A
mismatch of the caller's `--skill-protocol` is a separate error, `SKILL_OUTDATED` (exit 23):
the running agent must re-read the skill or start a new session.

`kb doctor` checks this ahead of time (check `snapshot-engine`): it reads `core/release.toml`
of the approved tip and of the host pin from the mirror (never the KB checkout) and fails
when the revision `--snapshot auto` selects needs another engine, warns when only the other
one does, and is skipped when no tip or pin is known locally. `details.revisions[]` lists
each revision with its `role` (`latest`, `pinned`), `status` (`ok`, `incompatible`,
`unknown`) and the `mismatches`.

## Isolated mirror and cache layout

kb writes only into its cache. The KB checkout and the host are read-only for every reading
command: no pull, merge, rebase, reset, checkout, stash or submodule update, and no ref of the
KB repository is written. `kb sync` states this and checks it:

```text
  nothing modified: kb writes only to its isolated mirror cache; the KB checkout (HEAD, branches, index, working tree) and host pins were left as they were
```

```text
<cache>/                                  $KB_CACHE_DIR, default <KB root>/.cache
  git/<16 hex>.git                        bare mirror of one approved source
  index/<profile>.sqlite                  SQLite FTS5 index (+ -wal, -shm)
  index/<profile>.sqlite.lock             lock for index initialization and recovery
  index/<profile>.sqlite.corrupt-<pid>-<nanos>   damaged index kept for inspection
  wt-stat-<profile>.json                  working-tree stat cache
<KB root>/.cache/runtime/<fingerprint>/   engine runtime managed by kbw
```

The mirror name is the first 16 hex characters of a SHA-256 over the normalized source
identity and the approved ref, so different projects and sources never share a mirror. Refs in
the mirror:

| Ref | Content |
|---|---|
| `refs/kb/approved` | the approved tip of the last successful fetch |
| `refs/kb/local/head` | the KB checkout's `HEAD`, copied for merge-base and overlay computations |
| `refs/kb/local/tracking` | the checkout's remote-tracking tip, copied for offline use |
| `refs/kb/selected/<sha>` | pinned or explicit revisions copied into the mirror |

Mirror commands run without hooks and without automatic gc. An unusable mirror directory is
moved aside (`<id>.git.unusable-<unique>`) and recreated. The mirror only grows; there is no
pruning command. The downstream `.gitignore` must ignore `.cache/`.

## Proposal overlay

With `--include-proposals` on a Git snapshot, kb computes the local knowledge changes that are
not yet approved: every file under the profile config, registry and knowledge roots that
differs between `merge-base(local HEAD, approved tip)` and the working tree, whether committed
on a local branch, staged, unstaged or untracked (`git ls-files --others --exclude-standard`).
A working-tree snapshot already contains all local changes and gets no overlay.

* Overlay records are indexed next to, never instead of, the accepted snapshot and appear as a
  labeled `proposal` tier (`proposal:new`, `proposal:modifies`, `proposal:removes`, `stale`);
  see [context.md](context.md#proposals).
* Changes outside the knowledge roots (registry, `project.toml`) and non-record files are
  never applied: `PROPOSAL_NOT_APPLIED` (warning). Registry and configuration changes take
  effect only after review and merge.
* A file byte-identical to the accepted file at the same path is not a proposal.
* `PROPOSAL_REPLACES_RECORD` (warning): a changed file now holds a different record id than
  the accepted file at that path. `PROPOSAL_UNREADABLE` (error): the changed content is
  missing. `PROPOSAL_INVALID` (context warning): the proposal does not parse.
* A symlink among the changed files fails the call with `UNSAFE_PATH`, a file above 4 MiB with
  `INVALID_INPUT`.
* `stale`: the approved tip changed the same file after the merge-base.

## Snapshot identity

The snapshot key is a SHA-256 over: profile, profile config path, normalized source identity,
approved ref, content (the commit id, or a digest of the working-tree listing), engine version,
index schema version, parser version and the overlay digest (its base commit and each changed
file's path, status, content id and stale flag). The working-tree digest covers the path and
content id of every file under the profile's config, registry and knowledge roots, plus
`core/release.toml`.

Freshness and the selection mode are not part of the key. The same commit read as `latest`,
as `pinned` or offline yields the same key (`3bc2e6131ef0…` in the examples above), so its
index entry is reused; the output still reports how it was selected and verified.

## Index behavior

The index is a rebuildable cache; Git-tracked text is the source of truth.

* **Content-addressed.** Documents are stored by content id (the Git blob id for Git
  snapshots, `sha256:` for working-tree files) and parser version. A new snapshot parses only
  content no earlier snapshot had: after one record changed, the build reported
  `21 record files, 2 parsed, 20 reused`. Snapshots, branches and overlays never mix; each
  snapshot owns only membership rows.
* **Transactional.** A snapshot is built in one `BEGIN IMMEDIATE` transaction, so a reader
  sees either no snapshot or a complete one; a read view holds one read transaction for its
  whole lifetime. The database uses WAL mode and a 30-second busy timeout.
* **Working-tree stat cache.** Content ids of unchanged working-tree files are reused from
  `wt-stat-<profile>.json`, keyed by size, mtime, ctime and inode. As in Git's index, entries
  whose mtime is within 2 seconds of the cache write are re-hashed.
* **Frozen working tree.** A working-tree snapshot is keyed and built from the same listing.
  Every byte read is checked against the listed content id; a file that changes, appears or
  disappears during the call fails it with a retryable `IO_ERROR`
  (`` `<path>` changed while it was being read ``) and nothing is stored, so a key never names
  other content.
* **Corruption recovery.** A damaged database (corrupt, not a database, a database without
  kb metadata, unreadable metadata, a missing table) or one that fails `PRAGMA quick_check`
  after a query error is moved aside, rebuilt, and the query retried once. The same retry
  covers a snapshot that another process removed meanwhile. Recovery is never silent:

  ```text
  kb: warning[INDEX_RECOVERED]: the index cache was unusable (unreadable database: file is not a database); the damaged file was moved to <scratch>/docs-ctx/.cache/index/project.sqlite.corrupt-55918-1790539208071307000
  ```

  The warning also appears in the JSON envelope `meta.diagnostics`; the context result itself
  is unaffected.
* **Outdated index.** An index whose metadata (`index_schema`, table `layout`,
  `parser_version`, `engine_version`, `profile`) differs or lacks a key, for example one
  written by another engine or before a key existed, is rebuilt in place and reported as info
  `INDEX_REBUILT` (`meta.index.recovery.kind = outdated`); nothing is moved aside.
* **Retention.** After each new build the 8 most recently built snapshots are kept.
  `kb index --gc` removes snapshots older than the selected one and unreferenced documents;
  `kb index --rebuild` drops the cache first.
* **Safe queries.** Search text is tokenized and every token is quoted as an FTS5 string; raw
  FTS syntax and SQL are never passed through.

## Offline work and recovery

| Situation | What happens | What to do |
|---|---|---|
| No network, remote down, credentials or transport refused | `FRESHNESS_UNVERIFIED` (20) | fix access or `source.allowed_protocols`; or decide to work with `--offline` (results are at best `partial`) |
| `--offline` and nothing was ever fetched | `SNAPSHOT_NOT_FOUND` (22) | run `./kbw sync` once while online, or read local files with `--offline --snapshot working-tree` (never approved) |
| Host pin differs from the approved tip | `UPDATE_REQUIRED` (21) under `auto` | choose `--snapshot pinned` or `--snapshot latest`, set `.kbw.toml selection`, or update the pin through review |
| Selected snapshot needs another engine | `UPDATE_REQUIRED` (21) | update the KB checkout through review, then `./kbw --kbw-bootstrap` (or `--kbw-install-artifact`) explicitly |
| Runtime for this checkout not built | `KBW_RUNTIME_NOT_BOOTSTRAPPED` (50, from `kbw`) | `./kbw --kbw-bootstrap` |
| Files changed while a working-tree call was reading them | `IO_ERROR` (62) | run the command again |
| Damaged index | `INDEX_RECOVERED` warning; rebuilt automatically | nothing; the damaged file is kept for inspection. `./kbw index --rebuild` forces a rebuild |
| Unusable mirror directory | moved aside and recreated | nothing; the next online call fetches again |

`./kbw sync` fetches and reports the approved tip, the local KB checkout (ahead, behind, dirty)
and the host pin (`current`, `behind`, `ahead`, `diverged`, `no-pin`, `unknown`) without
changing anything. `./kbw doctor --online` also checks the remote (it cannot be combined
with `--offline`), and `./kbw doctor` reports the `host-pin` and `snapshot-engine` checks
described above.

## Trust model

* **The approved ref is the trust boundary.** Knowledge counts as approved only when it is
  reachable from the configured `approved_ref` of the configured remote, verified in this
  call. Keeping that ref reviewed (protected branches, required approvals, merge rights) is the
  job of the Git host and its administrators, outside kb. kb does not call provider APIs to
  check approvals, and the presence of CI or MR templates guarantees nothing by itself.
* **`status = "accepted"` alone proves nothing.** Anyone can write `accepted` in a file on any
  branch. Only content from a verified approved revision yields `complete`; the same accepted
  record read from the working tree, an unapproved revision or offline yields `partial`
  (`WORKING_TREE`, `NOT_APPROVED`, `FRESHNESS_UNVERIFIED`). A branch name proves nothing
  either.
* **Records cannot raise their own trust.** The source settings come only from the local
  working-tree config, never from the snapshot being read. A record becomes mandatory only
  through its scope and status; proposals never override accepted records; registry and
  configuration changes in the overlay are not applied.
* **Explicit transport policy.** Fetches allow only the protocols in `allowed_protocols`
  (known names: `https`, `ssh`, `git`, `file`, `http`; default `https` and `ssh`), with
  `protocol.allow=never` for everything else, no tags, no submodule recursion and no
  interactive prompts. kb stores no credentials of its own.
* **Credential redaction.** User info in URLs is printed as `***@` and token-shaped words
  (`ghp_`, `gho_`, `ghs_`, `ghu_`, `github_pat_`, `glpat-`, `xoxb-`, `xoxp-` prefixes) as `***`
  in errors and in the `source` field; see the `FRESHNESS_UNVERIFIED` example above.
* **Documents are data.** kb never executes commands, procedures, hooks or download
  instructions found in records. Paths are validated, symlinks are refused, and text output
  escapes control and bidirectional characters. `kb show --raw` returns the exact bytes.
* **Typing does not make text safe or true.** Typed, atomic fields reduce ambiguity, but a
  record's natural-language text can still try to instruct an agent (prompt injection), and
  a valid schema does not establish that the knowledge is correct. Harness and system
  instructions take precedence over knowledge content; review knowledge changes like code.
  A test anchor does not prove the test runs, and a receipt proves delivery, not compliance.
