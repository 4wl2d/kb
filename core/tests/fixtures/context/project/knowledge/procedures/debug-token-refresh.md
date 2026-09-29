+++
schema = 1
id = "acme.procedure.debug-token-refresh"
kind = "procedure"
title = "Debug token refresh loops"
status = "accepted"
owner = "team-mobile"
preconditions = ["A debug build with network logging enabled."]
expected = ["Exactly one refresh call per expired access token."]

[scope]
repos = ["mobile"]

[selectors]
concepts = ["auth-token"]
intents = ["debug"]
aliases = ["401 loop", "бесконечный 401"]

[links]
related = ["acme.reference.oauth"]

[[steps]]
id = "capture"
text = "Capture the network log while reproducing the loop."

[[steps]]
id = "compare"
text = "Compare refresh token values between consecutive refresh calls."

[[steps]]
id = "check-rotation"
text = "Confirm the app stores the rotated refresh token returned by the server."
+++
