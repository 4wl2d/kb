+++
schema = 1
id = "kb.invariant.deterministic-context"
kind = "invariant"
title = "Deterministic context assembly"
status = "accepted"
owner = "maintainers"

[scope]
modules = ["kb.context", "kb.index"]

[selectors]
concepts = ["determinism"]

[[statements]]
id = "same-inputs-same-output"
level = "must"
text = "Identical logical requests against the same snapshot produce byte-identical `result` objects and receipts."

[[statements]]
id = "id-tie-break"
level = "must"
text = "Break ranking ties by stable record id in ascending order."
+++
