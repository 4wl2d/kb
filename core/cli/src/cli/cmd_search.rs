//! `kb search` handler: ranked record lookup in the selected snapshot (freshness per call
//! unless `--offline`). Search is not a context assembly and never decides obligations.

use super::Ctx;
use super::args::SearchArgs;
use super::session::{self, Options, Session, with_snapshot, with_snapshot_line};
use crate::context::search;
use crate::error::{KbError, Result};
use crate::model::Kind;
use crate::output::CommandOutput;

pub fn run(ctx: &Ctx, args: &SearchArgs) -> Result<CommandOutput> {
    let kinds = parse_kinds(&args.kinds)?;
    let host = session::detect_host(ctx)?;
    let mut s = Session::open(ctx, host, Options::reading(ctx, args.include_proposals))?;
    let result = s.with_view(|view| {
        search::search(
            view,
            &args.query,
            &kinds,
            args.limit,
            args.include_proposals,
        )
    })?;
    let out = CommandOutput::new(
        with_snapshot(search::to_json(&result), &s.info),
        with_snapshot_line(search::render(&result, ctx.format), &s.info, ctx.format),
    );
    Ok(s.finish(out))
}

/// `--kind` values; an unknown kind is `INVALID_INPUT` (never silently ignored).
fn parse_kinds(values: &[String]) -> Result<Vec<Kind>> {
    let mut kinds = Vec::new();
    for v in values {
        let k = Kind::parse(v).ok_or_else(|| {
            let known: Vec<&str> = Kind::ALL.iter().map(|k| k.as_str()).collect();
            KbError::invalid_input(format!(
                "unknown record kind `{v}`; expected one of: {}",
                known.join(", ")
            ))
        })?;
        if !kinds.contains(&k) {
            kinds.push(k);
        }
    }
    Ok(kinds)
}
