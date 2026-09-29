+++
schema = 1
id = "acme.bad.setting-stricter-invalid"
kind = "policy"
title = "Invalid policy setting-stricter-invalid"
status = "accepted"
owner = "arch"

[scope]
product = true

[[settings]]
name = "limit"
type = "integer"
value = 10
override = "stricter"
stricter = "superset"
+++
