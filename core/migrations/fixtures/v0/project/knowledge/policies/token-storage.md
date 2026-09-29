+++
schema = 0
id = "legacy.mobile.token-storage"
type = "policy"
title = "Token storage on mobile"
state = "active"
owner = "team-mobile"
# Query phrases (Russian included).
tags = ["token refresh", "обновление токена"]
depends_on = ["legacy.contract.token-api"]
see_also = []

[applies_to]
repos = ["mobile"]
modules = []
features = []

[[rule]]
must_not = "Store refresh tokens in plaintext storage."

[[rule]]
# Keychain on iOS, Keystore on Android.
must = "Keep refresh tokens in the platform secure storage." # reviewed
+++
Tokens are credentials; see the contract for the refresh protocol.

## Background

The body is optional explanation and is never rewritten by migrations:

```toml
schema = 0
state = "active"
```
