//! Host repository detection (the repository a developer works in), host pins and
//! registry repo identification. Detection uses Git context, never folder names.
//!
//! Everything here is read-only: host files are read through [`crate::util`] safe readers
//! (no symlinks) and Git is only queried (`rev-parse`, `ls-tree`, `config --get`).

pub mod facts;

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::env::Env;
use crate::error::{ErrorCode, KbError, Result};
use crate::git::{self, Git};
use crate::knowledge::{PinInfo, PinSource};
use crate::model::{HOST_BINDING_FILE, HostBinding, Registry};
use crate::util::{check_rel_path, read_file_limited, rel_string, safe_join};

/// Upper bound for small host files (`.kbw.toml`, version files).
const MAX_HOST_FILE_BYTES: u64 = 64 * 1024;

/// Git file mode of a gitlink (submodule commit) tree entry.
const GITLINK_MODE: &str = "160000";

/// The host repository and how the KB is bound to it.
#[derive(Debug, Clone, Serialize)]
pub struct HostContext {
    /// Host work tree root (a linked worktree root when applicable).
    pub root: PathBuf,
    /// Shared Git directory (`git rev-parse --git-common-dir`), absolute.
    pub git_common_dir: PathBuf,
    /// True when `root` is a linked worktree.
    pub linked_worktree: bool,
    /// Host `HEAD` commit, if any.
    pub head: Option<String>,
    /// Host remotes as (name, url). URLs may contain credentials: never print them unredacted.
    #[serde(skip)]
    pub remotes: Vec<(String, String)>,
    /// Host-relative path of the KB checkout when it is a submodule of the host.
    pub kb_submodule_path: Option<String>,
    /// Explicit host pin (submodule gitlink in host `HEAD`, or `.kbw.toml pin`).
    pub pin: Option<PinInfo>,
    /// Parsed `.kbw.toml`, if present.
    pub binding: Option<HostBinding>,
}

/// How the host repo id was determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepoSource {
    Argument,
    BindingFile,
    Remote,
}

/// Detect the host repository for this invocation.
///
/// Rules: `--host` wins; otherwise the Git top-level of the current directory; when that is
/// the KB checkout itself, the superproject (if the KB is a submodule), else no host.
pub fn detect(env: &Env, host_override: Option<&Path>) -> Result<Option<HostContext>> {
    let kb_root = canonical(&env.kb_root)?;
    let root = match host_override {
        Some(p) => {
            let abs = if p.is_absolute() {
                p.to_path_buf()
            } else {
                env.cwd.join(p)
            };
            let top = git::toplevel(&abs).ok_or_else(|| {
                KbError::invalid_input(format!(
                    "--host `{}` is not inside a Git work tree",
                    abs.display()
                ))
            })?;
            canonical(&top)?
        }
        None => {
            let Some(top) = git::toplevel(&env.cwd) else {
                return Ok(None);
            };
            let top = canonical(&top)?;
            if top != kb_root {
                top
            } else {
                match superproject(&kb_root)? {
                    Some(sp) => sp,
                    None => return Ok(None),
                }
            }
        }
    };
    inspect(&root, &kb_root).map(Some)
}

/// Collect the host facts for a known host work tree root.
fn inspect(root: &Path, kb_root: &Path) -> Result<HostContext> {
    let git = Git::new(root);
    let (git_dir, git_common_dir) = git_dirs(root)?;
    let head = git.resolve_commit("HEAD")?;
    let remotes = remotes(&git)?;
    let binding = read_binding(root)?;

    let mut kb_submodule_path = None;
    let mut pin = None;
    if let (Some(head), Some(rel)) = (&head, kb_path_in_host(root, &git_common_dir, kb_root)?)
        && let Some(oid) = gitlink(&git, head, &rel)?
    {
        pin = Some(PinInfo {
            revision: oid,
            source: PinSource::Submodule,
            path: Some(rel.clone()),
        });
        kb_submodule_path = Some(rel);
    }
    if pin.is_none()
        && let Some(raw) = binding.as_ref().and_then(|b| b.pin.as_deref())
    {
        let resolved = crate::snapshot::kb_git_at(kb_root)
            .map(|kb| kb.resolve_commit(raw))
            .transpose()?
            .flatten();
        pin = Some(PinInfo {
            revision: resolved.unwrap_or_else(|| raw.to_string()),
            source: PinSource::BindingFile,
            path: None,
        });
    }

    Ok(HostContext {
        root: root.to_path_buf(),
        linked_worktree: git_dir != git_common_dir,
        git_common_dir,
        head,
        remotes,
        kb_submodule_path,
        pin,
        binding,
    })
}

/// Identify the registry repo of the host: `.kbw.toml repo`, else remote URL match.
///
/// A binding `repo` that is not in the registry is ignored here (remote matching still
/// applies); callers report it with [`unknown_binding_repo`].
pub fn identify_repo(host: &HostContext, registry: &Registry) -> Option<(String, RepoSource)> {
    if let Some(id) = host.binding.as_ref().and_then(|b| b.repo.as_deref())
        && registry.repo(id).is_some()
    {
        return Some((id.to_string(), RepoSource::BindingFile));
    }
    let mut remotes: Vec<&(String, String)> = host.remotes.iter().collect();
    remotes.sort();
    remotes.into_iter().find_map(|(_, url)| {
        registry
            .repo_for_remote(url)
            .map(|r| (r.id.clone(), RepoSource::Remote))
    })
}

/// The `.kbw.toml repo` value when it names a repo that the registry does not define.
pub fn unknown_binding_repo(host: &HostContext, registry: &Registry) -> Option<String> {
    host.binding
        .as_ref()
        .and_then(|b| b.repo.as_deref())
        .filter(|id| registry.repo(id).is_none())
        .map(str::to_string)
}

/// Read the host code version for `repo` from its registry `version_file` (first line, semver).
///
/// A leading `v` is accepted (`v1.2.3`). Missing, unsafe (symlinked, absolute, `..`),
/// oversized or unparsable files yield `None` (the version is then unknown).
pub fn host_version(
    host: &HostContext,
    registry: &Registry,
    repo: &str,
) -> Option<semver::Version> {
    let file = registry.repo(repo)?.version_file.as_deref()?;
    check_rel_path(file).ok()?;
    let path = safe_join(&host.root, file).ok()?;
    let bytes = read_file_limited(&path, MAX_HOST_FILE_BYTES).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    let line = text.lines().next()?.trim();
    semver::Version::parse(line.strip_prefix('v').unwrap_or(line)).ok()
}

fn canonical(p: &Path) -> Result<PathBuf> {
    p.canonicalize().map_err(|e| KbError::io(p.display(), e))
}

/// Superproject work tree of the repository at `dir`, when it is a submodule checkout.
fn superproject(dir: &Path) -> Result<Option<PathBuf>> {
    let o = Git::new(dir).output(&["rev-parse", "--show-superproject-working-tree"])?;
    let s = o.stdout_str().trim().to_string();
    if !o.ok() || s.is_empty() {
        return Ok(None);
    }
    canonical(Path::new(&s)).map(Some)
}

/// Absolute, canonical (git dir, common dir) of the work tree at `root`.
fn git_dirs(root: &Path) -> Result<(PathBuf, PathBuf)> {
    let out = Git::new(root).run(&[
        "rev-parse",
        "--absolute-git-dir",
        "--path-format=absolute",
        "--git-common-dir",
    ])?;
    let mut lines = out.lines();
    match (lines.next(), lines.next()) {
        (Some(gd), Some(cd)) => Ok((canonical(Path::new(gd))?, canonical(Path::new(cd))?)),
        _ => Err(KbError::new(
            ErrorCode::GitError,
            format!("cannot determine the Git directory of `{}`", root.display()),
        )),
    }
}

/// Remotes as sorted (name, url) pairs.
fn remotes(git: &Git) -> Result<Vec<(String, String)>> {
    let o = git.output(&["config", "-z", "--get-regexp", r"^remote\..+\.url$"])?;
    if !o.ok() {
        // Exit 1: no remote URLs configured.
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for item in o.stdout.split(|b| *b == 0).filter(|i| !i.is_empty()) {
        let item = String::from_utf8_lossy(item);
        let Some((key, url)) = item.split_once('\n') else {
            continue;
        };
        if let Some(name) = key
            .strip_prefix("remote.")
            .and_then(|k| k.strip_suffix(".url"))
        {
            out.push((name.to_string(), url.to_string()));
        }
    }
    out.sort();
    Ok(out)
}

/// Host-relative path at which the host's `HEAD` tree could hold the KB gitlink.
///
/// Either the KB checkout lies inside the host work tree, or the KB is a submodule of
/// another work tree of the same repository (a linked worktree sharing the common dir),
/// in which case its path relative to that superproject is used.
fn kb_path_in_host(root: &Path, common_dir: &Path, kb_root: &Path) -> Result<Option<String>> {
    if kb_root.starts_with(root) {
        return Ok(rel_string(root, kb_root).filter(|r| !r.is_empty()));
    }
    let Some(sp) = superproject(kb_root)? else {
        return Ok(None);
    };
    let (_, sp_common) = git_dirs(&sp)?;
    if sp_common != common_dir {
        return Ok(None);
    }
    Ok(rel_string(&sp, kb_root).filter(|r| !r.is_empty()))
}

/// The gitlink commit recorded at `rel` in `commit`'s tree, if any.
fn gitlink(git: &Git, commit: &str, rel: &str) -> Result<Option<String>> {
    let out = git.run_bytes(&[
        "--literal-pathspecs",
        "ls-tree",
        "-z",
        "--full-tree",
        commit,
        "--",
        rel,
    ])?;
    for entry in out.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let entry = String::from_utf8_lossy(entry);
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let fields: Vec<&str> = meta.split_ascii_whitespace().collect();
        if path == rel && fields.len() == 3 && fields[0] == GITLINK_MODE {
            return Ok(Some(fields[2].to_string()));
        }
    }
    Ok(None)
}

/// Read and strictly parse `<root>/.kbw.toml` (never through a symlink).
fn read_binding(root: &Path) -> Result<Option<HostBinding>> {
    let path = safe_join(root, HOST_BINDING_FILE)?;
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(KbError::io(path.display(), e)),
    }
    let shown = path.display().to_string();
    let bytes = read_file_limited(&path, MAX_HOST_FILE_BYTES)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        KbError::new(
            ErrorCode::ConfigInvalid,
            format!("host binding `{shown}` is not UTF-8"),
        )
    })?;
    let binding: HostBinding = toml::from_str(text).map_err(|e| {
        KbError::new(
            ErrorCode::ConfigInvalid,
            format!("host binding `{shown}`: {e}"),
        )
    })?;
    // Host binding has its own stable format; record migrations do not rewrite hosts.
    if binding.schema != 1 {
        return Err(KbError::new(
            ErrorCode::UnsupportedSchemaVersion,
            format!(
                "host binding `{shown}` has schema {}; this engine supports {}",
                binding.schema, 1
            ),
        ));
    }
    if let Some(pin) = &binding.pin {
        git::check_revision_arg(pin).map_err(|e| {
            KbError::new(
                ErrorCode::ConfigInvalid,
                format!("host binding `{shown}` pin: {}", e.message),
            )
        })?;
    }
    Ok(Some(binding))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RegistryData;
    use crate::model::registry::Repo;

    fn host(binding_repo: Option<&str>, remotes: &[(&str, &str)]) -> HostContext {
        HostContext {
            root: PathBuf::from("/nonexistent"),
            git_common_dir: PathBuf::from("/nonexistent/.git"),
            linked_worktree: false,
            head: None,
            remotes: remotes
                .iter()
                .map(|(n, u)| (n.to_string(), u.to_string()))
                .collect(),
            kb_submodule_path: None,
            pin: None,
            binding: binding_repo.map(|r| HostBinding {
                schema: 1,
                repo: Some(r.into()),
                pin: None,
                selection: None,
            }),
        }
    }

    fn registry() -> Registry {
        let repo = |id: &str, remote: &str| Repo {
            id: id.into(),
            title: id.into(),
            remotes: vec![remote.into()],
            version_file: None,
        };
        Registry::new(RegistryData {
            repos: vec![
                repo("mobile", "example.invalid/acme/mobile"),
                repo("backend", "example.invalid/acme/backend"),
            ],
            ..Default::default()
        })
    }

    #[test]
    fn binding_repo_wins_and_unknown_binding_falls_back_to_remotes() {
        let reg = registry();
        let h = host(
            Some("backend"),
            &[("origin", "https://example.invalid/acme/mobile.git")],
        );
        assert_eq!(
            identify_repo(&h, &reg),
            Some(("backend".into(), RepoSource::BindingFile))
        );
        let h = host(
            Some("nope"),
            &[("origin", "git@example.invalid:acme/mobile.git")],
        );
        assert_eq!(
            identify_repo(&h, &reg),
            Some(("mobile".into(), RepoSource::Remote))
        );
        assert_eq!(unknown_binding_repo(&h, &reg).as_deref(), Some("nope"));
    }

    #[test]
    fn remotes_are_matched_in_name_order() {
        let reg = registry();
        let h = host(
            None,
            &[
                ("upstream", "https://example.invalid/acme/mobile"),
                ("fork", "https://example.invalid/acme/backend"),
            ],
        );
        assert_eq!(
            identify_repo(&h, &reg),
            Some(("backend".into(), RepoSource::Remote))
        );
        assert_eq!(identify_repo(&host(None, &[]), &reg), None);
    }
}
