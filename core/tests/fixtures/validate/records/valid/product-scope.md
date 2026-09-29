+++
schema = 1
id = "acme.security.transport"
kind = "policy"
title = "Transport security"
status = "accepted"
owner = "arch"

[scope]
product = true

[[settings]]
name = "allowed-ciphers"
type = "string-set"
value = ["tls13-aes-gcm", "tls13-chacha20"]
override = "stricter"
stricter = "subset"

[[settings]]
name = "require-pinning"
type = "boolean"
value = false
override = "stricter"
stricter = "true"

[[settings]]
name = "audit-events"
type = "string-set"
value = ["login"]
override = "stricter"
stricter = "superset"

[[settings]]
name = "min-tls"
type = "integer"
value = 12
override = "stricter"
stricter = "higher"

[[settings]]
name = "allow-debug-menu"
type = "boolean"
value = true
override = "stricter"
stricter = "false"
+++
