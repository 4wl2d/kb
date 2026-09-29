+++
schema = 1
id = "kb.policy.test-isolation"
kind = "policy"
title = "Test isolation"
status = "accepted"
owner = "maintainers"

[scope]
product = true

[selectors]
paths = ["core/cli/tests/**"]
intents = ["implement", "debug", "review"]
aliases = ["test", "tests", "integration test", "тест*"]

[[rules]]
id = "no-network"
level = "must-not"
text = "Reach public network services from tests; use local bare repositories addressed by file paths."

[[rules]]
id = "isolated-git"
level = "must"
text = "Run Git in tests with an isolated HOME, `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1`."

[[rules]]
id = "no-timing-thresholds"
level = "must-not"
text = "Assert absolute timing thresholds in tests; report measurements from the benchmark crate instead."
+++
