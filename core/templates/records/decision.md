+++
# Record template: decision. PLACEHOLDER TEXT: replace every value.
schema = 2
id = "example.template.decision"
kind = "decision"
title = "Placeholder: the decision in one line"
status = "draft"
owner = "architecture"
context = "Placeholder: the situation and forces that required a decision."
decision = "Placeholder: what was decided."
reasons = ["Placeholder: the main reason."]           # at least one
consequences = ["Placeholder: what follows from the decision, good and bad."]

[scope]
repos = ["mobile", "backend"]

[links]
# supersedes = ["<id of the decision this one replaces>"]
related = ["example.template.policy"]

[[alternatives]]
option = "Placeholder: an option that was considered"
rejected_because = "Placeholder: why it was not chosen."

[[anchors]]
kind = "change"
change = "!1"                               # reviewed change (MR number) or `commit = "<sha>"`
note = "Placeholder: the merge request where the decision was reviewed."
+++
