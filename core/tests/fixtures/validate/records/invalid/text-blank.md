+++
schema = 1
id = "acme.bad.text-blank"
kind = "policy"
title = "Invalid policy text-blank"
status = "accepted"
owner = "arch"

[scope]
product = true

[[rules]]
id = "r"
level = "must"
text = "  "
+++
