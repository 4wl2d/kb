+++
schema = 1
id = "kb.invariant.mandatory-completeness"
kind = "invariant"
title = "Mandatory knowledge is never silently dropped"
status = "accepted"
owner = "maintainers"

[scope]
features = ["context-assembly"]

[selectors]
concepts = ["budget", "determinism"]
aliases = ["completeness", "mandatory", "обязательн*"]

[[statements]]
id = "no-silent-drop"
level = "must-not"
text = "Omit an applicable obligation or a required dependency while reporting completeness `complete`."

[[statements]]
id = "budget-error"
level = "must"
text = "Fail with CONTEXT_BUDGET_EXCEEDED and the required amount when the header and mandatory units do not fit the budget."

[[statements]]
id = "whole-units"
level = "must-not"
text = "Truncate normative statements, conditions, exceptions or code blocks to fit a budget."
+++
