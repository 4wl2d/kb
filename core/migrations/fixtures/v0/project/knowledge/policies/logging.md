+++
schema = 0
id = "legacy.policy.logging"
type = "policy"
title = "Logging"
state = "active"
owner = "arch"
replaces = ["legacy.policy.logging-v1"]

[[rule]]
must_not = "Write access or refresh tokens to logs."

[[rule]]
must = "Redact the Authorization header in request logs."
+++
