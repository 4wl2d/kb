+++
schema = 1
id = "kb.procedure.add-migration"
kind = "procedure"
title = "Add a document schema migration"
status = "accepted"
owner = "maintainers"
preconditions = ["The new document schema version and its changes are agreed in an ADR."]
expected = ["`kb migrate` converts fixtures of the previous version byte-for-byte into the expected output, and re-applying is a no-op."]

[scope]
modules = ["kb.update", "kb.model"]

[selectors]
intents = ["implement"]
aliases = ["migration", "schema version", "миграци*"]

[links]
requires = ["kb.contract.cli-protocol"]

[[steps]]
id = "bump-schema"
text = "Bump `document_schema` in core/release.toml and DOCUMENT_SCHEMA in core/cli/src/versions.rs; add the previous version to `migrates_from`."

[[steps]]
id = "register"
text = "Register the transform in core/cli/src/migrate.rs and add fixtures under core/migrations/fixtures/."

[[steps]]
id = "schemas"
text = "Regenerate JSON Schemas with `./kbw schema --write` and update templates and examples."
+++
