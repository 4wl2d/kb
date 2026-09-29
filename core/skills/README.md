# kb skills (canonical sources)

Engine-owned sources of the agent integration. They are never installed directly:
`kbw integrate --generate` renders them with `project/skill-config/skill.toml` and the
profile config into `project/skill-config/generated/` (committed and reviewed), and
`<kb_path>/kbw integrate --apply` installs that bundle into each host repository.

| path | rendered to |
|---|---|
| `kb/SKILL.md.tmpl` | `skills/kb/SKILL.md` (one file valid for Claude Code, Codex and Cursor) |
| `kb/references/*.md.tmpl` | `skills/kb/references/*.md` (read lazily by agents) |
| `blocks/agents.md.tmpl` | `blocks/AGENTS.md.block` (codex, cursor) |
| `blocks/claude.md.tmpl` | `blocks/CLAUDE.md.block` (claude) |

Placeholders (strict; unknown placeholders are errors): `{{project_name}}`,
`{{namespace}}`, `{{kb_path}}`, `{{skill_protocol}}`, `{{engine_version}}`,
`{{default_intent}}`, `{{notes}}`, `{{snapshot_arg}}` (empty, or ` --snapshot latest|pinned`
from `snapshot` in skill.toml; appended to every snapshot-reading `kbw` command the skill
and the instruction blocks show), `{{agents_skill_path}}` and `{{claude_skill_path}}` (the
installed SKILL.md each instruction block points to).

Changing the protocol that `SKILL.md` describes requires bumping `skill_protocol` in
`core/release.toml` and the engine; agents that pass an older `--skill-protocol` then get
`SKILL_OUTDATED`. Harness discovery rules and source links: `kb/references/harnesses.md.tmpl`.
