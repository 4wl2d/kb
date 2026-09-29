+++
schema = 1
id = "acme.mobile.transport"
kind = "policy"
title = "Mobile transport overrides"
status = "accepted"
owner = "team-mobile"

[scope]
repos = ["mobile"]

[[overrides]]
target = "acme.security.transport#require-pinning"
value = true
reason = "Mobile clients pin the backend certificate."
+++
