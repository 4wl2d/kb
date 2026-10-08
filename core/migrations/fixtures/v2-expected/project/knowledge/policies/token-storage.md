+++
schema = 2
id = "legacy.mobile.token-storage"
kind = "policy"
title = "Token storage on mobile"
status = "accepted"
owner = "team-mobile"

[scope]
repos = ["mobile"]
modules = []
features = []

[selectors]
# Query phrases (Russian included).
aliases = ["token refresh", "обновление токена"]

[links]
requires = ["legacy.contract.token-api"]

[[rules]]
id = "rule-1"
level = "must-not"
text = "Store refresh tokens in plaintext storage."

[[rules]]
# Keychain on iOS, Keystore on Android.
id = "rule-2"
level = "must"
text = "Keep refresh tokens in the platform secure storage." # reviewed
+++
Tokens are credentials; see the contract for the refresh protocol.

## Background

The body is optional explanation and is never rewritten by migrations:

```toml
schema = 0
state = "active"
```
