//! Named managed blocks in host instruction files (`AGENTS.md`, `CLAUDE.md`).
//!
//! A block is delimited by whole-line markers `<!-- kb:begin <name> -->` and
//! `<!-- kb:end <name> -->` (surrounding whitespace on the marker line is ignored). kb only
//! rewrites the bytes strictly between the marker lines of its own block; every other byte
//! of the file is preserved exactly (files are handled as bytes, not text). Duplicate,
//! nested, unbalanced or malformed kb markers are errors: kb never guesses where a block
//! ends.

/// Name of the block kb manages in host instruction files.
pub const BLOCK_NAME: &str = "kb-instructions";

const BEGIN: &str = "<!-- kb:begin ";
const END: &str = "<!-- kb:end ";
const CLOSE: &str = " -->";
/// Any line starting with this is treated as a kb marker and must be well formed.
const MARKER_PREFIX: &str = "<!-- kb:";

pub fn begin_marker(name: &str) -> String {
    format!("{BEGIN}{name}{CLOSE}")
}

pub fn end_marker(name: &str) -> String {
    format!("{END}{name}{CLOSE}")
}

/// Byte offsets of one block inside a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockSpan {
    /// Start of the begin-marker line.
    pub start: usize,
    /// First byte after the begin-marker line (start of the managed content).
    pub inner_start: usize,
    /// Start of the end-marker line (end of the managed content, exclusive).
    pub inner_end: usize,
    /// First byte after the end-marker line (including its newline, if any).
    pub end: usize,
}

impl BlockSpan {
    pub fn inner<'a>(&self, bytes: &'a [u8]) -> &'a [u8] {
        &bytes[self.inner_start..self.inner_end]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Marker {
    Begin(String),
    End(String),
}

fn trim_ascii(b: &[u8]) -> &[u8] {
    let start = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    let end = b
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map_or(start, |i| i + 1);
    &b[start..end]
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Classify a line: `Ok(None)` for ordinary lines, `Err` for a malformed kb marker.
fn parse_marker(line: &[u8]) -> Result<Option<Marker>, String> {
    let t = trim_ascii(line);
    if !t.starts_with(MARKER_PREFIX.as_bytes()) {
        return Ok(None);
    }
    let text = std::str::from_utf8(t).map_err(|_| "kb marker is not valid UTF-8".to_string())?;
    let parsed = if let Some(rest) = text.strip_prefix(BEGIN) {
        rest.strip_suffix(CLOSE)
            .map(|n| Marker::Begin(n.to_string()))
    } else if let Some(rest) = text.strip_prefix(END) {
        rest.strip_suffix(CLOSE).map(|n| Marker::End(n.to_string()))
    } else {
        None
    };
    match parsed {
        Some(Marker::Begin(n) | Marker::End(n)) if !valid_name(&n) => {
            Err(format!("invalid block name in `{text}`"))
        }
        Some(m) => Ok(Some(m)),
        None => Err(format!(
            "malformed kb marker `{text}` (expected `{}` or `{}`)",
            begin_marker("<name>"),
            end_marker("<name>")
        )),
    }
}

/// Locate block `name`. Validates every kb marker in the file; returns an error message
/// (with a 1-based line number) for duplicate, nested, unbalanced or malformed markers.
pub fn find_block(bytes: &[u8], name: &str) -> Result<Option<BlockSpan>, String> {
    let mut found: Option<BlockSpan> = None;
    let mut seen: Vec<String> = Vec::new();
    let mut open: Option<(String, usize, usize, usize)> = None; // name, line, start, inner_start
    let mut pos = 0usize;
    let mut line_no = 0usize;
    while pos < bytes.len() {
        line_no += 1;
        let nl = bytes[pos..].iter().position(|&c| c == b'\n');
        let (line_end, next) = match nl {
            Some(i) => (pos + i, pos + i + 1),
            None => (bytes.len(), bytes.len()),
        };
        let marker =
            parse_marker(&bytes[pos..line_end]).map_err(|e| format!("line {line_no}: {e}"))?;
        match (marker, &open) {
            (None, _) => {}
            (Some(Marker::Begin(n)), None) => {
                if seen.contains(&n) {
                    return Err(format!("line {line_no}: duplicate block `{n}`"));
                }
                open = Some((n, line_no, pos, next));
            }
            (Some(Marker::Begin(n)), Some((o, l, _, _))) => {
                return Err(format!(
                    "line {line_no}: block `{n}` begins inside block `{o}` opened at line {l}"
                ));
            }
            (Some(Marker::End(n)), None) => {
                return Err(format!(
                    "line {line_no}: end of block `{n}` without a matching begin"
                ));
            }
            (Some(Marker::End(n)), Some((o, l, start, inner_start))) => {
                if &n != o {
                    return Err(format!(
                        "line {line_no}: end of block `{n}` does not match block `{o}` opened at line {l}"
                    ));
                }
                if n == name {
                    found = Some(BlockSpan {
                        start: *start,
                        inner_start: *inner_start,
                        inner_end: pos,
                        end: next,
                    });
                }
                seen.push(n);
                open = None;
            }
        }
        pos = next;
    }
    if let Some((o, l, _, _)) = open {
        return Err(format!("block `{o}` opened at line {l} is not closed"));
    }
    Ok(found)
}

/// Managed content always ends with a newline so that the end marker starts a line.
pub fn normalize_content(content: &[u8]) -> Vec<u8> {
    let mut c = content.to_vec();
    if !c.is_empty() && !c.ends_with(b"\n") {
        c.push(b'\n');
    }
    c
}

fn wrapped(name: &str, content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(content.len() + 64);
    out.extend_from_slice(begin_marker(name).as_bytes());
    out.push(b'\n');
    out.extend_from_slice(&normalize_content(content));
    out.extend_from_slice(end_marker(name).as_bytes());
    out.push(b'\n');
    out
}

/// Append a new block to `existing` (or create the file content). Existing bytes are kept
/// as an exact prefix; a newline and one blank separator line are added when needed.
pub fn append_block(existing: &[u8], name: &str, content: &[u8]) -> Vec<u8> {
    let mut out = existing.to_vec();
    if !out.is_empty() {
        if !out.ends_with(b"\n") {
            out.push(b'\n');
        }
        if !out.ends_with(b"\n\n") {
            out.push(b'\n');
        }
    }
    out.extend_from_slice(&wrapped(name, content));
    out
}

/// Replace the managed content of `span`; the marker lines and all other bytes are kept.
pub fn replace_inner(bytes: &[u8], span: &BlockSpan, content: &[u8]) -> Vec<u8> {
    let content = normalize_content(content);
    let mut out = Vec::with_capacity(bytes.len() + content.len());
    // `inner_start` always follows the begin marker's newline: a closed block has an end
    // marker on a later line.
    out.extend_from_slice(&bytes[..span.inner_start]);
    out.extend_from_slice(&content);
    out.extend_from_slice(&bytes[span.inner_end..]);
    out
}

/// Remove the whole block (both marker lines and the content).
pub fn remove_block(bytes: &[u8], span: &BlockSpan) -> Vec<u8> {
    let mut out = bytes[..span.start].to_vec();
    out.extend_from_slice(&bytes[span.end..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: &str = BLOCK_NAME;

    #[test]
    fn append_keeps_existing_bytes_as_prefix() {
        let original: &[u8] = b"# Rules\r\n\xff binary \xfe\nno trailing newline";
        let out = append_block(original, N, b"hello");
        assert!(out.starts_with(original));
        let span = find_block(&out, N).unwrap().unwrap();
        assert_eq!(span.inner(&out), b"hello\n");
        assert_eq!(
            append_block(b"", N, b"x\n"),
            b"<!-- kb:begin kb-instructions -->\nx\n<!-- kb:end kb-instructions -->\n"
        );
        let ends_blank = append_block(b"a\n\n", N, b"x\n");
        assert!(ends_blank.starts_with(b"a\n\n<!-- kb:begin"));
    }

    #[test]
    fn replace_and_remove_preserve_unmanaged_bytes() {
        let file = b"before\n  <!-- kb:begin kb-instructions -->  \nold\n<!-- kb:end kb-instructions -->\nafter\xff";
        let span = find_block(file, N).unwrap().unwrap();
        assert_eq!(span.inner(file), b"old\n");
        let replaced = replace_inner(file, &span, b"new");
        assert_eq!(
            replaced,
            b"before\n  <!-- kb:begin kb-instructions -->  \nnew\n<!-- kb:end kb-instructions -->\nafter\xff"
        );
        assert_eq!(remove_block(file, &span), b"before\nafter\xff");
        // Other blocks are validated but left alone.
        let two = b"<!-- kb:begin other -->\nx\n<!-- kb:end other -->\n";
        assert_eq!(find_block(two, N).unwrap(), None);
    }

    #[test]
    fn rejects_malformed_markers() {
        let cases: [&[u8]; 6] = [
            b"<!-- kb:begin kb-instructions -->\na\n<!-- kb:end kb-instructions -->\n<!-- kb:begin kb-instructions -->\nb\n<!-- kb:end kb-instructions -->\n",
            b"<!-- kb:begin kb-instructions -->\na\n",
            b"a\n<!-- kb:end kb-instructions -->\n",
            b"<!-- kb:begin kb-instructions -->\n<!-- kb:begin other -->\n<!-- kb:end other -->\n<!-- kb:end kb-instructions -->\n",
            b"<!-- kb:begin kb-instructions\n",
            b"<!-- kb:begin Bad Name -->\n<!-- kb:end Bad Name -->\n",
        ];
        for c in cases {
            assert!(
                find_block(c, N).is_err(),
                "{:?} should be rejected",
                String::from_utf8_lossy(c)
            );
        }
        // Mentions of markers inside prose are not markers.
        assert_eq!(
            find_block(b"Use `<!-- kb:begin kb-instructions -->` markers.\n", N).unwrap(),
            None
        );
    }
}
