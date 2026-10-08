+++
schema = 1
id = "kb.reference.harness-skill-formats"
kind = "reference"
title = "Harness skill discovery formats"
status = "accepted"
owner = "maintainers"
summary = "The generator installs one full skill, preferring .agents/skills for Codex/Cursor, then .claude/skills for Claude, .grok/skills for Grok, then .kbw/skills. Native targets are AGENTS.md for Codex/Grok/Junie, CLAUDE.md, Cursor .mdc and Copilot instructions. Secondary targets point to the shared workflow; existing masking overrides are preserved and receive pointers. Installation checks do not prove runtime loading."

[scope]
modules = ["kb.integrate"]

[selectors]
aliases = ["skill", "SKILL.md", "claude code", "codex", "cursor", "agents.md", "grok", "copilot", "junie"]

[[sources]]
title = "Claude Code skills"
url = "https://code.claude.com/docs/en/skills"

[[sources]]
title = "Codex build skills"
url = "https://learn.chatgpt.com/docs/customization/overview"

[[sources]]
title = "Cursor agent skills"
url = "https://cursor.com/docs/skills"

[[sources]]
title = "Grok skills"
url = "https://docs.x.ai/build/features/skills-plugins-marketplaces"

[[sources]]
title = "Copilot custom instructions"
url = "https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-custom-instructions"

[[sources]]
title = "Junie guidelines"
url = "https://junie.jetbrains.com/docs/guidelines-and-memory.html"
+++
