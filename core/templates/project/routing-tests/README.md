# Routing tests

Golden context requests for this knowledge base. `./kbw validate` runs every `*.toml` file in
this directory (skip with `--no-routing`) and fails with `ROUTING_TESTS_FAILED` when a case
does not hold.

```toml
schema = 1

[[case]]
name = "mobile token refresh gets the storage policy and the refresh contract"
intent = "implement"
task = "retry token refresh after a network error"
repos = ["mobile"]
paths = ["mobile:app/auth/TokenRefresher.kt"]   # `repo:path`, repo-relative
expect_mandatory = ["acme.mobile.token-storage"] # must be in the mandatory tier
expect_included = []                             # must be included in any tier
forbid = ["acme.backend.migration-compat"]       # must not appear at all
```

Other optional fields: `modules`, `features`, `concepts`, `host_versions` (`repo=x.y.z`),
`budget`, `budget_unit` (`tokens-est` or `bytes`), `expect_status`
(`complete`, `partial`, `conflict`, `incomplete`), `expect_ambiguous` (normalized phrases),
`expect_budget_exceeded`.

Write cases that are decided by scope, not by wording: give the repository and paths (or
modules/features) so every dimension a record constrains is known. Forbid only records that
are provably out of scope for the task and not required by an applicable record; forbidding
something the engine may legitimately rank as supplementary makes the test brittle.
