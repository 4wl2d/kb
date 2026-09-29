+++
schema = 0
id = "legacy.contract.token-api"
type = "contract"
title = "Token refresh API"
state = "active"
owner = "arch"
see_also = ["legacy.decision.token-rotation"]

[applies_to]
repos = ["mobile", "backend"]

[[parties]]
id = "provider"
repo = "backend"
modules = ["backend.api"]
role = "Issues and rotates tokens"

[[parties]]
id = "consumer"
repo = "mobile"
role = "Refreshes tokens before expiry"

[[obligations]]
id = "rotate"
party = "provider"
level = "must"
text = "Rotate the refresh token on every refresh call."
+++
