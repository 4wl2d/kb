+++
schema = 1
id = "acme.bad.setting-set-duplicate"
kind = "policy"
title = "Invalid policy setting-set-duplicate"
status = "accepted"
owner = "arch"

[scope]
product = true

[[settings]]
name = "hosts"
type = "string-set"
value = ["a", "a"]
+++
