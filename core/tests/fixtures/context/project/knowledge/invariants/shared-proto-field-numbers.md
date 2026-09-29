+++
schema = 1
id = "acme.shared.proto-field-numbers"
kind = "invariant"
title = "Stable proto field numbers"
status = "accepted"
owner = "team-shared"

[scope]
repos = ["shared"]

[[statements]]
id = "no-renumber"
level = "must-not"
text = "Renumber or reuse field numbers in published proto messages."
+++
