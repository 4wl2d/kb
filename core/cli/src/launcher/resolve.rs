//! Which KB checkout an invocation belongs to.
//!
//! `--root <dir>`, else `$KB_ROOT`, else the nearest directory from the current directory
//! upwards (then from `--host <dir>` upwards) that either is a KB checkout (has
//! `core/release.toml`) or mounts one at `.kb` (has `.kb/core/release.toml`, the
//! conventional submodule path).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::versions::MANIFEST_PATH;

/// Conventional host-relative path of a mounted KB checkout.
pub const MOUNT: &str = ".kb";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootSource {
    /// `--root`.
    Flag,
    /// `$KB_ROOT`.
    Env,
    /// A directory at or above the start that is a KB checkout.
    Checkout,
    /// `.kb` in a directory at or above the start.
    Mount,
}

impl RootSource {
    pub fn as_str(self) -> &'static str {
        match self {
            RootSource::Flag => "--root",
            RootSource::Env => "KB_ROOT",
            RootSource::Checkout => "checkout",
            RootSource::Mount => "mount",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Root {
    /// Absolute, not canonicalized.
    pub dir: PathBuf,
    pub source: RootSource,
}

impl Root {
    /// Whether the directory holds an engine manifest.
    pub fn has_manifest(&self) -> bool {
        self.dir.join(MANIFEST_PATH).is_file()
    }
}

/// `$KB_ROOT` when set and non-empty.
pub fn env_root() -> Option<OsString> {
    std::env::var_os("KB_ROOT").filter(|v| !v.is_empty())
}

/// Find the KB root. `flag` is `--root`, `env` is [`env_root`], `start` is `--host`, searched
/// when nothing is found from the current directory.
pub fn find(
    cwd: &Path,
    flag: Option<&Path>,
    env: Option<&Path>,
    start: Option<&Path>,
) -> Option<Root> {
    if let Some(dir) = flag {
        return Some(Root {
            dir: absolutize(cwd, dir),
            source: RootSource::Flag,
        });
    }
    if let Some(dir) = env {
        return Some(Root {
            dir: absolutize(cwd, dir),
            source: RootSource::Env,
        });
    }
    let host = start.map(|s| absolutize(cwd, s));
    nearest(cwd).or_else(|| host.as_deref().and_then(nearest))
}

/// The nearest directory at or above `start` that is a KB checkout or mounts one at `.kb`.
fn nearest(start: &Path) -> Option<Root> {
    for dir in start.ancestors() {
        if dir.join(MANIFEST_PATH).is_file() {
            return Some(Root {
                dir: dir.to_path_buf(),
                source: RootSource::Checkout,
            });
        }
        let mounted = dir.join(MOUNT);
        if mounted.join(MANIFEST_PATH).is_file() {
            return Some(Root {
                dir: mounted,
                source: RootSource::Mount,
            });
        }
    }
    None
}

pub fn absolutize(cwd: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkout(dir: &Path) {
        std::fs::create_dir_all(dir.join("core")).unwrap();
        std::fs::write(dir.join(MANIFEST_PATH), "manifest = 1\n").unwrap();
    }

    #[test]
    fn nearest_checkout_or_mount_wins() {
        let t = tempfile::tempdir().unwrap();
        let host = t.path().join("host");
        let deep = host.join("app/src");
        std::fs::create_dir_all(&deep).unwrap();
        assert!(find(&deep, None, None, None).is_none_or(|r| !r.dir.starts_with(&host)));
        checkout(&host.join(".kb"));
        let r = find(&deep, None, None, None).unwrap();
        assert_eq!(r.dir, host.join(".kb"));
        assert_eq!(r.source, RootSource::Mount);
        // Inside the mounted checkout, the checkout itself is nearest.
        let inside = host.join(".kb/core");
        let r = find(&inside, None, None, None).unwrap();
        assert_eq!(r.dir, host.join(".kb"));
        assert_eq!(r.source, RootSource::Checkout);
        // Without a checkout above the current directory, --host is searched; --root wins.
        let other = t.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        assert_eq!(
            find(&other, None, None, Some(&deep)).unwrap().dir,
            host.join(".kb")
        );
        let r = find(&deep, Some(Path::new("rel")), Some(Path::new("env")), None).unwrap();
        assert_eq!((r.dir, r.source), (deep.join("rel"), RootSource::Flag));
    }
}
