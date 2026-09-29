+++
schema = 1
id = "acme.mobile.analytics-consent"
kind = "policy"
title = "Screen analytics consent"
status = "accepted"
owner = "team-mobile"

[scope]
modules = ["mobile.analytics"]

[[rules]]
id = "consent-first"
level = "must"
text = "Send screen analytics events only after the user has granted analytics consent."
+++
