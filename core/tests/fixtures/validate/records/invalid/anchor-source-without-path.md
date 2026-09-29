+++
schema = 1
id = "acme.bad.anchor-source-without-path"
kind = "policy"
title = "Invalid policy anchor-source-without-path"
status = "accepted"
owner = "arch"

[scope]
product = true

[[anchors]]
kind = "source"
repo = "mobile"

[[rules]]
id = "r"
level = "must"
text = "Do."
+++
