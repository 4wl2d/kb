+++
schema = 1
id = "acme.mobile.ui-accessibility"
kind = "invariant"
title = "Accessible controls"
status = "accepted"
owner = "team-mobile"

[scope]
modules = ["mobile.ui"]

[[statements]]
id = "labels"
level = "must"
text = "Every interactive control exposes an accessibility label."

[[statements]]
id = "touch-target"
level = "should"
text = "Touch targets are at least 48 by 48 density-independent pixels."
+++
