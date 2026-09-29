+++
schema = 1
id = "acme.bad.applicability-bad-semver"
kind = "policy"
title = "Invalid policy applicability-bad-semver"
status = "accepted"
owner = "arch"

[scope]
product = true

[applicability]
versions = { mobile = "two point oh" }

[[rules]]
id = "r"
level = "must"
text = "Do."
+++
