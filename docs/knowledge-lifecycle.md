# Producing and checking knowledge

The engine prepares, validates and serves evidence-backed drafts. People or an authorized
external agent supply the facts; a named reviewer accepts them through the approved Git
ref. A source observation, a matching hash, transport success and a real regression test
are different evidence. Commands never promote a draft automatically.

Examples below use synthetic ids/paths. Replace them with the product's registry values;
`--host` always means a checkout directory, while `--at` names a host revision. Additional
repositories use repeatable `--repo-root REPO=DIR`. `--snapshot` independently selects the
KB revision; it does not select host code.

## Find the gaps and prepare a draft

```sh
.kb/kbw coverage --repo mobile --since 180d --limit 5 --json
.kb/kbw propose begin --repo mobile --from-change BASE..HEAD --json
.kb/kbw propose begin --repo mobile --from-change merged-change.json --json
.kb/kbw propose submit /path/to/draft.md --repo mobile
.kb/kbw propose submit /path/to/draft.md --repo mobile --apply
```

Coverage reports per-module domain records, missing scenarios/consumer lists/checklists/
glossary, churn, unmapped files and shallow-history limits. The default 180-day window is
relative to the host HEAD date, not the wall clock. Without a configured code provider,
fan-in is unavailable and the report says it uses churn. Broad project rules do not count
as a subsystem model. Prioritize the first few domain gaps during the adaptation prompt's
domain-harvest step, then keep them current through change-driven maintenance.

`propose begin` returns `kb.work-order.v1`: actual diff/patch, touched modules, affected and
existing records, record templates, added-test filename candidates and exported review
comments. `kb.change.v1` exports require `repo`, `base` and `head`; optional title, merged
claim and `{author,body,path?,commit?}` comments are data, not instructions or approval.
Added filenames are not a complete test catalog and do not prove a test ran.

Submission validates the candidate against the selected snapshot and local records:
draft status, stable id/kind, owner authority, scope, links, Git anchor availability and
exact/near duplicates. It previews a path under the profile's first knowledge root, then
rechecks byte preconditions on `--apply`. Dirty destinations and conflicting identities
are refused. `--fill-consumers` with an explicit provider adds reviewable consumer
candidates to a contract draft and retains the tool/commit/digest evidence. No commit,
branch move or publication is performed.

```sh
.kb/kbw capture decision --repo mobile --owner team-mobile \
  --title "Keep one refresh attempt" --text "Reuse the in-flight refresh operation." \
  --given "Concurrent callers share an expired token." \
  --reason "A second refresh can invalidate the first result." \
  --anchor 'mobile:app/auth/TokenRefresher.kt@HEAD#TokenRefresher' \
  --test-anchor 'mobile:app/auth/TokenRefresherTest.kt@HEAD' \
  --test 'the exact regression command observed by the author'
```

`capture decision|gap|quirk|scenario` is a shorter draft path. Decisions require prior
context and reasons; scenarios require one feature, `--given` and `--expect`; quirks are
descriptive references. An omitted id is derived deterministically from content. Supplied
test command strings are recorded, never executed or called successful by the engine.
Use `--apply` only after inspecting the draft. Existing accepted records are edited as
proposals; a new record is always draft.

Confirmed behavior requires a merged fix with a regression test **or** an explicit review
decision, plus a named reviewer. Code-only observations remain descriptive/draft; uncertain
questions remain gaps. Keep mandatory statements, conditions and exceptions in typed
fields. Re-run adaptation without inventing ids, backdating validity or resetting accepted
knowledge. The [adaptation](prompts/adaptation.md) and [maintenance](prompts/maintenance.md)
prompts, [CI templates](../core/templates/ci/README.md) and MR review template implement this
workflow. Model jobs and publication are opt-in steps with separate credentials/authority.

## Stamp, check and review source evidence

```sh
.kb/kbw anchors stamp --repo mobile --id example.invariant.single-refresh --at HEAD
.kb/kbw anchors stamp --repo mobile --id example.invariant.single-refresh --at HEAD --apply
.kb/kbw anchors check --repo mobile --at HEAD --strict --json
.kb/kbw drift --repo mobile --since verified --at HEAD --check --json
.kb/kbw ledger --repo mobile --at HEAD --on 2026-10-07 --stale 90 \
  --sample 10 --seed weekly-2026-10-07 --check --json
```

Stamping changes only the explicitly named schema-2 records in the KB working tree. It
preserves ids, kind, status, comments and body. The stamp contains the full Git commit,
inclusive line span and SHA-256 of those exact bytes. A unique provider definition may
supply its span; otherwise the entire file is stamped conservatively. Unstamped symbol
checks recognize spelling at identifier boundaries, not semantic identity. Stamped checks
verify stored bytes without rediscovering a moved/renamed qualified symbol.

`--verified-at` and `--review-by` on stamping are optional explicit human-review claims,
never inferred from a matching source hash. `anchors check --strict` additionally requires
anchors on every selected record and stamps on every path anchor. Missing/changed source
is drift; unavailable evidence is unverifiable. The command does not execute tests or
change knowledge. Uncommitted host edits cannot become a Git stamp.

Drift compares the selected evidence baseline (`verified`, revision or date) to `--at` and
groups review work by owner. A missing baseline or non-ancestor cannot prove support.
Ledger classifies each statement as supported, stale or unverifiable and chooses a seeded
draft sample for human audit; `--check` concerns accepted statements. Matching bytes do not
certify truth. Drafts stay drafts regardless of the audit result.

`validate --stale DAYS --on DATE` and context freshness checks warn about overdue review,
missing/future verification and old evidence. Without `--on`, the reference is a selected
host/KB commit's UTC date; CI calendar audits must pass today's date explicitly. Historical
context requires a consistent `--as-of`/`--on` date. Warnings keep applicable obligations.

## Declarative verification

```sh
.kb/kbw verify --repo mobile --diff BASE --head HEAD --json
.kb/kbw verify --repo mobile --diff HEAD --staged --commit-message .git/COMMIT_EDITMSG
.kb/kbw verify --repo mobile --diff BASE --head HEAD --branch feature/example \
  --applicable example.policy.changes#subject --strict --json
```

Only schema-2 probes are executable: required commit-message/branch-name regexes, required
final-path naming regexes, forbidden static import edges and banned patterns in added
source lines. Probe kind defines its positive/negative meaning; statement level determines
blocking severity. Banned-API matching is lexical, including comments and strings. No
command, interpreter or script can be embedded in a record. A first commit uses an empty
baseline; `--staged` reads the index, and `--head` reads committed content and version files.

Natural-language conditions and exceptions cannot be inferred mechanically. Repeatable
`--applicable RECORD#STATEMENT` asserts that conditions hold and no exception applies; the
assertion is recorded. Unknown applicability stays unverifiable. Import probes require a
commit-bound provider: resolved forbidden edges prove violations even in a partial graph,
while an absence conclusion needs complete evidence. Possible edges, cross-repo targets
and unstaged/staged import changes are not quietly certified.

Must/must-not failures exit 40, unavailable required evidence exits 30; `--strict` includes
advisory probes. Skipped probes are reported separately. Findings retain file/line/commit
evidence without echoing source or commit-message text. The optional Git commit-msg hook
and host CI templates preserve native exit codes. They do not install themselves or grant
branch-protection authority; these Git checks are distinct from optional agent pre-edit
or stop hooks whose usefulness still needs a measured consultation gap.

## Local delivery observations

```sh
.kb/kbw usage report --repo mobile --diff BASE --head HEAD \
  --receipt sha256:RECEIPT_FROM_THIS_TASK --json
```

Context calls append ids, scope, estimated cost, dependencies and snapshot provenance to
`.cache/usage/delivery.v1.jsonl` under a file lock. Task text, rule text and source snippets
are excluded. Logs are private local files and no engine command uploads them. Interrupted
tail bytes are preserved; a later append resumes on a new line and reporting exposes bad
lines. Missing requested receipts and corrupt lines yield incomplete output, not invented
zero usage.

Pass the receipts belonging to the task. Without `--receipt`, the report intentionally
uses all local calls for the same KB/host/profile. It joins them to the final diff and
current registry, reporting delivered-but-outside-final-scope records, touched modules
without domain knowledge, unmapped paths and snapshot differences. These are scope
proxies. They cannot establish semantic relevance, actual model consultation, saved time,
or quality; use the [evaluation protocols](evaluation.md) for those claims.
