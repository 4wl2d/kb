+++
schema = 1
id = "kb.decision.single-upstream-many-downstreams"
kind = "decision"
title = "One upstream, one downstream fork per product"
status = "accepted"
owner = "maintainers"
context = "Teams need the engine and their knowledge versioned together, mounted in several host repositories, without managing a separately installed CLI."
decision = "Ship a single upstream repository; each product keeps one downstream fork that contains the engine and project/ knowledge and is mounted as a Git submodule (or separate checkout)."
reasons = [
  "Engine, schemas, migrations and knowledge move together through one reviewed update.",
  "Host repositories pin a single KB revision through the submodule pointer.",
]
consequences = [
  "Engine changes go upstream; downstream engine drift is detected by `kb update divergence`.",
  "Routine origin syncs and upstream upgrades are different operations.",
]

[scope]
product = true

[selectors]
aliases = ["fork", "downstream", "upstream", "submodule"]

[[alternatives]]
option = "Globally installed CLI with separately versioned knowledge packages"
rejected_because = "Users would have to reconcile engine and knowledge versions by hand."

[[alternatives]]
option = "Hosted service"
rejected_because = "Out of scope: no daemon, web UI or cloud dependency."
+++
