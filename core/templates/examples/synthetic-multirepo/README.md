# Synthetic multi-repository example (SYNTHETIC)

**Everything here is invented.** The product, the repositories (`mobile`, `backend`,
`shared-contracts`), the teams, the records and the `.invalid` URLs exist only to demonstrate
and test kb. The example is never used as default project data: `kbw init` without
`--example` creates an empty project.

## What it demonstrates

| aspect | records |
|---|---|
| product-wide policy with an overridable setting (`min-reviewers`, stricter = higher) and a non-overridable one | `example.common.code-review` |
| a stricter override of that setting, scoped to payment code | `example.backend.payments-review` |
| cross-repository contracts with parties, obligations and typed exceptions | `example.contract.token-refresh`, `example.contract.payment-intent` |
| a shared contract that stays mandatory through `links.requires` for a task outside its own scope (backend payment code) | `example.contract.error-envelope` |
| invariants with conditions and exceptions | `example.mobile.single-refresh`, `example.backend.migration-compat` |
| `rationale`, `related` and `supersedes` links; a superseded decision that stays addressable | `example.decision.*` |
| features with behaviors and boundaries | `example.feature.login`, `example.feature.checkout` |
| procedures (data only; kb never executes them) | `example.procedure.*` |
| a known gap, delivered as mandatory context | `example.gap.offline-checkout` |
| a draft proposal (never mandatory) and a deprecated reference with version applicability | `example.mobile.biometric-unlock`, `example.reference.api-v1` |
| multilingual concept aliases and the ambiguous `композици*` shared by `ui-composition` and `object-composition` | `registry/concepts.toml` |

Golden routing cases with expected mandatory and forbidden ids are in
`project/routing-tests/`; `expected-queries.md` lists the same requests as commands.

## Try it

Use a scratch clone of the upstream repository so that no real downstream is touched:

```sh
git clone <upstream repository> kb-demo && cd kb-demo
./kbw init --example synthetic-multirepo            # dry-run: prints the plan
./kbw init --example synthetic-multirepo --apply    # writes project/ and the CI workflow
./kbw validate                                      # records, links, policies, routing tests
```

Context from the local working tree (the upstream approved ref does not contain the example,
so select the working tree explicitly and skip the remote check):

```sh
./kbw context --offline --snapshot working-tree --intent implement \
  --task "Retry the token refresh after a network error" \
  --repo mobile --path mobile:app/auth/TokenRefresher.kt
```

A working-tree snapshot is never approved and `--offline` leaves freshness unverified, so
completeness is at best `partial` and the command exits with 30 (`CONTEXT_INCOMPLETE`) after
printing the full result. That is the honest answer, not a failure of the example.

To see a `complete`, freshness-verified answer, publish the example to a local bare
repository and make it the approved source (all local, no network):

```sh
git checkout -b demo && git add -A && git commit -m "Synthetic example"
git init --bare ../kb-demo-origin.git
git remote set-url origin "$(cd .. && pwd)/kb-demo-origin.git"
git push origin demo:refs/heads/main
./kbw context --intent implement --task "Retry the token refresh after a network error" \
  --repo mobile --path mobile:app/auth/TokenRefresher.kt
```

The example allows the `file` transport (`allowed_protocols` in `project/project.toml`) for
exactly this purpose; real projects normally allow only `https` and/or `ssh`.
