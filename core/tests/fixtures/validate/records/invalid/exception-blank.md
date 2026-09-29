+++
schema = 1
id = "acme.bad.exception-blank"
kind = "policy"
title = "Invalid policy exception-blank"
status = "accepted"
owner = "arch"

[scope]
product = true

[[rules]]
id = "r"
level = "must"
text = "Do."

[[rules.exceptions]]
id = "e"
text = ""
+++
