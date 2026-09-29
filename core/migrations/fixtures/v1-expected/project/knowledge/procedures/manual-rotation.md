+++
schema = 1
id = "legacy.procedure.manual-rotation"
kind = "procedure"
title = "Manual signing key rotation"
status = "deprecated"
owner = "team-backend"
preconditions = ["Access to the key management console."]
expected = ["All new tokens are signed with the new key."]

[scope]
repos = ["backend"]

[[steps]]
id = "create-key"
text = "Create a new signing key."

[[steps]]
id = "switch"
text = "Switch the issuer to the new key."
+++
