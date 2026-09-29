+++
schema = 1
id = "kb.policy.machine-output"
kind = "policy"
title = "Machine output hygiene"
status = "accepted"
owner = "maintainers"

[scope]
features = ["cli-protocol", "context-assembly"]

[selectors]
paths = ["core/cli/src/output.rs", "core/cli/src/cli/**", "core/cli/src/context/**"]
aliases = ["json output", "stdout", "stderr", "exit code"]

[links]
requires = ["kb.contract.cli-protocol"]

[[rules]]
id = "stdout-protocol-only"
level = "must"
text = "In JSON mode write only the protocol envelope to stdout; send progress and diagnostics to stderr."

[[rules]]
id = "no-timings-in-result"
level = "must-not"
text = "Put timestamps, timings or other non-deterministic values into the `result` object; they belong in `meta`."

[[rules]]
id = "stable-codes"
level = "must"
text = "Keep symbolic error codes and their numeric exit codes stable within a protocol version."
+++
