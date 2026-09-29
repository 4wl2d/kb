+++
schema = 1
id = "acme.decision.encrypted-storage"
kind = "decision"
title = "Encrypted token store on the device"
status = "accepted"
owner = "team-mobile"
context = "Tokens were kept in shared preferences in early versions."
decision = "Keep tokens in an encrypted store backed by the platform keystore."
reasons = ["Rooted devices can read shared preferences.", "The platform keystore is hardware-backed on supported devices."]
consequences = ["Tokens are lost when the user clears app data."]

[scope]
repos = ["mobile"]

[[alternatives]]
option = "Server-side sessions only"
rejected_because = "Offline start would need a network round trip."
+++
