# Dependencies and licenses

kb is licensed under Apache-2.0 (see [LICENSE](../LICENSE)). Third-party dependencies retain
their own licenses. The list below is generated from `Cargo.lock`
with `cargo tree -p kb -e normal,build` and `cargo metadata` (license fields as declared by
each crate). Regenerate it when `Cargo.lock` changes:

```sh
cargo tree -p kb -e normal,build --prefix none --locked
cargo metadata --format-version 1 --locked | jq '.packages[] | {name, version, license}'
```

## Direct runtime dependencies (`core/cli/Cargo.toml`)

| crate | purpose |
|---|---|
| clap | command-line parsing (rejects unknown arguments) |
| globset | path globs for selectors and registries |
| regex | bounded declarative verification patterns |
| libc (Unix) | terminate only the owned subprocess group on a deadline/output failure |
| rusqlite (feature `bundled`) | embedded SQLite with FTS5 for the derived index |
| schemars | JSON Schema generation from the Rust model |
| semver | version applicability of records |
| serde, serde_json | serialization and the JSON protocol |
| sha2 | content ids, fingerprints, receipts |
| toml, toml_edit | strict TOML parsing; format-preserving migrations |
| unicode-normalization | NFKC normalization of aliases and queries |

The `bundled` feature of `libsqlite3-sys` compiles the SQLite amalgamation, which is in the
public domain (https://www.sqlite.org/copyright.html).

Development-only dependencies (not shipped in release archives): `tempfile`, `proptest`,
`jsonschema` (schema conformance tests). `core/benchmarks` depends only on `kb`, `serde`
and `serde_json`.
Optional `core/providers` and `core/eval` are separate executables, not engine dependencies.
They reuse workspace crates; the replay transport/sandbox controller uses Rust's standard
library rather than adding a network framework to `kb`.

## Complete runtime dependency graph (normal + build dependencies of `kb`)

| crate | version | license |
|---|---|---|
| aho-corasick | 1.1.5 | Unlicense OR MIT |
| anstyle | 1.0.14 | MIT OR Apache-2.0 |
| bitflags | 2.13.2 | MIT OR Apache-2.0 |
| block-buffer | 0.12.1 | MIT OR Apache-2.0 |
| bstr | 1.13.1 | MIT OR Apache-2.0 |
| cc | 1.5.1 | MIT OR Apache-2.0 |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 |
| clap | 4.6.7 | MIT OR Apache-2.0 |
| clap_builder | 4.6.7 | MIT OR Apache-2.0 |
| clap_derive | 4.6.7 | MIT OR Apache-2.0 |
| clap_lex | 1.1.1 | MIT OR Apache-2.0 |
| const-oid | 0.10.2 | Apache-2.0 OR MIT |
| cpufeatures | 0.3.1 | MIT OR Apache-2.0 |
| crypto-common | 0.2.2 | MIT OR Apache-2.0 |
| digest | 0.11.3 | MIT OR Apache-2.0 |
| dyn-clone | 1.0.20 | MIT OR Apache-2.0 |
| equivalent | 1.0.2 | Apache-2.0 OR MIT |
| fallible-iterator | 0.3.0 | MIT/Apache-2.0 |
| fallible-streaming-iterator | 0.1.9 | MIT/Apache-2.0 |
| find-msvc-tools | 0.1.14 | MIT OR Apache-2.0 |
| foldhash | 0.2.0 | Zlib |
| globset | 0.4.20 | Unlicense OR MIT |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 |
| hashlink | 0.12.2 | MIT OR Apache-2.0 |
| heck | 0.5.0 | MIT OR Apache-2.0 |
| hybrid-array | 0.4.15 | MIT OR Apache-2.0 |
| indexmap | 2.14.2 | Apache-2.0 OR MIT |
| itoa | 1.0.18 | MIT OR Apache-2.0 |
| libc | 0.2.189 | MIT OR Apache-2.0 |
| libsqlite3-sys | 0.38.2 | MIT |
| memchr | 2.8.3 | Unlicense OR MIT |
| pkg-config | 0.3.34 | MIT OR Apache-2.0 |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 |
| quote | 1.0.47 | MIT OR Apache-2.0 |
| ref-cast | 1.0.27 | MIT OR Apache-2.0 |
| ref-cast-impl | 1.0.27 | MIT OR Apache-2.0 |
| regex | 1.13.1 | MIT OR Apache-2.0 |
| regex-automata | 0.4.18 | MIT OR Apache-2.0 |
| regex-syntax | 0.8.11 | MIT OR Apache-2.0 |
| rusqlite | 0.40.2 | MIT |
| schemars | 1.2.2 | MIT |
| schemars_derive | 1.2.2 | MIT |
| semver | 1.0.28 | MIT OR Apache-2.0 |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 |
| serde_derive_internals | 0.30.0 | MIT OR Apache-2.0 |
| serde_json | 1.0.151 | MIT OR Apache-2.0 |
| serde_spanned | 1.1.1 | MIT OR Apache-2.0 |
| sha2 | 0.11.0 | MIT OR Apache-2.0 |
| shlex | 2.0.1 | MIT OR Apache-2.0 |
| smallvec | 1.16.2 | MIT OR Apache-2.0 |
| strsim | 0.11.1 | MIT |
| syn | 3.0.6 | MIT OR Apache-2.0 |
| tinyvec | 1.13.3 | Zlib OR Apache-2.0 OR MIT |
| toml | 1.1.6+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_datetime | 1.1.1+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_edit | 0.25.15+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_parser | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_writer | 1.1.2+spec-1.1.0 | MIT OR Apache-2.0 |
| typenum | 1.20.1 | MIT OR Apache-2.0 |
| unicode-ident | 1.0.26 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| unicode-normalization | 0.1.25 | MIT OR Apache-2.0 |
| vcpkg | 0.2.15 | MIT/Apache-2.0 |
| winnow | 1.0.4 | MIT |
| zmij | 1.0.23 | MIT |
