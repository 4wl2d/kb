+++
schema = 1
id = "kb.contract.cli-protocol"
kind = "contract"
title = "CLI protocol between the engine and generated skills"
status = "accepted"
owner = "maintainers"

[scope]
modules = ["kb.cli", "kb.integrate"]

[selectors]
aliases = ["protocol", "envelope", "skill protocol"]

[[parties]]
id = "engine"
repo = "kb"
modules = ["kb.cli"]
role = "Produces kb.cli.v1 envelopes and stable error codes"

[[parties]]
id = "skill"
repo = "kb"
modules = ["kb.integrate"]
role = "Instructs agents how to call the launcher and interpret results"

[[obligations]]
id = "bump-protocol"
party = "engine"
level = "must"
text = "Increase `protocol` in core/release.toml when an envelope field is removed or changes meaning."

[[obligations]]
id = "bump-skill-protocol"
party = "skill"
level = "must"
text = "Increase `skill_protocol` when generated skill instructions change the calls or the interpretation agents rely on."

[[obligations.exceptions]]
id = "wording-only"
text = "Wording changes that keep the same calls and interpretation do not require a bump."
+++
