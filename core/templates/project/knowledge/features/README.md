# Features

Behavior and boundaries of registry features (`registry/features.toml`): what the feature does, what it deliberately does not do, and related knowledge.

Typical kinds: feature, reference.

Every `*.md` file in this directory except `README.md` is a typed record: a `+++` line,
strict TOML front matter, a closing `+++` line, then optional non-normative Markdown
sections. Groups are only for people: kb routes by each record's `scope` and `selectors`,
never by directory. Start from the templates in `core/templates/records/`, and propose new
knowledge with `status = "draft"` plus evidence (`anchors`) until it is reviewed.
