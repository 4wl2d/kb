+++
schema = 1
id = "kb.policy.engine-changes-upstream"
kind = "policy"
title = "Engine changes flow through upstream"
status = "accepted"
owner = "maintainers"

[scope]
product = true

[selectors]
intents = ["implement", "refactor", "review"]
aliases = ["engine change", "downstream patch", "fork"]

[links]
rationale = ["kb.decision.single-upstream-many-downstreams"]

[[rules]]
id = "upstream-first"
level = "must"
text = "Change engine-owned paths (listed in core/release.toml `engine_paths`) in the upstream repository and deliver them to downstreams through `kb update prepare`."

[[rules.exceptions]]
id = "declared-patch"
text = "A downstream may carry an intentional engine patch when it is declared in project/upstream.toml `engine_patches` with a reason."

[[rules]]
id = "no-project-data-upstream"
level = "must-not"
text = "Commit real third-party project data to the upstream repository; examples must be synthetic and labeled as such."
+++
## Background

Downstreams merge upstream history. Undeclared engine edits in a downstream make every
later update a manual conflict and are reported by `kb update divergence`.
