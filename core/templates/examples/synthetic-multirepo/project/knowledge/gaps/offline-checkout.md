+++
schema = 1
id = "example.gap.offline-checkout"
kind = "gap"
title = "Checkout behavior when the device goes offline is undefined"
status = "accepted"
owner = "architecture"
gap = "missing"
description = "Nobody has specified what the app shows or retries when connectivity is lost after the payment request was sent but before the response arrived."
affects = ["example.feature.checkout", "example.contract.payment-intent"]
questions = [
  "Should the app poll the payment intent status after reconnecting?",
  "How long may the spinner stay on screen before the user can leave?",
]

[scope]
features = ["checkout"]
+++
