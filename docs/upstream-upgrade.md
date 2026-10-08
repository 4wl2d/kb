# Upstream knowledge upgrade

This implementation tracks the upstream upgrade proposal supplied on 2026-10-06.
It covers new and existing downstreams. Product knowledge and private evaluation inputs
remain outside this repository. Examples and replay tasks here are synthetic.

## Requirements and evidence

An unchecked item is unfinished. Code, fixtures and command results establish completion;
this document does not establish that a feature works or that an evaluation gate passed.

- [x] U1: schema 2, strict parsing/validation, adjacent migration, fixtures, templates,
  feature states/transitions/scenarios, contract consumers, glossary terms, freshness,
  temporal validity, change types, delivery, anchor stamps and declarative verify probes.
- [x] U2: coverage backlog; propose begin/submit; capture; evidence ladder; domain harvest;
  adaptation and maintenance workflows; host/KB CI and MR templates.
- [x] U3: path-free diagnose; identifiers from tracked host files; change-type detection;
  context --changed --base; temporal corpus filtering with --as-of.
- [x] U4: versioned provider protocol, fixture-tested reference adapters, deep impact,
  provenance, precedence suggestions and opt-in budgeted context with code.
- [x] U5: verifiable always-on delivery, receipt delta, terse output, outline, applicability
  pruning, harness targets and skill protocol 2.
- [x] U6: stamp/check, freshness validation, drift, ledger, verify, hook/CI integration,
  private local usage reporting and snapshot content digests in receipts.
- [x] U7 Tier A: routing metrics/order/budgets, history evaluation, CI on synthetic and
  maintainer corpora, downstream evaluation templates.
- [x] U7 Tier B implementation: portable Rust replay kit, isolation profiles, pinned agent adapters,
  blinded judge protocol, paired cluster bootstrap/Holm/SESOI/sensitivity and preregistration.
- [x] Delivery artifacts: format/context/protocol/downstream/verification docs; proposed ADRs 0010–0015;
  matching maintainer knowledge; schema and skill version changes; migration/update paths.
- [x] Local validation: fmt, clippy, workspace tests, ShellCheck, generated schema drift,
  maintainer/template validation and feature-specific deterministic integration tests.
- [ ] Empirical acceptance: held-out feature A/Bs; generation v1/v2/current comparison;
  competitor replay; a second product or realistic synthetic task set. Preserve raw
  observations, report uncertainty and distinguish equivalence from superiority.
- [x] U8 gate assessment: [each extension](optional-extensions.md) retains its explicit
  measurement/ADR gate. No trigger is established; the extensions remain unimplemented.
- [ ] Release acceptance: ADR review/merge, remote platform checks, qualifying downstream
  results and authorized release delivery. The current tree is an untagged proposal;
  local implementation does not satisfy the planned 0.2/0.3/0.4 release gates.

## Execution order

1. Establish the baseline; specify schema and authoring trust semantics in ADRs.
2. Implement schema/migration and compatible templates, then retrieval and delivery.
3. Implement authoring, provenance, verification and provider adapters.
4. Update generation, maintenance, integrations and CI together with their real commands.
5. Build both evaluation tiers and run available acceptance experiments.
6. Audit every requirement against the final tree and results before release delivery.

## Compatibility boundaries

The engine remains deterministic and contains no query-time model or source parser.
Schema-1 records retain their content and routing semantics; opting into new fields needs
schema 2. New drafts never become accepted through a command. The approved Git revision
continues to determine trust. Mandatory knowledge is never silently omitted for a budget,
an inferred change type, an unverified integration or an unavailable previous receipt.

Publication, external agent jobs and measurement-dependent defaults need evidence for the
specific operation. A local test pass cannot substitute for an empirical acceptance gate.

## Verification checkpoints

- Baseline `cargo test --workspace --locked`: exit 0 before implementation.
- Schema-2 focused suite (library, CLI, context goldens, integration, migration, schemas,
  schema evolution, snapshots and validation): exit 0. Existing schema-1 context goldens
  were retained. Clippy with `-D warnings`: exit 0 at this checkpoint.
- Real launcher/update delivery tests (`e2e_workflow`, `update_production`): exit 0 for
  schema-2 initialization and the 0 → 1 → 2 migration through an upstream update.
- Retrieval regression suite: Git diff scope including removed/extensionless paths,
  tracked identifiers, acronym splitting, symptom discovery, conservative category hints,
  temporal boundaries, missing dependencies, repeatable receipts and future-independent
  lexical ranking. Final focused retrieval/index/schema-evolution run: exit 0.

- Provider checkpoint: seven real CLI scenarios cover commit/hash rejection, uncertainty
  propagation, budgeted code context, deleted symbols, dirty diff limitations, consumers
  and fan-in. Process deadline/byte-limit tests and four adapter tests pass. Installed
  ast-index 3.54.0 and CodeGraph 1.6.1 produced byte-identical responses on repeated
  synthetic smoke runs, after independently indexing frozen Git trees. Provider-stage
  workspace clippy: exit 0.
- Delivery checkpoint: seven real CLI scenarios cover receipt delta/corruption, installed
  core validation, preserved host instructions, one full skill, terse output and outline.
  Integration and context suites pass. The schema-1 goldens changed only skill protocol
  1 -> 2 and the corresponding receipt hashes; units, normative text, order and old-format
  budgets are unchanged. Delivery-stage workspace clippy: exit 0. Real harness loading
  and downstream quality/efficiency A/Bs remain empirical acceptance work.
- Trust checkpoint: six real CLI scenarios cover previewable/idempotent stamps, preserved
  dirty host bytes and knowledge status, committed drift, ledger support, dated freshness,
  provider definition spans and seeded draft audits. Seven verification scenarios cover
  added/index/committed input, the first commit, non-executable message text, naming and
  branch constraints, explicit conditions, incomplete imports and invalid knowledge.
  Nine delivery scenarios now include append-only usage, final-scope joins and receipt
  isolation across identical host checkouts. Concurrent append and interrupted-tail
  recovery are checked without discarding evidence. Trust-stage workspace clippy passes.
  Host GitHub/GitLab verification jobs and a commit-msg hook are shipped as templates;
  enabling remote jobs and real harness consultation remain separate evidence gates.

These are intermediate checkpoints; final local results follow below. Empirical gates
remain unchecked above. Diff scope resolves path uncertainty; unknown host versions or change
categories remain explicit rather than manufacturing completeness. Historical queries
withhold undated records and require frozen registries/host inputs for replay acceptance.

- Production/Tier A checkpoint: adaptation and maintenance now require domain harvest and
  the explicit evidence ladder; both bootstrap prompt copies match. Init installs the
  subsystem/checklist/glossary scaffolding plus GitHub/GitLab CI and KB review templates,
  preserving existing files. Five real CLI evaluation scenarios and three offline CI-glue
  scenarios pass. Routing metrics distinguish absent labels from perfect scores; history
  preserves unlabeled/undated uncertainty and refuses silent sampling. The shipped
  synthetic (7 cases) and maintainer (3 cases) corpora pass locally. Platform workflows are
  syntax checked, not remotely executed; model drafting and draft-MR publication remain
  opt-in deployment steps requiring team configuration and authority.

## Final local verification (2026-10-07)

The current workspace passes fmt, workspace clippy with warnings denied, and the workspace
test suite: **407 passed, zero failed, one OS-dependent test excluded by default**. That
test was then run explicitly on macOS Seatbelt and passed. It uses a synthetic Rust
process, not an AI model. ShellCheck passes for the launcher and all shipped CI/hook
scripts. All 23 generated schemas match, all 20 maintainer records and templates validate
without warnings, and Tier A passes for the synthetic (7 cases) and maintainer (3 cases)
corpora with compact and terse rendering. The launcher source bootstrap and the small
benchmark smoke also pass. See [verification](verification.md) for scope and limitations.

The replay kit now preserves historical Git boundaries, qualifies base/golden hidden
tests, checks OS isolation with a canary, pins native clients, retains raw usage segments,
isolates blinded judges and reports paired statistical comparisons with missingness and
sensitivity. Installed Codex, Claude Code, Cursor and Grok passed isolated version-startup
preflight. This establishes neither live authentication nor model transport, instruction
loading, consultation behavior or downstream quality. Linux execution remains a remote
CI gate; the workflow has not run from this workspace.

A subsequent offline Android runtime probe found a concrete worker limitation: JDK 25
starts in Seatbelt, but the pinned Gradle 9.5.0 fails when binding its local cache-lock
socket, before project configuration. Qualification rejects both copies. A suitable
isolated build worker is required before product hidden tests can be qualified; the
synthetic Rust fixture does not establish that compatibility.

The required acceptance study has not started. There is no frozen set of forty newly
qualified held-out tasks, no approved model/account/budget plan, no completed v1/v2/current
generation comparison, no field comparison and no second-product result. Compatibility
or estimated rendering-cost measurements cannot replace these outcomes. Optional defaults
and U8 extensions stay gated. ADRs 0010–0015 and their new maintainer records remain proposed
or draft until reviewed; no release is claimed.
