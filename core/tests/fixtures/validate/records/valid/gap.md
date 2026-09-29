+++
schema = 1
id = "acme.gap.refresh-timeout"
kind = "gap"
title = "Refresh timeout is unspecified"
status = "accepted"
owner = "arch"
gap = "missing"
description = "No agreed client timeout for the refresh call."
affects = ["acme.contract.token-api"]
questions = ["What timeout should the mobile client use?"]

[scope]
repos = ["mobile", "backend"]
+++
