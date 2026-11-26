//! Source builds of a checkout's engine: `cargo build --release --locked -p kb` with the
//! pinned toolchain, in the checkout, into a Cargo target directory.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::platform;
use crate::error::{ErrorCode, KbError, Result};

/// `rustc -vV` as run in the checkout (where rustup honors `rust-toolchain.toml`).
#[derive(Debug, Clone, Default)]
pub struct Rustc {
    /// First line, e.g. `rustc 1.98.1 (...)`.
    pub version_line: String,
    /// `release:` value, e.g. `1.98.1`.
    pub release: String,
    /// `host:` value, e.g. `aarch64-apple-darwin`.
    pub host: String,
}

pub fn rustc(root: &Path) -> Option<Rustc> {
    let out = Command::new("rustc")
        .arg("-vV")
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key))
            .map(|v| v.trim().to_string())
            .unwrap_or_default()
    };
    Some(Rustc {
        version_line: text.lines().next().unwrap_or_default().to_string(),
        release: field("release:"),
        host: field("host:"),
    })
}

/// Fail unless rustc is the pinned toolchain (`rust_toolchain` in core/release.toml). A
/// channel name instead of a version can only be honored through rustup. With
/// `KB_ALLOW_TOOLCHAIN_MISMATCH=1` a mismatch is a warning.
pub fn check_toolchain(root: &Path, pinned: &str, quiet: bool) -> Result<Rustc> {
    let found = rustc(root);
    let numeric =
        pinned.split('.').count() >= 2 && pinned.starts_with(|c: char| c.is_ascii_digit());
    if let Some(r) = &found
        && (r.release == pinned || (!numeric && which("rustup")))
    {
        return Ok(r.clone());
    }
    let have = found.as_ref().map_or("(not found)", |r| r.release.as_str());
    let msg = format!(
        "rustc {have} is not the pinned toolchain {pinned} (core/release.toml rust_toolchain); \
         install it with rustup (it reads rust-toolchain.toml), or set \
         KB_ALLOW_TOOLCHAIN_MISMATCH=1 to build anyway"
    );
    if std::env::var("KB_ALLOW_TOOLCHAIN_MISMATCH").as_deref() == Ok("1") {
        if !quiet {
            eprintln!("kb: warning[TOOLCHAIN_MISMATCH]: {msg}");
        }
        return Ok(found.unwrap_or_default());
    }
    Err(KbError::new(ErrorCode::ToolchainMismatch, msg))
}

pub fn which(program: &str) -> bool {
    crate::tools::search_path(program).is_some()
}

/// `$KB_CARGO_TARGET_DIR` (relative to the current directory), else
/// `<root>/.cache/cargo-target`.
pub fn target_dir(cwd: &Path, root: &Path) -> PathBuf {
    match std::env::var_os("KB_CARGO_TARGET_DIR").filter(|v| !v.is_empty()) {
        Some(v) => super::resolve::absolutize(cwd, Path::new(&v)),
        None => root.join(".cache/cargo-target"),
    }
}

/// Build the engine of `root` (fingerprint `fp`) and return the binary's path. Cargo output
/// goes to stderr. `target` selects a Rust target other than rustc's host.
pub fn build(root: &Path, fp: &str, target: Option<&str>, target_dir: &Path) -> Result<PathBuf> {
    if !which("cargo") {
        return Err(KbError::new(
            ErrorCode::BootstrapFailed,
            "cargo is not on PATH",
        )
        .with_hint("install Rust (rustup reads rust-toolchain.toml), or install the release runtime with `kb engine install`"));
    }
    let mut cmd = Command::new("cargo");
    cmd.args([
        "build",
        "--release",
        "--locked",
        "-p",
        "kb",
        "--manifest-path",
    ])
    .arg(root.join("Cargo.toml"))
    .current_dir(root)
    .env("CARGO_TARGET_DIR", target_dir)
    .env_remove("CARGO_BUILD_TARGET")
    // kbw-era engines compile the fingerprint in from this variable; newer ones compute
    // it in build.rs.
    .env("KBW_BUILD_FINGERPRINT", fp)
    .stdin(Stdio::null())
    .stdout(std::io::stderr());
    if let Some(t) = target {
        cmd.args(["--target", t]);
    }
    if cfg!(target_os = "macos")
        && std::env::var_os("SDKROOT").is_none()
        && let Ok(out) = Command::new("xcrun").arg("--show-sdk-path").output()
        && out.status.success()
    {
        let sdk = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !sdk.is_empty() {
            cmd.env("SDKROOT", sdk);
        }
    }
    let status = cmd
        .status()
        .map_err(|e| KbError::new(ErrorCode::BootstrapFailed, format!("cannot run cargo: {e}")))?;
    if !status.success() {
        return Err(KbError::new(
            ErrorCode::BootstrapFailed,
            "cargo build --release --locked -p kb failed; any previously built runtime is unchanged",
        ));
    }
    let exe = target.map_or(platform::EXE, platform::exe_for);
    let mut bin = target_dir.to_path_buf();
    if let Some(t) = target {
        bin.push(t);
    }
    bin.push("release");
    bin.push(exe);
    if !bin.is_file() {
        return Err(KbError::new(
            ErrorCode::BootstrapFailed,
            format!("cargo did not produce {}", bin.display()),
        ));
    }
    Ok(bin)
}

/// `git:<commit>` of the checkout when it is the top level of a Git work tree.
pub fn built_from(root: &Path) -> String {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let top = git(&["rev-parse", "--show-toplevel"])
        .and_then(|t| crate::util::canonicalize(Path::new(&t)).ok());
    if top.is_some()
        && top == crate::util::canonicalize(root).ok()
        && let Some(rev) = git(&["rev-parse", "--verify", "-q", "HEAD^{commit}"])
    {
        return format!("git:{rev}");
    }
    "unknown".into()
}
