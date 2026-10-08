# kb skills (canonical sources)

Engine-owned sources of the agent integration. They are never installed directly:
`kbw integrate --generate` renders them with `project/skill-config/skill.toml` and the
profile config into `project/skill-config/generated/` (committed and reviewed), and
`<kb_path>/kbw integrate --apply` installs that bundle into each host repository.

| path | rendered to |
|---|---|
| `kb/SKILL.md.tmpl` | one full `skills/kb/SKILL.md`, shared by every enabled harness |
| `kb/references/*.md.tmpl` | `skills/kb/references/*.md` (read lazily by agents) |
| `blocks/workflow.md.tmpl` | primary four-step workflow with a verifiable always-on core |
| `blocks/agents.md.tmpl` | managed `AGENTS.md` workflow/pointer (Codex, Grok, Junie) |
| `blocks/claude.md.tmpl` | `blocks/CLAUDE.md.block` (claude) |
| `blocks/cursor.mdc.tmpl` | native `.cursor/rules/kb.mdc` with `alwaysApply` |
| `blocks/claude-command.md.tmpl` | short Claude `/kb` alias when the full skill is shared elsewhere |

Placeholders (strict; unknown placeholders are errors): `{{project_name}}`,
`{{namespace}}`, `{{kb_path}}`, `{{skill_protocol}}`, `{{engine_version}}`,
`{{default_intent}}`, `{{notes}}`, `{{snapshot_arg}}` (empty, or ` --snapshot latest|pinned`
from `snapshot` in skill.toml; appended to every snapshot-reading `kbw` command the skill
and the instruction blocks show), `{{agents_skill_path}}` and `{{claude_skill_path}}` (the
installed SKILL.md each instruction block points to), plus generated workflow/core values
`{{managed_instructions}}`, `{{core_lines}}`, `{{core_arg}}` and `{{core_source}}` where
used by the corresponding template. The renderer supplies only the values a template needs.

Changing the protocol that `SKILL.md` describes requires bumping `skill_protocol` in
`core/release.toml` and the engine; agents that pass an older `--skill-protocol` then get
`SKILL_OUTDATED`. Harness discovery rules and source links: `kb/references/harnesses.md.tmpl`.

Skill protocol 2 supports Codex, Claude Code, Cursor, Grok Build, Copilot and Junie. The
installer chooses one full skill and leaves secondary native pointers, preserving edits
outside managed markers and refusing modified locked files. Existing masking override
files are handled explicitly. `integrate --probe` verifies installation only; real harness
load/consultation remains a separate observation. Never create duplicate full skills to
guess around an unverified harness discovery issue.
