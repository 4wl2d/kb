+++
schema = 1
id = "acme.bad.selector-path-traversal"
kind = "policy"
title = "Invalid policy selector-path-traversal"
status = "accepted"
owner = "arch"

[scope]
product = true

[selectors]
paths = ["../outside/**"]

[[rules]]
id = "r"
level = "must"
text = "Do."
+++
