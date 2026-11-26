# ADR 0017: One installed `kb` command selects each checkout's engine

Status: accepted. Supersedes ADR 0006; amends ADR 0001.

## Context

The POSIX launcher `kbw` (ADR 0006) ran the runtime built from its own checkout's engine
sources, so knowledge was always read by the engine its checkout pins. It also meant: no
command on PATH, a different spelling per checkout (`./kbw`, `.kb/kbw`, `<path>/kbw`), no
Windows support without a POSIX shell, every checkout building or installing its own runtime,
and no way to update the tooling itself. Agents and people want one `kb` command that works
the same way in every repository, on Windows, Linux and macOS.

## Decision

* `kb` is one native binary, installed once per user. It is both the engine and the engine
  selector. On every invocation it finds the KB checkout (`--root`, `KB_ROOT`, else the
  nearest directory from the current directory, then from `--host`, upwards that has
  `core/release.toml` or mounts a checkout at `.kb`), computes the checkout's engine
  fingerprint (the definition of ADR 0006, with CRLF hashed as LF and `core/cli/build.rs` as
  an input when it exists) and:
  * runs the command in-process when it was built from exactly those engine sources (every
    build compiles its fingerprint in through `build.rs`);
  * otherwise runs the runtime of that fingerprint from the per-user runtime store
    (`KB_RUNTIME_DIR`, default `%LOCALAPPDATA%\kb\runtime`, `~/Library/Application
    Support/kb/runtime` or `${XDG_DATA_HOME:-~/.local/share}/kb/runtime`), or one that
    `kbw` built in the checkout (`.cache/runtime/<fingerprint>/kb`), in place of the
    process (`exec` on Unix; on Windows a child with inherited streams whose exit status is
    passed on);
  * fails with `RUNTIME_NOT_BOOTSTRAPPED` (exit 54) when no runtime exists. Normal commands
    never build or download an engine, unless `KB_AUTO_BOOTSTRAP=1` (CI).
  Checkouts without complete engine sources are pinned only by the versions in their
  manifest, which the engine checks (`RUNTIME_INCOMPATIBLE`); they run in-process.
* `kb engine bootstrap|install|package|info|fingerprint` replace the `--kbw-*` flags.
  `bootstrap` builds from source with the pinned toolchain (`cargo build --release --locked
  -p kb`) and activates by rename; `install` verifies a release archive (digest from
  `SHA256SUMS` of the checkout's release, or an explicit `--sha256`), its entries, its
  `BUILD-INFO` (engine version, target, fingerprint) and a smoke run before activation.
  Runtimes are content-addressed, so one store serves every checkout on the machine.
* Distribution: GitHub releases of the upstream repository carry `kb-<version>-<target>.tar.gz`
  for `aarch64-apple-darwin`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`
  (static, any distribution) and `x86_64-pc-windows-msvc`, plus `SHA256SUMS`. `install.sh`
  (macOS, Linux) and `install.ps1` (Windows PowerShell 5.1 and later) install `kb` from
  there; `kb self update` replaces the installed binary in place after the same
  verification (on Windows by renaming the running executable aside). `cargo install
  --locked --git <repository> kb` remains for other platforms. Package managers (Homebrew,
  Scoop, winget) need repositories and credentials outside this one; they can be added
  later without changing the command.
* Host binding: `.kb.toml`; `.kbw.toml` is read when it is the only one (doctor warns),
  both at once is `CONFIG_INVALID`. The host state directory `.kbw/` keeps its name:
  `.kb` is the conventional mount path, and renaming committed integration state would
  force every host to re-integrate for no functional gain.
* `kbw` stays for one release as a deprecated POSIX forwarder (`KB_ROOT=<its dir> kb ...`,
  `--kbw-*` flags mapped to `kb engine ...`, `KBW_*` variables passed on as `KB_*`), so host
  scripts, CI jobs, installed skills and the `update prepare` of kbw-era engines keep
  working while they move to `kb`.

## Consequences

* The invariant of ADR 0001 is kept: knowledge is read by the engine built from its
  checkout's own engine sources, now selected by `kb` instead of being started by a
  per-checkout launcher. A downstream still carries the engine with its knowledge.
* One spelling everywhere: generated skills call `kb` (`kb --root <kb_path>` when the KB is
  not mounted at `.kb`); CI templates install `kb` with `core/templates/ci/install-kb.sh`
  (the checkout's release, else a source build).
* Windows is supported natively (PowerShell or cmd); CI runs the test suite and the
  installers on `windows-latest`, `ubuntu-latest` and `macos-15`.
* Launcher errors are protocol errors in the `kb.cli.v1` envelope (`RUNTIME_NOT_BOOTSTRAPPED`
  54, `BOOTSTRAP_FAILED` 55, `TOOLCHAIN_MISMATCH` 56, `ARTIFACT_INVALID` 57,
  `PACKAGE_FAILED` 58, `DOWNLOAD_FAILED` 66); codes are added, none changes.
* Updating the installed `kb` (`kb self update`) and upgrading a downstream to a newer
  upstream (`kb update`) stay separate operations. A newer installed `kb` never changes
  which engine reads a checkout.
* The installers download over HTTPS and verify SHA-256 digests from the same release; they
  do not verify signatures. `install.sh`/`install.ps1` are part of the reviewed engine
  sources, so CI jobs run the checkout's copy rather than a fetched script.
