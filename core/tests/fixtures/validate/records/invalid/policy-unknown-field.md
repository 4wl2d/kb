+++
schema = 1
id = "acme.bad.unknown-field"
kind = "policy"
title = "Unknown top-level field"
status = "accepted"
owner = "arch"
severity = "high"

[scope]
product = true

[[rules]]
id = "r"
level = "must"
text = "Do the thing."
+++
