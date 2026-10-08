+++
schema = 2
id = "legacy.policy.logging"
kind = "policy"
title = "Logging"
status = "accepted"
owner = "arch"

[scope]
product = true

[links]
supersedes = ["legacy.policy.logging-v1"]

[[rules]]
id = "rule-1"
level = "must-not"
text = "Write access or refresh tokens to logs."

[[rules]]
id = "rule-2"
level = "must"
text = "Redact the Authorization header in request logs."
+++
