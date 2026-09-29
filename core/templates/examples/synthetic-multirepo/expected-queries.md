# Expected context queries (SYNTHETIC example)

Run these from the root of a KB initialized with `./kbw init --example synthetic-multirepo
--apply`. Add `--offline --snapshot working-tree` when the example is not yet on the approved
ref (see README.md). The mandatory ids below follow from scope alone (AND across dimensions,
OR within one) and are checked by `project/routing-tests/*.toml`; supplementary records may
vary with wording and budget and are not listed as expectations, except rationale targets of
mandatory records. "Required" ids are mandatory dependencies reached through `links.requires`
even though their own scope does not apply to the task.

## 1. Mobile token refresh (cross-repository contract)

```sh
./kbw context --intent implement --task "Retry the token refresh after a network error" \
  --repo mobile --path mobile:app/auth/TokenRefresher.kt
```

* Mandatory: `example.common.code-review`, `example.contract.error-envelope`,
  `example.contract.token-refresh`, `example.mobile.single-refresh`,
  `example.mobile.token-storage` (the refresh contract, owned across mobile and backend,
  applies because the path is in `mobile.auth`).
* Rationale: `example.decision.secure-token-storage`.
* Knowledge status `complete`; the command itself reports `partial` when run with
  `--offline` or against the working tree.
* Never: backend-only records (`example.backend.*`), `example.contract.payment-intent`,
  `example.mobile.state-hoisting`, the superseded `example.decision.token-in-preferences`
  and the draft `example.mobile.biometric-unlock`.
* Effective setting: `example.common.code-review#min-reviewers = 1`.

## 2. Weak wording in the same module

```sh
./kbw context --intent refactor --task "rename a local variable" \
  --repo mobile --path mobile:app/auth/SessionStore.kt
```

* Mandatory: the same five ids as query 1. Obligations are selected by scope, independent
  of full-text similarity.

## 3. "Композиция" with a UI path

```sh
./kbw context --intent implement --task "Исправить композицию экрана профиля" \
  --repo mobile --path mobile:app/ui/ProfileScreen.kt
```

* Mandatory: `example.common.code-review`, `example.mobile.state-hoisting`.
* The alias `композици*` belongs to `ui-composition` and `object-composition`; the path
  matches the `ui-composition` hint, so there is no ambiguity.
* Never: `example.backend.composition-over-inheritance`.

## 4. "Композиция" without a path

```sh
./kbw context --intent explain --task "Как устроена композиция?" --repo mobile
```

* Mandatory: `example.common.code-review` only.
* `ambiguities` lists the phrase with candidates `ui-composition` and `object-composition`;
  re-run with `--concept ui-composition` or a `--path`.
* Module-scoped obligations of `mobile` are listed as undetermined (the task names no path
  or module), so completeness is `partial`, never `complete`.

## 5. Backend payment code (override and known gap)

```sh
./kbw context --intent implement --task "Add a retry to the charge call" \
  --repo backend --path backend:src/payments/ChargeService.kt
```

* Mandatory: `example.backend.payments-review`, `example.common.code-review`,
  `example.contract.payment-intent`, `example.gap.offline-checkout`.
* Required: `example.contract.error-envelope`, through `links.requires` of the payment
  contract, although `backend.payments` is outside the envelope's own scope.
* Rationale: `example.decision.integer-money`.
* Effective setting: `example.common.code-review#min-reviewers = 2` (override from
  `example.backend.payments-review`).
* Never: mobile auth records and `example.contract.token-refresh`.

## 6. Backend domain refactoring

```sh
./kbw context --intent refactor --task "Split the order aggregate into smaller services" \
  --repo backend --path backend:src/domain/order/OrderAggregate.kt
```

* Mandatory: `example.backend.composition-over-inheritance`,
  `example.backend.migration-compat`, `example.common.code-review`.
* Never: `example.backend.payments-review` (another module), payment and token contracts.

## 7. Shared schema change

```sh
./kbw context --intent implement --task "Add a retry-after field to the error envelope" \
  --repo shared-contracts --path shared-contracts:schemas/errors/envelope.json
```

* Mandatory: `example.common.code-review`, `example.contract.error-envelope`.

## Other commands

```sh
./kbw show example.contract.payment-intent            # full record, authoritative text
./kbw show example.reference.auth-overview --section sequence
./kbw search "идемпотентность"                        # search is not a substitute for context
./kbw validate                                        # includes the routing tests above
```
