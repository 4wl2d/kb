# kb skills (canonical sources)

Engine-owned sources of the agent integration. They are never installed directly:
`kb integrate --generate` renders them with `project/skill-config/skill.toml` and the
profile config into `project/skill-config/generated/` (committed and reviewed), and
`kb integrate --apply` installs that bundle into each host repository.

The bundle is a skill set of four skills, installed side by side in the harness's skill
directory (for example `.claude/skills/<name>/` or `.agents/skills/<name>/`):

| skill | source | rendered to | use |
|---|---|---|---|
| `kb` | `kb/SKILL.md.tmpl`, `kb/references/*.md.tmpl` | `skills/kb/SKILL.md`, `skills/kb/references/*.md` (read lazily by agents) | the commands, how to read answers, error recovery, references |
| `kb-change` | `kb-change/SKILL.md.tmpl` | `skills/kb-change/SKILL.md` | the universal change procedure: locate, `doc`, `impact`/`callers`, `context` before the first edit, edit, `check`, documentation in the same change |
| `kb-review` | `kb-review/SKILL.md.tmpl` | `skills/kb-review/SKILL.md` | reviewing a change against obligations, invariants, callers and checks |
| `kb-document` | `kb-document/SKILL.md.tmpl` | `skills/kb-document/SKILL.md` | creating or updating feature documentation |

Skills name kb commands only, never a code tool, so connecting another tool in
`tools.toml` changes no skill ([docs/agent-cli.md](../../docs/agent-cli.md)). Each
generated `SKILL.md` is capped at 16 KiB.

Instruction blocks and pointers:

| path | rendered to |
|---|---|
| `blocks/workflow.md.tmpl` | the always-on workflow ("Code, knowledge and tools (kb)") with a verifiable core |
| `blocks/agents.md.tmpl` | managed `AGENTS.md` workflow/pointer (Codex, Grok, Junie) |
| `blocks/claude.md.tmpl` | `blocks/CLAUDE.md.block` (claude) |
| `blocks/cursor.mdc.tmpl` | native `.cursor/rules/kb.mdc` with `alwaysApply` |
| `blocks/claude-command.md.tmpl` | short Claude aliases `.claude/commands/<skill>.md`, one per skill, when the skill set is shared from another directory |

Placeholders (strict; unknown placeholders are errors): `{{project_name}}`,
`{{namespace}}`, `{{kb_path}}`, `{{kb}}` (how agents call kb: `kb`, or `kb --root <kb_path>`
when the KB is not mounted at `.kb`), `{{skill_protocol}}`, `{{engine_version}}`,
`{{default_intent}}`, `{{notes}}`, `{{snapshot_arg}}` (empty, or ` --snapshot latest|pinned`
from `snapshot` in skill.toml; appended to every snapshot-reading `kb` command the skills
and the instruction blocks show), `{{agents_skill_path}}` and `{{claude_skill_path}}` (the
installed `kb` SKILL.md each instruction block points to), `{{skills_dir}}` (the host
directory that holds the skill set, for example `.claude/skills`; skills and blocks use it
to point at each other), plus generated workflow/core values `{{managed_instructions}}`,
`{{core_lines}}`, `{{core_arg}}` and `{{core_source}}` where used by the corresponding
template. The renderer supplies only the values a template needs.

Changing the protocol that the skills describe requires bumping `skill_protocol` in
`core/release.toml` and the engine; agents that pass an older `--skill-protocol` then get
`SKILL_OUTDATED`. Harness discovery rules and source links: `kb/references/harnesses.md.tmpl`.

Skill protocol 3 (the agent CLI and the skill set, [ADR 0016](../../docs/adr/0016-agent-cli.md))
supports Codex, Claude Code, Cursor, Grok Build, Copilot and Junie. The installer chooses one
skill directory for the full set and leaves secondary native pointers, preserving edits
outside managed markers and refusing modified locked files. Existing masking override
files are handled explicitly. `integrate --probe` verifies installation only; real harness
load/consultation remains a separate observation. Never create duplicate copies of the
skill set to guess around an unverified harness discovery issue.
