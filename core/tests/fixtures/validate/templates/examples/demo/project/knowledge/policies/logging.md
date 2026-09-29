+++
schema = 1
id = "demo.app.logging"
kind = "policy"
title = "No secrets in logs"
status = "accepted"
owner = "arch"

[scope]
repos = ["app"]

[links]
rationale = ["demo.decision.structured-logs"]

[[rules]]
id = "no-secrets"
level = "must-not"
text = "Write tokens or passwords to logs."
+++
