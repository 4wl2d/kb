+++
schema = 1
id = "acme.bad.link-bad-id"
kind = "policy"
title = "Invalid policy link-bad-id"
status = "accepted"
owner = "arch"

[scope]
product = true

[links]
requires = ["Not An Id"]

[[rules]]
id = "r"
level = "must"
text = "Do."
+++
