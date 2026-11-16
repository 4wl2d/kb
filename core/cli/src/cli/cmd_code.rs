//! `kb sym`, `kb callers` and the symbol mode of `kb impact`: code requests routed through
//! the tool registry (docs/agent-cli.md). The answer names the tool and the state of what
//! it read; a symbol no tool knows is `NOT_FOUND` with the result still printed.

use super::Ctx;
use super::agent::{self, Detail};
use super::args::{CallersArgs, ImpactArgs, SymArgs};
use crate::answer::Answer;
use crate::error::{KbError, Result};
use crate::model::tools::Capability;
use crate::output::CommandOutput;
use crate::tools::{self, CodeQuery};

fn shell_word(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "_-.:/#".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

pub fn sym(ctx: &Ctx, args: &SymArgs) -> Result<CommandOutput> {
    let ws = agent::workspace(ctx)?;
    let lines = agent::lines(args.answer.lines);
    let q = CodeQuery {
        capability: Capability::Sym,
        name: args.name.clone(),
        file: args.file.clone(),
        kind: args.kind.clone(),
        depth: 1,
        limit: lines,
    };
    let outcome = tools::answer_code(&ws.registry, &ws.roots, &q, args.answer.tool.as_deref())?;
    let mut answer = Answer::new("sym", shell_word(&args.name));
    answer.source(outcome.source.clone());
    for n in &outcome.notes {
        answer.note(n.clone());
    }
    if outcome.found {
        answer.next(format!("kb callers {}", shell_word(&args.name)));
        if let Some(path) = outcome.items.first().and_then(|i| i.path.clone()) {
            answer.next(format!("kb doc {}", shell_word(&path)));
        }
    }
    answer.items = outcome.items;
    let failure = (!outcome.found).then(|| {
        agent::not_found(format!("no definition of `{}` was found", args.name)).with_hint(
            "check the spelling, try `kbw find <text>`, or see `kbw tools` for the connected tools",
        )
    });
    Ok(agent::output(answer, lines, failure))
}

pub fn callers(ctx: &Ctx, args: &CallersArgs) -> Result<CommandOutput> {
    let ws = agent::workspace(ctx)?;
    let lines = agent::lines(args.answer.lines);
    let q = CodeQuery {
        capability: Capability::Callers,
        name: args.symbol.clone(),
        file: args.file.clone(),
        kind: None,
        depth: 1,
        limit: lines,
    };
    let outcome = tools::answer_code(&ws.registry, &ws.roots, &q, args.answer.tool.as_deref())?;
    let mut answer = Answer::new("callers", shell_word(&args.symbol));
    answer.source(outcome.source.clone());
    for n in &outcome.notes {
        answer.note(n.clone());
    }
    if outcome.found && outcome.items.is_empty() {
        answer.note("no callers were found");
    }
    if outcome.found {
        answer.next(format!("kb impact {}", shell_word(&args.symbol)));
    }
    answer.items = outcome.items;
    let failure =
        (!outcome.found).then(|| agent::not_found(format!("`{}` was not found", args.symbol)));
    Ok(agent::output(answer, lines, failure))
}

/// `kb impact <symbol>`: the code that depends on the symbol, then the features and the
/// knowledge records that govern the affected files.
pub fn impact(ctx: &Ctx, args: &ImpactArgs, symbol: &str) -> Result<CommandOutput> {
    if super::code_options::provider(ctx, &args.code).configured() {
        return Err(KbError::invalid_input(
            "provider options belong to diff mode with --deep; symbol mode uses the tool registry",
        ));
    }
    if !(1..=4).contains(&args.depth) {
        return Err(KbError::invalid_input("impact depth must be 1..4"));
    }
    let ws = agent::workspace(ctx)?;
    let lines = agent::lines(args.answer.lines);
    let q = CodeQuery {
        capability: Capability::Impact,
        name: symbol.to_string(),
        file: args.file.clone(),
        kind: None,
        depth: args.depth,
        limit: lines,
    };
    let outcome = tools::answer_code(&ws.registry, &ws.roots, &q, args.answer.tool.as_deref())?;
    let mut answer = Answer::new("impact", shell_word(symbol));
    answer.source(outcome.source.clone());
    for n in &outcome.notes {
        answer.note(n.clone());
    }
    if !outcome.found {
        let failure = agent::not_found(format!("`{symbol}` was not found"));
        return Ok(agent::output(answer, lines, Some(failure)));
    }
    if !outcome.files.is_empty() {
        let mut knowledge = Answer::new("impact", "");
        match agent::knowledge(ctx, &ws, &outcome.files, Detail::Impact, &mut knowledge) {
            Ok(()) => {
                for s in knowledge.sources {
                    answer.source(s);
                }
                answer.items.extend(knowledge.items);
                for n in knowledge.notes {
                    answer.note(n);
                }
            }
            Err(e) => answer.note(format!("knowledge not consulted: {}", e.message)),
        }
    }
    let affected = outcome.items.len();
    answer.items.extend(outcome.items);
    if affected == 0 {
        answer.note("no dependent code was found");
    }
    answer.note(format!(
        "{affected} dependent symbol(s) in {} file(s); static analysis, not runtime reachability",
        outcome.files.len()
    ));
    answer.next(format!("kb callers {}", shell_word(symbol)));
    if let Some(path) = outcome.files.first() {
        answer.next(format!("kb doc {}", shell_word(path)));
    }
    Ok(agent::output(answer, lines, None))
}

#[cfg(test)]
mod tests {
    #[test]
    fn shell_words_quote_only_when_needed() {
        assert_eq!(super::shell_word("Type::name"), "Type::name");
        assert_eq!(super::shell_word("a b"), "'a b'");
        assert_eq!(super::shell_word("it's"), "'it'\\''s'");
    }
}
