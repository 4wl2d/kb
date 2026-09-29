+++
schema = 0
id = "legacy.procedure.manual-rotation"
type = "procedure"
title = "Manual signing key rotation"
state = "retired"
owner = "team-backend"
preconditions = ["Access to the key management console."]
expected = ["All new tokens are signed with the new key."]

[applies_to]
repos = ["backend"]

[[steps]]
id = "create-key"
text = "Create a new signing key."

[[steps]]
id = "switch"
text = "Switch the issuer to the new key."
+++
