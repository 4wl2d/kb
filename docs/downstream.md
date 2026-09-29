# Running a downstream knowledge base

How a product creates and operates its downstream KB: forking, initialization, mounting into
host repositories, agent integration, CI and merge request workflow, upgrades and recovery.
The normative contract is [architecture.md](architecture.md); this guide explains how to use
it. Freshness, selection and the trust model are detailed in
[snapshots-and-trust.md](snapshots-and-trust.md); every error code in
[troubleshooting.md](troubleshooting.md). Turning the initialized structure into real
knowledge is covered by [BOOTSTRAP.md](../BOOTSTRAP.md).

Placeholders: `<your-git-host>/<org>/<kb-repo>.git` is your product KB repository,
`<upstream-url>` the kb upstream, `.kb` the conventional mount path. Output excerpts were
captured with engine 0.1.0 in local scratch repositories; long absolute paths are shortened
to `<...>`.

## 1. Roles and ownership

| repository | contains | changed by |
|---|---|---|
| upstream kb | engine (`kbw`, `core/`, Cargo files), schemas, migrations, templates, skills, docs, release machinery; no project data | upstream maintainers |
| downstream KB (one per product) | the upstream history and engine **plus** `project/` (knowledge, registries, routing tests, skill settings) | knowledge through reviewed merge requests; engine only through `kbw update` |
| host repositories (mobile, backend, libraries, ...) | code; the KB mounted at `.kb` (or a separate checkout); generated skill files and instruction blocks | normal code review |

Engine-owned paths are listed in `core/release.toml` (`engine_paths`: `kbw`, Cargo files,
`rust-toolchain.toml`, `core`, `docs`, `README.md`, `BOOTSTRAP.md`, `AGENTS.md`,
`CONTRIBUTING.md`, `SECURITY.md`, `CHANGELOG.md`, `LICENSE`, `.gitignore` and the two
upstream workflows). Project-owned: `project/`. Edits to engine paths in the downstream are
reported by `./kbw update divergence` (`ENGINE_DIVERGED`, exit 43) unless declared as
`[[engine_patches]]` with a reason in `project/upstream.toml`. Put project documents in
`project/` or in paths outside `engine_paths`. The downstream security policy is one of
them: render `core/templates/security/SECURITY.md.tmpl` to `.github/SECURITY.md` (outside
`engine_paths`, so no divergence) and never edit the root `SECURITY.md`, which covers the
engine and is updated with it. GitHub looks for the policy in `.github/`, then the repository
root, then `docs/`, so reporters see the downstream policy rather than the engine's; on other
hosts, link `.github/SECURITY.md` from your own documentation.

## 2. Create the downstream

Create one repository per product that keeps the upstream history (a platform fork or a
private copy). Do not squash or re-initialize the history: `kbw update prepare` merges
upstream commits and `kbw update divergence` compares against the upstream commit recorded
in `project/upstream.toml`.

```sh
git clone <upstream-url> <kb-repo> && cd <kb-repo>
git remote rename origin upstream
git remote add origin <your-git-host>/<org>/<kb-repo>.git
git push -u origin main          # publish the product repository with the full history
```

`origin` must be the product repository: it is the approved source that every knowledge
query checks. The `upstream` remote is only a convenience; `kbw update` takes the upstream
URL explicitly.

## 3. Initialize and adapt

```sh
./kbw --kbw-bootstrap                                                    # explicit runtime build
git switch -c kb/init
./kbw init --name "<Product>" --namespace <ns> --upstream-url <upstream-url>          # dry-run
./kbw init --name "<Product>" --namespace <ns> --upstream-url <upstream-url> --apply
git add project .github/workflows/kb-knowledge.yml
git commit -m "Initialize the project knowledge base"
```

Run `init` right after forking: it records the current `HEAD` as the upstream base
(`revision` in `project/upstream.toml`). Options (`--remote`, `--approved-ref`, `--kb-path`,
`--harness`) and the created files are listed in [BOOTSTRAP.md](../BOOTSTRAP.md#2-initialize-project).
Then adapt the knowledge base with BOOTSTRAP.md (manually or with the
[adaptation prompt](prompts/adaptation.md)) and merge the result through review.

## 4. Mount the KB in host repositories

kb identifies the host repository through Git, never through folder names: the Git top
level of the current directory (or `--host`), or the superproject when you run inside the
KB submodule itself. The registry repo of the host comes from `.kbw.toml repo = "<id>"` or
from matching the host's remote URLs against `remotes` in `project/registry/repos.toml`
(scheme, credentials and a trailing `.git` are ignored). Linked worktrees of a host are
supported.

### 4.1 Git submodule (conventional `.kb`)

```sh
# in the host repository, on a branch
git submodule add <your-git-host>/<org>/<kb-repo>.git .kb
git commit -m "Mount the knowledge base at .kb"
.kb/kbw --kbw-bootstrap           # once per clone and engine version; .kb/.cache is git-ignored
```

Other developers run `git submodule update --init -- .kb` and `.kb/kbw --kbw-bootstrap`.
The gitlink recorded in the host's `HEAD` is the **host pin**. kb reads it and never
changes it: reading commands do not pull, merge, reset, check out or update submodules.

```text
$ .kb/kbw context --intent implement --task "Retry the token refresh" --path app/auth/TokenRefresher.kt
snapshot: c0e732dff499 (latest, freshness=verified); approved=yes; ref=origin refs/heads/main; ...; latest=c0e732dff499; pin=c0e732dff499; ...
host: repo=mobile (remote); head=319558f5b7ea; versions=mobile=2.3.0
scope: repos=mobile; modules=mobile.auth; features=login; concepts=auth-token (alias `token refresh`)
status: COMPLETE
```

### 4.2 Separate checkout with `.kbw.toml`

Without a submodule, bind the host with an optional `.kbw.toml` in the host root (strict
TOML; unknown fields are errors):

```toml
schema = 1
repo = "backend"          # registry repo id (otherwise matched by remote URL)
pin = "6a434be45089"      # KB revision this host is tested with (resolved in the KB checkout)
selection = "pinned"      # default for --snapshot auto: auto | latest | pinned
```

Run the KB's launcher from inside the host (`<kb-checkout>/kbw context ...`) or pass
`--host <dir>`. The generated skill and instruction blocks tell agents to run
`<kb_path>/kbw`, where `kb_path` is host-relative and may not contain `..`. If the checkout
is not at that path, `kbw integrate` warns `KB_PATH_MISMATCH`; the simplest layout is a
plain clone at `kb_path` inside the host, excluded from the host's Git (for example in
`.git/info/exclude`), with the pin in `.kbw.toml`.

### 4.3 Snapshot selection

Freshness and selection are separate. Every `context`, `show`, `search` and `impact` call
fetches the approved ref into an isolated mirror (`.cache/git/`); `--snapshot` chooses what
to read:

| `--snapshot` | reads | notes |
|---|---|---|
| `auto` (default) | `.kbw.toml selection` if set; else the approved tip, unless a host pin differs from it | a pin that differs from the tip → `UPDATE_REQUIRED` (exit 21) with both revisions |
| `latest` | the approved tip | the pin is still reported; never called "latest" itself |
| `pinned` | the host pin (gitlink or `.kbw.toml pin`) | freshness is still verified; the tip is reported |
| `<revision>` | that commit | not reachable from the approved tip → `approved=no`, at best `partial` |
| `working-tree` | the KB working tree | never approved; at best `partial` |

```text
$ .kb/kbw context --intent implement --task "Retry the token refresh" --path app/auth/TokenRefresher.kt
error[UPDATE_REQUIRED]: the host pins KB revision c0e732dff499 but the latest approved revision is 6a434be45089
  details: {"latest_approved":"6a434be4508908b10841c7f6f908bdd39e050373","pin":{"path":".kb","revision":"c0e732dff499c1d3a9d43db3ba81fa6acc6fa002","source":"submodule"}}
  hint: select --snapshot pinned to use the pinned knowledge (both revisions are reported), --snapshot latest to use the approved tip, or update the host pin through review
```

The `snapshot` setting of `project/skill-config/skill.toml` controls what agents pass:
`latest` or `pinned` is written as `--snapshot` into every command the generated skill
shows; `auto` (the default) writes nothing, so the host's `.kbw.toml selection` or the
`auto` rule decides.

Host detection and selection rules in full: [snapshots-and-trust.md](snapshots-and-trust.md#selection).

## 5. Host integration and managed blocks

The KB carries a committed skill bundle in `project/skill-config/generated/` (rendered by
`./kbw integrate --generate --apply` from `core/skills/` and `skill.toml`). Each host
installs it with the KB's launcher; dry-run is the default:

```text
$ .kb/kbw integrate --apply
kb integrate (apply): host <...>/mobile
  create    .agents/skills/kb/SKILL.md
  create    .agents/skills/kb/references/harnesses.md
  ...
  create    .claude/skills/kb/SKILL.md
  ...
  create    AGENTS.md [block kb-instructions]
  create    CLAUDE.md [block kb-instructions]
  create    .kbw/integration.lock
host integration written; review and commit it (including the lock)
```

| harness | skill directory | instruction file |
|---|---|---|
| Claude Code | `.claude/skills/kb/` | `CLAUDE.md` |
| Codex | `.agents/skills/kb/` | `AGENTS.md` |
| Cursor | `.agents/skills/kb/`, or `.claude/skills/kb/` when Claude Code is enabled | `AGENTS.md` |

* Instruction files change only between `<!-- kb:begin kb-instructions -->` and
  `<!-- kb:end kb-instructions -->`; every other byte is kept, and missing files are created.
* `.kbw/integration.lock` records what kb installed. A generated file or block edited by
  hand is a conflict: `--apply` writes nothing and fails with `CONFLICT` (exit 45);
  `--check` fails with `DRIFT_DETECTED` (exit 42). `--apply --force` overwrites explicitly.
  Project-specific additions belong in `notes` of `skill.toml`, not in generated files.
* A second `--apply` is a no-op ("host integration is up to date; nothing written").
* Enabling Claude Code and Codex together installs both directories, so Cursor sees two
  identical skills; the skill's `references/harnesses.md` explains the trade-off and the
  sources.

Instruction files and skills are text: they make the protocol available to agents but do
not enforce it, and kb uses no undocumented harness hooks.

## 6. CI and merge request templates

| template (`core/templates/`) | install as | checks |
|---|---|---|
| `ci/github/kb-knowledge.yml` | KB: `.github/workflows/kb-knowledge.yml` (created by `init`) | bootstrap, `validate`, `validate --base origin/<target>` on PRs, `integrate --generate --check`, `update divergence`, `schema --check` |
| `ci/gitlab/kb-knowledge.gitlab-ci.yml` | KB: include or copy into `.gitlab-ci.yml` | the same |
| `ci/github/host-kb-impact.yml` | host: `.github/workflows/kb-impact.yml`; set `KB_PATH` | submodule checkout (no recursion), bootstrap, `impact --base origin/<target> --statement <description> --snapshot pinned --check`, `integrate --check` |
| `ci/gitlab/host-kb-impact.gitlab-ci.yml` | host: copy the job into `.gitlab-ci.yml` | the same |
| `mr/github_pull_request_template.md` | host and KB: `.github/pull_request_template.md` | `kb-impact` block and linked-MR checklist |
| `mr/gitlab_merge_request_template.md` | host and KB: `.gitlab/merge_request_templates/Default.md` | the same |
| `ownership/CODEOWNERS.tmpl` | adapt by hand; owners are explicit parameters | none |
| `security/SECURITY.md.tmpl` | KB: `.github/SECURITY.md` (never the engine-owned root `SECURITY.md`); the contact is an explicit parameter | none |

Host CI reads the pinned KB revision (`--snapshot pinned`) because `auto` returns
`UPDATE_REQUIRED` whenever the approved tip is ahead of the pin, which is the normal state
between merging knowledge and updating the pin. Freshness is still verified.

The `kb-impact` block (exactly one per description) is how a merge request acknowledges its
knowledge impact:

```markdown
<!-- kb-impact:v1
kb_change = "linked"
reason = "The retry behavior is captured by the refresh-backoff invariant in the KB merge request."
change_id = "AUTH-42"
-->
```

| field | rule |
|---|---|
| `kb_change` | `none` (no knowledge affected), `linked` (a separate KB merge request), `included` (this change updates the KB submodule pointer) |
| `reason` | required, non-empty, in every mode |
| `kb_revision` | optional, 7–64 hex characters; not allowed with `none` |
| `change_id` | optional shared id, 1–128 characters on one line; `linked` needs `kb_revision` or `change_id` |

`kbw impact --check` fails with `IMPACT_UNACKNOWLEDGED` (exit 44) when the diff touches
knowledge-linked files or files with unknown coverage and neither the pointer changes nor a
valid block acknowledges it, when the block is malformed, and when it contradicts the diff
(`none` with a pointer change, `included` without one while a KB submodule is known). It
checks presence and structure only; reviewers judge the reasoning. Unknown coverage means
that no record is linked to a file, not that no knowledge is needed.

```text
$ .kb/kbw impact --base main --snapshot pinned --check
snapshot: c0e732dff499 (pinned, freshness=verified); approved=yes; latest=6a434be45089; pin=c0e732dff499
...
summary files=2 covered=1 unknown=1 affected=8 stale-anchors=0
...
error[IMPACT_UNACKNOWLEDGED]: 8 affected record(s) and 1 file(s) with unknown coverage are not acknowledged: ...
$ .kb/kbw impact --base main --snapshot pinned --check --statement mr.md
check ok acknowledgement-required=true: acknowledged: linked KB change (change_id AUTH-42); the link itself is not verified
```

The GitHub host workflow also triggers on `edited`, so fixing the block re-runs it. On GitLab,
editing the description does not start a pipeline, and long descriptions are truncated in
`CI_MERGE_REQUEST_DESCRIPTION`, so keep the block near the top. The template comments state
these prerequisites.

## 7. Linked merge requests

Knowledge is written together with the implementation, reviewed before it is accepted, and
published by merging into the approved ref. When code and knowledge change together:

1. **Prepare knowledge.** In the KB (for example in the host's `.kb` checkout, on a new
   branch), write draft records or edits with evidence, run `.kb/kbw validate`, and open the
   KB merge request.
2. **Test the code with that KB revision.** From the host:
   `.kb/kbw context --include-proposals ...` (local KB changes as a labeled overlay, never
   mandatory) or `.kb/kbw context --snapshot <kb-commit> ...` (that exact commit; reported as
   `approved=no` and at best `partial` until it is merged).
3. **Open the host merge request** with `kb_change = "linked"` and a `change_id` shared by
   both merge requests (or the KB commit as `kb_revision` once it exists).
4. **Merge the reviewed knowledge** into the approved ref. Automatic edits made after
   approval are not reviewed knowledge.
5. **Update the host pin through review**, in the host merge request or a follow-up:

   ```sh
   git -C .kb fetch origin
   git -C .kb checkout --detach origin/main
   git add .kb && git commit -m "Update the KB pin (AUTH-42)"
   ```

   With the pointer updated in the diff, the block becomes `kb_change = "included"`:

   ```text
   impact base 319558f5b7ea merge-base 319558f5b7ea head ead6bd607662 repo mobile kb-pointer c0e732dff499 -> 6a434be45089
   check ok acknowledgement-required=false: acknowledged: the KB change is included (KB pointer updated)
   ```

Rules: reference only commits that already exist (the KB commit from the host, never a host
SHA from the KB), so there are no cyclic SHA dependencies; use `change_id` to connect the
two requests. Merges in two repositories are never atomic: until the pin update lands, the
host runs with the previous KB revision, and several hosts may pin different revisions.

## 8. Routine sync versus upstream upgrade

These are different operations in the same downstream:

| | routine sync of the project origin | upstream upgrade |
|---|---|---|
| what moves | knowledge merged into the product KB's approved ref | engine, schemas, migrations, templates, skills from upstream |
| how | automatic per-call freshness; `kbw sync` to inspect; pin updates in hosts through review | `kbw update check` → `update prepare` → review → merge |
| writes | only kb's isolated mirror cache | `prepare` only: the ref `refs/kb/upstream/<ref>`, a `kb-update/<ref>` branch and a worktree under `.cache/update/` |

`kbw sync` fetches the approved ref and reports the tip, the local KB checkout and the host
pin; it modifies nothing else:

```text
$ .kb/kbw sync
kb sync: refs/heads/main on origin (<...>/kb-demo-origin.git)
  freshness:      verified (fetched now)
  approved tip:   6a434be45089 (unchanged)
  local KB HEAD:  c0e732dff499 on main: 0 ahead, 1 behind approved
  working tree:   clean
  host:           <...>/mobile
  host pin:       c0e732dff499 (submodule .kb): 1 commits behind approved
  nothing modified: kb writes only to its isolated mirror cache; the KB checkout (HEAD, branches, index, working tree) and host pins were left as they were
```

Upstream upgrades run in the KB checkout:

```sh
./kbw update check --upstream <upstream-url> --ref <tag-or-commit>     # read-only prediction
./kbw update prepare --upstream <upstream-url> --ref <tag-or-commit>   # reviewable branch
./kbw update divergence                                                # engine drift report
./kbw update abandon kb-update/<ref>                                   # dry-run: what would be removed
./kbw update abandon kb-update/<ref> --apply                           # discard a prepared update
```

```text
$ ./kbw update check --upstream <...>/upstream --ref v0.1.1
update check: upstream v0.1.1 = 323c06a5ab5e (from <...>/upstream)
  versions: unchanged
  schema: project [1] -> target 1 (migrates from [0])
  merge with HEAD 6a434be45089: clean
  engine divergence: 0 path(s), 0 declared patch(es)
```

* `check` fetches the upstream into an isolated cache and predicts the merge with
  `git merge-tree`; it writes nothing to the KB repository. Unsupported project schema →
  `UNSUPPORTED_SCHEMA_VERSION` (13); predicted conflicts → `UPDATE_CONFLICT` (51).
* `prepare` creates the branch `kb-update/<ref>` (or `--branch kb-update/<name>`) in a linked
  worktree under `.cache/update/`, merges the upstream commit with hooks disabled, then runs
  the target engine there: explicit `kbw --kbw-bootstrap` (a source build), `migrate
  --apply`, `integrate --generate --apply`, `validate`; it records the new base in
  `project/upstream.toml` and commits. The main checkout (HEAD, index, work tree, other
  branches, runtime) is not modified and nothing is pushed. Conflicts stop with
  `UPDATE_CONFLICT` and leave the worktree for manual resolution; kb never resolves
  semantic conflicts. A failing step stops with `UPDATE_FAILED` (53).
* Then: review `git diff HEAD...kb-update/<ref>`, push the branch yourself, open a merge
  request (the `kb-knowledge` CI runs), merge after review, and remove the worktree
  (`git worktree remove`).
* To discard an attempt instead, `update abandon <branch>` first shows what it would remove
  (the commit, the worktree, the number of uncommitted files in it and the commits not in
  `HEAD`); `--apply` removes the worktree and deletes the branch. A worktree with
  uncommitted, untracked or conflicted files, such as one left by `UPDATE_CONFLICT`, is
  refused with `CONFLICT` (45) unless you add `--force` (`--apply --force`). Only
  `kb-update/*` branches whose worktree is under `.cache/update/` are removed.
* Both commands fetch under the project's transport policy (`allowed_protocols` in
  `project/project.toml`); a local upstream path needs `"file"` there. `--offline` does not
  apply to them.

## 9. Coordinated upgrades

One reviewed update branch carries the engine, the migrated knowledge, the schema versions
and the regenerated skill together. After it is merged, hosts still pin older KB revisions:

* `auto` in a host whose pin is behind returns `UPDATE_REQUIRED`. When the update changed
  the engine version or a contract version, `--snapshot latest` with the host's older
  engine returns `UPDATE_REQUIRED` too: kb reads `core/release.toml` of a snapshot before
  interpreting it and refuses a different engine version, document schema, index schema or
  protocol. `--snapshot pinned` keeps working. `kbw doctor` shows this in advance: its
  `snapshot-engine` check reads `core/release.toml` of the approved tip and of the host pin
  and fails when the revision `--snapshot auto` would read needs another engine.
* Each host upgrades in its own reviewed pin update: move `.kb` to the new revision (as in
  §7); if the update changed engine build inputs, run `.kb/kbw --kbw-bootstrap` (the engine
  fingerprint changed, and `kbw` reports `KBW_RUNTIME_NOT_BOOTSTRAPPED` until you do); run
  `.kb/kbw integrate --apply` for the regenerated skill; commit the pointer and the
  integration files together. Host CI (`integrate --check`) catches a forgotten step.
* Agents started with an older skill fail with `SKILL_OUTDATED` (exit 23) once the skill
  protocol changes; they must re-read the skill or start a new session, because changing a
  file does not update instructions a running model already loaded.
* Knowledge-only commits do not change the engine fingerprint, so routine pin updates need
  no new bootstrap.

Instead of a source build, a runtime can be installed from a release archive built by the
upstream release workflow: `.kb/kbw --kbw-install-artifact <archive-or-https-url> --sha256
<hex>` (or `--sha256-file SHA256SUMS`). The digest is verified before extraction and the
archive's `BUILD-INFO` must match this checkout's engine fingerprint and platform. No
release has been published yet.

## 10. Offline and failure recovery

| situation | what you see | what to do |
|---|---|---|
| origin unreachable (network, VPN, credentials) | `FRESHNESS_UNVERIFIED` (exit 20); no automatic fallback | fix access; or accept unverified knowledge explicitly with `--offline` |
| explicit `--offline` | `freshness=unverified`, completeness at best `partial`, exit 30 | uses the last fetched approved revision (mirror), else the KB checkout's remote-tracking ref; report it as unverified |
| host pin differs from the tip | `UPDATE_REQUIRED` (21) | `--snapshot pinned` explicitly, or update the pin through review |
| requested revision missing | `SNAPSHOT_NOT_FOUND` (22) | check the revision; fetch it into the KB checkout (for example `git -C .kb fetch origin <branch>`) |
| no runtime for this engine | `kbw: error[KBW_RUNTIME_NOT_BOOTSTRAPPED]` (exit 50) | `kbw --kbw-bootstrap` (needs the pinned toolchain and the crates in `Cargo.lock`), or `--kbw-install-artifact` with a local archive path and its digest |
| index cache damaged | warning `INDEX_RECOVERED`, query retried once | nothing; `kbw index --rebuild` if `INDEX_ERROR` (61) persists; `.cache/` is derived data |
| edited generated files | `CONFLICT` (45) / `DRIFT_DETECTED` (42) | move edits into `skill.toml` `notes`, regenerate, re-apply |

More symptoms and fixes: [troubleshooting.md](troubleshooting.md#offline-recovery).

```text
$ .kb/kbw context --offline --intent implement --task "Retry the token refresh" --path app/auth/TokenRefresher.kt
snapshot: 6a434be45089 (latest, freshness=unverified); approved=yes; ...
status: PARTIAL
- [partial] FRESHNESS_UNVERIFIED: the approved ref was not checked against the remote in this call
error[CONTEXT_INCOMPLETE]: context is partial: FRESHNESS_UNVERIFIED
```

Fetches run with `GIT_TERMINAL_PROMPT=0`, so credentials must come from a credential helper
or an SSH agent. `kbw doctor` checks the environment, configuration, runtime, approved
source, host binding and pin, the engine compatibility of the approved tip and the pin
(`snapshot-engine`), index, installed skill and engine divergence; `kbw doctor --online` also
fetches the approved ref (it cannot be combined with `--offline`). Failed bootstraps, installs and updates leave the previous runtime and the
main checkout intact.

## 11. What administrators must configure

The shipped files report problems; they do not grant or enforce anything by themselves.

| where | setting | why |
|---|---|---|
| KB repository | protect the approved ref (default `refs/heads/main`): changes only through reviewed merge requests, no direct or force pushes | the approved ref is the trust boundary; kb does not call provider APIs to verify approvals |
| KB repository | required approvals, and code-owner review if you use CODEOWNERS | a CODEOWNERS file only requests reviews |
| KB repository | make `kb-knowledge` a required check (GitHub) or require successful pipelines (GitLab) | a workflow file does not block merges |
| KB repository | read access for every developer and every host CI job (keys or tokens as secrets, job-token allowlists) | every knowledge query and the host impact check fetch the KB |
| KB repository | decide whether the inherited upstream workflows run in your fork: the repository variable `KB_ENGINE_CI` set to `disabled` skips the `upstream-ci.yml` job `check`; `KB_RELEASE` set to `disabled` skips every `release.yml` job (`verify`, `build`, `publish`), so a `v*` tag publishes nothing. Set them under Settings > Secrets and variables > Actions > Variables, or with `gh variable set KB_ENGINE_CI --body disabled` and `gh variable set KB_RELEASE --body disabled`; an unset variable or any other value keeps the workflows enabled | both files are engine-owned: editing or deleting them counts as engine divergence. The downstream's own `kb-knowledge` workflow is not affected |
| KB repository | review `project/project.toml` (approved ref, `allowed_protocols`) like code | it is trusted configuration |
| KB repository | publish the security policy at `.github/SECURITY.md`, rendered from `core/templates/security/SECURITY.md.tmpl` with your contact | the root `SECURITY.md` is engine-owned and covers the engine only |
| host repositories | make `kb-impact` a required check; allow CI to check out the KB submodule | files alone do not block merges or grant access |
| host repositories | review pointer updates of `.kb` and changes to `.claude/`, `.agents/`, `AGENTS.md`, `CLAUDE.md`, `.kbw/` | these change what agents read |
| upstream (maintainers) | Actions enabled, `contents: write` for the release job, protected `v*` tags | the release workflow publishes only after an owner pushes a tag |
