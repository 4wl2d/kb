+++
schema = 1
id = "example.procedure.rotate-signing-keys"
kind = "procedure"
title = "Rotate token signing keys"
status = "accepted"
owner = "team-backend"
preconditions = ["The new key pair was generated in the key management service."]
expected = ["Tokens signed with either key validate during the overlap window.", "The old key is retired after the overlap window."]

[scope]
modules = ["backend.api"]

[selectors]
intents = ["implement", "debug"]
aliases = ["key rotation", "ротация ключей"]

[links]
related = ["example.contract.token-refresh"]

[[steps]]
id = "publish-new-key"
text = "Publish the new public key next to the current one."

[[steps]]
id = "sign-with-new-key"
text = "Switch token signing to the new private key."

[[steps]]
id = "retire-old-key"
text = "After the longest token lifetime has passed, remove the old public key."
+++
