+++
schema = 1
id = "acme.gap.offline-catalog"
kind = "gap"
title = "Offline catalog behavior is unspecified"
status = "accepted"
owner = "team-mobile"
gap = "missing"
description = "Nobody has decided what the catalog screen shows without a network connection."
affects = ["acme.feature.catalog"]
questions = ["Should the last loaded catalog be cached?", "How stale may prices be?"]

[scope]
features = ["catalog"]
+++
