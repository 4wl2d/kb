+++
schema = 1
id = "acme.backend.rotate-signing-key"
kind = "procedure"
title = "Rotate the token signing key"
status = "draft"
owner = "team-backend"
preconditions = ["A new key pair exists in the secret store."]
expected = ["Old tokens stay valid until expiry.", "New tokens use the new key id."]

[scope]
repos = ["backend"]

[[steps]]
id = "publish"
text = "Publish the new public key in the JWKS document."

[[steps]]
id = "switch"
text = "Switch signing to the new key id."
+++
