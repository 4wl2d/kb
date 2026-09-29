//! `kb impact` handler: relate a host diff to affected knowledge and unknown coverage
//! (docs/architecture.md §12 of the spec). The KB snapshot is selected with the usual
//! freshness rules; the host repository is only read.

use serde_json::json;

use super::Ctx;
use super::args::ImpactArgs;
use super::session::{self, Options, Session, with_snapshot, with_snapshot_line};
use crate::error::{ErrorCode, KbError, Result};
use crate::host;
use crate::impact::{self, ImpactStatement};
use crate::knowledge::{KnowledgeView, Origin};
use crate::model::Kind;
use crate::output::CommandOutput;
use crate::util::read_file_limited;

/// Upper bound for the MR description file given with `--statement`.
const MAX_STATEMENT_BYTES: u64 = 1024 * 1024;

pub fn run(ctx: &Ctx, args: &ImpactArgs) -> Result<CommandOutput> {
    let base = match (&args.base, args.working_tree) {
        (Some(b), _) => b.clone(),
        (None, true) => "HEAD".to_string(),
        (None, false) => {
            return Err(KbError::new(
                ErrorCode::Usage,
                "`kb impact` needs --base <rev> (or --working-tree, which defaults the base to HEAD)",
            )
            .with_hint("e.g. `kbw impact --base origin/main`, or `kbw impact --working-tree` for local changes"));
        }
    };
    let statement = read_statement(ctx, args)?;
    let host = session::detect_host(ctx)?.ok_or_else(|| {
        KbError::new(
            ErrorCode::NotFound,
            "no host repository was detected: `kb impact` analyses the diff of the repository the KB describes",
        )
        .with_hint("run it from the host repository (e.g. `.kb/kbw impact ...`) or pass --host <dir>")
    })?;
    let diff = impact::host_diff(
        &host.root,
        &base,
        args.head.as_deref(),
        args.working_tree,
        host.kb_submodule_path.as_deref(),
    )?;
    ctx.env.progress(format!(
        "impact: {} changed file(s) since merge-base {}",
        diff.files.len(),
        &diff.merge_base[..diff.merge_base.len().min(12)]
    ));

    let mut s = Session::open(ctx, Some(host.clone()), Options::reading(ctx, false))?;
    let report = s.with_view(|view| {
        let metas = view.metas_by_kind(&Kind::ALL, Origin::Accepted)?;
        let repo = host::identify_repo(&host, view.registry()).map(|(r, _)| r);
        Ok(impact::analyze(
            &diff,
            repo.as_deref(),
            view.registry(),
            &metas,
        ))
    })?;
    if report.repo.is_none() {
        s.note(crate::diag::Diagnostic::warning(
            "HOST_REPO_UNKNOWN",
            "the host repository is not identified in the registry (remote URL or `.kbw.toml repo`); \
             only repo-independent path selectors were evaluated",
        ));
    }
    let verdict =
        (args.check || statement.is_some()).then(|| impact::check(&report, statement.as_ref()));
    let mut out = CommandOutput::new(
        with_snapshot(impact::to_json(&report, verdict.as_ref()), &s.info),
        with_snapshot_line(
            impact::render(&report, verdict.as_ref(), ctx.format),
            &s.info,
            ctx.format,
        ),
    );
    if args.check
        && let Some(v) = &verdict
        && !v.ok
    {
        out = out.with_failure(
            KbError::new(ErrorCode::ImpactUnacknowledged, v.reasons.join("; "))
                .with_details(json!({
                    "acknowledgement_required": v.acknowledgement_required,
                    "affected": report.affected.len(),
                    "unknown_coverage": report.unknown_coverage.len(),
                }))
                .with_hint(
                    "add exactly one `<!-- kb-impact:v1 ... -->` block to the MR description \
                     (see core/templates/mr) and pass it with --statement",
                ),
        );
    }
    Ok(s.finish(out))
}

/// Read and parse `--statement`. A malformed block is `IMPACT_UNACKNOWLEDGED` under
/// `--check` (the gate fails) and `INVALID_INPUT` otherwise.
fn read_statement(ctx: &Ctx, args: &ImpactArgs) -> Result<Option<ImpactStatement>> {
    let Some(path) = &args.statement else {
        return Ok(None);
    };
    let path = if path.is_absolute() {
        path.clone()
    } else {
        ctx.env.cwd.join(path)
    };
    let bytes = read_file_limited(&path, MAX_STATEMENT_BYTES)?;
    let shown = path.display().to_string();
    let text = String::from_utf8(bytes)
        .map_err(|_| KbError::invalid_input(format!("statement file `{shown}` is not UTF-8")))?;
    match impact::parse_statement(&text) {
        Ok(s) => Ok(s),
        Err(msg) => {
            let code = if args.check {
                ErrorCode::ImpactUnacknowledged
            } else {
                ErrorCode::InvalidInput
            };
            Err(KbError::new(code, format!("statement `{shown}`: {msg}"))
                .with_details(json!({ "statement": shown, "problem": msg }))
                .with_hint("see core/templates/mr for the `kb-impact:v1` block format"))
        }
    }
}
