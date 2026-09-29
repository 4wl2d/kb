# ADR 0001: One upstream, one downstream fork per product

Status: accepted

## Context

Knowledge must be versioned with the engine that interprets it, mounted into several host
repositories (mobile, backend, libraries), and usable without installing or reconciling a
global CLI.

## Decision

A single upstream repository carries the engine (`core/`, `kbw`, Cargo files), schemas,
migrations, templates, shared skills and release machinery. Each product keeps exactly one
downstream fork/private copy that preserves upstream history and adds `project/`. Host
repositories mount the downstream as a Git submodule (conventional path `.kb`) or use a
separate checkout, and run `<kb>/kbw`.

Engine-owned paths are declared in `core/release.toml`; `kb update divergence` reports
undeclared engine edits against the upstream base recorded in `project/upstream.toml`.

## Consequences

* Engine, migrated knowledge, schema versions and generated skills travel together in one
  reviewed update branch (`kb update prepare`).
* Routine sync of the project origin (`kb sync`, per-call freshness) and upstream upgrades
  (`kb update`) are different operations.
* Rejected: separately installed knowledge packages, hosted services, multiple independently
  managed repositories.
