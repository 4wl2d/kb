+++
schema = 1
id = "acme.bad.contract-party"
kind = "contract"
title = "Contract with one party"
status = "accepted"
owner = "arch"

[scope]
repos = ["mobile"]

[[parties]]
id = "a"
repo = "mobile"
role = "Client"

[[obligations]]
id = "o"
party = "a"
level = "must"
text = "Call."
+++
