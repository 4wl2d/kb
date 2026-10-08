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
use crate::model::{BudgetUnit, Intent, Kind, Registry};
use crate::output::CommandOutput;

pub fn run(ctx: &Ctx, args: &ContextArgs) -> Result<CommandOutput> {
    run_inner(ctx, args, false)
}

pub fn outline(ctx: &Ctx, args: &ContextArgs) -> Result<CommandOutput> {
    if args.since_receipt.is_some()
        || args.core_receipt.is_some()
        || args.explain
        || !matches!(args.sections, SectionsArg::None)
    {
        return Err(KbError::invalid_input(
            "outline lists inventory; receipt reuse, --explain and --sections apply to context/show",
        ));
    }
    run_inner(ctx, args, true)
}

fn run_inner(ctx: &Ctx, args: &ContextArgs, outline: bool) -> Result<CommandOutput> {
    if args
        .on
        .as_deref()
        .is_some_and(|date| crate::model::date_days(date).is_none())
    {
        return Err(KbError::invalid_input("--on must be YYYY-MM-DD"));
    }
    if ctx.format == crate::output::Format::Terse
        && (args.explain || !matches!(args.sections, SectionsArg::None))
    {
        return Err(KbError::invalid_input(
            "terse context defers Markdown and explanations; use show --sections or another format",
        ));
    }
    let provider = super::code_options::provider(ctx, &args.code);
    if args.with_code && !provider.configured() {
        return Err(KbError::invalid_input(
            "--with-code needs --provider or --provider-file",
        ));
    }
    if !args.with_code && provider.configured() {
        return Err(KbError::invalid_input(
            "provider options on context require --with-code",
        ));
    }
    // Validate local input before any Git or network work.
    let host_versions = args
        .host_versions
        .iter()
        .map(|s| context::parse_host_version(s))
        .collect::<Result<Vec<_>>>()?;
    let host = session::detect_host(ctx)?;
    let receipt_subject = crate::receipts::subject(
        &ctx.env,
        host.as_ref().map(|h| h.root.as_path()),
        ctx.location()?.profile.as_str(),
    );
    let previous = args
        .since_receipt
        .as_ref()
        .map(|id| crate::receipts::load(&ctx.env.cache_dir, id, &receipt_subject))
        .transpose()?
        .unwrap_or_default();
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
    req.stale = args.stale;
    if args.stale.is_some_and(|days| days > 365_000) {
        return Err(KbError::invalid_input(
            "--stale must be at most 365000 days",
        ));
    }
    req.task = args.task.clone();
    req.repos = args.repos.clone();
    req.paths = paths;
    req.modules = args.modules.clone();
    req.features = args.features.clone();
    req.concepts = args.concepts.clone();
    req.change_types = args.change_types.clone();
    req.budget = args.budget;
    req.budget_unit = args.budget_unit.map(budget_unit);
    req.include_proposals = args.include_proposals;
    req.sections = sections(args.sections);
    req.max_supplementary = args.max_supplementary;
    req.host_versions = host_versions;
    if outline {
        req.budget = Some(100_000_000);
        req.budget_unit = Some(BudgetUnit::TokensEst);
    }

    let diff = if args.changed {
        let h = host
            .as_ref()
            .ok_or_else(|| KbError::invalid_input("--changed needs a host repository (--host)"))?;
        let working = args.working_tree || (args.base.is_none() && args.head.is_none());
        let diff = crate::impact::host_diff(
            &h.root,
            args.base.as_deref().unwrap_or("HEAD"),
            args.head.as_deref(),
            working,
            h.kb_submodule_path.as_deref(),
        )?;
        // Diff paths are bounded by the diff itself, not by the `--path` limit.
        for file in &diff.files {
            req.changed_paths.push(file.path.clone());
            req.changed_paths.extend(file.old_path.iter().cloned());
        }
        req.changed_paths.sort();
        req.changed_paths.dedup();
        req.paths.sort();
        req.paths.dedup();
        req.change = Some(context::ChangeScope {
            base: diff.base.clone(),
            merge_base: diff.merge_base.clone(),
            head: diff.head.clone(),
            working_tree: working,
        });
        Some(diff)
    } else {
        None
    };
    let changed_text = match (&host, &diff) {
        (Some(h), Some(diff)) => host::facts::diff_text(&h.root, diff)?,
        _ => Some(String::new()),
    };

    let mut s = Session::open(ctx, host, Options::reading(ctx, args.include_proposals))?;
    let host = s.host.clone();
    let info = s.info.clone();
    let format = ctx.format;
    let result = s.with_view(|view| {
        let mut env = task_env(host.as_ref(), view.registry(), info.clone());
        env.delivery.since_receipt = args.since_receipt.clone();
        env.delivery.previous = previous.clone();
        if let (Some(id), Some(source)) = (&args.core_receipt, &args.core_source) {
            if args.as_of.is_some() { return Err(KbError::invalid_input("historical context must not reuse a current always-on core; use a frozen, clean replay environment")); }
            let host = host.as_ref().ok_or_else(|| KbError::invalid_input("--core-receipt needs a host repository"))?;
            env.delivery.core = crate::integrate::core::verify_installed(&ctx.env.kb_root, &ctx.location()?, &host.root, id, source, view)?;
            env.delivery.core_receipt = Some(id.clone());
        }
        let mut req = req.clone();
        if env.host_repo.is_none()
            && let [repo] = req.repos.as_slice()
            && view.registry().repo(repo).is_some()
        {
            env.host_repo = Some(repo.clone());
            env.host_repo_source = Some("argument".into());
        }
        if let Some(diff) = &diff {
            env.changed_scope = true;
            env.set_changed_text(changed_text.clone());
            for file in &diff.files {
                env.known_files.insert(file.path.clone());
                env.known_files.extend(file.old_path.iter().cloned());
            }
            if let Some(head) = &diff.head {
                env.host_head = Some(head.clone());
            }
        }
        if let Some(requested) = &args.as_of {
            let ids = view
                .metas_by_kind(&Kind::ALL, crate::knowledge::Origin::Accepted)?
                .into_iter()
                .map(|e| e.meta.id.clone())
                .collect::<Vec<_>>();
            let mut records = view.records(&ids, crate::knowledge::Origin::Accepted)?;
            // Commit bounds of records scoped to other repositories name another history;
            // the slice withholds those records (AS_OF_BOUND_UNRESOLVED).
            if let Some(repo) = env.host_repo.as_deref()
                && view.registry().repo(repo).is_some()
            {
                records.retain(|r| {
                    context::temporal::scope_reaches_repo(
                        r.parsed.record.common().scope,
                        view.registry(),
                        repo,
                    )
                });
            }
            req.as_of = Some(host::facts::resolve_as_of(
                host.as_ref().map(|h| h.root.as_path()),
                requested,
                &records,
            )?);
        }
        let selected_revision = req
            .as_of
            .as_ref()
            .and_then(|p| p.host_revision.as_deref())
            .or_else(|| diff.as_ref().and_then(|d| d.head.as_deref()));
        if req.as_of.is_some() {
            env.host_head = req.as_of.as_ref().and_then(|p| p.host_revision.clone());
            // Never use the current work-tree version for historical applicability.
            env.host_versions.clear();
        }
        if let Some(h) = &host {
            if let Some(rev) = selected_revision
                && let Some(repo) = &env.host_repo
                && let Some(file) = view
                    .registry()
                    .repo(repo)
                    .and_then(|r| r.version_file.as_deref())
                && let Some(bytes) = host::facts::blob_at(&h.root, rev, file, 64 * 1024)?
                && let Ok(text) = std::str::from_utf8(&bytes)
                && let Some(line) = text.lines().next()
                && let Ok(version) = semver::Version::parse(line.trim().trim_start_matches('v'))
            {
                env.host_versions.insert(repo.clone(), version);
            }
            if let Some(task) = req.task.as_deref().filter(|t| !t.is_empty()) {
                let (files, skipped) = if req.as_of.is_some() && selected_revision.is_none() {
                    (Vec::new(), 0)
                } else {
                    host::facts::discoverable_files(&h.root, selected_revision)?
                };
                if skipped > 0 {
                    env.notes.push(crate::diag::Diagnostic::info(
                        "TRACKED_NAMES_SKIPPED",
                        format!(
                            "{skipped} tracked filename(s) are not UTF-8 or not safe relative \
                             paths and were not used for identifier discovery"
                        ),
                    ));
                }
                let candidates = context::discovery::identifier_paths(task, &files);
                env.notes.extend(candidates.note("tracked filenames"));
                for path in candidates.paths {
                    if h.kb_submodule_path
                        .as_ref()
                        .is_some_and(|kb| path == *kb || path.starts_with(&format!("{kb}/")))
                    {
                        continue;
                    }
                    env.known_files.insert(path.clone());
                    env.inferred_paths.push(path);
                }
            }
        }
        let on = args.on.as_deref().or_else(|| req.as_of.as_ref().map(|p| p.date.as_str()));
        if let (Some(on), Some(point)) = (&args.on, &req.as_of) && *on != point.date {
            return Err(KbError::invalid_input("historical freshness --on must match the --as-of date"));
        }
        let date_root = host.as_ref().map(|h| h.root.as_path()).unwrap_or(&ctx.env.kb_root);
        let date_rev = if host.is_some() { selected_revision.unwrap_or("HEAD") } else { info.revision.as_deref().unwrap_or("HEAD") };
        env.reference_date = host::facts::reference_date(date_root, date_rev, on)?;
        if args.with_code {
            let h = host
                .as_ref()
                .ok_or_else(|| KbError::invalid_input("--with-code needs --host"))?;
            let repo = env.host_repo.as_deref().ok_or_else(|| {
                KbError::invalid_input("--with-code needs an identified host repo")
            })?;
            if req.as_of.is_some() && selected_revision.is_none() {
                return Err(KbError::invalid_input(
                    "no host commit exists at the historical code point",
                ));
            }
            let mut code_request = crate::code::request(
                &h.root,
                repo,
                selected_revision.unwrap_or("HEAD"),
                crate::model::CodeOperation::Symbols,
            )?;
            code_request.task = req.task.clone();
            code_request.paths = req
                .paths
                .iter()
                .chain(&req.changed_paths)
                .filter_map(|p| {
                    let (qualified, path) = split_repo(p);
                    (qualified.is_none() || qualified == Some(repo)).then(|| path.to_string())
                })
                .collect();
            code_request.paths.extend(env.inferred_paths.clone());
            code_request.paths.sort();
            code_request.paths.dedup();
            let response = provider.load(&code_request)?;
            let inferred =
                crate::code::paths_for_task(&response, req.task.as_deref().unwrap_or_default());
            env.notes.extend(inferred.note("code symbols"));
            env.known_files.extend(inferred.paths.iter().cloned());
            env.inferred_paths.extend(inferred.paths);
            env.inferred_paths.sort();
            env.inferred_paths.dedup();
            let (units, omitted) = crate::code::brief(&response, &code_request, 8)?;
            env.code_units = units;
            let digest = crate::util::sha256_hex(
                context::canonical_json(&serde_json::to_value(&response)?).as_bytes(),
            );
            let mut limitations = response.limitations;
            limitations.extend(omitted);
            limitations.sort();
            env.code_info = Some(context::code::CodeInfo {
                repo: response.repo,
                commit: response.commit,
                tool: response.tool,
                complete: response.complete,
                limitations,
                digest,
            });
        }
        context::assemble(&req, &env, view, format)
    })?;
    ctx.env.progress(format!(
        "context: {} ({} mandatory, {} supplementary units)",
        result.status().as_str(),
        result.footer.counts.mandatory + result.footer.counts.dependencies,
        result.footer.counts.supplementary
    ));
    if outline {
        let unit = args
            .budget_unit
            .map(budget_unit)
            .unwrap_or(BudgetUnit::TokensEst);
        let limit = args.budget.unwrap_or(if unit == BudgetUnit::Bytes {
            8000
        } else {
            2000
        });
        let (value, text) = context::outline::render(&result, format, limit, unit)?;
        let mut out = CommandOutput::new(value, text);
        if result.status() != context::Completeness::Complete {
            out = out.with_failure(KbError::new(
                crate::error::ErrorCode::ContextIncomplete,
                "outline has the snapshot/scope limitations listed in result",
            ));
        }
        return Ok(s.finish(out));
    }
    if let Err(error) = crate::receipts::save(&ctx.env.cache_dir, &receipt_subject, &result) {
        s.note(crate::diag::Diagnostic::warning(
            "RECEIPT_NOT_SAVED",
            error.message,
        ));
    }
    if let Err(error) = crate::usage::append(&ctx.env.cache_dir, &receipt_subject, &result) {
        s.note(crate::diag::Diagnostic::warning(
            "USAGE_NOT_LOGGED",
            error.message,
        ));
    }
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
        IntentArg::Diagnose => Intent::Diagnose,
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
