//! `kb context` handler: resolve the snapshot (freshness per call unless `--offline`),
//! detect the host, and assemble task-scoped context (docs/architecture.md §5).
//!
//! Exit 0 only for `complete` context; otherwise the full result is printed and the exit
//! code is 30 (`CONTEXT_INCOMPLETE`).

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use super::Ctx;
use super::args::{BudgetUnitArg, ContextArgs, IntentArg, SectionsArg};
use super::session::{self, Options, Session};
use crate::context::{self, ContextRequest, SectionsMode, TaskEnv};
use crate::error::{KbError, Result};
use crate::glob::split_repo;
use crate::host::{self, HostContext};
use crate::knowledge::{KnowledgeView, SnapshotInfo};
use crate::model::{BudgetUnit, Intent, Registry};
use crate::output::CommandOutput;

pub fn run(ctx: &Ctx, args: &ContextArgs) -> Result<CommandOutput> {
    // Validate local input before any Git or network work.
    let host_versions = args
        .host_versions
        .iter()
        .map(|s| context::parse_host_version(s))
        .collect::<Result<Vec<_>>>()?;
    let host = session::detect_host(ctx)?;
    let cwd = ctx
        .env
        .cwd
        .canonicalize()
        .unwrap_or_else(|_| ctx.env.cwd.clone());
    let paths = args
        .paths
        .iter()
        .map(|p| host_relative(p, &cwd, host.as_ref().map(|h| h.root.as_path())))
        .collect::<Result<Vec<_>>>()?;

    let mut req = ContextRequest::new(intent(args.intent));
    req.task = args.task.clone();
    req.repos = args.repos.clone();
    req.paths = paths;
    req.modules = args.modules.clone();
    req.features = args.features.clone();
    req.concepts = args.concepts.clone();
    req.budget = args.budget;
    req.budget_unit = args.budget_unit.map(budget_unit);
    req.include_proposals = args.include_proposals;
    req.sections = sections(args.sections);
    req.max_supplementary = args.max_supplementary;
    req.host_versions = host_versions;

    let mut s = Session::open(ctx, host, Options::reading(ctx, args.include_proposals))?;
    let host = s.host.clone();
    let info = s.info.clone();
    let format = ctx.format;
    let result = s.with_view(|view| {
        let env = task_env(host.as_ref(), view.registry(), info.clone());
        context::assemble(&req, &env, view, format)
    })?;
    ctx.env.progress(format!(
        "context: {} ({} mandatory, {} supplementary units)",
        result.status().as_str(),
        result.footer.counts.mandatory + result.footer.counts.dependencies,
        result.footer.counts.supplementary
    ));
    let mut out = CommandOutput::new(
        context::to_json(&result, args.explain),
        context::render(&result, format, args.explain),
    );
    if let Some(f) = result.failure() {
        out = out.with_failure(f);
    }
    Ok(s.finish(out))
}

/// Host facts for the context engine: identified repo (or the unknown `.kbw.toml` repo, so
/// that the engine reports `HOST_REPO_UNKNOWN`), host `HEAD` and the version of the host
/// repo read from its registry `version_file` (explicit `--host-version` values win inside
/// the engine).
fn task_env(host: Option<&HostContext>, registry: &Registry, snapshot: SnapshotInfo) -> TaskEnv {
    let mut env = TaskEnv::new(snapshot);
    let Some(h) = host else {
        return env;
    };
    env.host_head = h.head.clone();
    match host::identify_repo(h, registry) {
        Some((repo, source)) => {
            if let Some(v) = host::host_version(h, registry, &repo) {
                env.host_versions = BTreeMap::from([(repo.clone(), v)]);
            }
            env.host_repo_source = serde_json::to_value(source)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string));
            env.host_repo = Some(repo);
        }
        None => {
            if let Some(unknown) = host::unknown_binding_repo(h, registry) {
                env.host_repo = Some(unknown);
                env.host_repo_source = Some("binding-file".into());
            }
        }
    }
    env
}

/// Convert a `--path` value to a host-relative path. `repo:path` values pass through
/// unchanged; relative paths are taken relative to the current directory when it lies in
/// the host work tree (else relative to the host root); paths that leave the host root are
/// rejected with `UNSAFE_PATH`. Without a host the value is used as given.
fn host_relative(spec: &str, cwd: &Path, host_root: Option<&Path>) -> Result<String> {
    if split_repo(spec).0.is_some() {
        return Ok(spec.to_string());
    }
    let Some(root) = host_root else {
        return Ok(spec.to_string());
    };
    let given = Path::new(spec);
    let joined = if given.is_absolute() {
        given.to_path_buf()
    } else if cwd.starts_with(root) {
        cwd.join(given)
    } else {
        root.join(given)
    };
    let outside = || {
        KbError::unsafe_path(format!(
            "--path `{spec}` is outside the host repository `{}`",
            root.display()
        ))
    };
    let normalized = lexical_normalize(&joined).ok_or_else(outside)?;
    // `root` is canonical; an absolute path may spell it through a symlinked prefix
    // (macOS /tmp or /var, a symlinked checkout): retry with that prefix resolved.
    let resolved;
    let rel = match normalized.strip_prefix(root) {
        Ok(rel) => rel,
        Err(_) if given.is_absolute() => {
            resolved = resolve_existing_prefix(&normalized).ok_or_else(outside)?;
            resolved.strip_prefix(root).map_err(|_| outside())?
        }
        Err(_) => return Err(outside()),
    };
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(p) => {
                parts.push(p.to_str().ok_or_else(|| {
                    KbError::invalid_input(format!("--path `{spec}` is not UTF-8"))
                })?)
            }
            _ => return Err(outside()),
        }
    }
    if parts.is_empty() {
        return Err(KbError::invalid_input(format!(
            "--path `{spec}` names the host root; pass a file or directory inside it"
        )));
    }
    let mut out = parts.join("/");
    if spec.ends_with('/') {
        out.push('/');
    }
    Ok(out)
}

/// Canonicalize the longest existing ancestor directory of the normalized absolute path
/// `p` and re-append the remaining components. The last component is never resolved (the
/// file may not exist, and a symlinked file keeps the name it was given). `None` when no
/// ancestor can be canonicalized.
fn resolve_existing_prefix(p: &Path) -> Option<PathBuf> {
    let mut tail = vec![p.file_name()?.to_os_string()];
    let mut base = p.parent()?.to_path_buf();
    let canonical = loop {
        if let Ok(c) = base.canonicalize() {
            break c;
        }
        tail.push(base.file_name()?.to_os_string());
        if !base.pop() {
            return None;
        }
    };
    let mut out = canonical;
    for part in tail.iter().rev() {
        out.push(part);
    }
    Some(out)
}

/// Resolve `.` and `..` without touching the filesystem (paths need not exist). `None` when
/// `..` climbs above the filesystem root.
fn lexical_normalize(p: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

fn intent(a: IntentArg) -> Intent {
    match a {
        IntentArg::Implement => Intent::Implement,
        IntentArg::Refactor => Intent::Refactor,
        IntentArg::Debug => Intent::Debug,
        IntentArg::Review => Intent::Review,
        IntentArg::Explain => Intent::Explain,
    }
}

fn budget_unit(a: BudgetUnitArg) -> BudgetUnit {
    match a {
        BudgetUnitArg::TokensEst => BudgetUnit::TokensEst,
        BudgetUnitArg::Bytes => BudgetUnit::Bytes,
    }
}

fn sections(a: SectionsArg) -> SectionsMode {
    match a {
        SectionsArg::None => SectionsMode::None,
        SectionsArg::Mandatory => SectionsMode::Mandatory,
        SectionsArg::All => SectionsMode::All,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_become_host_relative() {
        let root = Path::new("/h");
        let at_root = Path::new("/h");
        let in_app = Path::new("/h/app");
        let elsewhere = Path::new("/elsewhere");
        let rel = |s: &str, cwd: &Path| host_relative(s, cwd, Some(root));
        assert_eq!(rel("app/auth/A.kt", at_root).unwrap(), "app/auth/A.kt");
        assert_eq!(rel("auth/A.kt", in_app).unwrap(), "app/auth/A.kt");
        assert_eq!(rel("./auth/../auth/", in_app).unwrap(), "app/auth/");
        assert_eq!(rel("/h/app/x.kt", elsewhere).unwrap(), "app/x.kt");
        assert_eq!(rel("app/x.kt", elsewhere).unwrap(), "app/x.kt");
        assert_eq!(rel("mobile:app/x.kt", in_app).unwrap(), "mobile:app/x.kt");
        for bad in ["../outside.kt", "/etc/passwd", "../../../../x"] {
            let e = rel(bad, at_root).unwrap_err();
            assert_eq!(e.code, crate::error::ErrorCode::UnsafePath, "{bad}");
        }
        assert_eq!(
            rel(".", at_root).unwrap_err().code,
            crate::error::ErrorCode::InvalidInput
        );
        assert_eq!(
            host_relative("../x", in_app, None).unwrap(),
            "../x",
            "no host: passed through for the engine to check"
        );
    }

    #[cfg(unix)]
    #[test]
    fn absolute_paths_through_symlinked_prefixes_stay_inside_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let root = base.join("real/host");
        std::fs::create_dir_all(root.join("app/auth")).unwrap();
        std::fs::create_dir_all(base.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("link")).unwrap();
        std::os::unix::fs::symlink(root.join("app"), base.join("applink")).unwrap();
        std::os::unix::fs::symlink(base.join("elsewhere"), root.join("app/out")).unwrap();
        let rel = |p: PathBuf| host_relative(p.to_str().unwrap(), &base, Some(&root));
        // The file itself does not have to exist; directories keep their trailing slash.
        assert_eq!(
            rel(base.join("link/host/app/auth/New.kt")).unwrap(),
            "app/auth/New.kt"
        );
        assert_eq!(
            rel(base.join("link/host/app/new-dir/sub/X.kt")).unwrap(),
            "app/new-dir/sub/X.kt"
        );
        let dir_spec = format!("{}/", base.join("link/host/app/auth").display());
        assert_eq!(
            host_relative(&dir_spec, &base, Some(&root)).unwrap(),
            "app/auth/"
        );
        assert_eq!(
            rel(base.join("applink/auth/A.kt")).unwrap(),
            "app/auth/A.kt"
        );
        // Resolution only rescues spellings of the host root; outside stays outside.
        for bad in [
            base.join("elsewhere/x.kt"),
            base.join("link/other.kt"),
            base.join("link/host/../other/x.kt"),
        ] {
            let e = rel(bad.clone()).unwrap_err();
            assert_eq!(e.code, crate::error::ErrorCode::UnsafePath, "{bad:?}");
        }
        // A lexically inside path keeps its spelling (symlinks inside the host are not
        // resolved): unchanged behavior.
        assert_eq!(rel(root.join("app/out/x.kt")).unwrap(), "app/out/x.kt");
    }
}
