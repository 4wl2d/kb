+++
schema = 1
id = "example.decision.token-in-preferences"
kind = "decision"
title = "Store refresh tokens in shared preferences (superseded)"
status = "superseded"
owner = "team-mobile"
context = "The first app release needed persistent sessions quickly."
decision = "Store the refresh token in shared preferences."
reasons = ["It was the simplest persistent storage available."]

[scope]
modules = ["mobile.auth"]
+++
Kept for history: the id stays addressable and is never reused. Superseded by
`example.decision.secure-token-storage`.
