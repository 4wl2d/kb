# kb templates

Engine-owned source templates. `kbw init` renders `project/` from them; the rest are
templates that people (or a bootstrap agent) copy and adapt. None of these files is project
data, and nothing here is indexed as knowledge.

| path | used by | purpose |
|---|---|---|
| `project/` | `kbw init` | project skeleton rendered into `project/`, including the tool registry `project/tools.toml` (from `project/tools.toml.tmpl`: the default CodeGraph, ripgrep and git grep routes plus commented `code-provider` and check examples) |
| `records/<kind>.md` | authors | one valid record per kind (policy, feature, invariant, contract, decision, procedure, reference, gap) with placeholder text; the feature template is the compact per-feature document: at most 60 lines (`FEATURE_TOO_LONG`), with an entry-point `source` anchor that names a `symbol` |
| `ci/github/kb-knowledge.yml` | `kbw init` | downstream KB CI, installed as `.github/workflows/kb-knowledge.yml` |
| `ci/gitlab/kb-knowledge.gitlab-ci.yml` | `kbw init` | `.gitlab/ci/kb-knowledge.yml`, explicitly included by the team's entrypoint |
| `ci/github/host-kb-impact.yml` | host repositories | host CI: KB submodule checkout, `kbw impact --snapshot pinned --check` (re-run when the description is edited), `kbw integrate --check` |
| `ci/*/host-kb-verify*`, `ci/hooks/commit-msg` | host repositories | opt-in declarative checks on committed or staged input |
| `ci/*/host-knowledge-from-change*`, CI glue `*.sh` | teams enabling accrual | merged-change export, draft validation and separately authorized publication |
| `mr/*_template.md` | host repositories | knowledge learned, `kb-impact`, linked-MR checklist and optional evaluation labels |
| `mr/kb_review.md` | `kbw init` | KB GitHub/GitLab review templates with the evidence ladder and named reviewer |
| `ownership/CODEOWNERS.tmpl` | downstream KB | code owners; organization handles are explicit parameters |
| `security/SECURITY.md.tmpl` | downstream KB | security policy, published as `.github/SECURITY.md` (GitHub prefers it over the engine-owned root `SECURITY.md`); the contact is an explicit parameter |
| `examples/synthetic-multirepo/` | `kbw init --example synthetic-multirepo` | SYNTHETIC demo project with routing tests, expected queries, a feature record with entry-point anchors and a `tools.toml` with repo-qualified checks per repository |

## Rendering rules

Files ending in `.tmpl` are rendered and lose the suffix; all other files are copied byte for
byte. A placeholder is `{{name}}` (`[a-z][a-z0-9_]*`). Rendering is strict: an unknown or
malformed placeholder is an error, so no placeholder can survive, and values are substituted
in a single pass. Values rendered into `*.toml` files are TOML-escaped. Organization names,
remote URLs and contacts are always explicit parameters; kb never invents defaults for them.

Placeholders of `project/` (supplied by `kbw init`):

| placeholder | source |
|---|---|
| `{{name}}` | `--name` |
| `{{namespace}}` | `--namespace` |
| `{{remote}}` | `--remote` (default `origin`) |
| `{{approved_ref}}` | `--approved-ref` (default `refs/heads/main`) |
| `{{kb_path}}` | `--kb-path` (default `.kb`) |
| `{{harnesses}}` | `--harness` (repeatable; default claude, codex, cursor), rendered as a TOML array |
| `{{upstream_url_line}}` | `--upstream-url`, rendered as a `url = "..."` line (a comment when not given) |
| `{{upstream_revision_line}}` | the KB checkout's `HEAD` at init time (a comment when not resolvable) |

Parameters of the adapt-by-hand templates: `ownership/CODEOWNERS.tmpl` uses
`{{engine_owners}}` and `{{knowledge_owners}}`; `security/SECURITY.md.tmpl` uses
`{{project_name}}` and `{{security_contact}}`. The CI templates contain no placeholders:
set `KB_PATH` in the host workflows instead.

The skill templates live in `core/skills/` and are rendered by `kbw integrate --generate`.
New records/project registries use schema 2; routing and skill configuration retain schema
1, and the tool registry has its own schema 1 (`core/schemas/tools.v1.schema.json`, format in
[docs/agent-cli.md](../../docs/agent-cli.md#tool-registry)). Subsystem, checklist and
glossary groups are ordinary directories with existing record kinds. The [CI guide](ci/README.md) explains pinned host inputs, strict evidence checks,
team-owned model wrappers, credential separation and opt-in publishing. Init preserves
existing templates and never enables a paid job or a protected publication environment.
