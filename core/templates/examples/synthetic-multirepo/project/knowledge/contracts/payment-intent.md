+++
schema = 2
id = "example.contract.payment-intent"
kind = "contract"
title = "Idempotent payment intents between checkout and payments"
status = "accepted"
owner = "architecture"
interface = "POST /v1/payment-intents (synthetic)"

[scope]
modules = ["mobile.checkout", "backend.payments"]

[selectors]
concepts = ["idempotency"]
aliases = ["payment intent", "платежное намерение"]

[links]
requires = ["example.contract.error-envelope"]
rationale = ["example.decision.integer-money"]
related = ["example.feature.checkout"]

[[parties]]
id = "checkout-client"
repo = "mobile"
modules = ["mobile.checkout"]
role = "Creates payment intents"

[[parties]]
id = "payments-service"
repo = "backend"
modules = ["backend.payments"]
role = "Charges payment intents"

[[obligations]]
id = "send-idempotency-key"
party = "checkout-client"
level = "must"
text = "Send one idempotency key per checkout attempt and reuse it for every retry of that attempt."

[[obligations]]
id = "dedupe-by-key"
party = "payments-service"
level = "must"
text = "Return the original result for a repeated idempotency key instead of charging again."

[[obligations.exceptions]]
id = "expired-key"
text = "Keys older than 24 hours may be rejected with the `idempotency_key_expired` error code."

[[obligations]]
id = "minor-units"
party = "checkout-client"
level = "must"
text = "Send amounts as integer minor units with an ISO 4217 currency code."
+++
