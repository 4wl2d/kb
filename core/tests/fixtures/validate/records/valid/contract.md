+++
schema = 1
id = "acme.contract.token-api"
kind = "contract"
title = "Token refresh API"
status = "accepted"
owner = "arch"
interface = "POST /v1/token/refresh"

[scope]
repos = ["mobile", "backend"]

[[parties]]
id = "provider"
repo = "backend"
modules = ["backend.api"]
role = "Issues tokens"

[[parties]]
id = "consumer"
repo = "mobile"
modules = ["mobile.auth"]
role = "Refreshes tokens"

[[obligations]]
id = "rotate"
party = "provider"
level = "must"
text = "Rotate the refresh token on every refresh call."

[[obligations]]
id = "retry"
party = "consumer"
level = "should"
text = "Retry a failed refresh at most once."
conditions = ["The failure is a network error."]

[[obligations.exceptions]]
id = "offline"
text = "Offline mode never retries."
+++
