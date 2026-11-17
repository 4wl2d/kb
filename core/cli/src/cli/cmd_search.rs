//! `kb search` handler: ranked record lookup in the selected snapshot (freshness per call
//! unless `--offline`). Search is not a context assembly and never decides obligations.
//! In a host checkout the answer also lists the code files named like the query.

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
    let mut result = s.with_view(|view| {
        search::search(
            view,
            &args.query,
            &kinds,
            args.limit,
            args.include_proposals,
        )
    })?;
    if let Some(host) = &s.host {
        // The knowledge base mounted in the host is searched as records, not as code.
        let mount = host.kb_submodule_path.as_deref().map(|m| format!("{m}/"));
        let listed = super::cmd_read::listed_files(&host.root).unwrap_or_default();
        let paths = listed
            .iter()
            .map(String::as_str)
            .filter(|path| !mount.as_ref().is_some_and(|m| path.starts_with(m.as_str())));
        (result.files, result.files_total) = search::file_hits(&args.query, paths);
        for file in &mut result.files {
            file.declaration = declaration(&host.root, &file.path);
        }
    }
    let out = CommandOutput::new(
        with_snapshot(search::to_json(&result), &s.info),
        with_snapshot_line(search::render(&result, ctx.format), &s.info, ctx.format),
    );
    Ok(s.finish(out))
}

/// Largest file opened for its declaration line.
const MAX_DECLARATION_FILE: u64 = 2 * 1024 * 1024;

/// The line on which a file declares the name it carries (`TokenStore.kt` declares
/// `TokenStore`), so that a read can start there instead of at the imports. `None` when
/// the file declares no such name or cannot be read as text.
fn declaration(root: &std::path::Path, path: &str) -> Option<(u32, String)> {
    let stem = std::path::Path::new(path).file_stem()?.to_str()?;
    let file = crate::util::safe_join(root, path).ok()?;
    let meta = std::fs::metadata(&file).ok()?;
    if !meta.is_file() || meta.len() > MAX_DECLARATION_FILE {
        return None;
    }
    let text = std::fs::read_to_string(&file).ok()?;
    let line = crate::tools::grep::declaration_line(&text, stem)?;
    let source = text.lines().nth(line as usize - 1)?.trim();
    crate::tools::grep::declares(source, stem).then(|| (line, crate::answer::clip(source, 120)))
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
