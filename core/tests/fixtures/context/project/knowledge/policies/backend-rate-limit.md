+++
schema = 1
id = "acme.backend.rate-limit"
kind = "policy"
title = "API rate limiting"
status = "accepted"
owner = "team-backend"

[scope]
repos = ["backend"]

[selectors]
concepts = ["rate-limit"]

[[rules]]
id = "retry-after"
level = "must"
text = "Return HTTP 429 with a Retry-After header when a client exceeds its quota."
+++
