# Optional replay kit

`kb-eval` is a separate Rust executable. It is not linked into `kb`, installed by
`kbw init`, or run when knowledge is queried. It runs frozen coding tasks, checks patches
in separate sandboxes, prepares blinded judgments and analyzes preregistered comparisons.
Private product tasks, credentials, raw agent output and study results belong outside
engine-owned paths. The shipped examples and `kb-eval-fixture` are synthetic.

```sh
cargo build -p kb-eval --locked
target/debug/kb-eval --help
```

## Before a study

Copy [task.template.json](templates/task.template.json),
[study.template.json](templates/study.template.json) and the
[preregistration checklist](templates/preregistration.md) into a private study directory.
Replace every placeholder; a syntactically valid zero hash is not a usable pin. Freeze the
task pool, hypothesis family, metrics, margins, repetitions, exclusions, stopping rule,
model budgets and generation cutoffs before any scored run. Registration refuses to
overwrite its destination and every observation carries its content hash. This binds
artifacts together; it is not a trusted timestamp or proof of prospective registration.

Task kits contain a local repository, full base and golden commit ids, prompt, hidden-test
directory and content hash, shell-free build/regression/test argv, and separate quality
and rule checklists. `as_of` is the exact base commit. `history=ancestors` retains its past
history for commit-bound validity, anchor and blame checks; `base-only` (the default when
omitted) is smaller but intentionally cannot resolve older commit evidence. Register the
same choice for every arm; an already shallow source cannot supply complete ancestry.
Never recover a task prompt from its
future implementation or expose golden diffs to the coder. Human reviewers must confirm
that the prompt is recoverable, the checklist is applicable, hidden tests discriminate
the intended defect, and the golden patch does not depend on unrelated future changes.
Quality criteria must not include commit formatting or harness compliance. Optional
criterion `metric` names aggregate subscales as fractions; `Q` is weighted quality × 100,
and `rule_compliance` is a separate weighted fraction.

Commands execute in the checkout with a fresh HOME and no network. Use absolute tool
paths or checkout-relative executables such as `./gradlew`. Provide dependency caches and
toolchains explicitly through `isolation.read_roots` and nonsecret environment entries.
The kit never downloads missing build dependencies. Symlinks/submodules and oversized
source artifacts are refused instead of silently producing an incomplete patch.

Hash files, executables and directories with `kb-eval digest PATH`. A directory digest
covers sorted relative paths, bytes and executable bits, excluding Git metadata. Frozen
knowledge/provider artifacts are full-commit arm overlays at new relative directories;
their declared host cutoff must be an ancestor of the task base. They cannot overwrite
existing host paths. A pinned provider response replayed with `--provider-file` omits its
task-specific precedent candidates, so such an arm delivers no code analog units, unlike
a live provider. Include the arm's precise invocation in `instructions`, or commit
its tested host integration into the chosen base/overlay. No implicit installation or
network sync occurs. The runner includes the task's `--as-of` instruction, while the
frozen input artifacts prevent later knowledge text from leaking through date metadata.

```sh
kb-eval register --study study.json --output registration.json
kb-eval preflight --registration registration.json --output preflight
kb-eval qualify --registration registration.json --task TASK --output qualified-TASK
```

Preflight checks the pinned executable and isolated `--version` startup without model
credentials or network; it does not test live authentication or billable requests. Each
client pin includes family, executable SHA-256, exact version output in an isolated HOME,
model id, effort,
deadline and credential variable **names**. Pin the runtime/dependencies of wrapper
scripts separately in the study environment; an executable hash alone does not pin an
imported package. Use immutable model revisions where the provider offers them and record
aliases as a study limitation otherwise. Check new client versions against their actual
CLI help before changing pins. Supported adapters are Codex, Claude Code, Cursor and Grok.

Qualification creates independent base and golden checkouts. Both must build and pass
regression commands; the base must fail the hidden command, and the golden must pass it.
Hidden files are installed only after ordinary build/regression checks. The exact kit hash
must match the qualification used by a trial. An unavailable tool, failed isolation check
or non-discriminating hidden test prevents qualification.

## Isolation and execution

macOS uses a generated Seatbelt deny-default profile, including the host's dyld bootstrap
rules. Linux uses bubblewrap with fresh user, PID, mount and network namespaces. Writable
access is limited to the current stage's checkout, HOME and temporary directory. System
binaries and declared runtimes are read-only; real HOME, source repositories, task kits,
other trials and hidden inputs are not mounted/allowed. Every stage executes a canary
that proves its private sibling file cannot be read or written. A missing or broken
backend fails closed; there is no unsandboxed execution path. Linux requires a host that
permits bubblewrap's user namespaces; do not weaken host-wide policy to make a run pass.
When AppArmor's unprivileged user namespace restriction blocks bubblewrap (Ubuntu 24.04),
the upstream CI grants `userns` to `/usr/bin/bwrap` alone with a binary-scoped AppArmor
profile, and probes `bwrap --unshare-all` before the replay smoke; the host-wide
`kernel.apparmor_restrict_unprivileged_userns` setting stays unchanged.

The controller fetches the full base commit into a standalone repository, shallow or with
its past ancestry according to the registered task. It verifies that every object in the
result is reachable from that historical view; extra future blobs as well as commits are
refused before an agent starts.
It does not use worktrees, alternates, hardlinked object stores or the source's hooks and
global Git configuration. Future commits are not available through local object lookup.
After the coder finishes, the controller extracts regular changed files using **trusted**
Git metadata, honoring ignored files without running Git against the coder's config. A
fresh checkout receives that patch and the hidden tests. Even a lingering coder child
has no access to the test or judge workspace. Deadlines kill the owned process group;
raw output, exit/signal, timeout and output-limit results remain available on failure.

Network is disabled by default. For an authorized model run, exact `api_hosts` permit only
HTTPS CONNECT on port 443 through a local proxy. Other ports, arbitrary destinations and
private/loopback DNS results are rejected, and a refused request receives `403 Forbidden`.
On macOS, Seatbelt can express the proxy rule only as `localhost:PORT`, which admits that
port number on every local address: IPv4 and IPv6 loopback and the host's own interface
addresses. The proxy listens on that port on both loopback addresses, but a host service
bound to the same port number on an interface address would still be reachable. Linux
exposes the proxy's Unix socket, created in a fresh private directory under `/tmp` so its
path fits the socket address limit, and bridges it inside the private network namespace;
the bridge fails before starting the client when it cannot reach that socket.
Proxy payloads and credentials are never logged. Clients that ignore proxy configuration
fail instead of receiving unrestricted network access. The synthetic tests do not contact
the network; real transport compatibility still needs an authorized pilot per client.
This is a replay-isolation boundary, not certification against a malicious kernel exploit.

The default macOS profile has a verified compatibility limit: Gradle 9.5.0 fails before
project configuration while creating `FileLockContentionHandler`, with
`java.net.SocketException: Operation not permitted`. This was reproduced with the pinned
distribution, a historical Android checkout, JDK 25 and `--offline --no-daemon`; Java
startup itself passed. Gradle's local socket use is still network activity under Seatbelt.
Such a worker cannot qualify an Android task. Use a separately qualified isolated worker;
the Linux namespace profile still needs actual Gradle validation. Do not grant broad
host-loopback access or count an unavailable build as a successful task check.

Credentials are inherited only from the pinned client's explicitly named API-key
variables. No real auth/config directory, keychain, browsing session or global agent rule
is copied. Confirm the chosen client accepts that auth method in its isolated HOME before
budgeting the main study. The kit does not impose a provider-side monetary cap: arrange
that cap and the approved stopping policy independently. Deadlines are enforced locally.

Only `run --execute` and `judge --execute` start model jobs:

```sh
kb-eval run --registration registration.json --task TASK --arm kb-next \
  --block BLOCK --repetition 0 --qualification qualified-TASK/qualification.json \
  --output trial-TASK-kb-next-0 --execute
kb-eval judge --registration registration.json \
  --trial trial-TASK-kb-next-0/trial.json --judge 0 --execute
```

Each trial has a new directory; retries never overwrite it. Account for failed attempts
and deviations under the preregistered policy, not just successful final attempts. Do not
retry a missing cell until it looks favorable. Register at least two repetitions and 40
held-out tasks for the upstream generation gate; the kit also permits smaller pilots,
which cannot satisfy that gate.

## Blinded judgments and retained evidence

The judge gets a new random case directory and HOME, the task/checklists, neutral patch,
test pass/fail facts and bounded before/after source context. `judge_context` lists extra
historical paths needed for interpretation. Binary/oversized context is explicitly marked
omitted. No repository, `.git`, project/global instruction files, model/arm label, coder
transcript or golden patch is exposed as executable context. Source/rule text appears only
inside JSON/patch data. This prevents automatic rule loading; it does not make quoted
untrusted text safe to obey. The judge prompt explicitly treats it as evidence.

Judgments must contain every quality and rule id exactly once, bounded scores and concrete
evidence. Unknown ids, missing evidence, failed judges and malformed output do not become
zero-cost successful observations. Criteria requiring unavailable evidence remain an
evaluation-design problem: add the needed frozen source context or score that criterion
separately using a preregistered, inspectable observation. A code-only packet cannot prove
a commit-message rule or real-device behavior. Judge identity is retained only in the
private mapping, and each registered family produces independent observations. Run the
second judge over the same cells; never pool families to manufacture a larger sample.

The raw coder and judge logs, case mapping, hash-bound patch/source context, test command
outcomes, native usage segments, objective measurements and judgment scores are preserved.
Judge case paths are recorded in `judge-N-artifacts.json`. Preserve these directories
with the study; they are intentionally not auto-deleted. Raw artifacts can contain private
code and belong in the product's private evidence store, not upstream Git.

## Usage and analysis

`usage_basis` explicitly selects incremental events or cumulative session snapshots.
`input_convention` declares whether native input counts include cache reads/writes, or is
`unknown`. Do not guess a provider's convention. Native counters are always retained.
Separate reconnect logs can be normalized and reconciled without discarding prior cost:

```sh
kb-eval segment --family claude --log reconnect-0.jsonl --id part-0 \
  --session trial-A --sequence 0 --output segment-0.json
kb-eval segment --family claude --log reconnect-1.jsonl --id part-1 \
  --session trial-A --sequence 1 --output segment-1.json
kb-eval account --segment segment-0.json --segment segment-1.json \
  --input-convention excludes-cache --output accounting.json
```

Use `--cumulative` only when the native counters are cumulative for that session. Sequence
gaps, conflicting event ids, ambiguous duplicate anonymous terminals, truncated tails,
missing usage and unaccounted child sessions remain incomplete. Incremental segments sum;
cumulative snapshots must be monotonic and contribute only the last session total.
Reported USD cost is preserved; absent cost stays unknown. The kit does not silently
substitute remembered prices or equate token counters with billing charges. If pricing
must be derived externally, preregister the dated price source, cache convention and
estimation uncertainty, and retain the calculation artifact.

```sh
kb-eval analyze --registration registration.json \
  --observation trial-A/judge-0.json --observation trial-B/judge-0.json \
  --output analysis.json
```

Analysis uses exactly matched task/arm/block/judge/repetition cells, averages paired
repetitions within each task, then resamples whole task clusters. It reports marginal
percentile confidence intervals (linear empirical-quantile interpolation), exact two-sided
sign flips through 16 tasks and seeded Monte Carlo sign flips above that size. Sign flips
assume exchangeable/symmetric task effects under the null. Holm correction includes every
preregistered comparison within each block/judge family; missing hypotheses contribute
p=1. A separate Bonferroni interval (`alpha / family size`) governs every SESOI decision,
including equivalence, so a family of ordinary marginal intervals is not presented as
simultaneous evidence. Bootstrap coverage remains approximate; small task pools and sparse
resampling tails need caution. These choices and margins are fixed before scored outcomes.

Every arm mean in a comparison uses the same paired cohort. Cost/quality conclusions may
be combined only when their cohort hashes match. Missing planned cells or too few tasks
prevent acceptance decisions. Equivalence requires the entire family interval inside ±SESOI;
superiority beyond SESOI requires its lower benefit bound above SESOI and adjusted p below
alpha. The report also gives leave-one-task-out effects, a separate estimate excluding
deviations, absolute repeat differences and resolved flip fractions. Review these and
second-judge agreement before enabling defaults or making a best-on-all-criteria claim.

Method references: [bootstrap intervals](https://stat.ethz.ch/R-manual/R-devel/library/boot/help/boot.ci.html),
[Holm adjustment](https://stat.ethz.ch/R-manual/R-devel/library/stats/html/p.adjust.html).
Isolation reference: [bubblewrap's namespaces and mount boundary](https://github.com/containers/bubblewrap/blob/main/README.md).

## Local verification

```sh
cargo test -p kb-eval --locked
cargo test -p kb-eval --locked --test replay_workflow -- --ignored
```

The second command explicitly runs the OS-dependent synthetic coder/judge fixture and
makes no model or public network calls: its coder and judge send only an unapproved
target to the egress proxy and require its `403` refusal, which an unreachable proxy
cannot produce. On macOS it also loads the generated Seatbelt profile and checks that
only the proxy's port is reachable. The ordinary suite separately checks hidden/future
Git object exclusion, accounting, pairing, missingness, bootstrap/sign-flip calculations,
Holm, equivalence, ordering, duplicate cells, profile construction, the proxy waiting for
a client's request and refusing it with `403`, the proxy holding its port on IPv6
loopback (macOS) and the bridge failing without its socket. A synthetic fixture pass
verifies the runner; it provides no evidence that a model or knowledge arm is better.
