+++
schema = 1
id = "acme.bad.override-bad-target"
kind = "policy"
title = "Invalid policy override-bad-target"
status = "accepted"
owner = "arch"

[scope]
product = true

[[overrides]]
target = "acme.security.transport"
value = true
reason = "No setting name."
+++
