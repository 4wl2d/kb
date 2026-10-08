+++
schema = 2
id = "example.mobile.single-refresh"
kind = "invariant"
title = "At most one token refresh in flight per session"
status = "accepted"
owner = "team-mobile"

[scope]
modules = ["mobile.auth"]

[selectors]
concepts = ["auth-token"]
aliases = ["concurrent refresh", "параллельное обновление"]

[links]
related = ["example.contract.token-refresh"]

[[statements]]
id = "single-flight"
level = "must"
text = "Concurrent requests that need a new access token wait for the single refresh already in flight."

[[statements.exceptions]]
id = "account-switch"
text = "Switching accounts cancels the pending refresh and starts a new one for the new account."

[[anchors]]
kind = "test"
repo = "mobile"
path = "app/auth/RefreshCoordinatorTest.kt"
symbol = "concurrentCallersShareOneRefresh"
+++
