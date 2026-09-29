+++
# Record template: gap (missing knowledge, ambiguity or contradiction). An accepted gap is
# delivered as mandatory context so that agents ask instead of guessing.
# PLACEHOLDER TEXT: replace every value.
schema = 1
id = "example.template.gap"
kind = "gap"
title = "Placeholder: what is unknown"
status = "draft"
owner = "architecture"
gap = "missing"                             # missing, ambiguity, contradiction
description = "Placeholder: what is not specified and why it matters."
affects = ["example.template.feature"]      # records whose meaning depends on the answer
questions = ["Placeholder: the question an owner has to answer."]

[scope]
features = ["login"]
+++
