//! `kb doc <path|symbol>`: the feature documentation for the code a path or symbol belongs
//! to — purpose, invariant statements, contracts, entry points, limits and gaps — selected
//! by the same scope rules as `context --intent implement` and bounded like every agent
//! answer. A symbol is first located through the `sym` route.

use std::path::Path;

use super::Ctx;
use super::agent::{self, Detail};
use super::args::DocArgs;
use crate::answer::Answer;
use crate::error::Result;
use crate::model::tools::Capability;
use crate::output::CommandOutput;
use crate::tools::{self, CodeQuery};

/// Is `subject` a path rather than a symbol name?
pub fn is_path(subject: &str, cwd: &Path, code_root: &Path) -> bool {
    if subject.contains("::") {
        return false;
    }
    subject.contains('/') || cwd.join(subject).exists() || code_root.join(subject).exists()
}

pub fn run(ctx: &Ctx, args: &DocArgs) -> Result<CommandOutput> {
    let ws = agent::workspace(ctx)?;
    let lines = agent::lines(args.answer.lines);
    let cwd = ctx
        .env
        .cwd
        .canonicalize()
        .unwrap_or_else(|_| ctx.env.cwd.clone());
    let mut answer = Answer::new("doc", args.subject.clone());
    let paths = if is_path(&args.subject, &cwd, &ws.roots.code) {
        vec![super::cmd_context::host_relative(
            &args.subject,
            &cwd,
            Some(ws.roots.code.as_path()),
        )?]
    } else {
        let q = CodeQuery {
            capability: Capability::Sym,
            name: args.subject.clone(),
            file: None,
            kind: None,
            depth: 1,
            limit: lines,
        };
        let outcome = tools::answer_code(&ws.registry, &ws.roots, &q, args.answer.tool.as_deref())?;
        answer.source(outcome.source.clone());
        let mut paths: Vec<String> = Vec::new();
        for item in outcome.items.iter().filter(|i| i.kind == "definition") {
            if let Some(p) = &item.path
                && !paths.contains(p)
            {
                paths.push(p.clone());
            }
        }
        if !outcome.found || paths.is_empty() {
            for n in outcome.notes {
                answer.note(n);
            }
            let failure = agent::not_found(format!(
                "`{}` is neither a path nor a symbol a connected tool knows",
                args.subject
            ))
            .with_hint("pass a host-relative path, or check the name with `kbw sym <name>`");
            return Ok(agent::output(answer, lines, Some(failure)));
        }
        if paths.len() > 3 {
            answer.note(format!(
                "`{}` is defined in {} files; documenting the first 3",
                args.subject,
                paths.len()
            ));
            paths.truncate(3);
        }
        answer.note(format!(
            "`{}` is defined in {}",
            args.subject,
            paths.join(", ")
        ));
        paths
    };
    agent::knowledge(ctx, &ws, &paths, Detail::Doc, &mut answer)?;
    let path_args: Vec<String> = paths.iter().map(|p| format!("--path {p}")).collect();
    answer.next(format!(
        "kb context --intent implement {}",
        path_args.join(" ")
    ));
    Ok(agent::output(answer, lines, None))
}

#[cfg(test)]
mod tests {
    use super::is_path;
    use std::path::Path;

    #[test]
    fn paths_and_symbols_are_told_apart() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(is_path("src/lib.rs", here, here));
        assert!(is_path("Cargo.toml", here, here));
        assert!(is_path("mobile:app/auth/x.kt", here, here));
        assert!(!is_path("TokenRefresher::refresh", here, here));
        assert!(!is_path("refresh", here, here));
    }
}
