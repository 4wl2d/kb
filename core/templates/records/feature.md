+++
# Record template: feature. PLACEHOLDER TEXT: replace every value.
schema = 2
id = "example.template.feature"
kind = "feature"
title = "Placeholder: feature name and purpose"
status = "draft"
owner = "architecture"
feature = "login"                           # registry/features.toml id this record describes
summary = "Placeholder: what the feature does for the user, in one or two sentences."
clocks = ["Placeholder: authoritative clock and treatment of skew."]
data_sources = ["Placeholder: authoritative data source and recovery source."]

[scope]
features = ["login"]

[selectors]
aliases = ["placeholder feature phrase"]

[links]
related = ["example.template.contract", "example.template.reference"]

[[behaviors]]                               # at least one; each is atomic
id = "placeholder-behavior"
text = "Placeholder: an observable behavior of the feature."

[[boundaries]]                              # what the feature deliberately does not do
id = "placeholder-boundary"
text = "Placeholder: an explicit non-goal or limit."

[[states]]
id = "idle"
text = "Placeholder: no operation is pending."

[[states]]
id = "pending"
text = "Placeholder: work has started but is not yet confirmed."

[[transitions]]
id = "start"
from = "idle"
to = "pending"
when = "Placeholder: the user starts the operation."

[[scenarios]]
id = "pending-is-not-confirmed"
given = "Placeholder: the operation was sent but no confirmation was received."
expect = "Placeholder: show pending and retain the recovery information."
+++
