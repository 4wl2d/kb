+++
schema = 1
id = "acme.bad.setting-owners-unused"
kind = "policy"
title = "Invalid policy setting-owners-unused"
status = "accepted"
owner = "arch"

[scope]
product = true

[[settings]]
name = "limit"
type = "integer"
value = 10
override_owners = ["arch"]
+++
