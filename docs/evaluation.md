# Evaluation

Engine tests prove that the protocol and routing behave as specified. They do not prove
that generated knowledge improves an agent's engineering work. Keep Tier A routing and
source-evidence results separate from held-out replay outcomes.

## Tier A: routing

```sh
./kbw eval routing --json
./kbw eval routing --profile maintainer --json
./kbw eval routing --example synthetic-multirepo --json
./kbw eval routing --context-format terse --json
```

Ordinary evaluation uses the same validated corpus and schema-1 routing fixtures as
`validate`: working-tree source unless a snapshot is explicitly selected. `--example`
evaluates the shipped synthetic tree directly and never changes `project/`. `--strict`
treats validation warnings as failures. Invalid fixtures fail validation (40); unmet
expectations fail routing (41). Snapshot approval/freshness is deliberately excluded from
fixture `expect_status`, as in the original routing tests.

`--context-format compact|terse|human|json` selects the renderer measured in each case;
it is independent of the report's global `--format`/`--json`. The default is compact and
`validate` continues to use compact. The report records `routing.context_format`. Keep
fixture scope, expectations and ceilings fixed when comparing modes; different packing
can legitimately change optional delivery, so inspect ids/status alongside token totals.

Additional fixture fields:

| Field | Meaning |
|---|---|
| `change_types` | Explicit known categories for applicability |
| `expect_order` | Relative order of distinct delivered record ids; each must be present |
| `max_tokens` | Ceiling for the full response in the selected context renderer (compact by default) |
| `relevant` | Human-labeled relevant record ids; duplicates are rejected |
| `recall_k` | Distinct record cutoff in delivery order; default 10, range 1..10000 |
| `min_recall_percent` | Required recall@k (0..100), only with relevant labels |
| `not_applicable` | Human-adjudicated mandatory false positives |
| `applicability_reviewed` | Every delivered mandatory record has been adjudicated |

Recall is `relevant ids in top k / labeled relevant ids`. Sections do not add another
record to the ranking. Mandatory precision is `(mandatory ids - labeled false positives)
/ mandatory ids`, and exists only for explicitly reviewed cases. Empty denominators and
missing labels produce unknown values, never a perfect score. Reports retain exact counts
alongside fractions, response tokens per call, maximum/total tokens and exercised mandatory
ids. Token estimates are not model billing tokens. Labels and order checks assess routing,
not factual correctness, runtime behavior or instruction compliance.

## Tier A: history

```sh
./kbw eval history --host /synthetic/host --repo mobile \
  --snapshot working-tree --offline --range BASE..HEAD --json
./kbw eval history --host /synthetic/host --repo mobile \
  --snapshot KB_SHA --offline --range BASE..HEAD --as-of \
  --labels merged-descriptions.json --check --json
```

This is a first-parent history view, including ordinary/squash commits. The start is
exclusive, the end inclusive; the start must be on that first-parent chain. Each change
uses its real parent-to-commit diff, including deleted/renamed paths. `--max-changes`
defaults to 200 (maximum 1000); an oversized range fails instead of silently sampling.
Host versions come from parent Git blobs. No source checkout is modified.

Without `--as-of`, this is explicitly a **current-snapshot retrospective**: which records
from today's selected KB apply to past touched scope. With `--as-of`, records are filtered
at each change's first parent using `introduced`/`retired`; undated/future records are
withheld. This does not reconstruct older text, status or registries. For leak-free replay,
freeze the KB and all generated artifacts independently at the intended cutoff.

History reads `kb-impact:v1` blocks from commit messages, or complete MR descriptions in an
explicit `kb.history-labels.v1` export. Example shape (synthetic, replace SHA placeholders):

```json
{
  "protocol": "kb.history-labels.v1",
  "changes": [{
    "commit": "<full host SHA>",
    "description": "<complete MR description including kb-impact:v1>"
  }]
}
```

Optional fields inside the existing TOML block are `applicable`, `not_applicable` and
`applicability_reviewed = true`. The existing `kb_change`, `reason` and linked-change
requirements are unchanged. Contradictory/duplicate labels, malformed blocks, duplicate
commit entries and labels outside the selected range are errors. Labels do not authorize
acceptance or establish truth. Absent labels mean unknown precision/recall.

Reports include changed/unknown paths, affected records, compact token costs, delivered
ids, labeled recall/precision, unavailable label ids and changed/unverifiable drift counts
for affected or delivered accepted records. `--check` fails incomplete context/temporal
inputs or unavailable label ids. Drift counts are reported separately; use `drift --check`
or the evidence CI gate to enforce source support. A path covered by a generic rule is not
proof that the decisive domain fact was present; `coverage` provides the domain backlog.

## Tier B: optional replay

The separate [Rust replay kit](../core/eval/README.md) provides strict task/study formats,
immutable registration, base/golden qualification, native pinned adapters, Seatbelt and
bubblewrap profiles, isolated test stages, neutral judge packets and paired analysis.
Its synthetic agent fixture requires no model account or network. Do not put product
tasks or results in engine paths. Real model work is explicit and separately budgeted.

Reconnection accounting retains every segment and distinguishes incremental from
cumulative counters. Missing usage/cost remains unknown. Analysis averages repeated pairs
inside tasks, bootstraps task clusters, reports Holm-adjusted sign-flip tests and SESOI
equivalence, and preserves missing cells, cohort hashes, sensitivity and repeatability.
The kit's local pass establishes protocol/isolation behavior only; a feature or release
gate still needs the preregistered held-out study and independent review.

## CI and interpretation

Upstream runs routing on the maintainer corpus and synthetic multi-repository example.
Downstream CI templates add strict validation, pinned host evidence, drift queue and ledger
reports. See [CI templates](../core/templates/ci/README.md). CI job definitions and local
synthetic checks are distinct from executed remote jobs and real agent consultation.

Quality comparisons require fixed held-out tasks, frozen generation inputs, repeated
paired runs, isolated blinded judging, explicit missing-usage accounting and a registered
analysis. Do not tune fixtures or selection until a favored arm wins. A non-significant
difference is not evidence of equivalence.
