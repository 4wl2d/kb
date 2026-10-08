//! `kb init` handler: dry-run plan by default, writes with `--apply`.

use serde_json::json;

use super::Ctx;
use super::args::{HarnessArg, InitArgs, ProfileArg};
use crate::error::{ErrorCode, KbError, Result};
use crate::init::{InitOptions, InitPlan, apply, plan};
use crate::integrate::{Action, summarize};
use crate::model::Harness;
use crate::output::CommandOutput;

fn harness_of(h: HarnessArg) -> Harness {
    match h {
        HarnessArg::Claude => Harness::Claude,
        HarnessArg::Codex => Harness::Codex,
        HarnessArg::Cursor => Harness::Cursor,
        HarnessArg::Grok => Harness::Grok,
        HarnessArg::Copilot => Harness::Copilot,
        HarnessArg::Junie => Harness::Junie,
    }
}

pub fn run(ctx: &Ctx, args: &InitArgs) -> Result<CommandOutput> {
    if ctx.global.profile != ProfileArg::Project || ctx.global.config.is_some() {
        return Err(KbError::new(
            ErrorCode::Usage,
            "`kb init` creates the project profile (project/project.toml); --profile maintainer and --config are not supported",
        ));
    }
    let opts = InitOptions {
        name: args.name.clone(),
        namespace: args.namespace.clone(),
        remote: args.remote.clone(),
        approved_ref: args.approved_ref.clone(),
        kb_path: args.kb_path.clone(),
        harnesses: args.harnesses.iter().copied().map(harness_of).collect(),
        upstream_url: args.upstream_url.clone(),
        example: args.example.clone(),
    };
    let root = &ctx.env.kb_root;
    let p = plan(root, &opts)?;
    let written = if args.apply {
        let n = apply(root, &p)?;
        ctx.env.progress(format!("init: wrote {n} file(s)"));
        n
    } else {
        0
    };
    let mode = if args.apply { "apply" } else { "dry-run" };
    let result = json!({
        "mode": mode,
        "plan": p,
        "summary": summarize(p.changes.iter().map(|c| c.action)),
        "written": written,
    });
    Ok(CommandOutput::new(
        result,
        render_text(&p, args.apply, written),
    ))
}

fn render_text(p: &InitPlan, applied: bool, written: usize) -> String {
    let mut s = String::new();
    let what = match &p.example {
        Some(e) => format!("synthetic example `{e}` (namespace `{}`)", p.namespace),
        None => format!("project `{}` (namespace `{}`)", p.name, p.namespace),
    };
    if applied {
        s.push_str(&format!(
            "kb init: initialized {what}; wrote {written} file(s)\n"
        ));
    } else {
        s.push_str(&format!("kb init (dry-run, nothing written): {what}\n"));
    }
    for c in &p.changes {
        s.push_str(&format!("  {}\n", c.line()));
    }
    let count = |a: Action| p.changes.iter().filter(|c| c.action == a).count();
    s.push_str(&format!(
        "{} to create, {} to replace, {} kept (existing files are never overwritten), {} unchanged\n",
        count(Action::Create),
        count(Action::Replace),
        count(Action::Keep),
        count(Action::Unchanged)
    ));
    s.push_str(&format!(
        "approved source: remote `{}` ref `{}`; host mount path `{}`; harnesses: {}\n",
        p.remote,
        p.approved_ref,
        p.kb_path,
        p.harnesses
            .iter()
            .map(|h| h.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    if applied {
        s.push_str("next: review and commit project/ and .github/workflows/kb-knowledge.yml, then run `./kbw validate`\n");
    } else {
        s.push_str("next: re-run with --apply to write these files\n");
    }
    s
}
