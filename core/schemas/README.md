# Generated JSON Schemas

The `*.schema.json` files in this directory are **generated** from the canonical Rust model
in `core/cli/src/model/` (plus `core/cli/src/versions.rs` and the CLI envelope built in
`core/cli/src/schema_export.rs`). Do not edit them by hand:

```sh
kbw schema            # report whether the files match the model
kbw schema --check    # fail with DRIFT_DETECTED (exit 42) on missing/changed/extra files
kbw schema --write    # regenerate (and remove stale *.schema.json files)
```

The test `core/cli/tests/schema_conformance.rs` also fails on drift, checks that every file
is a valid JSON Schema draft 2020-12 document, and checks that the strict runtime parser and
the record schema agree on the fixtures in `core/tests/fixtures/validate/records/`.

All schemas use draft 2020-12 and identify themselves with a non-resolvable URI of the form
`kb:schema/<name>/v1`. No schema refers to a web location.

| File | Describes |
|---|---|
| `record.v1.schema.json` | TOML front matter of a knowledge record (document schema 1), one branch per `kind` |
| `project.v1.schema.json` | `project/project.toml` (and the maintainer `profile.toml`) |
| `registry-{owners,repos,modules,features,concepts}.v1.schema.json` | `project/registry/*.toml` |
| `routing-test.v1.schema.json` | `project/routing-tests/*.toml` |
| `skill-config.v1.schema.json` | `project/skill-config/skill.toml` |
| `host-binding.v1.schema.json` | `.kbw.toml` in a host repository |
| `upstream.v1.schema.json` | `project/upstream.toml` |
| `release-manifest.v1.schema.json` | `core/release.toml` |
| `cli-envelope.v1.schema.json` | the `kb.cli.v1` JSON document printed by every command with `--json` |

## Applying a schema to TOML

The schemas describe the JSON data model of the TOML files. Convert TOML to JSON value by
value (tables → objects, arrays → arrays, strings/integers/booleans as is). TOML floats and
datetimes have no distinct JSON representation (JSON Schema treats `1.0` as an integer), and
no field of any kb file accepts them; a converter should keep them distinguishable (the
conformance test maps them to tagged objects, which every schema rejects).

Syntax-level rules cannot be expressed on the converted value and are enforced only by the
parser: the `+++` front matter delimiters, TOML syntax, duplicate keys (rejected by the TOML
parser), UTF-8 and size limits (`RECORD_TOO_LARGE`, `FRONT_MATTER_TOO_LARGE`).

## What the record schema enforces

Everything the strict parser checks on a single record that JSON Schema can express:
unknown fields (`additionalProperties: false` everywhere), types, enumerations, required
fields, `schema = 1`, `kind` (one branch per kind), id patterns (record ids, local ids,
override targets), title shape (single non-blank line of 1..=200 characters), non-blank
text fields, required non-empty lists (`behaviors`, `statements`, `obligations`, `reasons`,
`steps`, `expected`, at least two `parties`, a non-empty policy), list length limits (500),
uniqueness of scalar lists (scope dimensions, selector concepts and aliases, link targets,
gap `affects`, string-set values), the scope shape (`product = true` XOR a non-empty
dimension), anchor requirements per anchor kind, commit id shape, and setting consistency
(value matches `type`; `stricter` present iff `override = "stricter"` and fitting the type;
`override_owners` only on overridable settings).

## Runtime-only rules

These rules are enforced by `kb validate` (parser: `core/cli/src/parse.rs`; cross-record
rules: `core/cli/src/validate.rs`) but are not expressible, or not exactly expressible, in
JSON Schema. A document valid against the schema can still be rejected by `kb validate`.

Single record (parser):

* Uniqueness of local ids inside arrays of objects: statement/item/party/obligation ids,
  exception ids per statement, setting names (`SETTING_DUPLICATE`), override targets
  (`OVERRIDE_DUPLICATE`), and unique body section slugs (`BODY_INVALID`).
* References inside one record: no self links (`LINK_SELF`), an override may not target its
  own policy (`OVERRIDE_SELF`), obligations name a declared party (`CONTRACT_PARTY_UNKNOWN`).
* Path safety of selector globs, anchor paths and source paths (relative, no `.`/`..`
  segments, no backslashes or control characters) and glob syntax (`SELECTOR_PATH_INVALID`,
  `ANCHOR_PATH_INVALID`, `SOURCE_PATH_INVALID`).
* Aliases must normalize to at least one token (Unicode NFKC normalization,
  `SELECTOR_ALIAS_INVALID`).
* `applicability.versions` values are semver requirements (`APPLICABILITY_INVALID`).
* Text limits are measured in UTF-8 bytes (16 KiB); the schema's `maxLength` counts
  characters and is therefore looser for non-ASCII text. The length limits of the two
  halves of an override target (`<record id ≤ 128>#<setting ≤ 64>`) are runtime-only.
* Lints (warnings): normative RFC 2119 keywords in optional body sections
  (`NORMATIVE_LANGUAGE_IN_BODY`).

Across records and against the profile (`kb validate`):

* Namespace (`NAMESPACE_MISMATCH`), duplicate ids across files (`DUPLICATE_ID`).
* Registry references: owners (`OWNER_UNKNOWN`), repos, modules, features and concepts in
  scope, selectors, applicability, anchors, feature records and contract parties
  (`UNKNOWN_REPO`, `UNKNOWN_MODULE`, `UNKNOWN_FEATURE`, `UNKNOWN_CONCEPT`).
* Owner authority (`OWNER_NOT_AUTHORIZED`), scope satisfiability (`SCOPE_UNSATISFIABLE`),
  contract parties (`CONTRACT_PARTY_MODULE_MISMATCH`, `CONTRACT_PARTY_OUT_OF_SCOPE`).
* Links: `DANGLING_LINK`, `REQUIRES_CYCLE`, `SUPERSEDES_CYCLE`, `SUPERSEDED_STATUS`,
  `SUPERSEDED_ORPHAN`, `REQUIRES_NOT_ACCEPTED`, `REQUIRES_DEPRECATED` (warning).
* Overrides: `OVERRIDE_TARGET_UNKNOWN`, `OVERRIDE_FORBIDDEN`, `OVERRIDE_TYPE_MISMATCH`,
  `OVERRIDE_WEAKENS`, `OVERRIDE_SCOPE_EXCEEDS`, `OVERRIDE_NOT_AUTHORIZED`,
  `OVERRIDE_AMBIGUOUS`.
* With `--base <rev>`: `ID_REMOVED`, `ID_KIND_CHANGED`.

Configuration and registries: registry ids are unique per file (`REGISTRY_DUPLICATE_ID`),
references between registries exist, globs and `version_file` paths are safe, aliases
normalize to tokens; in `project.toml`, knowledge roots are safe paths that do not overlap
reserved profile directories. The schemas only check the id shapes, `schema = 1`, the
namespace, remote name, `approved_ref` shape, the allowed protocol names, a non-empty
`knowledge.roots` and a positive `context.default_budget`.

A schema-valid, `kb validate`-clean document is structurally sound; neither proves that its
text is true or that a referenced test runs.
