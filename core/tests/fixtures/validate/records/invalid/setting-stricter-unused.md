+++
schema = 1
id = "acme.bad.setting-stricter-unused"
kind = "policy"
title = "Invalid policy setting-stricter-unused"
status = "accepted"
owner = "arch"

[scope]
product = true

[[settings]]
name = "limit"
type = "integer"
value = 10
override = "any"
stricter = "lower"
+++
