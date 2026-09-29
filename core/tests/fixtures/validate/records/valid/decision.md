+++
schema = 1
id = "acme.decision.keystore"
kind = "decision"
title = "Use the platform keystore for tokens"
status = "accepted"
owner = "team-mobile"
context = "Refresh tokens must survive app restarts without being readable by other apps."
decision = "Store tokens in the Android Keystore / iOS Keychain."
reasons = ["Hardware-backed keys where available.", "No custom crypto."]
consequences = ["Tokens are lost on device restore."]

[scope]
repos = ["mobile"]

[[alternatives]]
option = "Encrypted shared preferences"
rejected_because = "Key management is still app-controlled."
+++
## Notes
Revisit when the minimum OS version changes.
