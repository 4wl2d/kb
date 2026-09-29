+++
schema = 1
id = "acme.bad.anchor-bad-commit"
kind = "policy"
title = "Invalid policy anchor-bad-commit"
status = "accepted"
owner = "arch"

[scope]
product = true

[[anchors]]
kind = "change"
commit = "not-hex"

[[rules]]
id = "r"
level = "must"
text = "Do."
+++
