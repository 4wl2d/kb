+++
schema = 1
id = "acme.procedure.rotate-signing-key"
kind = "procedure"
title = "Rotate the token signing key"
status = "accepted"
owner = "team-backend"
expected = ["Tokens signed with the old key stay valid until they expire."]

[scope]
repos = ["backend"]

[selectors]
concepts = ["auth-token"]
intents = ["implement", "debug"]

[[steps]]
id = "publish"
text = "Publish the new public key next to the old one."

[[steps]]
id = "switch"
text = "Switch signing to the new key after one day."
+++
