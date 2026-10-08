+++
schema = 2
id = "example.common.code-review"
kind = "policy"
title = "Code review and knowledge impact for every change"
status = "accepted"
owner = "architecture"

[scope]
product = true

[selectors]
intents = ["implement", "refactor", "review"]
aliases = ["code review", "ревью кода"]

[[rules]]
id = "impact-statement"
level = "must"
text = "Every merge request states its knowledge-base impact in a kb-impact block."

[[rules]]
id = "owner-review"
level = "must"
text = "A change to a contract is reviewed by an owner of every party repository."

[[rules.exceptions]]
id = "typo-only"
text = "Typo fixes in contract prose that do not change any obligation need one party owner."

[[settings]]
name = "min-reviewers"
type = "integer"
value = 1
override = "stricter"
stricter = "higher"
override_owners = ["team-backend", "team-mobile"]
description = "Minimum number of approving reviewers per merge request."

[[settings]]
name = "require-green-ci"
type = "boolean"
value = true
description = "Merge only after the CI pipeline succeeded. Not overridable."
+++
## Why

Synthetic example: review is the only way knowledge becomes accepted, so the review policy
applies to every repository of the product.
