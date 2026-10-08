//! `kb integrate` handler.
//!
//! * `kb integrate --generate [--check|--apply]`: render the KB-level skill bundle.
//! * `kb integrate [--check|--apply [--force]] [--host DIR]`: install the committed bundle
//!   into the host repository.
//!
//! Dry-run is the default; `--check` fails with `DRIFT_DETECTED` on any difference;
//! conflicts (content edited outside kb) fail with `CONFLICT` unless `--apply --force`.

use serde_json::json;

use super::Ctx;
use super::args::IntegrateArgs;
use crate::error::{ErrorCode, KbError, Result};
use crate::integrate::generate::{GeneratePlan, apply_generate, plan_generate};
use crate::integrate::install::{
    InstallPlan, LOCK_PATH, apply_install, conflict_error, plan_install, resolve_host_root,
};
use crate::integrate::{Action, Change, Mode, summarize};
use crate::output::CommandOutput;

pub fn run(ctx: &Ctx, args: &IntegrateArgs) -> Result<CommandOutput> {
    let mode = Mode::from_flags(args.check, args.apply);
    let loc = ctx.location()?;
    if args.generate {
        if args.force {
            return Err(KbError::new(
                ErrorCode::Usage,
                "--force applies to host installation only; `--generate --apply` always rewrites the generated bundle",
            ));
        }
        let plan = plan_generate(&ctx.env.kb_root, &loc)?;
        return generate_output(ctx, mode, plan);
    }
    let host = resolve_host_root(&ctx.env.kb_root, &ctx.env.cwd, ctx.global.host.as_deref())?;
    let plan = plan_install(&ctx.env.kb_root, &loc, &host, args.force)?;
    if args.probe {
        let value = json!({
            "protocol":"kb.harness-probe.v1", "installation_verified":plan.is_clean(), "runtime_load_verified":false,
            "skill_protocol":plan.manifest.skill_protocol, "harnesses":plan.manifest.harnesses,
            "skill_paths":crate::integrate::generate::layout(&plan.manifest.harnesses).skill_dirs,
            "core_receipt":plan.manifest.core.as_ref().map(|c| &c.digest), "core_source":plan.manifest.core_source,
            "changes":plan.changes, "warnings":plan.warnings,
            "challenge":"In a new trusted session in this host, before using tools, report the kb skill protocol, loaded instruction source and always-on core receipt (or no core). Then run the task-start diagnose command from those instructions. Preserve the harness version, raw response and tool-call log.",
            "acceptance":"A matching installation is not runtime-load proof. The reported marker and observed diagnose call must match this installation. Repeat for each enabled harness and after harness updates."
        });
        let text = serde_json::to_string_pretty(&value)? + "\n";
        let out = CommandOutput::new(value, text);
        return Ok(if plan.is_clean() {
            out
        } else {
            out.with_failure(KbError::new(
                ErrorCode::DriftDetected,
                "harness installation probe found drift",
            ))
        });
    }
    install_output(ctx, mode, plan)
}

fn change_lines(changes: &[Change], all: bool) -> String {
    let mut s = String::new();
    for c in changes
        .iter()
        .filter(|c| all || c.action != Action::Unchanged)
    {
        s.push_str(&format!("  {}\n", c.line()));
    }
    s
}

fn human(ctx: &Ctx) -> bool {
    ctx.format == crate::output::Format::Human
}

fn generate_output(ctx: &Ctx, mode: Mode, plan: GeneratePlan) -> Result<CommandOutput> {
    let clean = plan.is_clean();
    if mode == Mode::Apply && !clean {
        apply_generate(&ctx.env.kb_root, &plan)?;
        ctx.env
            .progress("integrate: generated skill bundle updated");
    }
    let m = &plan.bundle.manifest;
    let result = json!({
        "level": "kb",
        "mode": mode,
        "skill_protocol": m.skill_protocol,
        "engine_version": m.engine_version,
        "harnesses": m.harnesses,
        "kb_path": m.kb_path,
        "changes": plan.changes,
        "summary": summarize(plan.changes.iter().map(|c| c.action)),
        "clean": clean,
    });
    let mut text = format!(
        "kb integrate --generate ({}): skill protocol {}, harnesses {}\n",
        mode.as_str(),
        m.skill_protocol,
        m.harnesses
            .iter()
            .map(|h| h.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    text.push_str(&change_lines(&plan.changes, human(ctx)));
    text.push_str(match (clean, mode) {
        (true, _) => "generated bundle is up to date\n",
        (false, Mode::Apply) => "generated bundle written; review and commit it\n",
        (false, Mode::Check) => "generated bundle is stale\n",
        (false, Mode::DryRun) => "re-run with --apply to write these changes\n",
    });
    let out = CommandOutput::new(result, text);
    if mode == Mode::Check && !clean {
        return Ok(out.with_failure(
            KbError::new(
                ErrorCode::DriftDetected,
                "the committed skill bundle differs from the rendered one",
            )
            .with_hint("run `./kbw integrate --generate --apply` and commit project/skill-config/generated/"),
        ));
    }
    Ok(out)
}

fn install_output(ctx: &Ctx, mode: Mode, plan: InstallPlan) -> Result<CommandOutput> {
    let clean = plan.is_clean();
    let conflicts = !plan.conflicts().is_empty();
    let result = json!({
        "level": "host",
        "mode": mode,
        "host": plan.host_root.display().to_string(),
        "skill_protocol": plan.manifest.skill_protocol,
        "engine_version": plan.manifest.engine_version,
        "harnesses": plan.manifest.harnesses,
        "kb_path": plan.manifest.kb_path,
        "changes": plan.changes,
        "lock": {"path": LOCK_PATH, "action": plan.lock_action},
        "warnings": plan.warnings,
        "summary": summarize(plan.changes.iter().map(|c| c.action)),
        "clean": clean,
    });
    let mut text = format!(
        "kb integrate ({}): host {}\n",
        mode.as_str(),
        plan.host_root.display()
    );
    text.push_str(&change_lines(&plan.changes, human(ctx)));
    if plan.lock_action != Action::Unchanged || human(ctx) {
        text.push_str(&format!("  {:<9} {LOCK_PATH}\n", plan.lock_action.as_str()));
    }
    for w in &plan.warnings {
        text.push_str(&format!("warning[{}]: {}\n", w.code, w.message));
    }
    if mode == Mode::Check {
        if !clean {
            text.push_str("host integration differs from the committed bundle\n");
            let out = CommandOutput::new(result, text);
            return Ok(out.with_failure(
                KbError::new(
                    ErrorCode::DriftDetected,
                    "the host integration differs from the committed skill bundle",
                )
                .with_hint(
                    "run `<kb_path>/kbw integrate --apply` in the host and commit the result",
                ),
            ));
        }
        text.push_str("host integration is up to date\n");
        return Ok(CommandOutput::new(result, text));
    }
    if conflicts {
        text.push_str(
            "nothing was written: resolve the conflicts or re-run with --apply --force\n",
        );
        return Ok(CommandOutput::new(result, text).with_failure(conflict_error(&plan)));
    }
    if mode == Mode::Apply {
        if !clean {
            apply_install(&plan)?;
            ctx.env.progress("integrate: host integration updated");
            text.push_str("host integration written; review and commit it (including the lock)\n");
        } else {
            text.push_str("host integration is up to date; nothing written\n");
        }
    } else if clean {
        text.push_str("host integration is up to date\n");
    } else {
        text.push_str("re-run with --apply to write these changes\n");
    }
    Ok(CommandOutput::new(result, text))
}
