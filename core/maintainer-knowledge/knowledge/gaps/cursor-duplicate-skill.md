+++
schema = 1
id = "kb.gap.cursor-duplicate-skill"
kind = "gap"
title = "Cursor behavior with two identical skill directories is unverified"
status = "accepted"
owner = "maintainers"
gap = "ambiguity"
description = "When both the claude and codex harnesses are enabled, the same `kb` skill exists in .claude/skills and .agents/skills; Cursor reads both locations and its de-duplication behavior is not documented."
questions = ["Does Cursor list the skill twice or de-duplicate by name?"]

[scope]
modules = ["kb.integrate"]

[selectors]
aliases = ["cursor", "duplicate skill"]

[links]
related = ["kb.reference.harness-skill-formats"]
+++
