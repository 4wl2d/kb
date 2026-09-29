+++
schema = 1
id = "kb.invariant.read-only-knowledge-queries"
kind = "invariant"
title = "Knowledge queries never mutate the developer's checkouts"
status = "accepted"
owner = "maintainers"

[scope]
features = ["freshness"]

[selectors]
paths = ["core/cli/src/snapshot.rs", "core/cli/src/snapshot/**", "core/cli/src/overlay.rs", "core/cli/src/host.rs"]
concepts = ["freshness", "snapshot"]

[links]
rationale = ["kb.decision.isolated-mirror"]

[[statements]]
id = "no-worktree-mutation"
level = "must-not"
text = "`context`, `show`, `search`, `sync` and `impact` run pull, merge, rebase, reset, checkout, stash or submodule update in the KB checkout or the host repository."

[[statements]]
id = "mirror-only-refs"
level = "must"
text = "Fetched refs and objects are written only to the isolated mirror under the cache directory."
+++
