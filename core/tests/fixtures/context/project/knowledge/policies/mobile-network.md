+++
schema = 1
id = "acme.mobile.network"
kind = "policy"
title = "Mobile network timeouts"
status = "accepted"
owner = "team-mobile"

[scope]
repos = ["mobile"]

[[overrides]]
target = "acme.product.network#request-timeout-ms"
value = 5000
reason = "Cellular networks fail slowly; fail fast and let the user retry."
+++
