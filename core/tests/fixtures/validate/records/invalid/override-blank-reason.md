+++
schema = 1
id = "acme.bad.override-blank-reason"
kind = "policy"
title = "Invalid policy override-blank-reason"
status = "accepted"
owner = "arch"

[scope]
product = true

[[overrides]]
target = "acme.security.transport#require-pinning"
value = true
reason = ""
+++
