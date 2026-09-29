+++
schema = 1
id = "acme.backend.sql-parameters"
kind = "policy"
title = "SQL parameters"
status = "accepted"
owner = "team-backend"

[scope]
modules = ["backend.api"]

[[rules]]
id = "parameterized"
level = "must"
text = "Use parameterized queries for every SQL statement."
+++
