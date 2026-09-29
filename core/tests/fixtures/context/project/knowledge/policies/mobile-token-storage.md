+++
schema = 1
id = "acme.mobile.token-storage"
kind = "policy"
title = "Token storage on devices"
status = "accepted"
owner = "team-mobile"

[scope]
repos = ["mobile"]
modules = ["mobile.auth"]

[selectors]
paths = ["app/src/auth/storage/**"]
concepts = ["auth-token"]
aliases = ["keychain", "keystore"]

[links]
requires = ["acme.contract.token-refresh", "acme.backend.refresh-rotation"]
rationale = ["acme.decision.encrypted-storage"]
related = ["acme.procedure.debug-token-refresh"]

[[rules]]
id = "no-plaintext"
level = "must-not"
text = "Store refresh tokens in plaintext storage such as shared preferences."

[[rules.exceptions]]
id = "fake-server"
text = "Debug builds that talk to the local fake auth server may keep tokens in memory only."

[[rules]]
id = "clear-on-logout"
level = "must"
text = "Delete stored tokens when the user signs out."
conditions = ["The user signs out explicitly or the account is removed from the device."]

[[anchors]]
kind = "source"
repo = "mobile"
path = "app/src/auth/storage/TokenStore.kt"
symbol = "TokenStore"

[[anchors]]
kind = "test"
repo = "mobile"
path = "app/src/test/auth/TokenStoreTest.kt"
+++
Tokens are the most sensitive data kept on the device.

## Background

Early app versions kept tokens in shared preferences. The encrypted store wraps the
platform keystore:

```kotlin
val store = EncryptedTokenStore(keystore)
store.save(refreshToken)
```
