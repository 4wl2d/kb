# ADR 0009: Synthetic legacy schema 0 exercises the migration machinery

Status: accepted

## Context

The migration registry must be tested with real transformations, but there is no published
history of earlier formats.

## Decision

Document schema 0 (`kb-legacy-synthetic-v0`) is an explicitly synthetic, never-published
format defined in `core/migrations/README.md`. The 0 → 1 migration and its fixtures prove the
dry-run/apply/idempotence/atomicity machinery. No public release history is implied.
