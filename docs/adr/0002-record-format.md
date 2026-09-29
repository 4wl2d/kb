# ADR 0002: Markdown with strict TOML front matter as the only authoring format

Status: accepted

## Decision

Records are Markdown files with `+++`-delimited TOML front matter. Front matter is parsed
twice: once generically (syntax, duplicate keys, `schema`, `kind`) and once into a per-kind
Rust struct with `deny_unknown_fields`. Normative content (statements, conditions,
exceptions, obligations, settings) is typed and atomic; Markdown body sections are optional,
non-normative explanations addressable via `kb show id#section`.

JSON Schemas in `core/schemas/` are generated from the same Rust types (schemars) and
drift-checked; conformance tests compare runtime and schema verdicts.

## Consequences

* One canonical model; no YAML/JSON authoring variants.
* TOML ordering rule: top-level keys must precede the first `[table]` header.
* Rules that JSON Schema cannot express (cross-record links, cycles, glob validity, semver)
  are runtime-only and documented in `core/schemas/README.md`.
