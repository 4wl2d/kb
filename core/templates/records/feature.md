+++
# Record template: feature. PLACEHOLDER TEXT: replace every value. One compact document per
# feature: at most 60 lines (FEATURE_TOO_LONG); rules go into separate invariant records.
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

[[anchors]]                                 # entry points: where the feature starts in code
kind = "source"
repo = "mobile"
path = "app/placeholder/Entry.kt"
symbol = "Placeholder.start"
note = "Placeholder: what starts here."

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
