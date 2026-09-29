+++
schema = 1
id = "acme.decision.compose-ui"
kind = "decision"
title = "Screens are built from small UI components"
status = "accepted"
owner = "team-mobile"
context = "Screens duplicated layout code."
decision = "Build every screen from small, stateless composable components."
reasons = ["Components are reused across screens.", "Stateless components are easy to preview."]

[scope]
modules = ["mobile.ui"]

[selectors]
concepts = ["ui-composition"]
+++
