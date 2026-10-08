# Reference code providers

`kb-code-provider` adapts installed ast-index **3.54.0** or CodeGraph **1.6.1** to
`kb.code.v1`. It is a separate Rust executable, outside the engine crate. It installs
nothing, calls no model, and disables CodeGraph daemon, telemetry and update checks.
Build with `cargo build --release -p kb-code-provider --locked`.

```sh
./kbw context --host /path/to/host --path src/store.rs --intent implement \
  --with-code --provider /path/to/kb-code-provider --provider-timeout 300 \
  --provider-arg=--backend --provider-arg=codegraph \
  --provider-arg=--tool --provider-arg=/path/to/codegraph
```

Use `--backend ast-index` for the other adapter. The adapter's `--timeout` (default
120 seconds) bounds each native command; the engine's `--provider-timeout` bounds the
entire invocation. Native tools and snapshot Git stay in the provider's process group, so
the engine's deadline ends them too. When one exceeds the adapter's `--timeout` or an
output limit, the provider removes its checkout and then ends that group, itself
included, so nothing the command started survives. It does so only while it leads the
group and finds `KB_CODE_PROVIDER_OWNS_GROUP=1`, which the engine sets for a provider it
starts in a process group of its own. Without that marker, as in a shell pipeline, it
ends only the stopped command and exits 1 with its error: pipeline peers keep running,
and so may anything the command started. The engine points `TMPDIR` at a private directory
that it removes after every invocation, including the temporary checkout of a provider
killed at the deadline. Plain invocation reads one request from stdin and writes one
response to stdout. Schemas live in `core/schemas/code-{request,response}.v1.schema.json`.
Alternatively, `--provider-file response.json` replays pinned facts without a native tool;
repeat this option for base and head when running `impact --deep`. Context does not use
a pinned response's `similar` candidates, which answered the task of the request that
produced them, and reports their omission as a code limitation. Other commands never
read them.

Every request materializes ordinary files from the requested Git commit into a temporary
checkout with an isolated home and index. It never checks out, cleans, stages or writes
the user's host. Source comes from Git objects, without checkout filters or hooks.
Symlinks, submodules, old tool-state directories and files over 2 MiB are omitted and
reported; 100,000 files and 256 MiB bound the materialized input. Indexing occurs afresh,
so a stale or mutable host index cannot masquerade as a historical graph. Large hosts
can supply a compatible cached provider after independently enforcing the same contract.

The adapters report static limitations. ast-index has line extents and name-based edges
from file scope, all marked possible. CodeGraph uses its version-checked read-only SQLite
schema 11 (extraction version 27); unresolved references or failed files make the response
incomplete. A `resolved` edge means native static resolution, never runtime reachability.
Test labels are filename/name candidates, and similar examples use lexical overlap.
Review them before treating them as a canonical test or precedent.

Supported uses are opt-in context composition, deep impact, coverage fan-in and contract
consumer suggestions on draft submission. `impact --deep` reads both base and head, so
deleted definitions still have consumers. A work-tree request also reports that new
uncommitted edges are unavailable. Whole code units share the context budget and cannot
displace mandatory KB units. No provider output becomes accepted knowledge.

Tests use labeled synthetic native-output fixtures and temporary local Git repositories;
they require no network or installed code tools. Real-tool smoke runs are separate from
these deterministic tests and from downstream A/B acceptance.
