# ADR 0001: One upstream, one downstream fork per product

Status: accepted; amended by [ADR 0017](0017-kb-command.md) (one installed `kb` command
selects the engine each checkout pins, instead of running `<kb>/kbw`).

## Context

Knowledge must be versioned with the engine that interprets it, mounted into several host
repositories (mobile, backend, libraries), and usable without reconciling a global engine
version.

## Decision

A single upstream repository carries the engine (`core/`, Cargo files, installers), schemas,
migrations, templates, shared skills and release machinery. Each product keeps exactly one
downstream fork/private copy that preserves upstream history and adds `project/`. Host
repositories mount the downstream as a Git submodule (conventional path `.kb`) or use a
separate checkout, and run the installed `kb` command, which reads each checkout with the
engine built from that checkout's own engine sources (ADR 0017).

Engine-owned paths are declared in `core/release.toml`; `kb update divergence` reports
undeclared engine edits against the upstream base recorded in `project/upstream.toml`.

## Consequences

* Engine, migrated knowledge, schema versions and generated skills travel together in one
  reviewed update branch (`kb update prepare`).
* Routine sync of the project origin (`kb sync`, per-call freshness) and upstream upgrades
  (`kb update`) are different operations.
* Rejected: separately installed knowledge packages, hosted services, multiple independently
  managed repositories.
