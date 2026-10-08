+++
# Record template: reference (explanations and pointers to sources; never mandatory).
# PLACEHOLDER TEXT: replace every value.
schema = 2
id = "example.template.reference"
kind = "reference"
title = "Placeholder: topic of the reference"
status = "draft"
owner = "architecture"
summary = "Placeholder: what the reader learns here and when it is useful."
terms = [{ term = "Confirmed", meaning = "Placeholder: the precise product meaning agreed in review.", source = "docs/placeholder.md" }]

[scope]
features = ["login"]

[applicability]                             # optional: host code versions the record is valid for
versions = { mobile = ">=2.0.0" }

[[sources]]
title = "Placeholder: document title"
path = "docs/placeholder.md"                # host-relative path, or `url = "https://..."`
+++
## Details

Placeholder: explanatory Markdown shown by `kbw show <id> --section details`.
