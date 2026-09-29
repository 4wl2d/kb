+++
# Record template: feature. PLACEHOLDER TEXT: replace every value.
schema = 1
id = "example.template.feature"
kind = "feature"
title = "Placeholder: feature name and purpose"
status = "draft"
owner = "architecture"
feature = "login"                           # registry/features.toml id this record describes
summary = "Placeholder: what the feature does for the user, in one or two sentences."

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
+++
