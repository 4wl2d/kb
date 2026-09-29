+++
schema = 1
id = "acme.product.logging"
kind = "policy"
title = "Logging hygiene"
status = "accepted"
owner = "arch"

[scope]
product = true

[selectors]
aliases = ["logging", "логирование"]

[[rules]]
id = "no-secrets"
level = "must-not"
text = "Write secrets, access tokens or personal data to logs."

[[anchors]]
kind = "doc"
path = "docs/logging.md"
+++
