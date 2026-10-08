+++
schema = 2
id = "legacy.contract.token-api"
kind = "contract"
title = "Token refresh API"
status = "accepted"
owner = "arch"

[scope]
repos = ["mobile", "backend"]

[links]
related = ["legacy.decision.token-rotation"]

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
