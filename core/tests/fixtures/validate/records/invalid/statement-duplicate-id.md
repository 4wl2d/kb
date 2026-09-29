+++
schema = 1
id = "acme.bad.statement-duplicate-id"
kind = "policy"
title = "Invalid policy statement-duplicate-id"
status = "accepted"
owner = "arch"

[scope]
product = true

[[rules]]
id = "r"
level = "must"
text = "Do."

[[rules]]
id = "r"
level = "should"
text = "Also do."
+++
