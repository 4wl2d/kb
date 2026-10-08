+++
schema = 2
id = "kb.decision.declarative-verification"
kind = "decision"
title = "Run bounded declarative probes without executing record text"
status = "draft"
owner = "maintainers"
context = "Mechanical rules can be checked, but arbitrary record commands and unproven applicability would weaken trust."
decision = "Evaluate typed regex, naming, API and import probes over explicit Git/index/work-tree facts, reporting unavailable evidence."
reasons = ["A partial graph cannot prove the absence of a forbidden dependency.", "Textual conditions and exceptions need an explicit decision.", "Source hashes establish bytes, not semantic truth or test execution."]
consequences = ["Must-level failures block; strict mode includes advisory checks.", "Commit hooks inspect pending messages and staged scope; MR jobs inspect committed targets.", "Freshness uses an explicit or commit-derived date; drift and ledger produce review work without accepting records."]

[scope]
modules = ["kb.cli", "kb.update", "kb.snapshot"]

[[anchors]]
kind = "doc"
repo = "kb"
path = "docs/adr/0015-declarative-verification.md"
+++
