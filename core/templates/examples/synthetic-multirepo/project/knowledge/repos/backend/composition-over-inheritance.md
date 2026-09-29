+++
schema = 1
id = "example.backend.composition-over-inheritance"
kind = "policy"
title = "Domain services prefer composition over inheritance"
status = "accepted"
owner = "team-backend"

[scope]
modules = ["backend.domain"]

[selectors]
paths = ["src/domain/**"]
concepts = ["object-composition"]

[[rules]]
id = "compose-services"
level = "should"
text = "Build domain services by composing small collaborators instead of extending base classes."

[[rules.exceptions]]
id = "framework-base"
text = "Persistence entities may extend the base entity class required by the ORM."
+++
