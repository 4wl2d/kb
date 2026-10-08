use std::collections::BTreeSet;

use serde_json::json;

use super::Ctx;
use super::args::VerifyArgs;
use super::session::{self, Options, Session, with_snapshot, with_snapshot_line};
use crate::context::{ContextRequest, TaskEnv};
use crate::error::{ErrorCode, KbError, Result};
use crate::knowledge::KnowledgeView;
use crate::model::{CodeOperation, Intent, VerifyProbe};
use crate::output::CommandOutput;

pub fn run(ctx: &Ctx, args: &VerifyArgs) -> Result<CommandOutput> {
    let host = session::detect_host(ctx)?
        .ok_or_else(|| KbError::invalid_input("verify needs a host checkout"))?;
    let mode = if args.head.is_some() {
        "commit"
    } else if args.staged {
        "staged"
    } else {
        "working-tree"
    };
    // During `commit -a` or a pathspec commit, Git commits a temporary index that only the
    // hook's GIT_INDEX_FILE names; a missing file would read as an empty index.
    let index_file = args.index_file.as_ref().map(|p| ctx.env.cwd.join(p));
    if index_file.as_ref().is_some_and(|p| !p.is_file()) {
        return Err(KbError::invalid_input(
            "--index-file must name an existing Git index file",
        ));
    }
    let index_file = index_file.as_deref();
    let diff = if host.head.is_none() && args.diff == "HEAD" && args.head.is_none() {
        crate::impact::initial_diff(
            &host.root,
            args.staged,
            host.kb_submodule_path.as_deref(),
            index_file,
        )?
    } else if args.staged {
        crate::impact::staged_diff(
            &host.root,
            &args.diff,
            host.kb_submodule_path.as_deref(),
            index_file,
        )?
    } else {
        crate::impact::host_diff(
            &host.root,
            &args.diff,
            args.head.as_deref(),
            args.head.is_none(),
            host.kb_submodule_path.as_deref(),
        )?
    };
    let mut session = Session::open(ctx, Some(host.clone()), Options::reading(ctx, false))?;
    let info = session.info.clone();
    let report = session.with_view(|view| {
        if view.diagnostics().iter().any(|d| d.is_error()) {
            return Err(KbError::new(
                ErrorCode::ValidationFailed,
                "cannot verify from an invalid knowledge snapshot",
            )
            .with_diagnostics(view.diagnostics().to_vec()));
        }
        let repo = args
            .repo
            .clone()
            .or_else(|| crate::host::identify_repo(&host, view.registry()).map(|(id, _)| id))
            .ok_or_else(|| KbError::invalid_input("verify needs an identified --repo"))?;
        if view.registry().repo(&repo).is_none() {
            return Err(KbError::invalid_input("unknown verify repo"));
        }
        let records = crate::trust::select(view, &args.ids, false)?;
        let mut request = ContextRequest::new(Intent::Review);
        request.repos = vec![repo.clone()];
        request.change_types = args.change_types.clone();
        // Diff paths are bounded by the diff, not by the `--path` limit.
        request.changed_paths = diff
            .files
            .iter()
            .flat_map(|f| std::iter::once(f.path.clone()).chain(f.old_path.clone()))
            .collect();
        request.host_versions = args
            .host_versions
            .iter()
            .map(|s| crate::context::parse_host_version(s))
            .collect::<Result<Vec<_>>>()?;
        let mut env = TaskEnv::new(info.clone());
        env.host_repo = Some(repo.clone());
        env.host_head = diff.head.clone().or(host.head.clone());
        env.changed_scope = true;
        env.known_files
            .extend(request.changed_paths.iter().cloned());
        if let Some(path) = view
            .registry()
            .repo(&repo)
            .and_then(|r| r.version_file.as_deref())
        {
            let bytes = if let Some(head) = &diff.head {
                crate::host::facts::blob_at(&host.root, head, path, 64 * 1024)?
            } else if args.staged {
                crate::host::facts::index_blob(&host.root, path, 64 * 1024, index_file)?
            } else {
                None
            };
            if mode == "working-tree" {
                if let Some(version) = crate::host::host_version(&host, view.registry(), &repo) {
                    env.host_versions.insert(repo.clone(), version);
                }
            } else if let Some(bytes) = bytes
                && let Ok(text) = std::str::from_utf8(&bytes)
                && let Some(line) = text.lines().next()
                && let Ok(version) = semver::Version::parse(line.trim().trim_start_matches('v'))
            {
                env.host_versions.insert(repo.clone(), version);
            }
        }
        let scope = crate::context::resolve_scope(&request, &env, view.registry())?;
        let only: BTreeSet<_> = args.only.iter().map(|k| k.as_str().to_string()).collect();
        let message = args.commit_message.as_ref().map(|p| ctx.env.cwd.join(p));
        let mut input = crate::verify::git::gather(
            &host.root,
            &repo,
            diff.clone(),
            &records,
            &crate::verify::git::Options {
                mode,
                index_file,
                branch: args.branch.as_deref(),
                commit_message: message.as_deref(),
                only: &only,
            },
        )?;
        let needs_import = (only.is_empty() || only.contains("forbidden-import"))
            && records
                .iter()
                .flat_map(|r| r.parsed.record.normative())
                .flat_map(|r| r.verify.iter())
                .any(|p| matches!(p, VerifyProbe::ForbiddenImport { .. }));
        let provider = super::code_options::provider(ctx, &args.code);
        if mode == "commit" && needs_import && provider.configured() {
            let mut code = crate::code::request(
                &host.root,
                &repo,
                input.diff.head.as_deref().unwrap(),
                CodeOperation::Refs,
            )?;
            code.paths = request.changed_paths.clone();
            input.code = Some(provider.load(&code)?);
        }
        crate::verify::evaluate(
            &records,
            &scope,
            &input,
            &crate::verify::Options {
                only,
                applicable: args.applicable.iter().cloned().collect(),
                strict: args.strict,
            },
        )
    })?;
    let failure = if report.blocking_failures > 0 {
        Some(KbError::new(
            ErrorCode::ValidationFailed,
            "declared verification probes failed",
        ))
    } else if report.blocking_unverifiable > 0 {
        Some(KbError::new(
            ErrorCode::ContextIncomplete,
            "declared verification probes need scope, condition or source evidence",
        ))
    } else {
        None
    };
    let value = json!({"verification":report});
    let text = serde_json::to_string_pretty(&value)? + "\n";
    let mut out = CommandOutput::new(
        with_snapshot(value, &session.info),
        with_snapshot_line(text, &session.info, ctx.format),
    );
    if let Some(failure) = failure {
        out = out.with_failure(failure);
    }
    Ok(session.finish(out))
}
