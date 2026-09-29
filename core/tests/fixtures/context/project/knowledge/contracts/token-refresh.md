+++
schema = 1
id = "acme.contract.token-refresh"
kind = "contract"
title = "Token refresh API"
status = "accepted"
owner = "arch"
interface = "POST /v2/token/refresh"

[scope]
modules = ["mobile.auth", "backend.api"]

[selectors]
concepts = ["auth-token"]

[links]
supersedes = ["acme.contract.legacy-auth"]

[applicability]
versions = { mobile = ">=2.0.0" }

[[parties]]
id = "provider"
repo = "backend"
modules = ["backend.api"]
role = "Issues and rotates refresh tokens"

[[parties]]
id = "consumer"
repo = "mobile"
modules = ["mobile.auth"]
role = "Refreshes access tokens before they expire"

[[obligations]]
id = "rotate"
party = "provider"
level = "must"
text = "Rotate the refresh token on every successful refresh call."

[[obligations]]
id = "retry-once"
party = "consumer"
level = "must"
text = "Retry the original request at most once after a successful refresh that followed HTTP 401."

[[obligations.exceptions]]
id = "offline"
text = "When the device is offline the request is queued instead of retried."

[[anchors]]
kind = "change"
change = "!42"
note = "introduced the v2 endpoint"
+++
