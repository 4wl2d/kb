//! The isolated bare Git mirror of a KB source (`<cache>/git/<id>.git`).
//!
//! The mirror is the only repository kb writes refs into. It holds the last fetched approved
//! tip (`refs/kb/approved`) plus local KB commits copied from the checkout (pins, local
//! `HEAD`) so that ancestry checks and tree reads never touch the developer's checkout.
//! Every command runs without hooks and without automatic gc/maintenance.

use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{ErrorCode, KbError, Result};
use crate::git::{Git, GitOutput, git_failure, transport_policy};

/// Last approved tip fetched from the remote.
pub(crate) const APPROVED_REF: &str = "refs/kb/approved";
/// Last local KB `HEAD` copied from the checkout.
pub(crate) const LOCAL_HEAD_REF: &str = "refs/kb/local/head";

/// Settings applied to every mutating mirror command: no hooks, no automatic maintenance.
const MIRROR_CONFIG: [&str; 5] = [
    "core.hooksPath=/dev/null",
    "gc.auto=0",
    "maintenance.auto=false",
    "fetch.prune=false",
    "fetch.writeCommitGraph=false",
];

/// A usable bare mirror.
#[derive(Debug, Clone)]
pub(crate) struct Mirror {
    git: Git,
}

/// Where to fetch a missing commit from over the network (remote URL + transport policy).
pub(crate) struct RemoteFetch<'a> {
    pub url: &'a str,
    pub allowed_protocols: &'a [String],
}

impl Mirror {
    /// Open the mirror `<cache_dir>/git/<id>.git`, creating it when missing. An unusable
    /// directory at that path is moved aside and replaced by a fresh mirror.
    pub fn open(cache_dir: &Path, id: &str) -> Result<Mirror> {
        let dir = cache_dir.join("git");
        fs::create_dir_all(&dir).map_err(|e| KbError::io(dir.display(), e))?;
        let path = dir.join(format!("{id}.git"));
        match fs::symlink_metadata(&path) {
            Ok(md) if md.is_dir() && usable(&path) => return Ok(Mirror::at(path)),
            Ok(_) => {
                let aside = dir.join(format!("{id}.git.unusable-{}", unique()));
                fs::rename(&path, &aside).map_err(|e| KbError::io(path.display(), e))?;
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(KbError::io(path.display(), e)),
        }
        create(&dir, &path, id)?;
        Ok(Mirror::at(path))
    }

    fn at(path: PathBuf) -> Mirror {
        Mirror {
            git: Git::bare(path),
        }
    }

    /// Read-only Git handle for the mirror.
    pub fn git(&self) -> &Git {
        &self.git
    }

    /// `git fetch <url> <refspec>` under an explicit transport policy. Retries briefly when
    /// another kb process holds a ref lock in the same mirror.
    fn fetch(&self, url: &str, refspec: &str, allowed_protocols: &[String]) -> Result<GitOutput> {
        let mut cfg = transport_policy(allowed_protocols)?;
        cfg.extend(MIRROR_CONFIG.iter().map(|s| s.to_string()));
        let args = [
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-recurse-submodules",
            "--no-write-fetch-head",
            "--end-of-options",
            url,
            refspec,
        ];
        let mut attempt = 0u64;
        loop {
            attempt += 1;
            let out = self.git.output_with(&args, &cfg)?;
            if out.ok() || attempt >= 3 || !lock_contention(&out.stderr) {
                return Ok(out);
            }
            std::thread::sleep(Duration::from_millis(50 * attempt));
        }
    }

    /// Fetch the approved ref from the remote into [`APPROVED_REF`].
    pub fn fetch_approved(
        &self,
        url: &str,
        approved_ref: &str,
        allowed_protocols: &[String],
    ) -> Result<GitOutput> {
        self.fetch(
            url,
            &format!("+{approved_ref}:{APPROVED_REF}"),
            allowed_protocols,
        )
    }

    /// The last fetched approved tip.
    pub fn approved_tip(&self) -> Result<Option<String>> {
        self.git.resolve_commit(APPROVED_REF)
    }

    /// Resolve `rev` to a commit present in the mirror.
    pub fn commit(&self, rev: &str) -> Result<Option<String>> {
        self.git.resolve_commit(rev)
    }

    /// Make the full commit id `sha` available in the mirror, keeping it reachable from
    /// `keep_as`. Sources, in order: the mirror itself, the local KB checkout (`local`,
    /// file transport only), then the remote (`remote`, under its transport policy).
    pub fn ensure_commit(
        &self,
        sha: &str,
        keep_as: &str,
        local: Option<&Path>,
        remote: Option<&RemoteFetch<'_>>,
    ) -> Result<bool> {
        if self.commit(sha)?.is_some() {
            return Ok(true);
        }
        if !is_full_oid(sha) {
            return Ok(false);
        }
        let refspec = format!("+{sha}:{keep_as}");
        if let Some(root) = local.and_then(Path::to_str)
            && self.fetch(root, &refspec, &["file".to_string()])?.ok()
            && self.commit(sha)?.is_some()
        {
            return Ok(true);
        }
        if let Some(r) = remote
            && self.fetch(r.url, &refspec, r.allowed_protocols)?.ok()
        {
            return Ok(self.commit(sha)?.is_some());
        }
        Ok(false)
    }
}

/// Is `s` a full hexadecimal object id (SHA-1 or SHA-256)?
pub(crate) fn is_full_oid(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `git merge-base a b` in `git` (`None` for unrelated histories).
pub(crate) fn merge_base(git: &Git, a: &str, b: &str) -> Result<Option<String>> {
    let o = git.output(&["merge-base", "--end-of-options", a, b])?;
    match o.code {
        0 => Ok(Some(o.stdout_str().trim().to_string())),
        1 => Ok(None),
        _ => Err(git_failure(&["merge-base"], &o)),
    }
}

/// Commits only in `a` and only in `b` (`git rev-list --left-right --count a...b`).
pub(crate) fn ahead_behind(git: &Git, a: &str, b: &str) -> Result<(u64, u64)> {
    let range = format!("{a}...{b}");
    let out = git.run(&[
        "rev-list",
        "--left-right",
        "--count",
        "--end-of-options",
        &range,
    ])?;
    let mut it = out.split_ascii_whitespace().map(str::parse::<u64>);
    match (it.next(), it.next()) {
        (Some(Ok(l)), Some(Ok(r))) => Ok((l, r)),
        _ => Err(KbError::new(
            ErrorCode::GitError,
            format!("unexpected rev-list output `{out}`"),
        )),
    }
}

/// Paths under `prefixes` that differ between commits `a` and `b`.
pub(crate) fn changed_paths(
    git: &Git,
    a: &str,
    b: &str,
    prefixes: &[String],
) -> Result<BTreeSet<String>> {
    let mut args = vec![
        "--literal-pathspecs",
        "diff",
        "--name-only",
        "-z",
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
        a,
        b,
        "--",
    ];
    args.extend(prefixes.iter().map(String::as_str));
    let out = git.run_bytes(&args)?;
    Ok(out
        .split(|c| *c == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect())
}

fn usable(path: &Path) -> bool {
    Git::bare(path)
        .output(&["rev-parse", "--is-bare-repository"])
        .is_ok_and(|o| o.ok() && o.stdout_str().trim() == "true")
}

/// Initialize a mirror in a temporary sibling and move it into place atomically.
fn create(dir: &Path, path: &Path, id: &str) -> Result<()> {
    let tmp = dir.join(format!(".{id}.tmp-{}", unique()));
    let tmp_str = tmp.to_str().ok_or_else(|| {
        KbError::new(
            ErrorCode::IoError,
            format!("cache path `{}` is not UTF-8", tmp.display()),
        )
    })?;
    // `--template=` copies no hooks or sample files into the mirror.
    Git::new(dir).run(&["init", "--bare", "--quiet", "--template=", tmp_str])?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Another kb process may have created the mirror concurrently.
            let _ = fs::remove_dir_all(&tmp);
            if usable(path) {
                Ok(())
            } else {
                Err(KbError::io(path.display(), e))
            }
        }
    }
}

fn lock_contention(stderr: &str) -> bool {
    stderr.contains(".lock") || stderr.contains("cannot lock ref")
}

/// Process-unique suffix for temporary names.
fn unique() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "{}-{nanos}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_oids() {
        assert!(is_full_oid("9e593dbdd9640b37665992cc31fedb00ad6c7f22"));
        assert!(!is_full_oid("9e593db"));
        assert!(!is_full_oid("HEAD"));
    }
}
