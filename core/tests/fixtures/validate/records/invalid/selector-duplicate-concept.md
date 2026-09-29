+++
schema = 1
id = "acme.bad.selector-duplicate-concept"
kind = "policy"
title = "Invalid policy selector-duplicate-concept"
status = "accepted"
owner = "arch"

[scope]
product = true

[selectors]
concepts = ["auth-token", "auth-token"]

[[rules]]
id = "r"
level = "must"
text = "Do."
+++
