+++
# Record template: contract between modules or repositories. PLACEHOLDER TEXT.
schema = 2
id = "example.template.contract"
kind = "contract"
title = "Placeholder: interface between two parties"
status = "draft"
owner = "architecture"
interface = "Placeholder: endpoint, event or schema name"
consumers = [{ repo = "mobile", path = "app/auth/Placeholder.kt", symbol = "Placeholder" }]
scenarios = [{ id = "missing-reply", given = "Placeholder: the provider has not replied.", expect = "Placeholder: the consumer does not infer success." }]

[scope]
modules = ["mobile.auth", "backend.api"]

[selectors]
paths = ["mobile:app/auth/**", "backend:src/api/**"]
concepts = ["auth-token"]

[links]
related = ["example.template.feature"]

[[parties]]                                 # at least two
id = "provider"
repo = "backend"
modules = ["backend.api"]
role = "Placeholder: what this party provides"

[[parties]]
id = "consumer"
repo = "mobile"
modules = ["mobile.auth"]
role = "Placeholder: what this party consumes"

[[obligations]]                             # at least one; `party` is a declared party id
id = "placeholder-provider-obligation"
party = "provider"
level = "must"
text = "Placeholder: what the provider guarantees."

[[obligations]]
id = "placeholder-consumer-obligation"
party = "consumer"
level = "must-not"
text = "Placeholder: what the consumer never does."
conditions = ["placeholder: when the obligation applies"]

[[obligations.exceptions]]
id = "placeholder-exception"
text = "Placeholder: the precise case in which the obligation does not apply."

[[anchors]]
kind = "doc"
repo = "shared-contracts"
path = "schemas/placeholder.yaml"
+++
