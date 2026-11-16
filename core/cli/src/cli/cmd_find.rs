//! `kb find "<text>"`: search the code root's text through the `find` route (ripgrep, else
//! the built-in `git grep`). Results are sorted by path and line, at most 20 per file, and
//! bounded like every agent answer; the KB checkout mounted in the host is not searched.

use super::Ctx;
use super::agent;
use super::args::FindArgs;
use crate::answer::Answer;
use crate::error::{KbError, Result};
use crate::model::tools::Capability;
use crate::output::CommandOutput;
use crate::tools::{self, grep};

pub fn run(ctx: &Ctx, args: &FindArgs) -> Result<CommandOutput> {
    if args.text.is_empty() || args.text.contains('\n') || args.text.contains('\0') {
        return Err(KbError::invalid_input(
            "search text must be one non-empty line",
        ));
    }
    for g in &args.globs {
        crate::glob::RepoGlob::parse(g.strip_prefix('!').unwrap_or(g))
            .map_err(KbError::invalid_input)?;
    }
    let ws = agent::workspace(ctx)?;
    let lines = agent::lines(args.answer.lines);
    let query = grep::FindQuery {
        text: args.text.clone(),
        regex: args.regex,
        globs: args.globs.clone(),
    };
    let mut notes = Vec::new();
    let mut result = None;
    for tool in tools::route_for(
        &ws.registry,
        Capability::Find.as_str(),
        args.answer.tool.as_deref(),
    )? {
        let probe = tools::probe(tool, &ws.roots, true);
        if !probe.available {
            notes.push(format!(
                "{} skipped: {}",
                tool.id,
                probe.reason.unwrap_or_default()
            ));
            continue;
        }
        let found = if tool.adapter == crate::model::tools::AdapterKind::Exec {
            if args.regex {
                notes.push(format!(
                    "{} receives the text as given; --regex is up to the tool",
                    tool.id
                ));
            }
            tools::exec::query(
                tool,
                &probe,
                &ws.roots,
                &tools::exec::ExecRequest {
                    capability: "find".into(),
                    subject: args.text.clone(),
                    globs: args.globs.clone(),
                    limit: lines,
                    ..tools::exec::ExecRequest::default()
                },
            )
        } else {
            grep::find(tool, &probe, &ws.roots, &query)
        };
        match found {
            Ok(outcome) => {
                result = Some(outcome);
                break;
            }
            Err(e) => notes.push(format!("{} failed: {}", tool.id, e.message)),
        }
    }
    let outcome =
        result.ok_or_else(|| tools::unavailable(Capability::Find.as_str(), notes.clone()))?;
    let mut answer = Answer::new("find", format!("{:?}", args.text));
    answer.source(outcome.source.clone());
    for n in notes.into_iter().chain(outcome.notes) {
        answer.note(n);
    }
    if let Some(first) = outcome.items.first().and_then(|i| i.path.clone()) {
        answer.next(format!("kb doc {first}"));
    }
    answer.items = outcome.items;
    let failure =
        (!outcome.found).then(|| agent::not_found(format!("no match for {:?}", args.text)));
    Ok(agent::output(answer, lines, failure))
}
