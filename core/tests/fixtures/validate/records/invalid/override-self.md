+++
schema = 1
id = "acme.bad.override-self"
kind = "policy"
title = "Invalid policy override-self"
status = "accepted"
owner = "arch"

[scope]
product = true

[[settings]]
name = "limit"
type = "integer"
value = 10
override = "any"

[[overrides]]
target = "acme.bad.override-self#limit"
value = 5
reason = "Self."
+++
