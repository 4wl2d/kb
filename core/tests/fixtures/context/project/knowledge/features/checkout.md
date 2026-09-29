+++
schema = 1
id = "acme.feature.checkout"
kind = "feature"
title = "Checkout"
status = "accepted"
owner = "arch"
feature = "checkout"
summary = "Customers pay for the cart with a saved card."

[scope]
features = ["checkout"]

[[behaviors]]
id = "single-charge"
text = "Pressing pay twice charges the customer once."
+++
