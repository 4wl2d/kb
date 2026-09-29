# Benchmark report

Measured with the `kb-bench` crate (`core/benchmarks`) on one machine. These numbers describe
this run only; they are not guarantees, and CI never asserts timings. Re-run them on your
hardware before drawing conclusions.

## How to reproduce

```sh
cargo build --release --locked -p kb -p kb-bench
./target/release/kb-bench run --sizes 1000,10000,50000 --queries 30 \
  --workdir /path/to/empty-dir --json results.json
```

`kb-bench gen --records N --out DIR` writes the same deterministic synthetic corpus on its
own (seed 42 by default), and `kb-bench verify --dir DIR` validates it (all corpora below
validate with 0 errors and 0 warnings). `kb-bench smoke` is the small sanity run used by CI.

## Environment of this run

| item | value |
|---|---|
| machine | Apple M5 Max, 18 CPU cores, 128 GiB RAM |
| OS | macOS 26.6.2 (Darwin 25.6.0, arm64) |
| toolchain | rustc 1.98.1 (Homebrew), release profile (`lto = "thin"`, `codegen-units = 4`) |
| git | 2.55.0 |
| engine | kb 0.1.0 (document schema 1, protocol 1, index schema 1) |
| binary | `target/release/kb` built by `cargo build --release` (so `build_fingerprint` is `unknown`; the launcher-built runtime is the same code) |
| date | 2026-09-27 |

## Corpus

Synthetic, deterministic (seed 42): 4 repositories, `records/200` modules per repository
(4–1024), 32 concepts with English and Russian aliases, records of all eight kinds (20 %
policies, 10 % invariants, 5 % contracts, 15 % features, 15 % decisions, 10 % procedures,
20 % references, 5 % gaps), ten product-wide policies, repo-wide scope only for about 2 % of
the first 5 000 records, module scope otherwise, and acyclic `requires` / `rationale` /
`related` links.

Queries: a fixed set of `implement` requests, each with one repository, one module path, a
task text (English or Russian) and a budget of 16 000 estimated tokens.

## Results

### In-process (library: `Index` + `context::assemble` + compact rendering)

| records | full index | warm `ensure` | incremental index (1 file changed) | index size | warm query p50 / p95 | `assemble` p50 / p95 | response bytes p50 / max | mandatory records (avg) |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 000 | 102.7 ms | 0.01 ms | 27.6 ms (1 parsed) | 4.0 MiB | 4.95 / 5.37 ms | 4.40 / 4.74 ms | 15 565 / 20 042 | 31.6 |
| 10 000 | 902.3 ms | 0.02 ms | 175.9 ms (1 parsed) | 39.5 MiB | 22.14 / 23.33 ms | 20.68 / 21.84 ms | 19 285 / 23 389 | 42.8 |
| 50 000 | 4 511 ms | 0.03 ms | 911.3 ms (1 parsed) | 201.1 MiB | 107.77 / 113.30 ms | 102.29 / 107.64 ms | 19 622 / 27 252 | 46.0 |

30 queries per size. "Warm query" opens the index, opens a snapshot view, assembles and
renders the context. The source is the KB working tree with the stat cache, so every
status is `partial` (working-tree knowledge is never approved).

### Process level (real `kb` executable, Git snapshot, local bare remote)

| records | startup (`kb version`) p50 / p95 | first `sync` | first `index` | warm `--offline context` p50 / p95 | `context` with freshness fetch (local file remote) p50 / p95 | response bytes | peak RSS |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 000 | 2.05 / 2.20 ms | 110.1 ms | 155.6 ms | 38.29 / 41.50 ms | 63.50 / 64.79 ms | 14 755 | 13.3 MiB |
| 10 000 | 1.71 / 1.91 ms | 186.1 ms | 832.4 ms | 53.04 / 56.95 ms | 69.90 / 71.72 ms | 19 328 | 39.7 MiB |
| 50 000 | 1.71 / 1.95 ms | 430.3 ms | 4 635 ms | 145.72 / 147.65 ms | 163.66 / 165.22 ms | 16 929 | 82.5 MiB |

Samples: 20 for startup, 30 for warm offline context, 6 for context with the freshness fetch.
Peak RSS comes from `/usr/bin/time -l` for one warm offline `context` call.

## Reading the numbers

* **Target.** The goal was tens of milliseconds for a warm local query on 10 000 records. On
  this machine a warm query takes about 22 ms in-process and about 53 ms through the real
  executable.
* **Freshness is separate from retrieval.** The difference between the last two
  process-level columns (about 15–25 ms) is the per-call fetch of the approved ref, here from
  a local file remote. Over a network the fetch costs whatever a `git fetch` of one ref costs
  on that network. Since there is no TTL by design, every non-offline knowledge query pays
  it.
* **Process overhead.** A warm offline call spawns 7 short `git` processes: host detection,
  mirror state, and one batched read of the snapshot manifest and config. On this macOS
  machine each spawn costs about 5–6 ms, which accounts for most of the gap between the
  in-process and process-level numbers.
* **Scaling.** Warm retrieval grows roughly linearly with the number of obligation records.
  Mandatory selection loads the metadata of every accepted policy, invariant, contract and
  gap (about 20 000 records at 50 000) and evaluates applicability in Rust, so an obligation
  is never excluded by an approximate prefilter. A SQL-side prefilter by repository or module
  would cut this cost. It is a known optimization, not implemented because it has to match
  the applicability rules exactly.
* **Incremental builds** parse only changed content. The remaining cost is membership rows,
  validation of the whole snapshot's metadata, and the working-tree stat walk.
* **Git snapshots** (the normal approved path) never walk or parse Markdown on a warm call:
  the snapshot key maps directly to an indexed snapshot.
