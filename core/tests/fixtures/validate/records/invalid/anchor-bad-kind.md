+++
schema = 1
id = "acme.bad.anchor-bad-kind"
kind = "policy"
title = "Invalid policy anchor-bad-kind"
status = "accepted"
owner = "arch"

[scope]
product = true

[[anchors]]
kind = "ticket"
path = "x"

[[rules]]
id = "r"
level = "must"
text = "Do."
+++
