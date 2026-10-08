# ADR 0010: Additive document schema 2

Status: proposed for the upstream upgrade

## Context

Schema 1 expresses obligations and applicability but cannot express the subsystem models,
scenarios, consumers, glossary, temporal validity and verifiable provenance needed to
produce and maintain useful domain knowledge. Encoding them only in Markdown would hide
meaning from validation and retrieval.

## Decision

Add optional typed fields in schema 2. Keep strict unknown-field rejection. Continue
reading schema 1 with its original field vocabulary and unchanged defaults; new fields
require schema 2. Preserve ids, status, owner, anchors and Markdown when migrating.
The adjacent 1 → 2 migration changes schema declarations and introduces no invented facts.
An absent optional field retains schema-1 behavior. Synthetic 0 → 1 remains testable.

All records may carry `introduced`, `retired`, `verified_at` and `review_by`.
Temporal bounds are ISO calendar dates or host commit ids; freshness values are ISO dates.
Ambiguous mixed temporal bounds are errors rather than comparisons of strings.
Scopes may constrain registered change types. Policy and invariant records may request
`delivery = "always"`; the delivery mechanism must prove the corresponding content was
installed before omitting any obligation from context (ADR 0013).

Features gain states, transitions, clock/data-source descriptions and scenarios.
Contracts gain scenarios and consumers. References gain glossary terms with sources.
Statements and contract obligations gain declarative verification probes. Anchors gain
stamps bound to a host commit and a precise source span. No probe contains shell commands;
execution semantics are specified separately in ADR 0015.

Regenerate version-2 JSON Schemas from the canonical Rust model. Keep version-1 schemas
available with their original vocabulary. Configuration and registries remain readable
at version 1 and migrate through the same adjacent step. Independently versioned host,
skill, routing, upstream and protocol formats do not change merely because records do.

## Consequences

New authoring defaults to schema 2. Existing documents remain readable before migration;
validation refuses schema-2 fields mislabeled as schema 1. Migration is previewable,
validates all outputs before writing, preserves bodies/comments and is idempotent.
Schema conformance and old/new context equivalence require tests.
