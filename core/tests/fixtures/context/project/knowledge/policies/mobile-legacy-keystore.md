+++
schema = 1
id = "acme.mobile.legacy-keystore"
kind = "policy"
title = "Legacy keystore migration"
status = "accepted"
owner = "team-mobile"

[scope]
modules = ["mobile.auth"]

[applicability]
versions = { mobile = "<2.0.0" }

[[rules]]
id = "migrate-first"
level = "must"
text = "Migrate keys from the legacy keystore before the first token read."
+++
