#!/bin/sh
# Install the kb command on macOS or Linux from a GitHub release (README.md, section
# "Install"). Windows: install.ps1.
#
#   sh install.sh [--version <x.y.z>] [--dir <bin dir>] [--force]
#
# Downloads kb-<version>-<target>.tar.gz and SHA256SUMS of the release (default: the latest),
# verifies the archive's SHA-256 digest and entries, checks that the binary runs and installs
# it as <dir>/kb (default: $KB_INSTALL_DIR, else ~/.local/bin). An existing kb that is not a
# kb binary (for example a wrapper script) is only replaced with --force. Update later with
# `kb self update`. KB_RELEASES_URL selects another release location (https:// or a local
# directory with the same download/v<version>/ layout).
set -eu
umask 022

KB_RELEASES=${KB_RELEASES_URL:-https://github.com/4wl2d/kb/releases}
KB_RELEASES=${KB_RELEASES%/}

die() {
	printf 'kb install: error: %s\n' "$*" >&2
	exit 1
}

note() {
	printf 'kb install: %s\n' "$*" >&2
}

usage() {
	sed -n '2,13s/^# \{0,1\}//p' "$0"
}

version=
dir=${KB_INSTALL_DIR:-${HOME:?HOME is not set}/.local/bin}
force=0
while [ $# -gt 0 ]; do
	case $1 in
	--version)
		[ $# -ge 2 ] || die "--version needs a value"
		version=$2
		shift 2
		;;
	--version=*)
		version=${1#*=}
		shift
		;;
	--dir)
		[ $# -ge 2 ] || die "--dir needs a value"
		dir=$2
		shift 2
		;;
	--dir=*)
		dir=${1#*=}
		shift
		;;
	--force)
		force=1
		shift
		;;
	-h | --help)
		usage
		exit 0
		;;
	*) die "unknown option $1 (see --help)" ;;
	esac
done
version=${version#v}
case $version in
*[!0-9A-Za-z.+-]*) die "malformed version '$version'" ;;
esac

case "$(uname -s)/$(uname -m)" in
Darwin/arm64 | Darwin/aarch64) targets=aarch64-apple-darwin ;;
Linux/x86_64 | Linux/amd64) targets="x86_64-unknown-linux-musl x86_64-unknown-linux-gnu" ;;
Linux/aarch64 | Linux/arm64) targets="aarch64-unknown-linux-musl aarch64-unknown-linux-gnu" ;;
*) die "no release archive for $(uname -s)/$(uname -m); build kb from source: cargo install --locked --git https://github.com/4wl2d/kb kb" ;;
esac

asset() { # <file name>
	if [ -n "$version" ]; then
		printf '%s/download/v%s/%s\n' "$KB_RELEASES" "$version" "$1"
	else
		printf '%s/latest/download/%s\n' "$KB_RELEASES" "$1"
	fi
}

fetch() { # <location> <destination>
	case $1 in
	https://*)
		command -v curl >/dev/null 2>&1 || die "curl is required to download $1"
		curl --proto =https --proto-redir =https --tlsv1.2 -fsSL --retry 2 -o "$2" "$1" ||
			die "download failed: $1"
		;;
	*://*) die "only https:// URLs and local directories are accepted: $1" ;;
	*) cp "$1" "$2" || die "cannot read $1" ;;
	esac
}

sha256() { # <file>
	if command -v sha256sum >/dev/null 2>&1; then
		set -- "$(sha256sum <"$1")"
	elif command -v shasum >/dev/null 2>&1; then
		set -- "$(shasum -a 256 <"$1")"
	else
		die "neither sha256sum nor shasum is available"
	fi
	printf '%s\n' "${1%% *}"
}

tmp=$(mktemp -d "${TMPDIR:-/tmp}/kb-install.XXXXXX") || die "cannot create a temporary directory"
trap 'rm -rf "$tmp"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

fetch "$(asset SHA256SUMS)" "$tmp/SHA256SUMS"
# The first target of this host with an archive in the release (static Linux builds first).
target=
for candidate in $targets; do
	found=$(awk -v suffix="-$candidate.tar.gz" -v version="$version" '{
		sub(/\r$/, ""); n = $2; sub(/^\*/, "", n)
		if (substr(n, 1, 3) == "kb-" && length(n) > length(suffix) + 3 &&
			substr(n, length(n) - length(suffix) + 1) == suffix) {
			v = substr(n, 4, length(n) - length(suffix) - 3)
			if (version == "" || v == version) { print v; exit }
		}
	}' "$tmp/SHA256SUMS")
	if [ -n "$found" ]; then
		version=$found
		target=$candidate
		break
	fi
done
[ -n "$target" ] || die "the release has no archive for this platform ($targets)"
top=kb-$version-$target
name=$top.tar.gz
expected=$(awk -v n="$name" '{ sub(/\r$/, "") } ($2 == n || $2 == "*" n) && length($1) == 64 { print tolower($1); c++ } END { exit c == 1 ? 0 : 1 }' "$tmp/SHA256SUMS") ||
	die "SHA256SUMS of release v$version does not list $name exactly once"

note "downloading $name"
fetch "$(asset "$name")" "$tmp/$name"
actual=$(sha256 "$tmp/$name")
[ "$actual" = "$expected" ] || die "sha256 mismatch for $name: expected $expected, got $actual"

# Only regular files and directories inside $top/, without '..' components.
tar -tzf "$tmp/$name" >"$tmp/names" 2>/dev/null || die "$name is not a gzip-compressed tar"
awk -v top="$top" '{ ok = ($0 == top || $0 == top "/" || substr($0, 1, length(top) + 1) == top "/");
	if (index("/" $0 "/", "/../") > 0) ok = 0; if (!ok) { print; exit 1 } }' "$tmp/names" >"$tmp/bad" ||
	die "unsafe archive entry: $(cat "$tmp/bad")"
tar -tvzf "$tmp/$name" | awk '{ t = substr($0, 1, 1); if (t != "-" && t != "d") { print; exit 1 } }' >"$tmp/bad" ||
	die "archive entry is not a regular file or directory: $(cat "$tmp/bad")"
mkdir "$tmp/x"
(cd "$tmp/x" && tar -xzof "../$name") || die "cannot extract $name"
new=$tmp/x/$top/kb
[ -f "$new" ] && [ ! -L "$new" ] || die "$top/kb is missing"
chmod 755 "$new"
reported=$("$new" --version 2>/dev/null) || die "the downloaded kb does not run on this system"
[ "$reported" = "kb $version" ] || die "the downloaded kb reports '$reported', expected 'kb $version'"

dest=$dir/kb
if [ -e "$dest" ] || [ -L "$dest" ]; then
	current=$("$dest" --version 2>/dev/null) || current=
	case $(head -c 2 "$dest" 2>/dev/null) in '#!') current= ;; esac
	case $current in
	kb\ *) ;;
	*)
		[ "$force" = 1 ] ||
			die "$dest exists and is not a kb binary (a wrapper script?); move it away or pass --force"
		;;
	esac
fi
mkdir -p "$dir" || die "cannot create $dir"
if ! { cp "$new" "$dir/.kb.new.$$" && chmod 755 "$dir/.kb.new.$$" && mv -f "$dir/.kb.new.$$" "$dest"; }; then
	rm -f "$dir/.kb.new.$$"
	die "cannot write $dest"
fi
note "installed kb $version ($target) as $dest"
case ":$PATH:" in
*":$dir:"*) ;;
*) note "add $dir to PATH, for example: export PATH=\"$dir:\$PATH\"" ;;
esac
note "update later with: kb self update"
