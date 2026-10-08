use std::collections::BTreeSet;

use serde_json::json;

use super::Ctx;
use super::args::{UsageCommand, UsageReportArgs};
use super::session::{self, Options, Session, with_snapshot, with_snapshot_line};
use crate::context::{ContextRequest, TaskEnv};
use crate::error::{ErrorCode, KbError, Result};
use crate::knowledge::KnowledgeView;
use crate::model::Intent;
use crate::output::CommandOutput;

pub fn run(ctx: &Ctx, args: &UsageCommand) -> Result<CommandOutput> {
    match args {
        UsageCommand::Report(args) => report(ctx, args),
    }
}

fn report(ctx: &Ctx, args: &UsageReportArgs) -> Result<CommandOutput> {
    let host = session::detect_host(ctx)?
        .ok_or_else(|| KbError::invalid_input("usage report needs a host checkout"))?;
    let subject =
        crate::receipts::subject(&ctx.env, Some(&host.root), ctx.location()?.profile.as_str());
    let receipts: BTreeSet<_> = args.receipts.iter().cloned().collect();
    let log = crate::usage::read(&ctx.env.cache_dir, &subject, &receipts)?;
    let found: BTreeSet<_> = log.calls.iter().map(|c| c.receipt.clone()).collect();
    let missing: Vec<_> = receipts.difference(&found).cloned().collect();
    let diff = if host.head.is_none() && args.diff == "HEAD" && args.head.is_none() {
        crate::impact::initial_diff(&host.root, false, host.kb_submodule_path.as_deref(), None)?
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
        let repo = args
            .repo
            .clone()
            .or_else(|| crate::host::identify_repo(&host, view.registry()).map(|(id, _)| id))
            .ok_or_else(|| KbError::invalid_input("usage report needs an identified --repo"))?;
        let mut request = ContextRequest::new(Intent::Review);
        request.repos = vec![repo.clone()];
        request.change_types = args.change_types.clone();
        request.host_versions = args
            .host_versions
            .iter()
            .map(|s| crate::context::parse_host_version(s))
            .collect::<Result<Vec<_>>>()?;
        request.paths = diff
            .files
            .iter()
            .flat_map(|f| std::iter::once(f.path.clone()).chain(f.old_path.clone()))
            .collect();
        let mut env = TaskEnv::new(info.clone());
        env.host_repo = Some(repo.clone());
        env.changed_scope = true;
        env.known_files.extend(request.paths.iter().cloned());
        if let Some(head) = &diff.head {
            if let Some(path) = view
                .registry()
                .repo(&repo)
                .and_then(|r| r.version_file.as_deref())
                && let Some(bytes) = crate::host::facts::blob_at(&host.root, head, path, 64 * 1024)?
                && let Ok(text) = std::str::from_utf8(&bytes)
                && let Some(line) = text.lines().next()
                && let Ok(version) = semver::Version::parse(line.trim().trim_start_matches('v'))
            {
                env.host_versions.insert(repo.clone(), version);
            }
        } else if let Some(version) = crate::host::host_version(&host, view.registry(), &repo) {
            env.host_versions.insert(repo.clone(), version);
        }
        let task = crate::context::resolve_scope(&request, &env, view.registry())?;
        Ok(crate::usage::report(
            &log,
            &repo,
            &diff,
            view.registry(),
            &task,
            info.content_digest.as_deref(),
        ))
    })?;
    let failure=(!log.invalid_lines.is_empty() || !missing.is_empty()).then(||KbError::new(ErrorCode::ContextIncomplete,"usage observations contain missing receipts or invalid log lines; the preserved log was not repaired or discarded"));
    let value = json!({"usage":report,"diff":diff,"missing_receipts":missing,"selection":if receipts.is_empty(){"all local calls for this KB/host/profile"}else{"all calls with the explicitly selected receipts"}});
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
