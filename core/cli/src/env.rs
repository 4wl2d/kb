//! Runtime environment: KB root discovery, cache location, stderr progress.

use std::path::{Path, PathBuf};

use crate::error::{ErrorCode, KbError, Result};
use crate::versions::{MANIFEST_PATH, ReleaseManifest};

#[derive(Debug, Clone)]
pub struct Env {
    /// KB checkout root (contains core/release.toml).
    pub kb_root: PathBuf,
    /// Current working directory of the invocation.
    pub cwd: PathBuf,
    /// Cache directory (`$KB_CACHE_DIR` or `<kb_root>/.cache`).
    pub cache_dir: PathBuf,
    pub quiet: bool,
    /// The local engine manifest.
    pub manifest: ReleaseManifest,
}

impl Env {
    /// Discover the KB root from `--root`, `$KB_ROOT`, or the nearest ancestor of `cwd`
    /// containing `core/release.toml`.
    pub fn discover(root: Option<&Path>, quiet: bool) -> Result<Env> {
        let cwd = std::env::current_dir().map_err(|e| KbError::io("current directory", e))?;
        let candidate = match root {
            Some(r) => Some(absolutize(&cwd, r)),
            None => match std::env::var_os("KB_ROOT") {
                Some(v) if !v.is_empty() => Some(absolutize(&cwd, Path::new(&v))),
                _ => cwd
                    .ancestors()
                    .find(|a| a.join(MANIFEST_PATH).is_file())
                    .map(Path::to_path_buf),
            },
        };
        let kb_root = candidate.ok_or_else(|| {
            KbError::new(
                ErrorCode::ConfigInvalid,
                "cannot find the KB root (no core/release.toml in this directory or its ancestors)",
            )
            .with_hint("run the project launcher (e.g. `.kb/kbw`) or pass --root")
        })?;
        let kb_root = kb_root
            .canonicalize()
            .map_err(|e| KbError::io(kb_root.display(), e))?;
        let manifest_path = kb_root.join(MANIFEST_PATH);
        let text = std::fs::read_to_string(&manifest_path).map_err(|e| {
            KbError::new(
                ErrorCode::ConfigInvalid,
                format!("cannot read {}: {e}", manifest_path.display()),
            )
        })?;
        let manifest = ReleaseManifest::parse(&text)?;
        let cache_dir = match std::env::var_os("KB_CACHE_DIR") {
            Some(v) if !v.is_empty() => absolutize(&cwd, Path::new(&v)),
            _ => kb_root.join(".cache"),
        };
        Ok(Env {
            kb_root,
            cwd,
            cache_dir,
            quiet,
            manifest,
        })
    }

    /// Progress line on stderr (never stdout).
    pub fn progress(&self, msg: impl AsRef<str>) {
        if !self.quiet {
            eprintln!("kb: {}", msg.as_ref());
        }
    }
}

fn absolutize(cwd: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}
