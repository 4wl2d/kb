+++
schema = 1
id = "acme.bad.link-duplicate"
kind = "policy"
title = "Invalid policy link-duplicate"
status = "accepted"
owner = "arch"

[scope]
product = true

[links]
related = ["acme.a.b", "acme.a.b"]

[[rules]]
id = "r"
level = "must"
text = "Do."
+++
