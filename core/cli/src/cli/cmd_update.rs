//! `kb update {check,prepare,divergence,abandon}` handler.

use super::Ctx;
use super::args::{UpdateCommand, UpdateSourceArgs};
use crate::error::{KbError, Result};
use crate::output::{CommandOutput, Format};
use crate::update::{self, KbwRunner, UpstreamSource};

pub fn run(ctx: &Ctx, cmd: &UpdateCommand) -> Result<CommandOutput> {
    let human = ctx.format == Format::Human;
    match cmd {
        UpdateCommand::Check(a) => {
            let report = update::check(&ctx.env, &ctx.location()?, &source(ctx, a)?)?;
            let text = update::render_check(&report, human);
            let failure = report.failure();
            with_failure(
                CommandOutput::new(serde_json::to_value(&report)?, text),
                failure,
            )
        }
        UpdateCommand::Prepare(a) => {
            let runner = KbwRunner::new(&ctx.env.kb_root);
            let report = update::prepare(
                &ctx.env,
                &ctx.location()?,
                &source(ctx, &a.source)?,
                a.branch.as_deref(),
                &runner,
            )?;
            let text = update::render_prepare(&report, human);
            Ok(CommandOutput::new(serde_json::to_value(&report)?, text))
        }
        UpdateCommand::Divergence => {
            let report = update::divergence(&ctx.env)?;
            let text = update::render_divergence(&report, human);
            let failure = report.failure();
            with_failure(
                CommandOutput::new(serde_json::to_value(&report)?, text),
                failure,
            )
        }
        UpdateCommand::Abandon(a) => {
            let report = update::abandon(&ctx.env, &a.branch, a.apply, a.force)?;
            let text = update::render_abandon(&report);
            Ok(CommandOutput::new(serde_json::to_value(&report)?, text))
        }
    }
}

fn source(ctx: &Ctx, a: &UpdateSourceArgs) -> Result<UpstreamSource> {
    if ctx.global.offline {
        return Err(KbError::invalid_input(
            "`update check` and `update prepare` fetch the upstream; --offline does not apply",
        ));
    }
    Ok(UpstreamSource {
        upstream: a.upstream.clone(),
        reference: a.reference.clone(),
    })
}

fn with_failure(out: CommandOutput, failure: Option<KbError>) -> Result<CommandOutput> {
    Ok(match failure {
        Some(f) => out.with_failure(f),
        None => out,
    })
}
