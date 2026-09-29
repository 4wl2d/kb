+++
schema = 1
id = "legacy.feature.login"
kind = "feature"
title = "Login on mobile"
status = "draft"
owner = "team-mobile"
feature = "login"
summary = "Password login that obtains an access and a refresh token."

[scope]
repos = ["mobile"]
modules = ["mobile.auth"]
features = ["login"]

[selectors]
aliases = ["sign in"]

[links]
requires = ["legacy.mobile.token-storage"]

[[behaviors]]
id = "store-token"
text = "After a successful login the refresh token is stored in secure storage."
+++
