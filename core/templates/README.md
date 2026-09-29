# kb templates

Engine-owned source templates. `kbw init` renders `project/` from them; the rest are
templates that people (or a bootstrap agent) copy and adapt. None of these files is project
data, and nothing here is indexed as knowledge.

| path | used by | purpose |
|---|---|---|
| `project/` | `kbw init` | project skeleton rendered into `project/` |
| `records/<kind>.md` | authors | one valid record per kind (policy, feature, invariant, contract, decision, procedure, reference, gap) with placeholder text |
| `ci/github/kb-knowledge.yml` | `kbw init` | downstream KB CI, installed as `.github/workflows/kb-knowledge.yml` |
| `ci/github/host-kb-impact.yml` | host repositories | host CI: KB submodule checkout, `kbw impact --snapshot pinned --check` (re-run when the description is edited), `kbw integrate --check` |
| `ci/gitlab/*.gitlab-ci.yml` | GitLab users | equivalents of both workflows |
| `mr/*_template.md` | host and KB repositories | merge request templates with the `kb-impact` block and the linked-MR checklist |
| `ownership/CODEOWNERS.tmpl` | downstream KB | code owners; organization handles are explicit parameters |
| `security/SECURITY.md.tmpl` | downstream KB | security policy, published as `.github/SECURITY.md` (GitHub prefers it over the engine-owned root `SECURITY.md`); the contact is an explicit parameter |
| `examples/synthetic-multirepo/` | `kbw init --example synthetic-multirepo` | SYNTHETIC demo project with routing tests and expected queries |

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
