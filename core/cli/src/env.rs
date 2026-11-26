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
    /// Discover the KB root: `--root`, else `$KB_ROOT`, else the nearest directory from the
    /// current directory (then from `--host`) upwards that has `core/release.toml` or mounts
    /// a KB checkout at `.kb` (see [`crate::launcher::resolve`]).
    pub fn discover(root: Option<&Path>, host: Option<&Path>, quiet: bool) -> Result<Env> {
        let cwd = std::env::current_dir().map_err(|e| KbError::io("current directory", e))?;
        let env_root = crate::launcher::resolve::env_root();
        let found =
            crate::launcher::resolve::find(&cwd, root, env_root.as_deref().map(Path::new), host);
        let kb_root = found.map(|f| f.dir).ok_or_else(|| {
            KbError::new(
                ErrorCode::ConfigInvalid,
                "cannot find the KB root (no core/release.toml or .kb/core/release.toml in this directory or above)",
            )
            .with_hint("run kb inside a KB checkout or a host repository that mounts one at .kb, or pass --root <dir>")
        })?;
        let kb_root =
            crate::util::canonicalize(&kb_root).map_err(|e| KbError::io(kb_root.display(), e))?;
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
