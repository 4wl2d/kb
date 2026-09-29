+++
schema = 1
id = "acme.bad.anchor-change-without-ref"
kind = "policy"
title = "Invalid policy anchor-change-without-ref"
status = "accepted"
owner = "arch"

[scope]
product = true

[[anchors]]
kind = "change"
note = "Which change?"

[[rules]]
id = "r"
level = "must"
text = "Do."
+++
