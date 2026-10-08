+++
# Record template: invariant. PLACEHOLDER TEXT: replace every value.
schema = 2
id = "example.template.invariant"
kind = "invariant"
title = "Placeholder: condition every change preserves"
status = "draft"
owner = "team-mobile"

[scope]
modules = ["mobile.auth"]

[selectors]
concepts = ["auth-token"]

[links]
related = ["example.template.policy"]

[[statements]]                              # at least one
id = "placeholder-statement"
level = "must"
text = "Placeholder: the preserved condition, stated so a reviewer can check it."
conditions = ["placeholder: when the statement applies"]

[[statements.exceptions]]
id = "placeholder-exception"
text = "Placeholder: the precise case in which the statement does not hold."

[[anchors]]
kind = "test"                               # a test anchor does not prove the test runs
repo = "mobile"
path = "app/auth/PlaceholderTest.kt"
+++
