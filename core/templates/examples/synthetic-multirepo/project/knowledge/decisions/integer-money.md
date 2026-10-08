+++
schema = 2
id = "example.decision.integer-money"
kind = "decision"
title = "Monetary amounts are integer minor units"
status = "accepted"
owner = "architecture"
context = "Amounts travel between mobile, backend and the payment provider; rounding errors caused reconciliation mismatches."
decision = "Represent every amount as an integer number of minor units plus an ISO 4217 currency code."
reasons = [
  "Integer arithmetic is exact.",
  "The payment provider API already uses minor units.",
]
consequences = [
  "Display code converts minor units to localized strings.",
  "Currencies with three decimal places need a per-currency exponent table.",
]

[scope]
repos = ["mobile", "backend"]

[selectors]
aliases = ["money", "minor units", "денежные суммы"]

[[alternatives]]
option = "Decimal strings in JSON"
rejected_because = "Every client needs a decimal library and parsing rules drift between platforms."

[[alternatives]]
option = "Binary floating point"
rejected_because = "Cannot represent most decimal fractions exactly."
+++
