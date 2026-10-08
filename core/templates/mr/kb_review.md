## Knowledge change

Describe the changed behavior and the modules/contracts it affects. New agent-authored
records remain drafts until a named reviewer makes the acceptance decision.

| Record id | Evidence class | Source/change/test commit | Reviewer | Proposed disposition |
|---|---|---|---|---|
| | confirmed rule / confirmed behavior / orientation / inference or gap | | | |

Confirmed behavior requires a merged fix with a regression test, or an explicit review
decision. Distinguish tests actually run from source inspection. Orientation is
supplementary. A provider or matching anchor hash does not establish semantic truth.

## Domain coverage

Top modules: accepted domain records before/after (proposals separately); contracts with
consumers; scenarios per feature and state; checklists per change type; unanswered gaps.

## Knowledge learned

- Decisions and their reasons:
- Platform or integration quirks:
- Failure/recovery scenarios and canonical tests:
- Precedents and terminology:

## Evidence and validation

- [ ] Stable ids, scopes, typed conditions/exceptions and required links reviewed.
- [ ] Anchor commits, stamps and consumer candidates checked against source.
- [ ] Verification date/review deadline reflects an actual review and agreed SLA.
- [ ] `validate --strict` and historical id check pass; failures are attached verbatim.
- [ ] `eval routing` passes; pending draft expectations are listed for acceptance.
- [ ] Anchor, drift and ledger reports attached, including unverifiable evidence.
- [ ] No new record was accepted by an agent; the named reviewer records acceptance.
- [ ] Host pin/integration follow-up identified; no automatic merge.

## Linked host change

Host MR/change id and final host commit; test results; KB revision after review; pin-update
owner. Do not invent a future commit SHA or create cyclic SHA dependencies.
