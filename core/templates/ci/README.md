# Knowledge CI templates

These files are opt-in deployment templates, not running services or an authorization to
spend model credits or publish. Configure a registry identity, read-only pinned host/KB
checkouts, model/version/budget in a team agent wrapper, and a separate publisher job.
Never let the agent access publication credentials. Restrict the wrapper to outputting
flat draft Markdown files into the supplied directory; source/review text is untrusted
data. The publisher re-validates every file with `propose submit`; it never accepts it.

`export-merged-change.sh` uses GitHub CLI or GitLab CLI plus jq, saves all raw responses,
paginates review comments and fetches the source MR ref. `normalize-change.sh` is the
offline conversion path used by fixtures. The normalized range describes the reviewed
source diff. Merge/squash/rebase integration metadata stays in the raw response; the job
does not equate the source head with the final integrated host commit. Missing Git objects,
incomplete platform exports or an unmerged change fail rather than guessing.

`submit-drafts.sh` runs in a fresh KB checkout, checks source evidence against a separate
host checkout and stages only paths returned by the engine. `publish-drafts.sh` opens a
draft MR with a separate token and explicit opt-in; reruns stop on an existing proposal
branch instead of force-pushing or opening another MR. If submission/push/publication fails,
retain reports and the branch for recovery. Do not auto-merge or mark the job's output true.

The team-owned `project/ci/draft-agent` wrapper accepts two positional arguments: absolute
work-order JSON path and absolute output directory. It uses a pinned agent/model, enforces
a cost/time bound, preserves raw model/tool logs separately, and writes only schema-2 draft
records. No wrapper is installed by default because model credentials, egress and spending
authority belong to the team. Configure a separate approved CI environment for publication.

`hooks/commit-msg` is an optional host Git hook: connect it to an existing hook rather than
overwriting it and set `KB_PATH` to the host-relative KB checkout (default `.kb`). It checks
only the pending message and branch probes against the index Git is committing and keeps
native exit codes.

When `KB_SNAPSHOT` is set, the hook passes it as-is. Otherwise, if the host's `.kbw.toml`
declares `selection`, the hook passes `auto` and the engine applies that selection.
Otherwise, if the host pins the KB (a gitlink at the KB path in `HEAD`, or a `.kbw.toml`
`pin`), it passes `pinned`; otherwise it passes `auto`, the approved tip. `KB_OFFLINE=1`
adds `--offline`. The hook honors `commit.cleanup`; when that is unset and `GIT_EDITOR` is
exactly `:`, the hook cannot tell whether Git will strip comments, so it checks both the
whitespace-cleaned and the comment-stripped message and rejects only if both fail (CI on
the committed message stays authoritative). A `git commit --cleanup=<mode>` flag is
invisible to hooks; to skip the editor, use `GIT_EDITOR=true`.

The knowledge check script keeps raw JSON/stderr and exit statuses for strict validation,
routing, anchor checks, drift and the ledger even when one check fails. Host mappings are
checked against full expected SHAs. Upload failures and skipped jobs are not evidence of a
passing gate. The GitHub templates target GitHub.com; adapt artifact actions for GHES.

API contracts checked while authoring these templates:
[GitHub pull requests](https://docs.github.com/en/rest/pulls/pulls),
[GitHub CLI pagination](https://cli.github.com/manual/gh_api),
[GitLab merge requests](https://docs.gitlab.com/api/merge_requests/),
[GitLab discussions](https://docs.gitlab.com/api/discussions/),
[GitLab CLI](https://docs.gitlab.com/cli/api/).
