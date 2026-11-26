//! Compiles the engine fingerprint of the sources being built into the binary
//! (`KB_BUILD_FINGERPRINT`, reported by `kb --json version` as `build_fingerprint`). `kb`
//! accepts a binary as the runtime of a checkout only when it reports that checkout's
//! fingerprint (docs/architecture.md, section 9).

use std::path::PathBuf;

#[allow(dead_code)]
#[path = "src/launcher/fingerprint.rs"]
mod fingerprint;

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let root = manifest_dir.join("../..");
    for rel in fingerprint::REQUIRED_FILES {
        println!("cargo:rerun-if-changed={}", root.join(rel).display());
    }
    println!("cargo:rerun-if-changed=build.rs");
    for tree in fingerprint::SOURCE_TREES {
        println!("cargo:rerun-if-changed={}", root.join(tree).display());
    }
    // Outside a full checkout (no workspace files) the build has no fingerprint.
    let fp = fingerprint::compute(&root).unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=KB_BUILD_FINGERPRINT={fp}");
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=KB_BUILD_TARGET={target}");
}
