+++
schema = 1
id = "acme.mobile.token-storage"
kind = "policy"
title = "Token storage on mobile"
status = "accepted"
owner = "team-mobile"

[scope]
repos = ["mobile"]
modules = ["mobile.auth"]

[selectors]
paths = ["app/auth/**", "mobile:app/src/**/token/*.kt"]
concepts = ["auth-token"]
intents = ["implement", "debug"]
aliases = ["token refresh", "обновление токена"]

[links]
requires = ["acme.contract.token-api"]
rationale = ["acme.decision.keystore"]
related = ["acme.reference.oauth"]

[applicability]
versions = { mobile = ">=2.0.0, <3.0.0" }

[[anchors]]
kind = "source"
repo = "mobile"
path = "app/auth/TokenStore.kt"
symbol = "TokenStore"

[[anchors]]
kind = "test"
repo = "mobile"
path = "app/auth/TokenStoreTest.kt"

[[anchors]]
kind = "change"
change = "!42"
commit = "0a1b2c3d"

[[anchors]]
kind = "doc"
path = "docs/security.md"
note = "Security overview"

[[rules]]
id = "no-plaintext"
level = "must-not"
text = "Store refresh tokens in plaintext storage."
conditions = ["The build is a release build."]

[[rules.exceptions]]
id = "fake-server"
text = "Debug builds against the fake auth server."

[[settings]]
name = "max-token-age-days"
type = "integer"
value = 30
override = "stricter"
stricter = "lower"
override_owners = ["team-mobile"]
description = "Maximum refresh token age."

[[settings]]
name = "log-level"
type = "string"
value = "warn"
override = "any"
+++
Intro text explaining the policy.

## Background
Why this exists. Tokens should be kept out of logs.
