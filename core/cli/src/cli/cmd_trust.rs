use serde_json::{Value, json};

use super::Ctx;
use super::args::{
    AnchorCheckArgs, AnchorStampArgs, AnchorsCommand, DriftArgs, KnowledgeScopeArgs, LedgerArgs,
};
use super::session::{self, Options, Session, with_snapshot, with_snapshot_line};
use crate::error::{ErrorCode, KbError, Result};
use crate::host::HostContext;
use crate::knowledge::{KnowledgeView, RecordEntry};
use crate::output::CommandOutput;
use crate::provenance::HostRoots;

fn read(
    ctx: &Ctx,
    scope: &KnowledgeScopeArgs,
    include_drafts: bool,
    work: impl Fn(
        &dyn KnowledgeView,
        &HostRoots,
        &[RecordEntry],
        Option<&HostContext>,
    ) -> Result<(Value, Option<KbError>)>,
) -> Result<CommandOutput> {
    let host = session::detect_host(ctx)?;
    let mut session = Session::open(ctx, host.clone(), Options::reading(ctx, false))?;
    let (value, failure) = session.with_view(|view| {
        if view.diagnostics().iter().any(|d| d.is_error()) {
            return Err(KbError::new(
                ErrorCode::ValidationFailed,
                "cannot check provenance from an invalid knowledge snapshot",
            )
            .with_diagnostics(view.diagnostics().to_vec()));
        }
        let roots = crate::provenance::host_roots(
            host.as_ref(),
            view.registry(),
            scope.hosts.repo.as_deref(),
            &scope.hosts.repo_roots,
            &ctx.env.cwd,
        )?;
        let records =
            crate::trust::select(view, &scope.ids, include_drafts || scope.include_drafts)?;
        work(view, &roots, &records, host.as_ref())
    })?;
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

pub fn anchors(ctx: &Ctx, args: &AnchorsCommand) -> Result<CommandOutput> {
    match args {
        AnchorsCommand::Stamp(args) => stamp(ctx, args),
        AnchorsCommand::Check(args) => check(ctx, args),
    }
}

fn stamp(ctx: &Ctx, args: &AnchorStampArgs) -> Result<CommandOutput> {
    if ctx
        .global
        .snapshot
        .as_deref()
        .is_some_and(|s| s != "working-tree")
    {
        return Err(KbError::invalid_input(
            "anchors stamp edits the KB working tree; use --snapshot working-tree, and --at for the host revision",
        ));
    }
    let loc = ctx.location()?;
    let corpus = crate::corpus::load_corpus(
        &crate::source::WorkingTreeSource::new(&ctx.env.kb_root),
        &loc,
    )?;
    let host = session::detect_host(ctx)?;
    let roots = crate::provenance::host_roots(
        host.as_ref(),
        &corpus.registry,
        args.hosts.repo.as_deref(),
        &args.hosts.repo_roots,
        &ctx.env.cwd,
    )?;
    let plan = crate::trust::plan_stamps(
        &ctx.env.kb_root,
        &corpus,
        &roots,
        &super::code_options::provider(ctx, &args.code),
        &crate::trust::StampOptions {
            ids: &args.ids,
            at: &args.at,
            verified_at: args.verified_at.as_deref(),
            review_by: args.review_by.as_deref(),
        },
    )?;
    let written = if args.apply {
        crate::trust::apply_stamps(&ctx.env.kb_root, &plan)?
    } else {
        0
    };
    let result = json!({"mode":if args.apply {"apply"}else{"dry-run"},"source":"working-tree","written":written,"plan":plan});
    Ok(CommandOutput::new(
        result.clone(),
        serde_json::to_string_pretty(&result)? + "\n",
    ))
}

fn check(ctx: &Ctx, args: &AnchorCheckArgs) -> Result<CommandOutput> {
    read(ctx, &args.scope, false, |_, roots, records, _| {
        let report = crate::trust::check_anchors(&ctx.env.kb_root, roots, records, &args.at);
        let unstamped: Vec<_> = report
            .anchors
            .iter()
            .filter(|a| a.anchor.path.is_some() && a.anchor.stamp.is_none())
            .map(|a| format!("{}:{}", a.record, a.index))
            .collect();
        let failure = if report.changed + report.missing > 0 {
            Some(KbError::new(
                ErrorCode::DriftDetected,
                "anchor bytes or paths differ from their declared evidence",
            ))
        } else if report.unverifiable > 0
            || (args.strict && (!report.without_anchors.is_empty() || !unstamped.is_empty()))
        {
            Some(KbError::new(
                ErrorCode::ContextIncomplete,
                "anchor evidence is unavailable or incomplete under the selected check",
            ))
        } else {
            None
        };
        Ok((
            json!({"anchors":report,"strict":args.strict,"unstamped":unstamped}),
            failure,
        ))
    })
}

pub fn drift(ctx: &Ctx, args: &DriftArgs) -> Result<CommandOutput> {
    read(ctx, &args.scope, false, |_, roots, records, _| {
        let report =
            crate::trust::drift::report(&ctx.env.kb_root, roots, records, &args.since, &args.at);
        let failure = if args.check && report.changed_records > 0 {
            Some(KbError::new(
                ErrorCode::DriftDetected,
                "anchored knowledge changed and needs owner review",
            ))
        } else if args.check && report.unverifiable_records > 0 {
            Some(KbError::new(
                ErrorCode::ContextIncomplete,
                "drift cannot be checked for every selected record",
            ))
        } else {
            None
        };
        Ok((json!({"drift":report}), failure))
    })
}

pub fn ledger(ctx: &Ctx, args: &LedgerArgs) -> Result<CommandOutput> {
    if args.sample > 1000 || args.seed.len() > 256 || args.stale.is_some_and(|days| days > 365_000)
    {
        return Err(KbError::invalid_input(
            "ledger limits: sample <= 1000, seed <= 256 bytes, stale <= 365000 days",
        ));
    }
    read(
        ctx,
        &args.scope,
        args.sample > 0,
        |_, roots, records, host| {
            let root = host.map(|h| h.root.as_path()).unwrap_or(&ctx.env.kb_root);
            let on = crate::host::facts::reference_date(root, &args.at, args.on.as_deref())?;
            let report = crate::trust::ledger::report(
                &ctx.env.kb_root,
                roots,
                records,
                &crate::trust::ledger::Options {
                    at: &args.at,
                    on,
                    max_age: args.stale,
                    sample: args.sample,
                    seed: &args.seed,
                },
            );
            let failure = if args.check && report.accepted_counts["stale"] > 0 {
                Some(KbError::new(
                    ErrorCode::DriftDetected,
                    "ledger contains stale accepted statements",
                ))
            } else if args.check && report.accepted_counts["unverifiable"] > 0 {
                Some(KbError::new(
                    ErrorCode::ContextIncomplete,
                    "ledger contains unverifiable accepted statements",
                ))
            } else {
                None
            };
            Ok((json!({"ledger":report}), failure))
        },
    )
}
