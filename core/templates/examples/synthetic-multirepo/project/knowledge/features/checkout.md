+++
schema = 2
id = "example.feature.checkout"
kind = "feature"
title = "Checkout: pay for the cart once"
status = "accepted"
owner = "architecture"
feature = "checkout"
summary = "The user confirms the cart and pays; retries never charge twice."

[scope]
features = ["checkout"]

[selectors]
aliases = ["checkout", "payment", "оплата", "оформление заказа"]

[links]
related = ["example.contract.payment-intent"]

[[behaviors]]
id = "single-charge"
text = "Retrying a failed or timed-out payment reuses the same payment intent."

[[boundaries]]
id = "no-split-payments"
text = "Paying one cart with several payment methods is not supported."
+++
