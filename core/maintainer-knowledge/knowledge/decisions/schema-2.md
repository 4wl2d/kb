+++
schema = 2
id = "kb.decision.schema-2"
kind = "decision"
title = "Add typed domain knowledge through schema 2"
status = "draft"
owner = "maintainers"
context = "Schema 1 cannot describe subsystem states, scenarios, consumers, glossary, temporal validity or executable checks."
decision = "Add optional schema-2 fields, retain strict schema-1 reading, and migrate without inventing facts."
reasons = ["Typed facts can be validated and delivered deterministically.", "Existing ids, status, comments and Markdown remain intact."]
consequences = ["New fields require schema 2.", "Independent CLI, skill, host and routing versions are not implicitly bumped."]

[scope]
modules = ["kb.model", "kb.update", "kb.context"]

[[anchors]]
kind = "doc"
repo = "kb"
path = "docs/adr/0010-schema-2.md"
+++
