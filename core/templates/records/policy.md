+++
# Record template: policy (rules and/or named settings). PLACEHOLDER TEXT: replace every value.
# Ids, owners, repos, modules, features and concepts below come from the synthetic example
# registry (namespace `example`); use your own namespace and registry ids.
schema = 2
id = "example.template.policy"              # `<namespace>.<segment>(.<segment>)*`, stable forever
kind = "policy"
title = "Placeholder: one-line policy title"
status = "draft"                            # draft -> accepted (by review) -> deprecated/superseded
owner = "team-mobile"                       # registry/owners.toml; must have authority over the scope

[scope]                                     # mandatory applicability: AND across dimensions, OR within
repos = ["mobile"]
modules = ["mobile.auth"]
# features = ["login"]
# product = true                            # product-wide instead of repos/modules/features

[selectors]                                 # relevance only; never excludes an applicable rule
paths = ["app/auth/**"]                     # repo-relative globs, optionally "repo:glob"
concepts = ["auth-token"]
intents = ["implement", "review"]
aliases = ["placeholder phrase", "фраза-заглушка"]

[links]
requires = ["example.template.contract"]    # mandatory dependencies (acyclic)
rationale = ["example.template.decision"]   # why; optional context
related = ["example.template.reference"]    # one-hop suggestions
# supersedes = ["<old id>"]                 # targets get status = "superseded"

[[rules]]
id = "placeholder-rule"                     # local id, unique in this record
level = "must"                              # must, must-not, should, should-not, may
text = "Placeholder: one atomic, checkable obligation."
conditions = ["placeholder: the condition under which the rule applies"]

[[rules.exceptions]]                        # exceptions are normative and never dropped
id = "placeholder-exception"
text = "Placeholder: the precise case in which the rule does not apply."

[[settings]]                                # a named, typed setting other policies may override
name = "placeholder-limit"
type = "integer"                            # integer, boolean, string, string-set
value = 3
override = "stricter"                       # forbidden (default), stricter, any
stricter = "lower"                          # lower/higher (integer), true/false, superset/subset
override_owners = ["team-mobile"]           # owners allowed to override (empty = any owner)
description = "Placeholder: what the setting controls."

# An override of another policy's setting (the overriding policy's scope must be within the
# target's scope, and the value may not be weaker when override = "stricter"):
# [[overrides]]
# target = "example.common.code-review#min-reviewers"
# value = 2
# reason = "Placeholder: why this scope needs a different value."

[[anchors]]                                 # provenance: source, test, change, doc
kind = "source"
repo = "mobile"
path = "app/auth/Placeholder.kt"
symbol = "Placeholder"
+++
Optional, non-normative explanation. Everything an agent has to obey belongs in the typed
fields above; this text is shown only on request.

## Background

Placeholder: history and context of the rule.
