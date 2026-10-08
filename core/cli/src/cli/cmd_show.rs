//! `kb show` handler: one full record or one named section from the selected snapshot
//! (freshness per call unless `--offline`), headed by the snapshot provenance line.
//! `--raw` prints the authoritative file bytes exactly as stored; the provenance line then
//! goes to stderr when the content is unverified or not approved.

use super::Ctx;
use super::args::ShowArgs;
use super::session::{self, Options, Session, with_snapshot, with_snapshot_line};
use crate::context::show;
use crate::error::{KbError, Result};
use crate::output::{CommandOutput, Format};

pub fn run(ctx: &Ctx, args: &ShowArgs) -> Result<CommandOutput> {
    if args.sections && args.id.contains('#') {
        return Err(KbError::invalid_input(
            "--sections cannot be combined with id#section",
        ));
    }
    let host = session::detect_host(ctx)?;
    let mut s = Session::open(ctx, host, Options::reading(ctx, args.include_proposals))?;
    let result = s.with_view(|view| {
        show::show(
            view,
            &args.id,
            args.section.as_deref(),
            args.raw,
            args.include_proposals,
        )
    })?;
    if args.sections {
        let text = show::render_sections(&result);
        return Ok(s.finish(CommandOutput::new(with_snapshot(serde_json::json!({"id":result.id,"status":result.record.record.status(),"path":result.path,"sections":result.record.sections}), &s.info), with_snapshot_line(text, &s.info, ctx.format))));
    }
    let text = show::render(&result, ctx.format);
    let out = if args.raw {
        // Raw stdout stays the authoritative bytes; unverified or unapproved content is
        // still flagged on stderr, even with --quiet (it is not progress).
        if ctx.format != Format::Json && s.info.needs_caution() {
            eprintln!("kb: {}", s.info.summary_line());
        }
        CommandOutput::new(with_snapshot(show::to_json(&result), &s.info), text).with_exact_text()
    } else {
        CommandOutput::new(
            with_snapshot(show::to_json(&result), &s.info),
            with_snapshot_line(text, &s.info, ctx.format),
        )
    };
    Ok(s.finish(out))
}
