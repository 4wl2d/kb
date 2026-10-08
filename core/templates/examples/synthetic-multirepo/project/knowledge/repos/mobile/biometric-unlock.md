+++
schema = 2
id = "example.mobile.biometric-unlock"
kind = "policy"
title = "Biometric unlock before showing stored sessions"
status = "draft"
owner = "team-mobile"

[scope]
modules = ["mobile.auth"]

[links]
related = ["example.mobile.token-storage"]

[[rules]]
id = "biometric-gate"
level = "should"
text = "Ask for biometric confirmation before restoring a session older than seven days."

[[anchors]]
kind = "change"
change = "!214"
note = "Synthetic proposal under discussion; drafts are never mandatory context."
+++
