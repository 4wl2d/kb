+++
schema = 1
id = "kb.decision.isolated-mirror"
kind = "decision"
title = "Fetch approved refs into an isolated bare mirror"
status = "accepted"
owner = "maintainers"
context = "Every knowledge query must verify the approved ref against the remote without disturbing the developer's checkout."
decision = "Fetch the approved ref into a bare mirror under the cache directory and read snapshot content directly from Git objects."
reasons = [
  "The KB work tree, index, HEAD, branches and submodule pointers stay untouched.",
  "Snapshots are immutable and addressable by commit id.",
]
consequences = ["Each KB checkout keeps its own mirror; caches are isolated per project and per checkout."]

[scope]
features = ["freshness"]

[selectors]
concepts = ["freshness", "snapshot"]
+++
