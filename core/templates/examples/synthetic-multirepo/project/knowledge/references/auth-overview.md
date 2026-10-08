+++
schema = 2
id = "example.reference.auth-overview"
kind = "reference"
title = "Authentication overview"
status = "accepted"
owner = "architecture"
summary = "How sign-in, token refresh and logout fit together across mobile and backend."

[scope]
features = ["login"]

[selectors]
concepts = ["auth-token"]

[[sources]]
title = "Authentication architecture notes (synthetic)"
path = "docs/auth/overview.md"

[[sources]]
title = "Refresh endpoint schema (synthetic)"
path = "schemas/auth/refresh.yaml"
+++
## Sequence

1. The app signs in and receives an access token and a refresh token.
2. Before the access token expires, the app refreshes it once (see the invariant on single
   refresh) and stores the rotated refresh token in the keystore.
