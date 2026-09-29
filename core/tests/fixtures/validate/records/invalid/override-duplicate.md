+++
schema = 1
id = "acme.bad.override-duplicate"
kind = "policy"
title = "Invalid policy override-duplicate"
status = "accepted"
owner = "arch"

[scope]
product = true

[[overrides]]
target = "acme.security.transport#require-pinning"
value = true
reason = "One."

[[overrides]]
target = "acme.security.transport#require-pinning"
value = true
reason = "Two."
+++
