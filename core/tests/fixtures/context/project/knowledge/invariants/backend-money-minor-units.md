+++
schema = 1
id = "acme.backend.money-minor-units"
kind = "invariant"
title = "Money in minor units"
status = "accepted"
owner = "team-backend"

[scope]
repos = ["backend"]

[selectors]
concepts = ["money"]

[[statements]]
id = "minor-units"
level = "must"
text = "Money amounts are stored and transmitted as integer minor units with an ISO 4217 currency code."
+++
