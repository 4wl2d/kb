# Migrations

This directory holds the documentation and fixtures of the `kb migrate` machinery. The code
lives in `core/cli/src/migrate.rs` (registry `MIGRATIONS`); integration tests live in
`core/cli/tests/migrate_update.rs`.

## Version history (honest statement)

Document schema `1` is the first schema served by this engine. There is **no earlier
published format**. The historical step `v0-to-v1` (schema `0` → `1`) migrates from
`kb-legacy-synthetic-v0`: a **synthetic** pre-release format defined below solely to
exercise the migration machinery with real transformations. It was never published, never
shipped by any release and never used by a real project. `core/release.toml` lists it in
`migrates_from = [0, 1]`: 0 is synthetic; 1 is the published format.

Schema `2` adds optional domain models, scenarios, consumers, glossary terms, temporal
validity, freshness, delivery, change categories, anchor stamps and verification probes.
The `v1-to-v2` step only changes schema declarations. Defaults are absent, never invented.
Schema-1 records remain readable before migration; new fields require schema 2.

## Using `kb migrate`

```sh
kbw migrate                # dry-run: per-file plan and unified diff, writes nothing
kbw migrate --apply        # transform, verify, then write
kbw migrate --to 2         # explicit target (default: the engine's document schema)
kbw --json migrate         # machine-readable plan (kb.cli.v1 envelope)
```

What is scanned (directly in the working tree, so it works while the current engine
rejects a legacy profile config):

* the profile config (`project/project.toml`, or `--config` / `--profile maintainer`),
* registry files `registry/{owners,repos,modules,features,concepts,change-types}.toml`
  (`change-types.toml` exists only at schema 2: `kb validate` rejects any other declared
  `schema`, and the `v1-to-v2` step bumps a schema-1 declaration),
* record files (`*.md` except `README.md`) under the knowledge roots declared by the
  config (`[knowledge] roots`, default `["knowledge"]`).

Per file the plan is one of: `up-to-date`, `migrate` (with the chain of steps) or `skipped`
(the schema cannot be read: not UTF-8, no front matter, TOML syntax error, symlink; such
files are left untouched and reported by `kb validate`). A file whose schema is newer than
the engine, older than the oldest migratable schema, or above the requested target fails
the whole plan with `UNSUPPORTED_SCHEMA_VERSION` (exit 13) listing the files; nothing is
written.

Safety guarantees of `--apply`:

1. every pending file is transformed **in memory** by the chain of registered steps;
2. every result is verified with the current engine: records with the strict record
   parser, the profile config with the strict config parser, registry files with the
   strict registry parsers (and the declared `schema` must equal the target);
3. only when all files pass is each file written with an atomic replace (temporary file,
   fsync, rename). Any transform or verification failure fails with `MIGRATION_FAILED`
   (exit 52) listing every failing file, and **nothing is written**. A file modified on
   disk between planning and writing also aborts before the first write;
4. TOML is edited with `toml_edit`, so comments, key order and formatting survive; record
   bodies after the closing `+++` line are byte-identical;
5. re-applying is a no-op (`written` is empty).

Migration is local and deterministic; it never touches Git. Review the diff and commit it
like any other knowledge change (or let `kb update prepare` run it on an update branch).

## `kb-legacy-synthetic-v0` (document schema 0)

### Profile config

```toml
schema = 0
[kb]
name = "Example"
namespace = "example"
[origin]
remote = "origin"
branch = "main"                 # branch name or a fully qualified ref
protocols = ["https", "ssh"]    # optional
[knowledge]                     # optional, identical to schema 1
roots = ["knowledge"]
[context]                       # optional, identical to schema 1
```

| schema 0 | schema 1 |
|---|---|
| `schema = 0` | `schema = 1` |
| `[kb] name, namespace` | `[project] name, namespace` |
| `[origin] remote` | `[source] remote` |
| `[origin] branch = "<b>"` | `[source] approved_ref = "refs/heads/<b>"` (kept as-is when it already starts with `refs/`) |
| `[origin] protocols` | `[source] allowed_protocols` (absent → schema-1 default `["https", "ssh"]`) |
| other tables | unchanged |

`[kb]` and `[origin]` are required; `[project]` or `[source]` in a schema-0 config is an
error.

### Registry files

Identical to schema 1 except `schema = 0`; the step only bumps the version.

### Records

Records use the same `+++` TOML front matter and Markdown body as schema 1, with these
common-field differences:

| schema 0 | schema 1 |
|---|---|
| `schema = 0` | `schema = 1` |
| `type = "<kind>"` | `kind = "<kind>"` |
| `state = "proposed"` / `"active"` / `"retired"` / `"replaced"` | `status = "draft"` / `"accepted"` / `"deprecated"` / `"superseded"` |
| `[applies_to] repos, modules, features` | `[scope] repos, modules, features` |
| `[applies_to]` absent, or all three lists empty/absent | `[scope] product = true` |
| `tags = [..]` | `[selectors] aliases = [..]` |
| `depends_on = [..]` | `[links] requires = [..]` |
| `see_also = [..]` | `[links] related = [..]` |
| `replaces = [..]` | `[links] supersedes = [..]` |

Empty `tags` / `depends_on` / `see_also` / `replaces` lists are dropped. Policies declare
normative rules as

```toml
[[rule]]
must = "Do this."          # or: must_not = "Never do that."
```

which become, numbered in order,

```toml
[[rules]]
id = "rule-1"
level = "must"             # must_not → "must-not"
text = "Do this."
```

Other keys of a legacy rule (e.g. `conditions`) are carried over unchanged. A rule with
both or neither of `must` / `must_not` is an error. All other kinds (`feature`,
`invariant`, `contract`, `decision`, `procedure`, `reference`, `gap`) already used their
schema-1 kind-specific fields. Unknown legacy `state` values and schema-1 field names
(`kind`, `status`, `scope`, `selectors`, `links`, `rules`) inside a schema-0 record are
errors. Everything else is carried over unchanged; the verification step then rejects
anything the schema-1 parser does not accept.

## Fixtures

* `fixtures/v0/project/` — a complete synthetic legacy project covering every
  transformation above (config, all five registries, all four states, present / empty /
  absent `[applies_to]`, every renamed list, `must` and `must_not` rules, comments and a
  body containing a TOML code block).
* `fixtures/v1-expected/project/` — the exact expected output of `kb migrate --to 1 --apply`. Tests
  compare it byte-for-byte, check that it loads under the current engine without
  diagnostics, and that re-applying changes nothing. `kb update prepare` tests migrate the
  same legacy project on an update branch.

* `fixtures/v2-expected/project/` — output of the adjacent 1 → 2 step and the default
  0 → 1 → 2 chain. Tests compare bytes, retain schema-1 semantics and check idempotence.

All fixtures are synthetic (`legacy` namespace, `example.invalid` remotes) and are never
indexed as project data.

## Adding a migration (maintainers)

1. Bump `DOCUMENT_SCHEMA` in `core/cli/src/versions.rs` and `document_schema` in
   `core/release.toml`; add the previous version to `migrates_from`.
2. Add a `Migration { from, to, name, description, record, config, registry }` entry to
   `MIGRATIONS` with pure text transforms (use `toml_edit`; keep bodies byte-identical;
   fail on unexpected shapes instead of dropping data).
3. Add `fixtures/v<from>/` and `fixtures/v<to>-expected/` and extend the tests.
4. Document the step here, including what is lossy (nothing should be).
