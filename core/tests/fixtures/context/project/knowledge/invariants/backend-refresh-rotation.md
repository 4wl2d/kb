+++
schema = 1
id = "acme.backend.refresh-rotation"
kind = "invariant"
title = "Single-use refresh tokens"
status = "accepted"
owner = "team-backend"

[scope]
repos = ["backend"]

[[statements]]
id = "single-use"
level = "must"
text = "A refresh token is accepted exactly once; replaying it revokes the whole token family."
+++
