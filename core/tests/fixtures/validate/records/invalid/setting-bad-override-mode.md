+++
schema = 1
id = "acme.bad.setting-bad-override-mode"
kind = "policy"
title = "Invalid policy setting-bad-override-mode"
status = "accepted"
owner = "arch"

[scope]
product = true

[[settings]]
name = "limit"
type = "integer"
value = 10
override = "sometimes"
+++
