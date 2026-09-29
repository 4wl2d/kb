+++
schema = 1
id = "example.backend.payments-review"
kind = "policy"
title = "Stricter review and money handling for payment code"
status = "accepted"
owner = "team-backend"

[scope]
repos = ["backend"]
modules = ["backend.payments"]

[selectors]
paths = ["src/payments/**"]
concepts = ["idempotency"]

[links]
rationale = ["example.decision.integer-money"]

[[rules]]
id = "no-float-money"
level = "must-not"
text = "Represent monetary amounts with binary floating-point types."

[[overrides]]
target = "example.common.code-review#min-reviewers"
value = 2
reason = "Payment code moves money; two approvals catch more mistakes."

[[anchors]]
kind = "source"
repo = "backend"
path = "src/payments/Money.kt"
symbol = "Money"
note = "Synthetic anchor: amounts are stored as integer minor units."
+++
