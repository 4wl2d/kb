+++
schema = 0
id = "legacy.feature.login"
type = "feature"
title = "Login on mobile"
state = "proposed"
owner = "team-mobile"
tags = ["sign in"]
depends_on = ["legacy.mobile.token-storage"]
feature = "login"
summary = "Password login that obtains an access and a refresh token."

[applies_to]
repos = ["mobile"]
modules = ["mobile.auth"]
features = ["login"]

[[behaviors]]
id = "store-token"
text = "After a successful login the refresh token is stored in secure storage."
+++
