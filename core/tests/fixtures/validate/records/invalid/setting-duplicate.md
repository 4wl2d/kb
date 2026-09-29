+++
schema = 1
id = "acme.bad.setting-duplicate"
kind = "policy"
title = "Invalid policy setting-duplicate"
status = "accepted"
owner = "arch"

[scope]
product = true

[[settings]]
name = "limit"
type = "integer"
value = 10

[[settings]]
name = "limit"
type = "integer"
value = 20
+++
