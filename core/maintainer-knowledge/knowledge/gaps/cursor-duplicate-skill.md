+++
schema = 1
id = "kb.gap.cursor-duplicate-skill"
kind = "gap"
title = "Real harness loading after duplicate-skill removal is unverified"
status = "accepted"
owner = "maintainers"
gap = "ambiguity"
description = "Skill protocol 2 installs one full skill and native pointers, with local migration/conflict tests. Installation and an integrate --probe result do not establish that a fresh Cursor or other harness session actually loads the workflow, invokes context, and avoids duplicate discovery on an upgraded host."
questions = ["Does each enabled pinned harness load exactly one full kb skill and perform the requested context call after a real host upgrade?"]

[scope]
modules = ["kb.integrate"]

[selectors]
aliases = ["cursor", "duplicate skill"]

[links]
related = ["kb.reference.harness-skill-formats"]
+++
