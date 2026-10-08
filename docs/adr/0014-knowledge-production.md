# ADR 0014: Evidence-based knowledge production

Status: proposed for the upstream upgrade

## Context

Migrating instruction files alone produces generic rules. Agents need domain behavior,
known failure modes, consumers, test scenarios and examples of established solutions.
Source observations cannot automatically become mandatory policy.

## Decision

The deterministic engine produces work orders and validates submissions. An agent or
human supplies semantic judgments. `coverage` prioritizes authoring using tracked files,
registry coverage and explicit history/provider evidence. `propose begin` assembles change
evidence, existing records and relevant templates. `propose submit` and `capture` validate
schema, anchors and duplicates and write only drafts into the proposal workflow.

Acceptance remains a reviewer merging into the configured approved ref. These commands
cannot grant accepted status. A local working tree or a draft cannot establish approval.
Submission reports duplicate candidates for review and never silently replaces a record.
Changes are previewable and require the normal explicit `--apply` write switch.

The evidence ladder is:

| Class | Required evidence | Permitted use |
|---|---|---|
| Confirmed rule | Authoritative source or enforced CI | Accepted obligation after review/merge |
| Confirmed behavior | Merged fix plus regression test, or explicit review decision; named reviewer | Accepted invariant or contract after review/merge |
| Orientation | Current source with a precise anchor | Descriptive feature; supplementary context |
| Proposal or unknown | Inference, missing or ambiguous evidence | Draft or gap |

Generation harvests subsystem models, typed exceptions, consumers, scenarios, change
checklists, platform quirks, canonical tests and precedents. Its final report states actual
coverage and unresolved gaps. Maintenance consumes merged changes, checks drift and
refreshes coverage. CI may create a draft review request through a team-configured agent;
the engine neither runs an implicit model nor auto-merges knowledge.

## Consequences

Provenance is inspectable but is not proof that a test ran or a claim is true. Missing
evidence remains explicit. Evaluation must separately measure engine correctness, useful
knowledge production and downstream task outcomes. No private product data is required
in the upstream repository.
