# ADR 0006: POSIX launcher with fingerprinted runtimes

Status: accepted

## Decision

`kbw` computes a fingerprint over the engine build inputs and runs
`.cache/runtime/<fingerprint>/kb`. The runtime is created only by explicit actions:
`kbw --kbw-bootstrap` builds it from source with the pinned toolchain and `Cargo.lock`
(`--locked`) and activates it by atomic rename, or `kbw --kbw-install-artifact` installs a
release archive whose SHA-256 digest is supplied explicitly and whose `BUILD-INFO` must match
the local engine fingerprint and host target. The fingerprint is also compiled into the
binary (`KBW_BUILD_FINGERPRINT`, a tracked Cargo input, so a changed fingerprint always
recompiles even when mtimes look fresh), and every activation smoke-runs `kb --json version`
and requires the matching `build_fingerprint`. Reinstalling the same fingerprint replaces
`BUILD-INFO` and `kb` by rename, so the runtime path resolves at every moment.
Knowledge-only commits do not change the fingerprint. A stamp file plus `find -newer` keeps
warm starts free of hashing and Cargo.

## Consequences

* No global `kb` binary is ever used.
* Normal commands never build or install an engine; a missing runtime fails with
  `KBW_RUNTIME_NOT_BOOTSTRAPPED` (opt-in `KBW_AUTO_BOOTSTRAP=1` for CI), so reading knowledge
  can never silently start a different engine.
* A failed bootstrap or install never removes a previously working runtime.
* The first upstream can work without any published release; release CI produces archives
  and checksums for future publication by the owner.
