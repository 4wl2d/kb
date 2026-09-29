//! Proposal overlay: local knowledge changes relative to `merge-base(HEAD, approved)`.
//!
//! The overlay covers the profile's content prefixes (config, registry, knowledge roots) and
//! includes committed-but-unapproved, staged, unstaged and untracked changes: tracked
//! changes come from `git diff <base>` against the working tree, untracked files from
//! `git ls-files --others --exclude-standard`. Contents are read from the working tree
//! (never through symlinks). The KB checkout is only read; the merge-base is computed in the
//! isolated mirror after copying the local `HEAD` into it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;

use crate::env::Env;
use crate::error::{ErrorCode, KbError, Result};
use crate::git::Git;
use crate::knowledge::{Overlay, OverlayFile, OverlayStatus};
use crate::model::{ProfileConfig, ProfileLocation};
use crate::snapshot::mirror::{self, LOCAL_HEAD_REF, Mirror};
use crate::snapshot::{ApprovedSource, kb_git};
use crate::source::MAX_SOURCE_FILE_BYTES;
use crate::util::{FieldHasher, check_rel_path, read_file_limited, safe_join, sha256_hex};

/// Compute the overlay for the KB checkout at `env.kb_root`.
///
/// `approved` is the approved tip known to the caller (a commit in the KB mirror or the
/// checkout). Without it the base is the local `HEAD` and nothing is marked stale.
pub fn compute(
    env: &Env,
    loc: &ProfileLocation,
    cfg: &ProfileConfig,
    approved: Option<&str>,
) -> Result<Overlay> {
    let kb = kb_git(env);
    let mirror = match (approved, &kb) {
        (Some(_), Some(k)) => ApprovedSource::configured(env, Some(k), cfg)?.mirror(env)?,
        _ => None,
    };
    compute_with(env, loc, cfg, approved, kb.as_ref(), mirror.as_ref())
}

/// [`compute`] with an already opened checkout and mirror.
pub(crate) fn compute_with(
    env: &Env,
    loc: &ProfileLocation,
    cfg: &ProfileConfig,
    approved: Option<&str>,
    kb: Option<&Git>,
    mirror: Option<&Mirror>,
) -> Result<Overlay> {
    let kb = kb.ok_or_else(|| {
        KbError::new(
            ErrorCode::GitError,
            "proposals need the KB root to be a Git checkout",
        )
    })?;
    let prefixes = loc.content_prefixes(cfg);
    let head = kb.resolve_commit("HEAD")?;
    let (base, history) = base_and_history(env, kb, mirror, head.as_deref(), approved)?;

    let mut changes = match base.as_deref() {
        Some(b) => tracked_changes(kb, b, &prefixes)?,
        // No commit yet: everything in the index is new.
        None => list_files(kb, "--cached", &prefixes)?
            .into_iter()
            .map(|p| (p, OverlayStatus::Added))
            .collect(),
    };
    for path in list_files(kb, "--others", &prefixes)? {
        let status = match changes.get(&path) {
            Some(OverlayStatus::Deleted) => OverlayStatus::Modified,
            Some(s) => *s,
            None => OverlayStatus::Added,
        };
        changes.insert(path, status);
    }

    let upstream = match (base.as_deref(), approved, &history) {
        (Some(b), Some(a), Some(g)) if b != a => mirror::changed_paths(g, b, a, &prefixes)?,
        _ => BTreeSet::new(),
    };

    let mut files = Vec::with_capacity(changes.len());
    for (path, status) in changes {
        check_rel_path(&path).map_err(KbError::unsafe_path)?;
        let content = match status {
            OverlayStatus::Deleted => None,
            _ => read_work_file(env, &path)?,
        };
        let status = match (status, &content) {
            (_, Some(_)) | (OverlayStatus::Deleted, None) => status,
            // Vanished between listing and reading.
            (OverlayStatus::Added, None) => continue,
            (OverlayStatus::Modified, None) => OverlayStatus::Deleted,
        };
        let content_id = content
            .as_ref()
            .map(|c| format!("sha256:{}", sha256_hex(c)));
        files.push(OverlayFile {
            stale: upstream.contains(&path),
            path,
            status,
            content,
            content_id,
        });
    }
    let digest = digest(base.as_deref(), &files);
    Ok(Overlay {
        base,
        digest,
        files,
    })
}

/// The proposal base and a repository that holds both `HEAD` and the approved tip.
///
/// Prefers the mirror (after copying the local `HEAD` into it); falls back to the checkout
/// when it already has the approved commit. Unrelated histories fall back to `HEAD`.
fn base_and_history(
    env: &Env,
    kb: &Git,
    mirror: Option<&Mirror>,
    head: Option<&str>,
    approved: Option<&str>,
) -> Result<(Option<String>, Option<Git>)> {
    let (Some(head), Some(approved)) = (head, approved) else {
        return Ok((head.map(str::to_string), None));
    };
    let mut history = None;
    if let Some(m) = mirror
        && m.commit(approved)?.is_some()
        && m.ensure_commit(head, LOCAL_HEAD_REF, Some(&env.kb_root), None)?
    {
        history = Some(m.git().clone());
    } else if kb.resolve_commit(approved)?.is_some() {
        history = Some(kb.clone());
    }
    let base = match &history {
        Some(g) => mirror::merge_base(g, head, approved)?,
        None => None,
    };
    match base {
        Some(b) => Ok((Some(b), history)),
        None => Ok((Some(head.to_string()), None)),
    }
}

/// `git diff --name-status <base>` against the working tree (staged + unstaged + committed).
fn tracked_changes(
    kb: &Git,
    base: &str,
    prefixes: &[String],
) -> Result<BTreeMap<String, OverlayStatus>> {
    let mut args = vec![
        "--literal-pathspecs",
        "diff",
        "--name-status",
        "-z",
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=all",
        base,
        "--",
    ];
    args.extend(prefixes.iter().map(String::as_str));
    let out = kb.run_bytes(&args)?;
    let mut fields = out.split(|b| *b == 0).filter(|f| !f.is_empty());
    let mut map = BTreeMap::new();
    while let (Some(code), Some(path)) = (fields.next(), fields.next()) {
        let status = match code.first() {
            Some(b'A') => OverlayStatus::Added,
            Some(b'D') => OverlayStatus::Deleted,
            Some(b'M' | b'T' | b'U') => OverlayStatus::Modified,
            _ => continue,
        };
        map.insert(String::from_utf8_lossy(path).into_owned(), status);
    }
    Ok(map)
}

/// `git ls-files <mode>` under the prefixes (`--others` excludes ignored files).
fn list_files(kb: &Git, mode: &str, prefixes: &[String]) -> Result<Vec<String>> {
    let mut args = vec!["--literal-pathspecs", "ls-files", "-z", mode];
    if mode == "--others" {
        args.push("--exclude-standard");
    }
    args.push("--");
    args.extend(prefixes.iter().map(String::as_str));
    let out = kb.run_bytes(&args)?;
    Ok(out
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect())
}

/// Working-tree bytes of `path` (`None` when absent). Symlinks are refused.
fn read_work_file(env: &Env, path: &str) -> Result<Option<Vec<u8>>> {
    let p = safe_join(&env.kb_root, path)?;
    match fs::symlink_metadata(&p) {
        Ok(_) => read_file_limited(&p, MAX_SOURCE_FILE_BYTES).map(Some),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(KbError::io(p.display(), e)),
    }
}

/// sha256 over the base and the sorted (path, status, content id, stale) entries.
fn digest(base: Option<&str>, files: &[OverlayFile]) -> String {
    let mut h = FieldHasher::new();
    h.field("kb-overlay/1").field(base.unwrap_or(""));
    for f in files {
        let status = match f.status {
            OverlayStatus::Added => "added",
            OverlayStatus::Modified => "modified",
            OverlayStatus::Deleted => "deleted",
        };
        h.field(&f.path)
            .field(status)
            .field(f.content_id.as_deref().unwrap_or(""))
            .field(if f.stale { "stale" } else { "" });
    }
    h.finish_hex()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, status: OverlayStatus, id: Option<&str>, stale: bool) -> OverlayFile {
        OverlayFile {
            path: path.into(),
            status,
            content: None,
            content_id: id.map(str::to_string),
            stale,
        }
    }

    #[test]
    fn digest_depends_on_every_field() {
        let a = vec![file(
            "p/k/a.md",
            OverlayStatus::Added,
            Some("sha256:1"),
            false,
        )];
        let d = digest(Some("base"), &a);
        assert_eq!(d, digest(Some("base"), &a.clone()));
        assert_ne!(d, digest(None, &a));
        assert_ne!(
            d,
            digest(
                Some("base"),
                &[file(
                    "p/k/a.md",
                    OverlayStatus::Modified,
                    Some("sha256:1"),
                    false
                )]
            )
        );
        assert_ne!(
            d,
            digest(
                Some("base"),
                &[file(
                    "p/k/a.md",
                    OverlayStatus::Added,
                    Some("sha256:2"),
                    false
                )]
            )
        );
        assert_ne!(
            d,
            digest(
                Some("base"),
                &[file(
                    "p/k/a.md",
                    OverlayStatus::Added,
                    Some("sha256:1"),
                    true
                )]
            )
        );
    }
}
