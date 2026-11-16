//! `kb check [files] [--base REV]`: run the checks the configuration provides for changed
//! files. Three kinds, all optional:
//!
//! 1. `command` tools of the registry whose `paths` match a changed file (argv only, no
//!    shell, from the KB checkout that supplies the engine);
//! 2. knowledge validation when changed files include knowledge of this profile (code and
//!    documentation changes travel together);
//! 3. with `--base`, the static probes that knowledge records declare (`kb verify`), which
//!    never execute record text.
//!
//! Any failure is `CHECKS_FAILED` (46) with the result printed; `--plan` runs nothing.

use super::Ctx;
use super::agent;
use super::args::{CheckArgs, CodeProviderArgs, VerifyArgs};
use crate::answer::{Answer, Item, Source};
use crate::error::{ErrorCode, KbError, Result};
use crate::impact::ChangeStatus;
use crate::output::CommandOutput;
use crate::source::WorkingTreeSource;
use crate::tools::{self, command};

const TAIL: usize = 6;

pub fn run(ctx: &Ctx, args: &CheckArgs) -> Result<CommandOutput> {
    let ws = agent::workspace(ctx)?;
    let lines = agent::lines(args.lines);
    let loc = ctx.location()?;
    let cwd = ctx
        .env
        .cwd
        .canonicalize()
        .unwrap_or_else(|_| ctx.env.cwd.clone());
    let (files, subject) = if args.files.is_empty() {
        let base = args.base.as_deref().unwrap_or("HEAD");
        let diff = crate::impact::host_diff(
            &ws.roots.code,
            base,
            None,
            true,
            ws.roots.kb_mount.as_deref(),
        )?;
        let files: Vec<String> = diff
            .files
            .iter()
            .filter(|f| f.status != ChangeStatus::Deleted)
            .map(|f| f.path.clone())
            .collect();
        (files, format!("changes since {base}"))
    } else {
        let mut files = Vec::new();
        for f in &args.files {
            files.push(super::cmd_context::host_relative(
                f,
                &cwd,
                Some(ws.roots.code.as_path()),
            )?);
        }
        files.sort();
        files.dedup();
        (files, format!("{} file(s)", args.files.len()))
    };
    let mut answer = Answer::new("check", subject);
    answer.source(Source {
        id: "registry".into(),
        adapter: "tools".into(),
        version: None,
        state: ws.registry.origin(),
    });

    // Knowledge of this profile among the changes: in the code root when the KB is the code
    // root or mounted in it, plus uncommitted changes of a separate KB checkout.
    let profile_prefix = format!("{}/", loc.dir);
    let in_profile = |p: &str| -> bool {
        match &ws.roots.kb_mount {
            Some(m) => p
                .strip_prefix(&format!("{m}/"))
                .is_some_and(|rest| rest.starts_with(&profile_prefix)),
            None => ws.roots.code == ws.roots.kb && p.starts_with(&profile_prefix),
        }
    };
    let mut knowledge_changed = files.iter().any(|f| in_profile(f));
    if !knowledge_changed
        && ws.roots.code != ws.roots.kb
        && args.files.is_empty()
        && let Ok(kb_diff) = crate::impact::host_diff(&ws.roots.kb, "HEAD", None, true, None)
    {
        knowledge_changed = kb_diff
            .files
            .iter()
            .any(|f| f.path.starts_with(&profile_prefix));
    }

    let checks = ws
        .registry
        .config
        .checks_for(ws.roots.repo.as_deref(), &files);
    if checks.is_empty() {
        answer.note(format!(
            "no command check in {} matches the {} changed file(s)",
            ws.registry.origin(),
            files.len()
        ));
    }
    let mut failed = Vec::new();
    for (tool, matched) in &checks {
        let label = tool.description.clone().unwrap_or_else(|| {
            let mut argv = vec![tool.program.clone().unwrap_or_default()];
            argv.extend(command::argv(tool, &ws.roots, &["{files}".to_string()]));
            argv.join(" ")
        });
        if args.plan {
            answer.items.push(
                Item::new("check", "registry")
                    .name(tool.id.clone())
                    .detail(format!("would run for {} file(s): {label}", matched.len())),
            );
            continue;
        }
        let probe = tools::probe(tool, &ws.roots, false);
        if !probe.available {
            failed.push(tool.id.clone());
            answer.items.push(
                Item::new("check", "registry")
                    .name(tool.id.clone())
                    .detail(format!("not run: {}", probe.reason.unwrap_or_default())),
            );
            continue;
        }
        ctx.env
            .progress(format!("check: {} ({} file(s))", tool.id, matched.len()));
        let run = command::run(tool, &probe, &ws.roots, matched, TAIL)?;
        let status = match &run.status {
            command::Status::Passed => "passed".to_string(),
            command::Status::Failed(code) => {
                failed.push(tool.id.clone());
                format!("failed (exit {code})")
            }
            command::Status::Error(e) => {
                failed.push(tool.id.clone());
                format!("error: {e}")
            }
        };
        answer.items.push(
            Item::new("check", "registry")
                .name(tool.id.clone())
                .detail(format!("{status} · {label}")),
        );
        for line in run.tail {
            answer.items.push(
                Item::new("output", "registry")
                    .name(tool.id.clone())
                    .detail(line),
            );
        }
    }

    if knowledge_changed {
        answer.source(Source {
            id: "knowledge".into(),
            adapter: "knowledge".into(),
            version: None,
            state: "working tree".into(),
        });
        if args.plan {
            answer.items.push(
                Item::new("check", "knowledge")
                    .name("knowledge")
                    .detail(format!("would validate {}", loc.dir)),
            );
        } else {
            let corpus =
                crate::corpus::load_corpus(&WorkingTreeSource::new(&ctx.env.kb_root), &loc)?;
            let report = crate::validate::validate_corpus(&corpus);
            if report.errors > 0 {
                failed.push("knowledge".into());
            }
            answer.items.push(
                Item::new("check", "knowledge")
                    .name("knowledge")
                    .detail(format!(
                        "{} · {} records, {} error(s), {} warning(s)",
                        if report.errors > 0 {
                            "failed"
                        } else {
                            "passed"
                        },
                        report.records,
                        report.errors,
                        report.warnings
                    )),
            );
            for d in report
                .diagnostics
                .iter()
                .filter(|d| d.severity != crate::diag::Severity::Info)
                .take(TAIL)
            {
                let mut it = Item::new("output", "knowledge")
                    .name(d.code.clone())
                    .detail(d.message.clone());
                if let Some(p) = &d.path {
                    it = it.at(p.clone(), None);
                }
                answer.items.push(it);
            }
        }
    }

    if let Some(base) = &args.base {
        if ws.host.is_some() {
            probes(ctx, base, args.plan, &mut answer, &mut failed);
        } else {
            answer.note("declared probes need a host repository; skipped (pass --host)");
        }
    } else {
        answer.next("kb check --base <target> (adds the probes knowledge declares)");
    }
    if files.is_empty() && !knowledge_changed {
        answer.note("no changed files");
    }
    let failure = (!failed.is_empty()).then(|| {
        KbError::new(
            ErrorCode::ChecksFailed,
            format!("{} check(s) failed: {}", failed.len(), failed.join(", ")),
        )
        .with_details(serde_json::json!({ "failed": failed }))
    });
    Ok(agent::output(answer, lines, failure))
}

/// Declared static probes over `base`..work tree, through `kb verify`.
fn probes(ctx: &Ctx, base: &str, plan: bool, answer: &mut Answer, failed: &mut Vec<String>) {
    if plan {
        answer.items.push(
            Item::new("check", "knowledge")
                .name("probes")
                .detail(format!("would evaluate declared probes against {base}")),
        );
        return;
    }
    let verify = VerifyArgs {
        diff: base.to_string(),
        head: None,
        staged: false,
        index_file: None,
        repo: None,
        ids: Vec::new(),
        commit_message: None,
        branch: None,
        only: Vec::new(),
        applicable: Vec::new(),
        change_types: Vec::new(),
        host_versions: Vec::new(),
        strict: false,
        code: CodeProviderArgs {
            provider: None,
            provider_args: Vec::new(),
            provider_files: Vec::new(),
            provider_timeout: 30,
        },
    };
    match super::cmd_verify::run(ctx, &verify) {
        Ok(out) => {
            let counts = &out.result["verification"]["counts"];
            let n = |k: &str| counts[k].as_u64().unwrap_or(0);
            let state = match &out.failure {
                Some(f) => {
                    failed.push("probes".into());
                    format!("failed ({})", f.code.as_str())
                }
                None => "passed".into(),
            };
            answer.items.push(
                Item::new("check", "knowledge")
                    .name("probes")
                    .detail(format!(
                        "{state} · {} passed, {} failed, {} unverifiable, {} skipped",
                        n("passed"),
                        n("failed"),
                        n("unverifiable"),
                        n("skipped")
                    )),
            );
        }
        Err(e) if e.code == ErrorCode::ProjectNotInitialized => {
            answer.note("probes skipped: the project knowledge is not initialized");
        }
        Err(e) => {
            failed.push("probes".into());
            answer.items.push(
                Item::new("check", "knowledge")
                    .name("probes")
                    .detail(format!("error: {}", e.message)),
            );
        }
    }
}
