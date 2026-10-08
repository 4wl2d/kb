+++
schema = 2
id = "example.contract.token-refresh"
kind = "contract"
title = "Token refresh API between mobile and backend"
status = "accepted"
owner = "architecture"
interface = "POST /v2/auth/refresh (synthetic)"

[scope]
modules = ["mobile.auth", "backend.api"]

[selectors]
paths = ["mobile:app/auth/**", "backend:src/api/auth/**"]
concepts = ["auth-token"]

[links]
requires = ["example.contract.error-envelope"]
related = ["example.feature.login"]

[[parties]]
id = "provider"
repo = "backend"
modules = ["backend.api"]
role = "Issues and rotates tokens"

[[parties]]
id = "consumer"
repo = "mobile"
modules = ["mobile.auth"]
role = "Refreshes tokens before they expire"

[[obligations]]
id = "rotate-on-refresh"
party = "provider"
level = "must"
text = "Issue a new refresh token on every successful refresh and invalidate the previous one."

[[obligations]]
id = "grace-window"
party = "provider"
level = "should"
text = "Accept the previous refresh token for 30 seconds after rotation."
conditions = ["the previous token was issued to the same device"]

[[obligations]]
id = "replace-atomically"
party = "consumer"
level = "must"
text = "Replace the stored refresh token only after the refresh response was fully received."

[[obligations.exceptions]]
id = "logout-in-flight"
text = "If the user logged out while the request was in flight, discard the response instead."

[[anchors]]
kind = "doc"
repo = "shared-contracts"
path = "schemas/auth/refresh.yaml"
+++
