+++
schema = 1
id = "acme.feature.login"
kind = "feature"
title = "Login"
status = "accepted"
owner = "arch"
feature = "login"
summary = "Users sign in with email and password."

[scope]
features = ["login"]

[[behaviors]]
id = "lockout"
text = "Five failed attempts lock the account for 15 minutes."

[[boundaries]]
id = "no-sso"
text = "Single sign-on is out of scope."
+++
