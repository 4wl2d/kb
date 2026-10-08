# ADR 0013: Explicit, content-bound delivery reuse

Status: proposed for the upstream upgrade

## Decision

`delivery = "always"` is opt-in on unconditional product policies and invariants.
Integration renders every statement, condition and exception, plus typed settings and
overrides, into one primary managed instruction source. The complete core must fit 600
estimated tokens. No automatic selection, abbreviation or truncation removes obligations.
Other harness targets point to that source. One full skill avoids duplicate workflow
copies; Claude's existing `/kb` entry remains a short native command alias in mixed setups.

A loaded core's explicit `--core-receipt` and `--core-source` permit id-only references
only after the selected snapshot's entire always-on set, bundle, lock, source bytes and
launcher mount match. Missing, modified or stale proof is an error with a full-context
fallback. Installation is not evidence that a model loaded or followed instructions;
`integrate --probe` separates installation checks from its runtime challenge.

`context --since-receipt ID` reads an explicit previous local receipt for the same KB,
host and profile. Applicability, dependencies, conflicts and completeness are recomputed.
Only delivered units with identical content hashes become `unchanged` references. Changed
and new units are delivered in full. No hidden session state or elapsed-time heuristic
chooses a receipt. After compaction, a new session or hand-off, callers must request full
context again. A reference retains its tier and never converts missing proof into success.

CLI receipts use `kb.receipt.v2`, with a snapshot-content digest and unit content hashes.
The digest hashes sorted profile file paths and their source content identities, source
issues and proposal overlay digest; Git and working-tree identity domains remain explicit
in those inputs. It is independent of the local index/cache path. Cached receipts validate
their canonical result hash and subject before reuse and stay local.

`--format terse` retains typed content, conditions and exceptions in a compact tiered
form. Markdown and explanations are deferred to `show --sections` or another format.
Packing measures the same pieces that are printed and errors if required units cannot
fit. `outline` is a separate, at-most-2000-token inventory; it cannot become a delivery
receipt. Token counts are deterministic estimates, not vendor-tokenizer claims.

## Consequences

Optimization requires explicit caller evidence and can fail closed without dropping
obligations. Normal calls continue delivering complete units. Historical replay cannot
reuse a current always-on block: the evaluation environment must freeze its instructions.
Per-target file caps preserve inspectable failures rather than relying on silent harness
truncation. Output parity tests keep schema-1 normative content and ordering; the skill
protocol bump necessarily changes version metadata and receipt hashes.

Default-on composed code context and other empirical choices still require downstream
measurements. File/hash checks and synthetic delivery tests do not satisfy that gate.
