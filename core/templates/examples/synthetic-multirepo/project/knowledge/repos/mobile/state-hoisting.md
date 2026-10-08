+++
schema = 2
id = "example.mobile.state-hoisting"
kind = "policy"
title = "Screens are composed of stateless components"
status = "accepted"
owner = "team-mobile"

[scope]
modules = ["mobile.ui"]

[selectors]
paths = ["app/ui/**"]
concepts = ["ui-composition"]
aliases = ["state hoisting", "подъем состояния"]

[[rules]]
id = "stateless-components"
level = "should"
text = "Keep reusable UI components stateless and pass state and callbacks from the screen."

[[rules.exceptions]]
id = "animation-state"
text = "Purely visual animation state may stay inside a component."

[[rules]]
id = "no-network-in-ui"
level = "must-not"
text = "Call network or storage APIs from UI components."
+++
