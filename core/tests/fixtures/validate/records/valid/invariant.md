+++
schema = 1
id = "acme.backend.idempotent-refresh"
kind = "invariant"
title = "Token refresh is idempotent"
status = "accepted"
owner = "team-backend"

[scope]
repos = ["backend"]
modules = ["backend.api"]

[[statements]]
id = "same-result"
level = "must"
text = "Repeating a refresh call with the same token returns the same token pair within 10 seconds."

[[statements]]
id = "no-extra-rotation"
level = "should-not"
text = "Rotate twice for concurrent duplicate requests."
+++
