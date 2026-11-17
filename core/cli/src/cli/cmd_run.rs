//! `kb run <capability> [args...]`: a request the project defines itself. Any command-line
//! tool connected as an `exec` entry of `tools.toml` declares the capabilities it answers;
//! kb routes the request like a built-in one (route order, availability, fallback) and
//! returns the same bounded answer. Skills never change when a capability is added: agents
//! discover them with `kb tools`.

use super::Ctx;
use super::agent;
use super::args::RunArgs;
use crate::answer::Answer;
use crate::error::{KbError, Result};
use crate::model::tools::{Capability, RESERVED_CAPABILITIES};
use crate::output::CommandOutput;
use crate::tools::{self, exec::ExecRequest};

pub fn run(ctx: &Ctx, args: &RunArgs) -> Result<CommandOutput> {
    let cap = args.capability.as_str();
    if Capability::routed(cap).is_some() || RESERVED_CAPABILITIES.contains(&cap) {
        return Err(KbError::invalid_input(format!(
            "`{cap}` is a kb command; run `kbw {cap} ...` instead of `kbw run {cap}`"
        )));
    }
    let ws = agent::workspace(ctx)?;
    let declared = ws.registry.config.custom_capabilities();
    if !declared.contains(cap) {
        let known: Vec<&str> = declared.into_iter().collect();
        return Err(KbError::invalid_input(format!(
            "no tool in {} declares the capability `{cap}`",
            ws.registry.origin()
        ))
        .with_details(serde_json::json!({ "capability": cap, "declared": known }))
        .with_hint(if known.is_empty() {
            "add an exec tool with `capabilities = [\"<name>\"]` to tools.toml (docs/agent-cli.md)"
                .to_string()
        } else {
            format!(
                "declared capabilities: {}; `kbw tools` lists them",
                known.join(", ")
            )
        }));
    }
    let lines = agent::lines(args.answer.lines);
    let request = ExecRequest {
        capability: cap.to_string(),
        subject: args.args.join(" "),
        args: args.args.clone(),
        depth: 2,
        limit: lines,
        ..ExecRequest::default()
    };
    let outcome = tools::answer_exec(
        &ws.registry,
        &ws.roots,
        &request,
        args.answer.tool.as_deref(),
    )?;
    let subject = std::iter::once(cap.to_string())
        .chain(args.args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    let mut answer = Answer::new("run", subject);
    answer.source(outcome.source.clone());
    for n in &outcome.notes {
        answer.note(n.clone());
    }
    if outcome.items.is_empty() {
        answer.note("the tool returned nothing");
    }
    answer.items = outcome.items;
    Ok(agent::output(answer, args.answer.lines, None))
}
