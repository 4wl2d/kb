# Optional extensions and their gates

The requested upgrade makes these extensions conditional. They remain disabled/unimplemented
unless an identified downstream measurement establishes the trigger and the design is
reviewed. Completing the deterministic engine and its fixtures does not satisfy that gate.

| Extension | Required evidence before implementation/default change | Current decision |
|---|---|---|
| MCP facade | A target harness cannot use the shell; ADR changing the README non-goal | No qualifying shellless integration established; keep the CLI boundary |
| Pre-edit/stop hooks | Measured consultation below 95%, plus a targeted A/B showing benefit without material quality/cost regression | Keep skill protocol 2 and actual consultation probes; do not add coercive hooks speculatively |
| Local embedding rerank | Tier A held-out lexical recall misses; rerank supplementary units only; quality/cost A/B | Keep deterministic lexical ranking; new metric support is not evidence of a miss |
| Git-derived path facts | Held-out debug/review improvement from a small opt-in brief, at most 300 estimated tokens | Churn is available for the authoring backlog; no additional context delivery without a measured gain |
| Vendor skill registry | A demonstrated project gap covered by scoped, version-pinned vendor material | No registry or implicit network fetch in the engine |
| Markdown/OKF/agents-index export | A concrete fallback consumer and delivery A/B motivating another maintained format | Existing inspectable context/show output remains available; do not invent another trust channel |

`context --with-code` is implemented but opt-in. Its default is also gated by the downstream
composition A/B. The three acceptance tracks are feature A/Bs, generation v1/v2/current-KB
comparison, and comparison with alternative systems on the same task pool. Repeat generation
evaluation on another product or a realistic synthetic task set. Report equivalence where
supported, and leave unknown outcomes unknown. No MCP ADR is created before its trigger.

Keep private product task kits, observations and benchmark source data outside upstream.
Only synthetic fixtures and the portable method belong here. This document records the
decision rule; completed acceptance results belong in a separately inspected evidence
report, not an unchecked claim in a release note.
