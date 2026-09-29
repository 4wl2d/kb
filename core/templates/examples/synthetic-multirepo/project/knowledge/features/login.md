+++
schema = 1
id = "example.feature.login"
kind = "feature"
title = "Login: sign-in and silent session refresh"
status = "accepted"
owner = "architecture"
feature = "login"
summary = "Users sign in once; the app keeps the session alive by refreshing tokens in the background."

[scope]
features = ["login"]

[selectors]
aliases = ["sign in", "login", "вход", "авторизац*"]

[links]
related = ["example.contract.token-refresh", "example.reference.auth-overview"]

[[behaviors]]
id = "silent-refresh"
text = "An expired access token is refreshed without user interaction while the refresh token is valid."

[[behaviors]]
id = "forced-logout"
text = "A rejected refresh token signs the user out and shows the sign-in screen."

[[boundaries]]
id = "no-password-storage"
text = "The app never stores the user's password; only tokens are kept."
+++
