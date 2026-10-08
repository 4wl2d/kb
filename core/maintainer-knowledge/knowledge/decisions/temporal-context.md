+++
schema = 2
id = "kb.decision.temporal-context"
kind = "decision"
title = "Historical context uses explicit validity and frozen evidence"
status = "draft"
owner = "maintainers"
context = "A current corpus and index can leak later facts or change historical ranking even when the host checkout is old."
decision = "Filter every lookup and lexical statistic through a pure temporal view, while replay separately freezes host, KB, registries and provider inputs."
reasons = ["Undated records cannot establish that a fact was known at the cutoff.", "Date metadata does not reconstruct earlier text or prove a review occurred."]
consequences = ["Historical queries withhold undated accepted records and report partial context.", "Required records outside the slice remain missing obligations."]

[scope]
modules = ["kb.context", "kb.snapshot", "kb.evaluation"]

[[anchors]]
kind = "doc"
repo = "kb"
path = "docs/adr/0012-temporal-context.md"
+++
