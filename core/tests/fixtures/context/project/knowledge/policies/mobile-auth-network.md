+++
schema = 1
id = "acme.mobile.auth-network"
kind = "policy"
title = "Authentication request timeouts"
status = "accepted"
owner = "team-mobile"

[scope]
modules = ["mobile.auth"]

[[overrides]]
target = "acme.product.network#request-timeout-ms"
value = 3000
reason = "Sign-in screens block the user, so token calls answer quickly."
+++
