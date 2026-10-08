+++
schema = 2
id = "kb.decision.code-provider"
kind = "decision"
title = "Bind external code facts to an immutable host commit"
status = "draft"
owner = "maintainers"
context = "Symbol lookup and indirect impact need code intelligence without embedding a parser, daemon or model in the engine."
decision = "Use a versioned one-shot provider response, validate its Git source hashes, and preserve completeness and uncertainty."
reasons = ["A current host index is not evidence about an older commit.", "Static relationships do not establish runtime reachability or normative truth."]
consequences = ["Reference adapters remain outside core/cli.", "Composed context is opt-in until a downstream A/B demonstrates a gain.", "The engine budgets whole code units after mandatory knowledge."]

[scope]
modules = ["kb.context", "kb.update"]

[[anchors]]
kind = "doc"
repo = "kb"
path = "docs/adr/0011-code-provider.md"
+++
