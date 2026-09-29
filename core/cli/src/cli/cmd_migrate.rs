//! `kb migrate` handler: dry-run by default (per-file unified diff), `--apply` writes.

use serde_json::json;

use super::Ctx;
use super::args::MigrateArgs;
use crate::error::Result;
use crate::migrate;
use crate::output::{CommandOutput, Format};

pub fn run(ctx: &Ctx, args: &MigrateArgs) -> Result<CommandOutput> {
    let loc = ctx.location()?;
    let plan = migrate::plan(&ctx.env.kb_root, &loc, args.to)?;
    let human = ctx.format == Format::Human;
    if args.apply {
        let report = migrate::apply(&plan)?;
        if !report.written.is_empty() {
            ctx.env.progress(format!(
                "migrated {} file(s) to schema {}",
                report.written.len(),
                report.target
            ));
        }
        let mut result = serde_json::to_value(&report)?;
        result["mode"] = json!("apply");
        result["config"] = json!(plan.config);
        return Ok(CommandOutput::new(
            result,
            migrate::render_apply(&report, human),
        ));
    }
    let changes = migrate::preview(&plan)?;
    Ok(CommandOutput::new(
        migrate::plan_json(&plan, &changes),
        migrate::render_plan(&plan, &changes, human),
    ))
}
