+++
schema = 1
id = "example.contract.error-envelope"
kind = "contract"
title = "Common error envelope for all public APIs"
status = "accepted"
owner = "architecture"
interface = "schemas/errors/envelope.json (synthetic)"

[scope]
# The modules of the parties below; other API contracts reach this one through
# `links.requires` even when a task is outside these modules.
modules = ["shared-contracts.schemas", "backend.api", "mobile.auth"]

[selectors]
paths = ["shared-contracts:schemas/errors/**"]
concepts = ["error-envelope"]

[[parties]]
id = "schema-owner"
repo = "shared-contracts"
modules = ["shared-contracts.schemas"]
role = "Defines the error schema"

[[parties]]
id = "server"
repo = "backend"
modules = ["backend.api"]
role = "Returns errors"

[[parties]]
id = "client"
repo = "mobile"
modules = ["mobile.auth"]
role = "Handles errors"

[[obligations]]
id = "envelope-shape"
party = "server"
level = "must"
text = "Return every error body as the envelope with a stable machine-readable code field."

[[obligations]]
id = "unknown-codes"
party = "client"
level = "must"
text = "Treat unknown error codes as a generic failure instead of crashing."

[[obligations]]
id = "additive-changes"
party = "schema-owner"
level = "must-not"
text = "Remove or rename envelope fields without a new schema version."
+++
## Why other contracts require it

Synthetic example. The schema is defined in `shared-contracts` and the contract is scoped to
the modules of its parties. The payment contract requires it through `links.requires`, so a
task in backend payment code receives it as a mandatory dependency even though that module
is outside this contract's own scope.
