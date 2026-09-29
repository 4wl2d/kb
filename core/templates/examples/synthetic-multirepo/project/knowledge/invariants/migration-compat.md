+++
schema = 1
id = "example.backend.migration-compat"
kind = "invariant"
title = "Database migrations stay compatible with the previous release"
status = "accepted"
owner = "team-backend"

[scope]
modules = ["backend.domain"]

[selectors]
paths = ["src/domain/**", "db/migrations/**"]
aliases = ["database migration", "миграция базы"]

[[statements]]
id = "expand-contract"
level = "must"
text = "A migration keeps the schema readable and writable by the previous backend release."
conditions = ["the migration is deployed before the code that uses it"]

[[statements]]
id = "no-destructive-in-release"
level = "must-not"
text = "Drop columns or tables in the same release that stops using them."
+++
