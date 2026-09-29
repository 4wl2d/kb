# Repository-specific knowledge

Knowledge that belongs to one repository or module. Use one subdirectory per repository if the group grows (for example `repos/mobile/`).

Typical kinds: policy, invariant, procedure, reference.

Every `*.md` file in this directory except `README.md` is a typed record: a `+++` line,
strict TOML front matter, a closing `+++` line, then optional non-normative Markdown
sections. Groups are only for people: kb routes by each record's `scope` and `selectors`,
never by directory. Start from the templates in `core/templates/records/`, and propose new
knowledge with `status = "draft"` plus evidence (`anchors`) until it is reviewed.
