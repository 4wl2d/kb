+++
schema = 1
id = "acme.bad.link-self"
kind = "policy"
title = "Invalid policy link-self"
status = "accepted"
owner = "arch"

[scope]
product = true

[links]
related = ["acme.bad.link-self"]

[[rules]]
id = "r"
level = "must"
text = "Do."
+++
