//! `kb show` handler: one full record or one named section from the selected snapshot
//! (freshness per call unless `--offline`), headed by the snapshot provenance line.
//! `--raw` prints the authoritative file bytes exactly as stored; the provenance line then
//! goes to stderr when the content is unverified or not approved.

use super::Ctx;
use super::args::ShowArgs;
use super::session::{self, Options, Session, with_snapshot, with_snapshot_line};
use std::path::Path;

use crate::context::show::{self, AnchorTarget};
use crate::error::{KbError, Result};
use crate::knowledge::KnowledgeView;
use crate::model::{Anchor, AnchorKind};
use crate::output::{CommandOutput, Format};
use crate::tools::grep;
use crate::util::safe_join;

/// Largest anchored file scanned for its symbol.
const MAX_ANCHOR_FILE: u64 = 2 * 1024 * 1024;

/// Look an anchor up in the code that is checked out here: the host for anchors of its
/// repository, or the KB checkout for anchors without one when no host is in play. Other
/// repositories are not consulted. A present file is annotated only with a line that
/// names its symbol.
fn anchor_target(
    anchor: &Anchor,
    code: Option<&(String, &Path)>,
    kb_root: &Path,
    names: &[String],
) -> Option<AnchorTarget> {
    let path = anchor.path.as_deref()?;
    let root = match (&anchor.repo, code) {
        (Some(repo), Some((host_repo, root))) if repo == host_repo => *root,
        (None, None) => kb_root,
        _ => return None,
    };
    let file = safe_join(root, path).ok()?;
    let Ok(meta) = std::fs::metadata(&file) else {
        return Some(AnchorTarget::Missing);
    };
    if !meta.is_file() || meta.len() > MAX_ANCHOR_FILE {
        return None;
    }
    let text = std::fs::read_to_string(&file).ok()?;
    match anchor.symbol.as_deref() {
        Some(symbol) => grep::declaration_line(&text, symbol).map(AnchorTarget::Line),
        // A document is read next: say how long it is and where it speaks of the record.
        None if anchor.kind == AnchorKind::Doc => Some(AnchorTarget::Document {
            lines: u32::try_from(text.lines().count()).ok()?,
            mentions: show::document_mentions(&text, anchor.note.as_deref(), names),
        }),
        None => None,
    }
}

/// Every string of a record's typed part, as written (not as JSON escapes it), for the
/// code names it puts in backticks.
fn strings(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => out.push(text.clone()),
        serde_json::Value::Array(items) => items.iter().for_each(|item| strings(item, out)),
        serde_json::Value::Object(fields) => fields.values().for_each(|field| strings(field, out)),
        _ => {}
    }
}

pub fn run(ctx: &Ctx, args: &ShowArgs) -> Result<CommandOutput> {
    if args.sections && args.id.contains('#') {
        return Err(KbError::invalid_input(
            "--sections cannot be combined with id#section",
        ));
    }
    let host = session::detect_host(ctx)?;
    let mut s = Session::open(ctx, host, Options::reading(ctx, args.include_proposals))?;
    let host = s.host.clone();
    let kb_root = ctx.env.kb_root.clone();
    let result = s.with_view(|view| {
        let mut result = show::show(
            view,
            &args.id,
            args.section.as_deref(),
            args.raw,
            args.include_proposals,
        )?;
        if !args.raw && !args.sections && result.section.is_none() {
            let code = host.as_ref().and_then(|h| {
                let (repo, _) = crate::host::identify_repo(h, view.registry())?;
                Some((repo, h.root.as_path()))
            });
            // The names the record writes as code, in its statements and then its sections.
            // Each string is read on its own, so that a stray backtick in one does not
            // turn the others inside out.
            let mut texts = Vec::new();
            if let Ok(value) = serde_json::to_value(&result.record.record) {
                strings(&value, &mut texts);
            }
            texts.extend(result.record.sections.iter().map(|s| s.markdown.clone()));
            let mut names: Vec<String> = Vec::new();
            for name in texts.iter().flat_map(|text| show::code_names(text)) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            result.targets = result
                .record
                .record
                .common()
                .anchors
                .iter()
                .map(|anchor| anchor_target(anchor, code.as_ref(), &kb_root, &names))
                .collect();
        }
        Ok(result)
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
