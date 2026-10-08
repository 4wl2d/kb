+++
schema = 2
id = "example.decision.secure-token-storage"
kind = "decision"
title = "Keep refresh tokens in the platform keystore"
status = "accepted"
owner = "team-mobile"
context = "Refresh tokens were stored in shared preferences and leaked through device backups."
decision = "Store refresh tokens in the platform keystore and exclude them from backups."
reasons = ["The keystore is hardware-backed on supported devices.", "Backups no longer contain tokens."]
consequences = ["Users re-authenticate after restoring a backup on a new device."]

[scope]
modules = ["mobile.auth"]

[links]
supersedes = ["example.decision.token-in-preferences"]

[[alternatives]]
option = "Encrypt tokens in shared preferences with an app-level key"
rejected_because = "The key would ship inside the app binary."

[[anchors]]
kind = "change"
change = "!87"
note = "Synthetic merge request that introduced the keystore."
+++
