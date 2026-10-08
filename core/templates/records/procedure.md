+++
# Record template: procedure. Procedures are data: kb shows them and never executes them.
# PLACEHOLDER TEXT: replace every value.
schema = 2
id = "example.template.procedure"
kind = "procedure"
title = "Placeholder: checklist for a migration change"
status = "draft"
owner = "team-backend"
preconditions = ["Placeholder: what has to be true before starting."]
expected = ["Placeholder: the observable result when the procedure succeeded."]   # at least one

[scope]
modules = ["backend.api"]
change_types = ["migration"]

[selectors]
intents = ["implement", "debug"]

[[steps]]                                   # at least one, in order
id = "placeholder-first-step"
text = "Placeholder: the first step."

[[steps]]
id = "placeholder-second-step"
text = "Placeholder: the second step."
+++
