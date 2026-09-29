+++
schema = 1
id = "acme.feature.login"
kind = "feature"
title = "Sign-in"
status = "accepted"
owner = "arch"
feature = "login"
summary = "Users sign in with email and password and stay signed in through refresh tokens."

[scope]
features = ["login"]

[[behaviors]]
id = "remember"
text = "A successful sign-in keeps the user signed in until they sign out."

[[behaviors]]
id = "lockout"
text = "Five failed attempts lock the account for 15 minutes."

[[boundaries]]
id = "no-social"
text = "Social sign-in is out of scope."
+++
