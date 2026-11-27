//! Engine fingerprint of a KB checkout (docs/architecture.md, section 9).
//!
//! The fingerprint is the SHA-256 over the byte-sorted lines `<sha256>  <path>\n`, one per
//! engine build input: `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`,
//! `core/release.toml`, `core/cli/Cargo.toml`, `core/cli/build.rs` (when present) and every
//! regular file under `core/cli/src`. Paths are root-relative with `/` separators; files whose
//! name starts with `.` or ends with `~` are ignored, symlinks are never followed, and CRLF
//! line endings are hashed as LF, so a Windows checkout hashes like a Unix one. For a tree
//! with LF line endings and no `build.rs` this is exactly what the retired `kbw` launcher
//! computed, so runtimes it built keep their names.
//!
//! `build.rs` includes this file to compile the fingerprint of the sources being built into
//! the binary (`kb --json version`: `build_fingerprint`), so it may use only `std` and `sha2`.

use std::fs;
use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

/// Single-file build inputs that must exist.
pub const REQUIRED_FILES: [&str; 5] = [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "core/release.toml",
    "core/cli/Cargo.toml",
];

/// Build script of the engine crate; an input when present (kbw-era engines have none).
pub const BUILD_SCRIPT: &str = "core/cli/build.rs";

/// Directory trees whose regular files are build inputs.
pub const SOURCE_TREES: [&str; 1] = ["core/cli/src"];

/// Lowercase hex SHA-256 of `bytes`, hashing CRLF as LF.
fn content_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    let mut rest = bytes;
    while let Some(i) = rest.windows(2).position(|w| w == b"\r\n") {
        hasher.update(&rest[..i]);
        hasher.update(b"\n");
        rest = &rest[i + 2..];
    }
    hasher.update(rest);
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[usize::from(b >> 4)] as char);
        s.push(DIGITS[usize::from(b & 15)] as char);
    }
    s
}

fn ignored_name(name: &str) -> bool {
    name.starts_with('.') || name.ends_with('~')
}

/// The build inputs of `root` as root-relative `/` paths of regular files, unsorted.
pub fn input_paths(root: &Path) -> io::Result<Vec<String>> {
    let mut out = Vec::new();
    for rel in REQUIRED_FILES {
        let meta = fs::symlink_metadata(root.join(rel))?;
        if meta.is_file() {
            out.push(rel.to_string());
        }
    }
    if fs::symlink_metadata(root.join(BUILD_SCRIPT)).is_ok_and(|m| m.is_file()) {
        out.push(BUILD_SCRIPT.to_string());
    }
    for tree in SOURCE_TREES {
        let meta = fs::symlink_metadata(root.join(tree))?;
        if meta.is_dir() {
            walk(root, tree, &mut out)?;
        }
    }
    Ok(out)
}

fn walk(root: &Path, rel: &str, out: &mut Vec<String>) -> io::Result<()> {
    for entry in fs::read_dir(root.join(rel))? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("non-UTF-8 file name under {rel}"),
            ));
        };
        let kind = entry.file_type()?;
        let child = format!("{rel}/{name}");
        if kind.is_dir() {
            walk(root, &child, out)?;
        } else if kind.is_file() && !ignored_name(name) {
            out.push(child);
        }
    }
    Ok(())
}

/// The fingerprint of the engine build inputs under `root`.
pub fn compute(root: &Path) -> io::Result<String> {
    digest(root, &input_paths(root)?)
}

/// The fingerprint over the given input paths (as listed by [`input_paths`]).
pub fn digest(root: &Path, paths: &[String]) -> io::Result<String> {
    let mut lines = Vec::with_capacity(paths.len());
    for rel in paths {
        let bytes = fs::read(root.join(rel))?;
        lines.push(format!("{}  {rel}\n", content_digest(&bytes)));
    }
    lines.sort_unstable();
    let mut hasher = Sha256::new();
    for line in &lines {
        hasher.update(line.as_bytes());
    }
    Ok(hex(&hasher.finalize()))
}
