+++
schema = 2
id = "kb.decision.knowledge-production"
kind = "decision"
title = "Produce domain knowledge from explicit evidence"
status = "draft"
owner = "maintainers"
context = "Instruction migration alone does not collect the domain facts needed to implement or diagnose changes."
decision = "Use deterministic work orders and draft validation, with reviewer acceptance through the approved Git ref."
reasons = ["Code observations need a distinct descriptive status.", "A source anchor or a test command alone does not establish a mandatory rule."]
consequences = ["Capture and submission never accept knowledge.", "Generation and maintenance report evidence, coverage and unresolved gaps."]

[scope]
product = true

[[anchors]]
kind = "doc"
repo = "kb"
path = "docs/adr/0014-knowledge-production.md"
+++
