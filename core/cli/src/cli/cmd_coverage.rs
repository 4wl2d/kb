use serde_json::json;

use super::Ctx;
use super::args::CoverageArgs;
use super::session::{self, Options, Session, with_snapshot, with_snapshot_line};
use crate::error::{KbError, Result};
use crate::host;
use crate::knowledge::{KnowledgeView, Origin};
use crate::output::CommandOutput;

pub fn run(ctx: &Ctx, args: &CoverageArgs) -> Result<CommandOutput> {
    if !(1..=1000).contains(&args.limit) {
        return Err(KbError::invalid_input("coverage --limit must be 1..1000"));
    }
    let host = session::detect_host(ctx)?
        .ok_or_else(|| KbError::invalid_input("coverage needs --host <checkout>"))?;
    let history = host::facts::history_window(&host.root, &args.since)?;
    let paths: Vec<_> = host::facts::tracked_files(&host.root, None)?
        .into_iter()
        .filter(|p| {
            !host
                .kb_submodule_path
                .as_ref()
                .is_some_and(|kb| p == kb || p.starts_with(&format!("{kb}/")))
        })
        .collect();
    let mut session = Session::open(ctx, Some(host.clone()), Options::reading(ctx, false))?;
    let provider = super::code_options::provider(ctx, &args.code);
    let (report, code) = session.with_view(|view| {
        let repo = args
            .hosts
            .repo
            .clone()
            .or_else(|| host::identify_repo(&host, view.registry()).map(|(id, _)| id))
            .ok_or_else(|| {
                KbError::invalid_input("host is not identified; pass coverage --repo <registry-id>")
            })?;
        if view.registry().repo(&repo).is_none() {
            return Err(KbError::invalid_input(format!("unknown repo {repo}")));
        }
        let code = if provider.configured() {
            Some(provider.load(&crate::code::request(
                &host.root,
                &repo,
                &history.head,
                crate::model::CodeOperation::Refs,
            )?)?)
        } else {
            None
        };
        let fan_in = code
            .as_ref()
            .map(|c| crate::code::fan_in(c, view.registry()));
        Ok((
            crate::coverage::analyze(
                &repo,
                &paths,
                &history.file_changes,
                view.registry(),
                &view.all_records(Origin::Accepted)?,
                fan_in.as_ref(),
                args.limit,
            ),
            code,
        ))
    })?;
    let result = json!({"coverage": report, "history": history,
        "code": code.as_ref().map(|c| json!({"tool": c.tool, "commit": c.commit, "complete": c.complete, "limitations": c.limitations}))});
    let text = serde_json::to_string_pretty(&result)? + "\n";
    Ok(session.finish(CommandOutput::new(
        with_snapshot(result, &session.info),
        with_snapshot_line(text, &session.info, ctx.format),
    )))
}
