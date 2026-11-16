+++
schema = 2
id = "kb.decision.agent-cli"
kind = "decision"
title = "Route agent requests for code, knowledge and tools through one CLI"
status = "draft"
owner = "maintainers"
context = "Agents chose between code graphs, grep and file search themselves, spent tokens on unstructured output and met the architectural rules only when they asked for context."
decision = "Add agent commands that route each request through a trusted tool registry to engine-owned adapters and return one bounded answer naming its sources, on top of the unchanged knowledge layer."
reasons = ["Routing by code and configuration chooses the same tool on every machine instead of leaving the choice to a model.", "A bounded, labelled answer costs fewer tokens than raw tool output and states its static or lexical limits.", "Feature records and invariant records already carry feature documentation; a second format would drift from them."]
consequences = ["kb check runs argv only from the registry of the KB checkout that supplies the engine; record text is never executed.", "Live CodeGraph answers follow the work tree after an incremental sync; commit-bound providers remain for historical correctness, and kb never creates a tool index.", "Skill protocol 3 installs the kb, kb-change, kb-review and kb-document skills; the CLI protocol, document schema and index schema stay unchanged."]

[scope]
modules = ["kb.cli", "kb.model", "kb.integrate", "kb.code"]

[[anchors]]
kind = "doc"
repo = "kb"
path = "docs/adr/0016-agent-cli.md"
+++
