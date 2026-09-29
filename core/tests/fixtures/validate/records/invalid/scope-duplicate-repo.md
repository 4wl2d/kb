+++
schema = 1
id = "acme.bad.scope-duplicate"
kind = "invariant"
title = "Repo listed twice"
status = "accepted"
owner = "arch"

[scope]
repos = ["mobile", "mobile"]

[[statements]]
id = "s"
level = "must"
text = "Hold."
+++
