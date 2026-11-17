//! A faithful numbered excerpt of one working-tree file. Keep its locator once so its
//! source can be cited without making each line repeat a path or kind. Several ranges may
//! be asked for in one call, which costs an agent one round trip instead of one per range.

use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::Path;

use serde_json::{Value, json};

use super::{Ctx, args::ReadArgs, session};
use crate::answer::{EXCERPT_LINES, Excerpt};
use crate::error::{ErrorCode, KbError, Result};
use crate::git::Git;
use crate::output::CommandOutput;
use crate::source::{SourceTree, WorkingTreeSource};
use crate::util::{check_rel_path, safe_join};

const DEFAULT_LINES: u32 = 40;
pub(super) const MAX_LINES: usize = 80;
/// Lines one call returns over all its ranges.
const MAX_CALL_LINES: usize = 240;
const MAX_RANGES: usize = 12;
const MAX_EXCERPT_BYTES: usize = 32 * 1024;
const MAX_LIST_BYTES: usize = 16 * 1024 * 1024;
/// Files larger than this are not opened for an answer's excerpt.
const MAX_ATTACH_BYTES: usize = 2 * 1024 * 1024;

struct Range {
    file: String,
    first: u32,
    last: u32,
}

pub fn run(ctx: &Ctx, args: &ReadArgs) -> Result<CommandOutput> {
    let host = session::detect_host(ctx)?;
    let root = host.as_ref().map_or(ctx.env.kb_root.as_path(), |h| &h.root);
    let first = args.first;
    let last = args.last.unwrap_or(first.saturating_add(DEFAULT_LINES - 1));
    if args.more.is_empty() {
        let (value, text, _) = range(root, &args.file, first, last, MAX_LINES)?;
        return Ok(CommandOutput::new(value, text));
    }
    let mut ranges = vec![Range {
        file: args.file.clone(),
        first,
        last,
    }];
    ranges.extend(further(&args.file, &args.more)?);
    if ranges.len() > MAX_RANGES {
        return Err(KbError::invalid_input(format!(
            "one call reads at most {MAX_RANGES} ranges"
        )));
    }
    let mut text = String::new();
    let mut values = Vec::new();
    let mut left = MAX_CALL_LINES;
    let mut unread = Vec::new();
    let mut failure = None;
    for r in &ranges {
        if left == 0 {
            unread.push(format!("{} {} {}", r.file, r.first, r.last));
            continue;
        }
        match range(root, &r.file, r.first, r.last, MAX_LINES.min(left)) {
            Ok((value, part, shown)) => {
                left -= shown;
                text.push_str(&part);
                values.push(value);
            }
            // One bad range does not cost the others; the call fails only when all do.
            Err(e) => {
                let _ = writeln!(text, "-- {} {} {}: {}", r.file, r.first, r.last, e.message);
                values.push(json!({"path": r.file, "error": e.message}));
                failure = failure.or(Some(e));
            }
        }
    }
    if !unread.is_empty() {
        let _ = writeln!(
            text,
            "-- not read, {MAX_CALL_LINES} lines per call: {}",
            unread.join("; ")
        );
    }
    let all_failed = values.iter().all(|v| v.get("error").is_some());
    let out = CommandOutput::new(
        json!({"source": "working-tree", "ranges": values, "unread": unread}),
        text,
    );
    Ok(match failure {
        Some(e) if all_failed => out.with_failure(e),
        _ => out,
    })
}

/// The ranges after the first: `FILE FIRST LAST` groups, or `FIRST LAST` alone for another
/// range of the file named last. Two numbers in a row are therefore always a range; a file
/// whose name is a number is read by its path (`./7`).
fn further(file: &str, more: &[String]) -> Result<Vec<Range>> {
    let usage = || {
        KbError::invalid_input(
            "each further range is FILE FIRST LAST, or FIRST LAST for the same file",
        )
    };
    let number = |text: &String| text.parse::<u32>().ok().filter(|n| *n > 0);
    let mut ranges = Vec::new();
    let mut file = file.to_owned();
    let mut rest = more;
    while !rest.is_empty() {
        let (first, last, used) = match rest {
            [first, last, ..] if number(first).is_some() && number(last).is_some() => {
                (number(first), number(last), 2)
            }
            [name, first, last, ..] => {
                file = name.clone();
                (number(first), number(last), 3)
            }
            _ => return Err(usage()),
        };
        let (Some(first), Some(last)) = (first, last) else {
            return Err(usage());
        };
        ranges.push(Range {
            file: file.clone(),
            first,
            last,
        });
        rest = &rest[used..];
    }
    Ok(ranges)
}

/// One range: its JSON, its text and the number of lines shown.
fn range(
    root: &Path,
    file: &str,
    first: u32,
    last: u32,
    max_lines: usize,
) -> Result<(Value, String, usize)> {
    let source = WorkingTreeSource::new(root);
    if last < first {
        return Err(KbError::invalid_input("LAST must be at or after FIRST"));
    }
    let selector = file.trim_start_matches("./");
    check_rel_path(selector).map_err(KbError::unsafe_path)?;
    if selector
        .split('/')
        .any(|part| part.eq_ignore_ascii_case(".git"))
    {
        return Err(KbError::unsafe_path(
            "Git metadata is outside the source tree",
        ));
    }

    let (path, bytes) = match source.read_path(selector)? {
        Some(bytes) => (selector.to_owned(), bytes),
        None => {
            let path = match_path(root, selector)?;
            let bytes = source.read_path(&path)?.ok_or_else(|| missing(&path))?;
            (path, bytes)
        }
    };
    let contents = std::str::from_utf8(&bytes)
        .map_err(|_| KbError::invalid_input(format!("`{path}` is not UTF-8 text")))?;
    excerpt(&path, contents, first, last, max_lines)
}

/// Lines `first..=end` of a code-root file for an answer's excerpt, at most `max`.
/// `None` when the file is missing, unsafe, large or not plain text: the excerpt is a
/// convenience on an answer that already holds.
pub(super) fn attach(root: &Path, path: &str, first: u32, end: u32, max: usize) -> Option<Excerpt> {
    check_rel_path(path).ok()?;
    if path
        .split('/')
        .any(|part| part.eq_ignore_ascii_case(".git"))
    {
        return None;
    }
    let bytes = WorkingTreeSource::new(root).read_path(path).ok()??;
    if bytes.len() > MAX_ATTACH_BYTES {
        return None;
    }
    let contents = std::str::from_utf8(&bytes).ok()?;
    let total = u32::try_from(contents.lines().count()).ok()?;
    let first = first.max(1);
    let end = end.min(total);
    if first > end {
        return None;
    }
    let lines: Vec<String> = contents
        .lines()
        .skip((first - 1) as usize)
        .take(((end - first + 1) as usize).min(max))
        .map(str::to_owned)
        .collect();
    // An excerpt is as large as one `read` may be, in JSON too: a minified file whose
    // few lines run to megabytes is named by its item and read on request.
    if lines
        .iter()
        .any(|line| line.chars().any(|c| c.is_control() && c != '\t'))
        || lines.iter().map(|line| line.len() + 12).sum::<usize>() > MAX_EXCERPT_BYTES
    {
        return None;
    }
    let shown_last = first + lines.len() as u32 - 1;
    Some(Excerpt {
        path: path.to_owned(),
        first_line: first,
        lines,
        next_line: (shown_last < end).then_some(shown_last + 1),
    })
}

/// Excerpts for several located spans `(path, first, end)`: spans of one file that touch
/// are merged, files keep the order in which they were first named, and all excerpts
/// together hold at most [`EXCERPT_LINES`] lines. Spans beyond that are left out; the
/// items that located them still name their lines.
pub(super) fn attach_spans(root: &Path, spans: &[(String, u32, u32)]) -> Vec<Excerpt> {
    let mut files: Vec<(&str, Vec<(u32, u32)>)> = Vec::new();
    for (path, first, end) in spans {
        let span = ((*first).max(1), (*end).max(*first).max(1));
        match files.iter_mut().find(|(p, _)| p == path) {
            Some((_, list)) => list.push(span),
            None => files.push((path, vec![span])),
        }
    }
    let mut left = EXCERPT_LINES;
    let mut bytes = MAX_EXCERPT_BYTES;
    let mut excerpts = Vec::new();
    for (path, mut list) in files {
        list.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::new();
        for (first, end) in list {
            match merged.last_mut() {
                Some(last) if first <= last.1.saturating_add(1) => last.1 = last.1.max(end),
                _ => merged.push((first, end)),
            }
        }
        for (first, end) in merged {
            if left == 0 {
                return excerpts;
            }
            if let Some(excerpt) = attach(root, path, first, end, left) {
                // The excerpts of one answer are together as large as one `read` may be.
                let size: usize = excerpt.lines.iter().map(|line| line.len() + 12).sum();
                if size > bytes {
                    continue;
                }
                bytes -= size;
                left -= excerpt.lines.len();
                excerpts.push(excerpt);
            }
        }
    }
    excerpts
}

fn missing(selector: &str) -> KbError {
    KbError::new(ErrorCode::NotFound, format!("no file matches `{selector}`"))
}

/// The files `read` covers: tracked and untracked files of the work tree that are not
/// ignored, sorted. Empty outside a Git work tree.
pub(super) fn listed_files(root: &Path) -> Result<Vec<String>> {
    if crate::git::toplevel(root).is_none() {
        return Ok(Vec::new());
    }
    let files = Git::new(root).run_bytes_limited(
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
        ],
        MAX_LIST_BYTES,
    )?;
    let mut listed: Vec<String> = files
        .split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .filter_map(|p| std::str::from_utf8(p).ok())
        .map(str::to_owned)
        .collect();
    listed.sort_unstable();
    listed.dedup();
    Ok(listed)
}

fn match_path(root: &Path, selector: &str) -> Result<String> {
    if crate::git::toplevel(root).is_none() {
        return Err(missing(selector));
    }
    let files = Git::new(root).run_bytes_limited(
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
        ],
        MAX_LIST_BYTES,
    )?;
    let listed = files.split(|&b| b == 0).filter(|p| !p.is_empty()).count();
    let matches: BTreeSet<_> = files
        .split(|&b| b == 0)
        .filter_map(|p| std::str::from_utf8(p).ok())
        .filter(|path| {
            let path = Path::new(path);
            path.ends_with(selector)
                || (!selector.contains('/') && path.file_stem() == Some(selector.as_ref()))
        })
        .filter(|path| safe_join(root, path).is_ok_and(|p| p.is_file()))
        .collect();
    match matches.len() {
        // The whole listing was checked: another spelling of the name will not find it.
        0 => Err(KbError::new(
            ErrorCode::NotFound,
            format!("no file matches `{selector}` among the {listed} file(s) of this checkout"),
        )),
        1 => Ok(matches.into_iter().next().expect("one match").to_owned()),
        n => {
            let shown: Vec<_> = matches.into_iter().take(6).collect();
            Err(KbError::invalid_input(format!(
                "`{selector}` matches {n} paths: {}",
                shown.join(", ")
            ))
            .with_hint("pass a longer suffix or the exact path"))
        }
    }
}

fn excerpt(
    path: &str,
    contents: &str,
    first: u32,
    last: u32,
    max_lines: usize,
) -> Result<(Value, String, usize)> {
    let mut text = format!("{path}\n");
    let mut lines = Vec::new();
    let mut next_line = None;
    for (index, body) in (1..=last).zip(contents.lines()).skip((first - 1) as usize) {
        if lines.len() == max_lines || text.len() + body.len() + 12 > MAX_EXCERPT_BYTES {
            if lines.is_empty() {
                return Err(KbError::invalid_input(format!(
                    "`{path}:{index}` exceeds the {MAX_EXCERPT_BYTES}-byte excerpt limit"
                )));
            }
            next_line = Some(index);
            break;
        }
        if body.chars().any(|c| c.is_control() && c != '\t') {
            return Err(KbError::invalid_input(format!(
                "`{path}:{index}` contains non-text control characters"
            )));
        }
        let _ = writeln!(text, "{index}:{body}");
        lines.push(body);
    }
    let total = u32::try_from(contents.lines().count()).unwrap_or(u32::MAX);
    if lines.is_empty() {
        return Err(KbError::new(
            ErrorCode::NotFound,
            format!("`{path}` ends at line {total}; there is no line {first}"),
        ));
    }
    let shown_last = first + (lines.len() - 1) as u32;
    // Say where the file ends, so the next read neither overlaps nor runs past it.
    if let Some(next) = next_line {
        let end = if last >= total { " (end of file)" } else { "" };
        let _ = writeln!(
            text,
            "-- truncated; resume at line {next} through {}{end}",
            last.min(total)
        );
    } else if shown_last == total {
        let lines = if total == 1 { "line" } else { "lines" };
        let _ = writeln!(text, "-- end of file ({total} {lines})");
    }
    let shown = lines.len();
    Ok((
        json!({
            "source": "working-tree",
            "path": path,
            "first_line": first,
            "last_line": shown_last,
            "next_line": next_line,
            "total_lines": total,
            "lines": lines,
        }),
        text,
        shown,
    ))
}
