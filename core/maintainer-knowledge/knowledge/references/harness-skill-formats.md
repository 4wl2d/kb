+++
schema = 1
id = "kb.reference.harness-skill-formats"
kind = "reference"
title = "Harness skill discovery formats"
status = "accepted"
owner = "maintainers"
summary = "Claude Code reads .claude/skills/<name>/SKILL.md; Codex reads .agents/skills/<name>/SKILL.md from the working directory up to the repository root; Cursor reads .agents/skills and .cursor/skills and also .claude/skills and .codex/skills for compatibility. Codex and Cursor read AGENTS.md; Claude Code reads CLAUDE.md."

[scope]
modules = ["kb.integrate"]

[selectors]
aliases = ["skill", "SKILL.md", "claude code", "codex", "cursor", "agents.md"]

[[sources]]
title = "Claude Code skills"
url = "https://code.claude.com/docs/en/skills"

[[sources]]
title = "Codex build skills"
url = "https://developers.openai.com/codex/build-skills"

[[sources]]
title = "Cursor agent skills"
url = "https://cursor.com/docs/skills"
+++
