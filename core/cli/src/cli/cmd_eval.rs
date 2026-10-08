use serde_json::json;

use super::Ctx;
use super::args::{EvalCommand, EvalHistoryArgs, ValidateArgs};
use super::session::{self, Options, Session, with_snapshot, with_snapshot_line};
use crate::error::{ErrorCode, KbError, Result};
use crate::knowledge::KnowledgeView;
use crate::output::CommandOutput;

pub fn run(ctx: &Ctx, args: &EvalCommand) -> Result<CommandOutput> {
    match args {
        EvalCommand::Routing(args) if args.example.is_some() => example(ctx, args),
        EvalCommand::Routing(args) => super::cmd_validate::run_with_context_format(
            ctx,
            &ValidateArgs {
                stale: None,
                on: None,
                no_routing: false,
                base: None,
                templates: false,
                strict: args.strict,
            },
            args.context_format.into(),
        ),
        EvalCommand::History(args) => history(ctx, args),
    }
}

fn example(ctx: &Ctx, args: &super::args::EvalRoutingArgs) -> Result<CommandOutput> {
    let name = args.example.as_deref().unwrap_or_default();
    if !crate::init::list_examples(&ctx.env.kb_root)
        .iter()
        .any(|n| n == name)
        || ctx
            .global
            .snapshot
            .as_deref()
            .is_some_and(|s| s != "working-tree")
    {
        return Err(KbError::invalid_input(
            "eval routing --example needs a shipped example and its working-tree source",
        ));
    }
    let dir = format!("{}/{name}/project", crate::init::EXAMPLES_DIR);
    let loc = crate::model::ProfileLocation {
        profile: crate::model::Profile::Project,
        config: format!("{dir}/project.toml"),
        dir,
    };
    let source = crate::source::WorkingTreeSource::new(&ctx.env.kb_root);
    let corpus = crate::corpus::load_corpus(&source, &loc)?;
    let mut validation = crate::validate::validate_corpus(&corpus);
    let (cases, diags) = crate::context::routing::load_cases(&source, &loc)?;
    validation.extend(diags);
    let view =
        crate::context::memory::MemoryView::from_corpus(&corpus, validation.diagnostics.clone());
    let env = crate::context::TaskEnv::new(super::cmd_validate::working_tree_info(&loc, &corpus));
    let routing = crate::context::routing::run_cases_in_format(
        &view,
        &cases,
        &env,
        args.context_format.into(),
    );
    let failure = (!validation.is_ok(args.strict))
        .then(|| {
            KbError::new(ErrorCode::ValidationFailed, "example validation failed")
                .with_diagnostics(validation.diagnostics.clone())
        })
        .or_else(|| routing.failure());
    let value = json!({"example":name,"validation":validation,"routing":crate::context::routing::report_to_json(&routing)});
    let text = crate::context::routing::render_report(&routing, ctx.format);
    let mut output = CommandOutput::new(value, text);
    if let Some(error) = failure {
        output = output.with_failure(error);
    }
    Ok(output)
}

fn history(ctx: &Ctx, args: &EvalHistoryArgs) -> Result<CommandOutput> {
    let host = session::detect_host(ctx)?
        .ok_or_else(|| KbError::invalid_input("eval history needs --host <checkout>"))?;
    let labels = args
        .labels
        .as_ref()
        .map(|path| crate::evaluation::load_labels(&ctx.env.cwd.join(path)))
        .transpose()?
        .unwrap_or_default();
    let changes = crate::evaluation::changes(&host.root, &args.range, args.max_changes)?;
    let mut session = Session::open(ctx, Some(host.clone()), Options::reading(ctx, false))?;
    let info = session.info.clone();
    let report = session.with_view(|view| {
        if view.diagnostics().iter().any(|d| d.is_error()) {
            return Err(KbError::new(
                ErrorCode::ValidationFailed,
                "cannot evaluate invalid knowledge",
            )
            .with_diagnostics(view.diagnostics().to_vec()));
        }
        let repo = args
            .hosts
            .repo
            .clone()
            .or_else(|| crate::host::identify_repo(&host, view.registry()).map(|(id, _)| id))
            .ok_or_else(|| KbError::invalid_input("eval history needs an identified --repo"))?;
        if view.registry().repo(&repo).is_none() {
            return Err(KbError::invalid_input("unknown eval history --repo"));
        }
        let roots = crate::provenance::host_roots(
            Some(&host),
            view.registry(),
            Some(&repo),
            &args.hosts.repo_roots,
            &ctx.env.cwd,
        )?;
        crate::evaluation::history(
            view,
            &info,
            &changes,
            &labels,
            &crate::evaluation::HistoryOptions {
                kb_root: &ctx.env.kb_root,
                host: &host,
                roots: &roots,
                repo: &repo,
                as_of: args.as_of,
            },
        )
    })?;
    let fail = args.check && report["incomplete_changes"].as_u64().unwrap_or_default() > 0;
    let value = json!({"history": report});
    let text = serde_json::to_string_pretty(&value)? + "\n";
    let mut out = CommandOutput::new(
        with_snapshot(value, &info),
        with_snapshot_line(text, &info, ctx.format),
    );
    if fail {
        out = out.with_failure(KbError::new(ErrorCode::ContextIncomplete, "history includes incomplete context, missing label ids or unverifiable temporal inputs"));
    }
    Ok(session.finish(out))
}
