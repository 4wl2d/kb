# Maintenance prompt v2

Use this with the generated skill protocol 2. Code and knowledge changes remain separately
reviewable. Fill in the parameters and paste the fenced prompt into the host session.

~~~~text
Maintain the product's knowledge while implementing the requested host change.

PARAMETERS
- Host: <path>; KB mount: <.kb>; registry repo: <id>
- Target branch: <origin/main>; task: <one-line task>
- KB work branch: <feature/knowledge-change>
- Approved KB revision: <resolved commit>; reviewer: <name or unknown>

1. DIAGNOSE BEFORE THE FIRST EDIT
Run <.kb>/kbw context --intent diagnose --task "<symptom/task>" --skill-protocol 2.
Path-free scope is provisional. Once files/modules are known, run scoped implement/debug/
review context before editing. Use explicit repo:path for other repositories.
Mandatory units include their typed conditions and exceptions. Resolve gaps and conflicts;
do not claim complete coverage after CONTEXT_BUDGET_EXCEEDED, UPDATE_REQUIRED or unknown
scope. Explain offline, stale, unapproved or unavailable evidence.
Re-run scoped context when the task expands, a shared contract changes, the KB pin changes,
or work moves to a new session. --since-receipt is only for a receipt whose full context
you still retain. A receipt proves delivery, not compliance. Always-on omissions require
the engine to verify the installed core receipt and source.

2. CAPTURE WHAT THE CHANGE TEACHES
Keep host/KB dirty work and stable knowledge ids. Work on a KB proposal branch and run
capture and propose submit there with --snapshot working-tree --offline, so drafts are
validated against that branch's registries and records.
Use propose begin --from-change <BASE..HEAD> for committed work, or the team's
kb.change.v1 export after the change is merged. Inspect touched modules, existing facts,
added test candidates and review comments; none of these automatically proves a rule.
For a small finding use capture decision|gap|quirk|scenario with explicit title/text/owner/
scope and source/test anchors. --test "<command actually run>" records evidence text;
the engine does not execute it. Preview before --apply.
For richer records use the typed templates, then propose submit <file> and inspect
validation, anchor and duplicate results before --apply. Reuse an existing draft id.

Evidence classes:
- confirmed rule: authoritative spec/ADR, required CI check or explicit human direction;
- confirmed behavior: merged fix + regression test, or explicit review decision, with an
  actual change anchor and a named reviewer in the knowledge MR;
- orientation: source behavior, feature model, scenario, precedent or glossary entry,
  delivered as supplementary context;
- inference/unknown: draft proposal or scoped gap with an owner and specific question.
All new records stay draft. Never promote code observations into mandatory invariants.
Preserve accepted record ids and status; describe corrections as reviewable proposals.
Never delete accepted knowledge, silently weaken it, or reuse a retired id.
Obligations and exceptions stay typed; explanatory Markdown is non-normative.

3. KEEP THE SUBSYSTEM PACK CURRENT
Update evidenced states/transitions, clocks/data sources, consumers, failure/recovery
scenarios, canonical tests, platform quirks and change-type checklists where affected.
Missing evidence becomes a gap. Set introduced/retired from evidence, never guessed dates.
Anchor stamps verify Git bytes; verified_at/review_by require a real review and SLA.
Add routing cases for accepted obligations, symptoms, category scope and changed aliases.
Pending draft expectations stay comments until review. Use Tier A to check order, recall
and response-size limits, not as proof of agent solution quality.

4. VERIFY THE ACTUAL FINAL SCOPE
Run <.kb>/kbw impact --base <target> --working-tree.
Run <.kb>/kbw verify --diff <target> for declared probes; committed changes may use
--head <commit>, and the commit hook uses --staged plus the pending message file.
A probe with unavailable provider facts or unevaluated conditions stays unverifiable.
Do not assert --applicable unless the human/agent has actually checked its conditions and
exceptions. No record command is executed.
Run usage report --diff <target> --receipt <each receipt from this task> to compare delivered
scope with final touched scope. Its irrelevance/coverage is a scope proxy.
If knowledge changed, run validate --strict, eval routing and historical id checks.
Report all failed/skipped checks and unavailable runtime/device evidence.

5. PREPARE LINKED REVIEW
Prepare the kb-impact:v1 block and Knowledge learned section using core/templates/mr.
Use none with a reason, linked with a real KB revision and/or shared change_id, or included
for a reviewed KB pointer change. Validate the description with impact --check --statement.
Optional applicable/not_applicable labels are human adjudications for eval history; absence
does not mean no knowledge applies. Set applicability_reviewed only after reviewing all
mandatory ids. Commit-format/rule checks are not solution-quality checklist items.
Merge the reviewed knowledge first, then update the host pin through its own review.
Do not push, open an MR, approve or merge unless explicitly authorized for that action.
Never edit generated skills/managed blocks manually or use integrate --force.

6. REPORT
Context receipts and completeness; applied obligations and unresolved gaps; final impact
and verification; proposed knowledge ids/paths and evidence class; tests actually run;
coverage/usage limitations; named acceptance decisions still needed.
~~~~

## Change-driven accrual

After each merged host MR, export the reviewed change and comments as `kb.change.v1`.
Run `propose begin --from-change export.json`, have the configured agent prepare drafts,
and run `propose submit --snapshot working-tree --offline` on the KB proposal branch
before putting them into a draft KB MR. The templates under `core/templates/ci/` keep
this opt-in and separate model credentials from publishing credentials. A merged host MR
does not auto-accept knowledge.

## Weekly owner review

Use a pinned host checkout for each registry repository; pass every required
`--repo-root id=path`. Keep raw JSON reports in the CI artifacts/review branch.

```sh
./kbw validate --strict --stale 90 --on YYYY-MM-DD
./kbw anchors check --snapshot working-tree --offline --at HEAD --repo-root repo=/host
./kbw drift --snapshot working-tree --offline --since verified --repo-root repo=/host --json
./kbw ledger --snapshot working-tree --offline --on YYYY-MM-DD --sample 10 --seed review-week --repo-root repo=/host --json
./kbw coverage --snapshot working-tree --offline --host /host --repo repo --since 180d
./kbw eval routing
./kbw integrate --generate --check
./kbw update divergence
```

Dates and stale-day limits are team parameters. Without `--on`, freshness is relative to
the selected commit date, not the current calendar. The weekly work is to review changed
anchors with their owners, inspect the seeded draft sample against sources, answer gaps,
update overdue evidence through review, and re-run coverage to choose the next subsystem
pack. Do not clear a warning merely to make CI green. Log what was verified and what remains
unavailable. Upstream upgrades remain a separate reviewed operation.
