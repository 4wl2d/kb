+++
schema = 0
id = "legacy.decision.token-rotation"
type = "decision"
title = "Rotate refresh tokens"
state = "active"
owner = "arch"
context = "Long-lived refresh tokens leak through logs and backups."
decision = "Refresh tokens are single-use and rotated on every refresh."
reasons = ["A leaked token stops working after its next legitimate use."]
consequences = ["Clients must persist the new token atomically."]

# An empty applicability meant product-wide in the legacy format.
[applies_to]
repos = []
+++
## Alternatives considered

Sliding expiry without rotation was rejected.
