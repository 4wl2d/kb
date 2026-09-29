+++
schema = 1
id = "acme.contract.checkout-api"
kind = "contract"
title = "Checkout payment API"
status = "accepted"
owner = "arch"

[scope]
features = ["checkout"]

[selectors]
concepts = ["money"]

[links]
related = ["acme.backend.money-minor-units"]

[[parties]]
id = "app"
repo = "mobile"
modules = ["mobile.payments"]
role = "Starts checkout and shows the result"

[[parties]]
id = "billing"
repo = "backend"
modules = ["backend.billing"]
role = "Charges the customer"

[[obligations]]
id = "idempotency"
party = "app"
level = "must"
text = "Send an Idempotency-Key header with every payment request."

[[obligations]]
id = "minor-units"
party = "billing"
level = "must"
text = "Accept amounts only as integer minor units."
+++
