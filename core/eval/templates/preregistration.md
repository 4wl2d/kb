# Replay study preregistration

This is a template. Complete it in a private product study; do not put real product data
in the upstream repository. Hash the finalized task and study files before scored runs.

## Purpose and frozen inputs

- Product, generation cutoff and held-out post-cutoff range:
- Task selection procedure, inclusion/exclusion rules and pre-fix prompt source:
- At least 40 held-out tasks, two repetitions, independent task clusters:
- Per-task base/golden SHAs, hidden file hashes, tested build/regression/hidden commands:
- Human review that tests fail on base and pass on golden for the intended reason:
- Adaptation v1/v2 source revisions, engine pins, frozen generation inputs and KB hashes:
- Current curated KB pin, with temporal leakage audit and explicitly disclosed limits:
- Second product or realistic synthetic task set, with its own frozen selection:
- Registration hash and evidence that registration preceded scored observations:

## Arms and execution

- Feature A/B: exact baseline, one changed feature and intended metric:
- Generation: v1, v2 and curated KB on the same task pool:
- Field: kb-next, CodeGraph, CodeWiki, Conductor, OpenWiki, Serena, none and legacy:
- Every tool/provider/agent executable, version, model revision/alias and effort:
- Runtime/toolchain and dependency-cache provenance; hardware/OS isolation backend:
- Counterbalanced run order, cold/warm cache policy and retry/stopping policy:
- Exact model API hosts and approved API credential variable names:
- No source of future code, parent/global agent rules or hidden tests in coder context:
- Monetary budget, provider-side cap, per-trial deadline, authorized operator:
- Preserve all failed attempts; decide before running how partial usage and retries count:

Do not make default-on changes based on a smaller pilot or change the chosen task pool
after seeing arm scores. Consultation/load probes, code-only tests and model quality are
different kinds of evidence. A real transport pilot must pass before the full paid matrix.

## Judging and metrics

- Blinded neutral artifacts and private arm mapping; no inherited project instructions:
- Quality rubric with concrete behavioral/edge-case/root-cause/blast-radius evidence:
- Separate rule-compliance rubric, including commit-format rules only with real evidence:
- Independent second judge family, same cells; discrepancy/adjudication policy:
- Q/resolved/rules/edge cases and optional subscale definitions and units:
- Actual vs estimated monetary cost; dated rate source if estimates are unavoidable:
- Native token/cache conventions, reconnect segments and child-session reconciliation:
- Latency boundary (agent-only vs setup/test/judging), warmup and excluded work:
- Consultation timing/count and drafting/review labor measured separately:

## Analysis and decision

- All paired hypotheses, direction, SESOI, guardrails, alpha and Holm family:
- Whole-task cluster bootstrap draws/seed and fixed sign-flip assumptions:
- Marginal reporting intervals and Bonferroni family intervals for SESOI decisions:
- Missing cells and exclusions; no aggregate cost means over different cohorts:
- Leave-one-task-out, exclusion sensitivity and second-judge sensitivity:
- Repeat mean absolute Q difference and resolved flip fraction limits:
- Equivalence requires an interval within ±SESOI, not merely p > alpha:
- A feature needs its targeted gain beyond SESOI and no material guardrail regression:
- Generation v2 must beat v1 on Q/resolved and match or beat the curated KB:
- Record superiority, equivalence, inconclusive and failed gates separately:
- Publish no best-on-all-criteria claim unless every preregistered requirement is met:

## Retained artifacts and deviations

- Study/task/qualification hashes, raw agent and judge logs, neutral packets, code patches:
- Per-attempt usage, outcomes, scorer evidence and all planned/missing cells:
- Raw Tier A fixtures, accepted vs proposed knowledge counts, freshness/ledger evidence:
- Every deviation, timing, reason and whether it was known before unblinding:
- Private evidence destination, review owner and separately authorized publication scope:
