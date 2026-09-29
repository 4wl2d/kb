+++
schema = 1
id = "legacy.decision.token-rotation"
kind = "decision"
title = "Rotate refresh tokens"
status = "accepted"
owner = "arch"
context = "Long-lived refresh tokens leak through logs and backups."
decision = "Refresh tokens are single-use and rotated on every refresh."
reasons = ["A leaked token stops working after its next legitimate use."]
consequences = ["Clients must persist the new token atomically."]

# An empty applicability meant product-wide in the legacy format.
[scope]
product = true
+++
## Alternatives considered

Sliding expiry without rotation was rejected.
