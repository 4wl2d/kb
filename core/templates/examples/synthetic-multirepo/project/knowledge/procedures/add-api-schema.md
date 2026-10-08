+++
schema = 2
id = "example.procedure.add-api-schema"
kind = "procedure"
title = "Add or change an API schema"
status = "accepted"
owner = "team-platform"
preconditions = ["The owning teams of every party agreed on the change."]
expected = ["Generated clients in mobile and backend compile against the new schema version."]

[scope]
repos = ["shared-contracts"]

[links]
requires = ["example.contract.error-envelope"]

[[steps]]
id = "edit-schema"
text = "Edit the schema under schemas/ and bump its version when a field is removed or renamed."

[[steps]]
id = "regenerate-clients"
text = "Regenerate the client code in mobile and backend from the new schema."
+++
