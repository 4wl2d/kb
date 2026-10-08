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
    let provider = super::code_options::provider(ctx, &args.code);
    if args.deep && !provider.configured() {
        return Err(KbError::invalid_input(
            "--deep needs --provider or pinned --provider-file responses",
        ));
    }
    if !args.deep && provider.configured() {
        return Err(KbError::invalid_input(
            "provider options on impact require --deep",
        ));
    }
    if !(1..=4).contains(&args.depth) {
        return Err(KbError::invalid_input("deep impact depth must be 1..4"));
    }
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
    let (report, deep) = s.with_view(|view| {
        let metas = view.metas_by_kind(&Kind::ALL, Origin::Accepted)?;
        let repo = args
            .repo
            .clone()
            .or_else(|| host::identify_repo(&host, view.registry()).map(|(r, _)| r));
        if repo
            .as_ref()
            .is_some_and(|id| view.registry().repo(id).is_none())
        {
            return Err(KbError::invalid_input("unknown impact --repo"));
        }
        let deep = if args.deep {
            let repo = repo
                .as_deref()
                .ok_or_else(|| KbError::invalid_input("deep impact needs an identified --repo"))?;
            Some(deep_report(
                &provider, &host, &diff, repo, view, args.depth,
            )?)
        } else {
            None
        };
        Ok((
            impact::analyze(&diff, repo.as_deref(), view.registry(), &metas),
            deep,
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
    let mut result = impact::to_json(&report, verdict.as_ref());
    let mut text = impact::render(&report, verdict.as_ref(), ctx.format);
    if let Some(deep) = &deep {
        result["deep"] = deep.clone();
        text.push_str("\nCode dependents (static provider evidence):\n");
        text.push_str(&serde_json::to_string_pretty(deep)?);
        text.push('\n');
    }
    let mut out = CommandOutput::new(
        with_snapshot(result, &s.info),
        with_snapshot_line(text, &s.info, ctx.format),
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
    } else if args.check && deep.as_ref().is_some_and(|d| d["complete"] == false) {
        out = out.with_failure(KbError::new(
            ErrorCode::ContextIncomplete,
            "deep impact has incomplete provider evidence",
        ));
    }
    Ok(s.finish(out))
}

fn deep_report(
    provider: &crate::code::Provider,
    host: &host::HostContext,
    diff: &impact::HostDiff,
    repo: &str,
    view: &dyn KnowledgeView,
    depth: u32,
) -> Result<serde_json::Value> {
    use std::collections::{BTreeMap, BTreeSet};
    let changed: BTreeSet<_> = diff
        .files
        .iter()
        .flat_map(|f| std::iter::once(f.path.clone()).chain(f.old_path.clone()))
        .collect();
    let head = diff
        .head
        .as_deref()
        .or(host.head.as_deref())
        .ok_or_else(|| KbError::invalid_input("host has no HEAD"))?;
    let points: BTreeSet<_> = [diff.merge_base.as_str(), head].into_iter().collect();
    let mut sources = Vec::new();
    let mut dependents = Vec::new();
    let mut grouped: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let metas = view.metas_by_kind(&Kind::ALL, Origin::Accepted)?;
    let mut complete = true;
    let mut limitations = Vec::new();
    if diff.head.is_none() && !diff.files.is_empty() {
        complete = false;
        limitations.push("The work-tree diff is seeded against committed graphs; new uncommitted definitions and edges are not indexed.");
    }
    for point in points {
        let mut request = crate::code::request(
            &host.root,
            repo,
            point,
            crate::model::CodeOperation::Dependents,
        )?;
        request.depth = depth;
        request.paths = changed.iter().cloned().collect();
        let response = provider.load(&request)?;
        complete &= response.complete;
        let found = crate::code::dependents(&response, &changed, depth);
        let mut indirect = diff.clone();
        indirect.files = found
            .iter()
            .map(|d| impact::ChangedFile {
                path: d.path.clone(),
                old_path: None,
                status: impact::ChangeStatus::Modified,
            })
            .collect();
        indirect.files.sort();
        indirect.files.dedup();
        let coverage = impact::analyze(&indirect, Some(repo), view.registry(), &metas);
        for dependent in found {
            let modules: Vec<_> = view
                .registry()
                .modules_for_path(repo, &dependent.path)
                .into_iter()
                .map(|m| m.id.clone())
                .collect();
            let records: Vec<_> = coverage
                .affected
                .iter()
                .filter(|r| r.files.contains(&dependent.path))
                .map(|r| r.id.clone())
                .collect();
            for module in &modules {
                grouped
                    .entry(module.clone())
                    .or_default()
                    .insert(dependent.path.clone());
            }
            if modules.is_empty() {
                grouped
                    .entry("<unmapped>".into())
                    .or_default()
                    .insert(dependent.path.clone());
            }
            dependents.push(json!({"at": response.commit, "dependent": dependent, "modules": modules, "records": records}));
        }
        sources.push(json!({"commit": response.commit, "tool": response.tool, "complete": response.complete, "limitations": response.limitations}));
    }
    dependents.sort_by_key(crate::context::canonical_json);
    Ok(
        json!({"depth":depth, "complete":complete, "limitations":limitations, "sources":sources, "dependents":dependents, "by_module":grouped,
        "note":"Static dependents outside the diff; possible and file-level relationships remain suggestions, not runtime proof."}),
    )
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
