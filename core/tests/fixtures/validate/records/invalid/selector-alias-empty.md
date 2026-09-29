+++
schema = 1
id = "acme.bad.selector-alias-empty"
kind = "policy"
title = "Invalid policy selector-alias-empty"
status = "accepted"
owner = "arch"

[scope]
product = true

[selectors]
aliases = ["!!!"]

[[rules]]
id = "r"
level = "must"
text = "Do."
+++
