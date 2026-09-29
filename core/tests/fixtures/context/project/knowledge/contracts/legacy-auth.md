+++
schema = 1
id = "acme.contract.legacy-auth"
kind = "contract"
title = "Legacy session-cookie authentication"
status = "superseded"
owner = "arch"

[scope]
repos = ["mobile", "backend"]

[[parties]]
id = "server"
repo = "backend"
role = "Issues session cookies"

[[parties]]
id = "app"
repo = "mobile"
role = "Sends the session cookie"

[[obligations]]
id = "cookie-expiry"
party = "server"
level = "must"
text = "Expire session cookies after 30 days."
+++
Kept for history: the cookie flow was replaced by the token refresh API.
