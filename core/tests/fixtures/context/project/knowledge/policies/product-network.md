+++
schema = 1
id = "acme.product.network"
kind = "policy"
title = "Outbound network calls"
status = "accepted"
owner = "arch"

[scope]
product = true

[[rules]]
id = "explicit-timeout"
level = "must"
text = "Set an explicit timeout on every outbound network request."

[[settings]]
name = "request-timeout-ms"
type = "integer"
value = 10000
override = "stricter"
stricter = "lower"
description = "Upper bound for a single request."

[[settings]]
name = "retry-count"
type = "integer"
value = 3
override = "any"

[[settings]]
name = "tls-min-version"
type = "string"
value = "1.2"
+++
