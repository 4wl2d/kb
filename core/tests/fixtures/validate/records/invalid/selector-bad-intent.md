+++
schema = 1
id = "acme.bad.selector-bad-intent"
kind = "policy"
title = "Invalid policy selector-bad-intent"
status = "accepted"
owner = "arch"

[scope]
product = true

[selectors]
intents = ["deploy"]

[[rules]]
id = "r"
level = "must"
text = "Do."
+++
