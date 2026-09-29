//! `kb doctor` handler. Exit 0 unless a check fails (`VALIDATION_FAILED`, 40); warnings
//! and skipped checks do not fail the command.

use serde_json::json;

use super::Ctx;
use super::args::DoctorArgs;
use crate::doctor::{self, DoctorOptions};
use crate::error::{ErrorCode, KbError, Result};
use crate::output::CommandOutput;

pub fn run(ctx: &Ctx, args: &DoctorArgs) -> Result<CommandOutput> {
    if args.online && ctx.global.offline {
        return Err(KbError::invalid_input(
            "`doctor --online` fetches the approved ref; --offline does not apply",
        )
        .with_hint("drop --offline to check freshness, or --online to diagnose offline"));
    }
    let loc = ctx.location()?;
    if args.online {
        ctx.env
            .progress("doctor: fetching the approved ref (--online)");
    }
    let opts = DoctorOptions {
        host: ctx.global.host.as_deref(),
        online: args.online,
        fingerprint: std::env::var("KBW_FINGERPRINT").ok(),
    };
    let report = doctor::run(&ctx.env, &loc, &opts);
    let text = doctor::render(&report, ctx.format);
    let failed = report.failed();
    let mut out = CommandOutput::new(serde_json::to_value(&report)?, text);
    if !failed.is_empty() {
        out = out.with_failure(
            KbError::new(
                ErrorCode::ValidationFailed,
                format!(
                    "doctor: {} check(s) failed: {}",
                    failed.len(),
                    failed.join(", ")
                ),
            )
            .with_details(json!({ "failed": failed })),
        );
    }
    Ok(out)
}
