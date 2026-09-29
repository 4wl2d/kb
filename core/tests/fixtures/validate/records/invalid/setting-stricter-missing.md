+++
schema = 1
id = "acme.bad.setting-stricter-missing"
kind = "policy"
title = "Invalid policy setting-stricter-missing"
status = "accepted"
owner = "arch"

[scope]
product = true

[[settings]]
name = "limit"
type = "integer"
value = 10
override = "stricter"
+++
