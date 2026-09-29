+++
schema = 1
id = "acme.bad.contract-undeclared"
kind = "contract"
title = "Obligation for an undeclared party"
status = "accepted"
owner = "arch"

[scope]
repos = ["mobile", "backend"]

[[parties]]
id = "a"
repo = "mobile"
role = "Client"

[[parties]]
id = "b"
repo = "backend"
role = "Server"

[[obligations]]
id = "o"
party = "c"
level = "must"
text = "Call."
+++
