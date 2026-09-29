+++
schema = 1
id = "acme.decision.domain-composition"
kind = "decision"
title = "Domain models prefer composition over inheritance"
status = "accepted"
owner = "team-backend"
context = "Deep inheritance trees made pricing rules hard to change."
decision = "Model domain objects by composing small value objects instead of subclassing."
reasons = ["Pricing rules change independently.", "Value objects are easy to test."]

[scope]
modules = ["backend.domain"]

[selectors]
concepts = ["object-composition"]
+++
