//! `kb validate` handler: records, references, policies and routing fixtures of one profile.
//!
//! * Source: the KB working tree by default (no Git, no network). An explicit `--snapshot`
//!   other than `working-tree` resolves that snapshot (fetching unless `--offline`).
//! * `--base REV`: ids present at `REV` (a commit of the KB checkout) must still exist and
//!   keep their kind.
//! * `--templates`: also validate the shipped templates and examples. On an uninitialized
//!   project only the templates are validated (a clean upstream must pass this check); the
//!   missing project is reported as an info diagnostic.
//! * Routing fixtures run against the validated corpus unless `--no-routing`.
//!
//! Exit codes: `VALIDATION_FAILED` (40) takes precedence over `ROUTING_TESTS_FAILED` (41).

use serde_json::json;

use super::Ctx;
use super::args::ValidateArgs;
use super::session;
use crate::context::memory::MemoryView;
use crate::context::{TaskEnv, routing};
use crate::corpus::{Corpus, load_corpus};
use crate::diag::{Diagnostic, Severity};
use crate::error::{ErrorCode, KbError, Result};
use crate::git::check_revision_arg;
use crate::knowledge::{Freshness, Selection, SnapshotInfo};
use crate::model::ProfileLocation;
use crate::output::{CommandOutput, Format};
use crate::snapshot::{self, GitTreeSource, SelectionRequest, SnapshotRequest};
use crate::source::{SourceTree, WorkingTreeSource};
use crate::util::FieldHasher;
use crate::validate::{
    ValidationReport, meta_inputs, render_report, validate_against_base, validate_corpus,
    validate_templates,
};
use crate::versions::{ENGINE_VERSION, INDEX_SCHEMA, PARSER_VERSION};

pub fn run(ctx: &Ctx, args: &ValidateArgs) -> Result<CommandOutput> {
    run_with_context_format(ctx, args, Format::Compact)
}

pub(super) fn run_with_context_format(
    ctx: &Ctx,
    args: &ValidateArgs,
    context_format: Format,
) -> Result<CommandOutput> {
    if args.stale.is_some_and(|days| days > 365_000) {
        return Err(KbError::invalid_input(
            "--stale must be at most 365000 days",
        ));
    }
    let loc = ctx.location()?;
    let (source, explicit): (Box<dyn SourceTree>, Option<SnapshotInfo>) =
        match ctx.global.snapshot.as_deref() {
            None | Some("working-tree") => {
                (Box::new(WorkingTreeSource::new(&ctx.env.kb_root)), None)
            }
            Some(_) => {
                let host = session::detect_host(ctx)?;
                let req = SnapshotRequest {
                    selection: SelectionRequest::parse(ctx.global.snapshot.as_deref())?,
                    offline: ctx.global.offline,
                    include_proposals: false,
                };
                let snap = snapshot::resolve(&ctx.env, &loc, host.as_ref(), &req)?;
                ctx.env
                    .progress(format!("validate: snapshot {}", snap.info.label()));
                (snap.source, Some(snap.info))
            }
        };

    let corpus = match load_corpus(source.as_ref(), &loc) {
        Ok(c) => Some(c),
        Err(e) if e.code == ErrorCode::ProjectNotInitialized && args.templates => {
            ctx.env
                .progress("validate: the project is not initialized; checking templates only");
            None
        }
        Err(e) => return Err(e),
    };

    let dated = args.stale.is_some()
        || args.on.is_some()
        || corpus.as_ref().is_some_and(|c| {
            c.records().any(|(_, p)| {
                p.record.status() == crate::model::Status::Accepted
                    && crate::freshness::has_dates(&p.record)
            })
        });
    let reference_date = if dated {
        let host = session::detect_host(ctx)?;
        let root = host
            .as_ref()
            .map(|h| h.root.as_path())
            .unwrap_or(&ctx.env.kb_root);
        let revision = host
            .as_ref()
            .and_then(|h| h.head.as_deref())
            .or_else(|| explicit.as_ref().and_then(|s| s.revision.as_deref()))
            .unwrap_or("HEAD");
        crate::host::facts::reference_date(root, revision, args.on.as_deref())?
    } else {
        None
    };

    let (report, routing_report) = match &corpus {
        Some(corpus) => {
            let mut report = validate_corpus(corpus);
            for (entry, parsed) in corpus.records() {
                report.extend(
                    crate::freshness::warnings(&parsed.record, reference_date.as_ref(), args.stale)
                        .into_iter()
                        .map(|d| d.at_path(&entry.path)),
                );
            }
            // The routing view sees exactly the snapshot diagnostics the index would store.
            let view_diagnostics = report.diagnostics.clone();
            let mut routing_report = None;
            if !args.no_routing {
                let (cases, load_diags) = routing::load_cases(source.as_ref(), &loc)?;
                report.extend(load_diags);
                let info = explicit
                    .clone()
                    .unwrap_or_else(|| working_tree_info(&loc, corpus));
                let view = MemoryView::from_corpus(corpus, view_diagnostics);
                routing_report = Some(routing::run_cases_in_format(
                    &view,
                    &cases,
                    &TaskEnv::new(info),
                    context_format,
                ));
            }
            if let Some(rev) = &args.base {
                report.extend(base_diagnostics(ctx, &loc, corpus, rev)?);
            }
            (report, routing_report)
        }
        None => {
            let mut report = ValidationReport::new(
                loc.profile.as_str(),
                0,
                0,
                vec![Diagnostic::info(
                    "PROJECT_NOT_INITIALIZED",
                    format!(
                        "`{}` does not exist: only the shipped templates were validated \
                         (run `kbw init` to create the project)",
                        loc.config
                    ),
                )],
            );
            if args.base.is_some() {
                report.extend([Diagnostic::info(
                    "BASE_CHECK_SKIPPED",
                    "--base was not checked: there are no project records",
                )]);
            }
            (report, None)
        }
    };
    let mut report = report;
    let templates = if args.templates {
        let found = validate_templates(&ctx.env.kb_root)?;
        let count = |sev: Severity| found.iter().filter(|d| d.severity == sev).count();
        let counts = (count(Severity::Error), count(Severity::Warning));
        report.extend(found);
        Some(counts)
    } else {
        None
    };

    let mut result = json!({
        "source": match &explicit {
            Some(info) => serde_json::to_value(info)?,
            None => json!({ "selection": "working-tree", "freshness": "unverified" }),
        },
        "strict": args.strict,
        "templates": args.templates,
        "base": args.base,
        "validation": report,
        "routing": routing_report.as_ref().map(routing::report_to_json),
    });
    if dated {
        result["freshness"] = json!({"reference":reference_date,"max_age_days":args.stale});
    }
    let mut text = render(ctx.format, &report, templates, routing_report.as_ref());
    if let Some(date) = &reference_date {
        text.push_str(&format!(
            "freshness reference: {} ({})\n",
            date.date, date.source
        ));
    }
    let mut out = CommandOutput::new(result, text);
    if !report.is_ok(args.strict) {
        let strict = if args.strict && report.errors == 0 {
            " (--strict: warnings count as errors)"
        } else {
            ""
        };
        out = out.with_failure(
            KbError::new(
                ErrorCode::ValidationFailed,
                format!(
                    "validation failed: {} error(s), {} warning(s){strict}",
                    report.errors, report.warnings
                ),
            )
            .with_details(json!({
                "errors": report.errors,
                "warnings": report.warnings,
                "strict": args.strict,
            })),
        );
    } else if let Some(f) = routing_report
        .as_ref()
        .and_then(routing::RoutingReport::failure)
    {
        out = out.with_failure(f);
    }
    Ok(out)
}

/// `templates`: error and warning counts of the shipped templates and examples when
/// `--templates` checked them (they are included in the report totals).
fn render(
    format: Format,
    report: &ValidationReport,
    templates: Option<(usize, usize)>,
    routing: Option<&routing::RoutingReport>,
) -> String {
    if format == Format::Json {
        return String::new();
    }
    let mut s = render_report(report, format);
    if let Some((errors, warnings)) = templates {
        s.push_str(&match format {
            Format::Human => format!(
                "\nShipped templates and examples were validated (--templates): {errors} error(s), {warnings} warning(s)\n"
            ),
            _ => format!(
                "templates: checked (shipped templates and examples): {errors} error(s), {warnings} warning(s)\n"
            ),
        });
    }
    match routing {
        Some(r) => s.push_str(&routing::render_report(r, format)),
        None if report.records > 0 || report.files > 0 => {
            s.push_str("routing tests: not run\n");
        }
        None => {}
    }
    s
}

/// `ID_REMOVED` / `ID_KIND_CHANGED` against the records of `rev` in the KB checkout.
fn base_diagnostics(
    ctx: &Ctx,
    loc: &ProfileLocation,
    current: &Corpus,
    rev: &str,
) -> Result<Vec<Diagnostic>> {
    check_revision_arg(rev)?;
    let kb = snapshot::kb_git(&ctx.env).ok_or_else(|| {
        KbError::new(
            ErrorCode::GitError,
            "--base needs the KB root to be a Git checkout",
        )
    })?;
    let commit = kb.resolve_commit(rev)?.ok_or_else(|| {
        KbError::invalid_input(format!(
            "--base `{rev}` does not resolve to a commit in the KB checkout"
        ))
    })?;
    let base_source = GitTreeSource::new(kb, commit.clone())?;
    let base = match load_corpus(&base_source, loc) {
        Ok(c) => c,
        Err(e) if e.code == ErrorCode::ProjectNotInitialized => {
            return Ok(vec![Diagnostic::info(
                "BASE_NOT_INITIALIZED",
                format!("the project did not exist at {rev} ({commit}); no ids to compare"),
            )]);
        }
        Err(e) => return Err(e),
    };
    ctx.env.progress(format!(
        "validate: comparing ids with {rev} ({})",
        &commit[..commit.len().min(12)]
    ));
    Ok(validate_against_base(
        &meta_inputs(current),
        &meta_inputs(&base),
    ))
}

/// Provenance of a working-tree validation (never approved, freshness not checked).
pub(super) fn working_tree_info(loc: &ProfileLocation, corpus: &Corpus) -> SnapshotInfo {
    let mut h = FieldHasher::new();
    h.field("kb-validate/working-tree")
        .field(loc.profile.as_str())
        .field(&loc.config)
        .field(ENGINE_VERSION)
        .field(INDEX_SCHEMA.to_string())
        .field(PARSER_VERSION.to_string());
    for e in &corpus.entries {
        h.field(&e.path).field(&e.content_id);
    }
    SnapshotInfo {
        profile: loc.profile.as_str().to_string(),
        remote: corpus.config.source.remote.clone(),
        source: String::new(),
        approved_ref: corpus.config.source.approved_ref.clone(),
        selection: Selection::WorkingTree,
        freshness: Freshness::Unverified,
        revision: None,
        latest_approved: None,
        approved: Some(false),
        pin: None,
        overlay: None,
        engine_version: ENGINE_VERSION.to_string(),
        key: h.finish_hex(),
        content_digest: None,
    }
}
