//! The runtime store: one directory per engine fingerprint, `<store>/<fingerprint>/` with
//! the `kb` binary and its `BUILD-INFO`. Runtimes are content-addressed, so one store serves
//! every checkout on the machine. A staged runtime is activated by renames only, so
//! `<fingerprint>/kb` resolves at every moment and running processes keep their binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use super::platform;
use crate::error::{ErrorCode, KbError, Result};

/// Directory of runtimes built by the retired `kbw` launcher inside a checkout.
pub const LEGACY_RUNTIME_DIR: &str = ".cache/runtime";

pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    /// `$KB_RUNTIME_DIR` (relative to the current directory), else `<data dir>/runtime`.
    pub fn locate(cwd: &Path) -> Result<Store> {
        if let Some(v) = std::env::var_os("KB_RUNTIME_DIR").filter(|v| !v.is_empty()) {
            return Ok(Store {
                dir: super::resolve::absolutize(cwd, Path::new(&v)),
            });
        }
        let dir = platform::data_dir().ok_or_else(|| {
            KbError::new(
                ErrorCode::ConfigInvalid,
                "cannot locate the runtime store: no home or local data directory is set",
            )
            .with_hint("set KB_RUNTIME_DIR to a writable directory")
        })?;
        Ok(Store {
            dir: dir.join("runtime"),
        })
    }

    pub fn runtime_dir(&self, fp: &str) -> PathBuf {
        self.dir.join(fp)
    }

    /// The runtime binary for `fp`, when it exists in the store.
    pub fn binary(&self, fp: &str) -> Option<PathBuf> {
        let p = self.runtime_dir(fp).join(platform::EXE);
        is_binary(&p).then_some(p)
    }

    /// A fresh private directory `<store>/.tmp.<name>.<pid>.<n>`, removed when dropped.
    pub fn temp(&self, name: &str) -> Result<TempDir> {
        TempDir::create_in(&self.dir, &format!(".tmp.{name}"))
    }

    /// Activate the runtime staged in `staged` (a directory with `kb` and `BUILD-INFO`) as
    /// `<store>/<fp>`. A new runtime directory is moved into place by one rename. An existing
    /// one keeps its directory and has `BUILD-INFO`, then `kb`, replaced by renames; with
    /// `replace == false` an existing complete runtime (a concurrent bootstrap) is kept.
    pub fn activate(&self, staged: &Path, fp: &str, replace: bool) -> Result<PathBuf> {
        let dest = self.runtime_dir(fp);
        if !replace && self.binary(fp).is_some() {
            return Ok(dest);
        }
        if fs::symlink_metadata(&dest).is_err() {
            match fs::rename(staged, &dest) {
                Ok(()) => return Ok(dest),
                // Appeared concurrently: fall through and replace its files (or keep it).
                Err(_) if fs::symlink_metadata(&dest).is_ok() => {
                    if !replace && self.binary(fp).is_some() {
                        return Ok(dest);
                    }
                }
                Err(e) => return Err(KbError::io(dest.display(), e)),
            }
        }
        replace_files(staged, &dest)?;
        Ok(dest)
    }
}

/// Replace the files of an existing runtime directory by renames: `BUILD-INFO` first, so an
/// interruption leaves the old binary described by the new `BUILD-INFO` of the same
/// fingerprint; a failed binary rename restores the previous `BUILD-INFO`.
fn replace_files(staged: &Path, dest: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(dest).map_err(|e| KbError::io(dest.display(), e))?;
    let exe = platform::EXE;
    if !meta.is_dir() || dest.join(exe).is_dir() || dest.join("BUILD-INFO").is_dir() {
        return Err(KbError::new(
            ErrorCode::IoError,
            format!(
                "{} is not a runtime directory ({exe} and BUILD-INFO files); remove it and retry",
                dest.display()
            ),
        ));
    }
    let io = |what: &Path, e| KbError::io(what.display(), e);
    let previous = fs::read(dest.join("BUILD-INFO")).ok();
    fs::rename(staged.join("BUILD-INFO"), dest.join("BUILD-INFO"))
        .map_err(|e| io(&dest.join("BUILD-INFO"), e))?;
    if let Err(e) = fs::rename(staged.join(exe), dest.join(exe)) {
        match previous {
            Some(bytes) => {
                let _ = fs::write(dest.join("BUILD-INFO"), bytes);
            }
            None => {
                let _ = fs::remove_file(dest.join("BUILD-INFO"));
            }
        }
        return Err(KbError::new(
            ErrorCode::IoError,
            format!(
                "cannot replace {}: {e} (is that runtime still running?)",
                dest.join(exe).display()
            ),
        ));
    }
    // Accompanying documents of an artifact (LICENSE, README.md).
    if let Ok(entries) = fs::read_dir(staged) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_file()) {
                let _ = fs::rename(entry.path(), dest.join(entry.file_name()));
            }
        }
    }
    Ok(())
}

/// The runtime a `kbw`-era checkout built for itself, `<root>/.cache/runtime/<fp>/kb`.
pub fn legacy_binary(root: &Path, fp: &str) -> Option<PathBuf> {
    let p = root.join(LEGACY_RUNTIME_DIR).join(fp).join(platform::EXE);
    is_binary(&p).then_some(p)
}

fn is_binary(p: &Path) -> bool {
    let Ok(meta) = fs::metadata(p) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    meta.is_file()
}

/// A temporary directory owned by this process; removed (best effort) when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    /// A fresh directory `<parent>/<prefix>.<pid>.<n>` (the parent is created).
    pub fn create_in(parent: &Path, prefix: &str) -> Result<TempDir> {
        fs::create_dir_all(parent).map_err(|e| KbError::io(parent.display(), e))?;
        for _ in 0..16 {
            let p = parent.join(format!(
                "{prefix}.{}.{:08x}",
                std::process::id(),
                crate::util::unique_suffix() as u32
            ));
            match fs::create_dir(&p) {
                Ok(()) => return Ok(TempDir(p)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(KbError::io(p.display(), e)),
            }
        }
        Err(KbError::new(
            ErrorCode::IoError,
            format!(
                "cannot create a temporary directory in {}",
                parent.display()
            ),
        ))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `BUILD-INFO`: `key=value` lines describing a runtime (engine_version, fingerprint,
/// target, source, rustc, built_from, and for artifacts artifact_sha256).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildInfo {
    pub lines: Vec<(String, String)>,
}

impl BuildInfo {
    pub fn parse(text: &str) -> BuildInfo {
        BuildInfo {
            lines: text
                .lines()
                .filter_map(|l| l.trim_end_matches('\r').split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    pub fn read(path: &Path) -> Option<BuildInfo> {
        let bytes = crate::util::read_file_limited(path, 64 * 1024).ok()?;
        Some(BuildInfo::parse(&String::from_utf8_lossy(&bytes)))
    }

    /// The single value of `key`; `None` when missing or repeated.
    pub fn get(&self, key: &str) -> Option<&str> {
        let mut it = self.lines.iter().filter(|(k, _)| k == key);
        let first = it.next()?;
        it.next().is_none().then_some(first.1.as_str())
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.lines.retain(|(k, _)| k != key);
        self.lines.push((key.to_string(), value.to_string()));
    }

    pub fn render(&self) -> String {
        self.lines
            .iter()
            .map(|(k, v)| format!("{k}={v}\n"))
            .collect()
    }
}

/// What a binary reports through `kb --json version`.
#[derive(Debug, Clone)]
pub struct Reported {
    pub engine_version: String,
    pub build_fingerprint: String,
}

/// Run `<bin> --root <empty dir> --json version` (never selecting another engine: the root
/// has no manifest) and return what it reports. `scratch` is an empty directory.
pub fn smoke(bin: &Path, scratch: &Path) -> std::result::Result<Reported, String> {
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("--root")
        .arg(scratch)
        .args(["--json", "version"])
        .current_dir(scratch)
        .env_remove("KB_ROOT");
    let limits = crate::process::Limits {
        timeout: Duration::from_secs(120),
        stdout: 1 << 20,
        stderr: 1 << 20,
    };
    let out = crate::process::capture(&mut cmd, b"", limits)
        .map_err(|e| format!("`kb --json version` could not run: {}", e.message))?;
    if out.exit_code != 0 {
        return Err(format!("`kb --json version` exited with {}", out.exit_code));
    }
    let v: Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| "`kb --json version` did not print a JSON envelope".to_string())?;
    let field = |k: &str| {
        v.pointer(&format!("/result/{k}"))
            .and_then(Value::as_str)
            .unwrap_or("(none)")
            .to_string()
    };
    if v.get("ok") != Some(&Value::Bool(true)) {
        return Err("`kb --json version` did not report ok".into());
    }
    Ok(Reported {
        engine_version: field("engine_version"),
        build_fingerprint: field("build_fingerprint"),
    })
}

/// Smoke-test `bin` and require `version` and `fp`.
pub fn smoke_expect(
    bin: &Path,
    scratch: &Path,
    version: &str,
    fp: &str,
) -> std::result::Result<(), String> {
    let r = smoke(bin, scratch)?;
    if r.engine_version == version && r.build_fingerprint == fp {
        Ok(())
    } else {
        Err(format!(
            "it reports engine {} and build fingerprint {}, expected engine {version} and build fingerprint {fp}",
            r.engine_version, r.build_fingerprint
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_info_values_are_single() {
        let mut b = BuildInfo::parse("engine_version=1.0\r\nsource=a\nsource=b\nbad\n");
        assert_eq!(b.get("engine_version"), Some("1.0"));
        assert_eq!(b.get("source"), None);
        b.set("source", "artifact");
        assert_eq!(b.get("source"), Some("artifact"));
        assert_eq!(b.render(), "engine_version=1.0\nsource=artifact\n");
    }

    #[test]
    fn activation_renames_new_runtimes_and_replaces_files_of_existing_ones() {
        let t = tempfile::tempdir().unwrap();
        let store = Store {
            dir: t.path().join("store"),
        };
        let stage = |label: &str| {
            let tmp = store.temp("t").unwrap();
            fs::write(tmp.path().join(platform::EXE), label).unwrap();
            fs::write(tmp.path().join("BUILD-INFO"), format!("source={label}\n")).unwrap();
            tmp
        };
        let fp = "a".repeat(64);
        let first = stage("one");
        store.activate(first.path(), &fp, false).unwrap();
        let dest = store.runtime_dir(&fp);
        assert_eq!(fs::read_to_string(dest.join(platform::EXE)).unwrap(), "one");
        // A reinstall replaces the files inside the existing directory.
        let second = stage("two");
        store.activate(second.path(), &fp, true).unwrap();
        assert_eq!(fs::read_to_string(dest.join(platform::EXE)).unwrap(), "two");
        assert_eq!(
            fs::read_to_string(dest.join("BUILD-INFO")).unwrap(),
            "source=two\n"
        );
        drop((first, second));
        let leftovers: Vec<_> = fs::read_dir(&store.dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(leftovers, vec![fp]);
    }
}
