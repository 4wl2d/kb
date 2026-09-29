+++
schema = 1
id = "acme.bad.unknown-nested"
kind = "policy"
title = "Unknown field inside a statement"
status = "accepted"
owner = "arch"

[scope]
product = true

[[rules]]
id = "r"
level = "must"
text = "Do the thing."
priority = 1
+++
