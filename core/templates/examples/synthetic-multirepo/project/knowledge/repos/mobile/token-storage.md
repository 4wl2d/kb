+++
schema = 1
id = "example.mobile.token-storage"
kind = "policy"
title = "Refresh tokens live only in the platform keystore"
status = "accepted"
owner = "team-mobile"

[scope]
modules = ["mobile.auth"]

[selectors]
paths = ["app/auth/**"]
concepts = ["auth-token"]
intents = ["implement", "debug", "review"]
aliases = ["token storage", "хранение токена"]

[links]
requires = ["example.contract.token-refresh"]
rationale = ["example.decision.secure-token-storage"]
related = ["example.reference.auth-overview"]

[[rules]]
id = "keystore-only"
level = "must"
text = "Store refresh tokens only in the platform keystore."

[[rules]]
id = "no-plaintext"
level = "must-not"
text = "Write refresh tokens to logs, preferences, files or crash reports."

[[rules.exceptions]]
id = "local-fake-server"
text = "Debug builds that talk to the local fake auth server may log the fake token prefix."

[[anchors]]
kind = "source"
repo = "mobile"
path = "app/auth/TokenStore.kt"
symbol = "TokenStore"

[[anchors]]
kind = "test"
repo = "mobile"
path = "app/auth/TokenStoreTest.kt"
note = "A test anchor does not prove the test runs in CI."
+++
## Background

Synthetic example. Tokens leaked through logs were the motivating incident; see the linked
decision for the alternatives that were considered.
