+++
schema = 1
id = "acme.bad.setting-stricter-string"
kind = "policy"
title = "Invalid policy setting-stricter-string"
status = "accepted"
owner = "arch"

[scope]
product = true

[[settings]]
name = "mode"
type = "string"
value = "strict"
override = "stricter"
stricter = "lower"
+++
