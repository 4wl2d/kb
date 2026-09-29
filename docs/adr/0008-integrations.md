# ADR 0008: Generated skills and managed instruction blocks

Status: accepted

## Decision

A canonical skill (`core/skills/kb`) plus typed project settings
(`project/skill-config/skill.toml`) render a committed bundle in
`project/skill-config/generated/`. Host integration copies the bundle to
`.claude/skills/kb/` (Claude Code) and `.agents/skills/kb/` (Codex; Cursor also reads it) and
maintains named managed blocks in `CLAUDE.md` / `AGENTS.md`, tracking installed hashes in
`.kbw/integration.lock` so user edits are detected instead of overwritten. Every call carries
`--skill-protocol`; a mismatch fails with `SKILL_OUTDATED`, telling the running agent to
re-read the skill, because changing a file does not update instructions already loaded by a
model.

## Consequences

* No undocumented hooks; text instructions do not guarantee agent behavior.
* With both Claude and Codex enabled, Cursor may see two identical skills (tracked as a known
  gap in the maintainer knowledge).
