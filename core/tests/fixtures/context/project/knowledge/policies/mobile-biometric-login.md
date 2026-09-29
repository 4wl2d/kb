+++
schema = 1
id = "acme.mobile.biometric-login"
kind = "policy"
title = "Biometric sign-in"
status = "draft"
owner = "team-mobile"

[scope]
modules = ["mobile.auth"]

[[rules]]
id = "fallback"
level = "must"
text = "Offer a password fallback whenever biometric sign-in is unavailable."
+++
