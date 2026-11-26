#!/bin/sh
# CI glue: put the kb command for a KB checkout into a directory (add it to PATH afterwards)
# and make sure a runtime serves the checkout's engine.
#
#   sh install-kb.sh <kb-checkout> <bin-dir>
#
# 1. Installs the kb release of the checkout's engine_version with the checkout's own
#    install.sh; when that release cannot be installed (not published, no network), builds
#    kb from the checkout itself with its pinned toolchain.
# 2. Runs `kb engine bootstrap`, the explicit step that builds the checkout's engine from
#    source only when it differs from the installed kb (declared engine patches); otherwise
#    it changes nothing.
# KB_RUNTIME_DIR and KB_CARGO_TARGET_DIR are honored, so CI can cache both.
set -eu
kb_root=${1:?usage: install-kb.sh <kb-checkout> <bin-dir>}
kb_bin=${2:?usage: install-kb.sh <kb-checkout> <bin-dir>}
kb_version=$(sed -n 's/^engine_version[[:space:]]*=[[:space:]]*"\([^"]*\)".*$/\1/p' "$kb_root/core/release.toml")
[ -n "$kb_version" ] || {
    printf '%s\n' "install-kb: no engine_version in $kb_root/core/release.toml" >&2
    exit 1
}
mkdir -p "$kb_bin"
if ! sh "$kb_root/install.sh" --version "$kb_version" --dir "$kb_bin"; then
    printf '%s\n' "install-kb: kb $kb_version could not be installed from its release; building kb from $kb_root" >&2
    kb_target=${KB_CARGO_TARGET_DIR:-$kb_root/.cache/cargo-target}
    (cd "$kb_root" && rustup toolchain install &&
        CARGO_TARGET_DIR=$kb_target cargo build --release --locked -p kb)
    cp "$kb_target/release/kb" "$kb_bin/kb"
fi
if "$kb_bin/kb" --root "$kb_root" --json engine info | grep -q '"status": "missing"'; then
    (cd "$kb_root" && rustup toolchain install)
    "$kb_bin/kb" --root "$kb_root" engine bootstrap >/dev/null
fi
"$kb_bin/kb" --root "$kb_root" engine info >&2
