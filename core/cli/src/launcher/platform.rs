//! Platform facts for runtimes: executable name, target triples, the user data directory,
//! and running another engine binary in place of this process.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// File name of a `kb` binary on this platform.
pub const EXE: &str = if cfg!(windows) { "kb.exe" } else { "kb" };

/// File name of a `kb` binary built for `target`.
pub fn exe_for(target: &str) -> &'static str {
    if target.contains("-windows-") {
        "kb.exe"
    } else {
        "kb"
    }
}

/// The target triple this binary was compiled for.
pub const BUILD_TARGET: &str = env!("KB_BUILD_TARGET");

/// Release archive targets that run on this host, preferred first. Linux releases are
/// static (musl), so a glibc build accepts them too; release archives exist only for the
/// targets the release workflow builds.
pub fn release_targets() -> Vec<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => vec!["aarch64-apple-darwin"],
        ("macos", "x86_64") => vec!["x86_64-apple-darwin"],
        ("linux", "x86_64") => vec!["x86_64-unknown-linux-musl", "x86_64-unknown-linux-gnu"],
        ("linux", "aarch64") => vec!["aarch64-unknown-linux-musl", "aarch64-unknown-linux-gnu"],
        ("windows", "x86_64") => vec!["x86_64-pc-windows-msvc"],
        ("windows", "aarch64") => vec!["aarch64-pc-windows-msvc", "x86_64-pc-windows-msvc"],
        _ => Vec::new(),
    }
}

/// Per-user data directory of kb: `%LOCALAPPDATA%\kb` on Windows, `$XDG_DATA_HOME/kb` when
/// set (absolute), else `~/Library/Application Support/kb` on macOS and `~/.local/share/kb`
/// elsewhere.
pub fn data_dir() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    if cfg!(windows) {
        return var("LOCALAPPDATA").map(|d| d.join("kb"));
    }
    if let Some(xdg) = var("XDG_DATA_HOME") {
        return Some(xdg.join("kb"));
    }
    let home = var("HOME")?;
    Some(if cfg!(target_os = "macos") {
        home.join("Library/Application Support/kb")
    } else {
        home.join(".local/share/kb")
    })
}

/// Run `binary` with `args` in place of this process and return its exit status. On Unix
/// the process image is replaced (`exec`), so this returns only when that fails. On Windows
/// the child runs with inherited standard streams while this process waits; Ctrl+C reaches
/// the child (both share the console) and this process outlives it to pass on its status.
pub fn run_in_place(
    binary: &Path,
    args: &[OsString],
    envs: &[(&str, &std::ffi::OsStr)],
) -> std::io::Result<i32> {
    let mut cmd = Command::new(binary);
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(cmd.exec())
    }
    #[cfg(not(unix))]
    {
        #[cfg(windows)]
        ignore_console_interrupts();
        let status = cmd.status()?;
        Ok(status.code().unwrap_or(1))
    }
}

/// Keep this process alive on Ctrl+C/Ctrl+Break while it waits for the engine it started:
/// the child receives the event itself. A handler routine (unlike a null handler) is not
/// inherited, so the child keeps the default behavior.
#[cfg(windows)]
fn ignore_console_interrupts() {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    unsafe extern "system" fn handled(_ctrl_type: u32) -> windows_sys::core::BOOL {
        1
    }
    // SAFETY: registers a handler with the documented signature that touches no state.
    unsafe {
        SetConsoleCtrlHandler(Some(handled), 1);
    }
}
