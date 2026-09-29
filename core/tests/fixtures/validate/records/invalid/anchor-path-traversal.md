+++
schema = 1
id = "acme.bad.anchor-path-traversal"
kind = "policy"
title = "Invalid policy anchor-path-traversal"
status = "accepted"
owner = "arch"

[scope]
product = true

[[anchors]]
kind = "doc"
path = "../secrets.md"

[[rules]]
id = "r"
level = "must"
text = "Do."
+++
