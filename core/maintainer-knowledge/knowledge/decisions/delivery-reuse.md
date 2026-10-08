+++
schema = 2
id = "kb.decision.delivery-reuse"
kind = "decision"
title = "Reuse delivered knowledge only with explicit matching receipts"
status = "draft"
owner = "maintainers"
context = "Repeated universal and unchanged obligations consume context, but silent omission can hide required knowledge."
decision = "Use a verified installed core or an explicitly supplied local prior receipt, compare content hashes, and recompute applicability before rendering references."
reasons = ["An id alone does not prove unchanged content.", "An installed file does not prove that a harness loaded it."]
consequences = ["Missing or stale evidence cannot suppress an obligation.", "Terse output preserves typed content and defers Markdown.", "Outline is an inventory and never a delivery receipt."]

[scope]
modules = ["kb.context", "kb.integrate", "kb.snapshot"]

[[anchors]]
kind = "doc"
repo = "kb"
path = "docs/adr/0013-delivery-reuse.md"
+++
