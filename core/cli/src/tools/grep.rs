//! Text search adapters: ripgrep, and the built-in `git grep` (tracked and untracked,
//! non-ignored files), which also serves as the lexical fallback for symbol requests.
//! Lexical answers are labelled `lexical`: a text match is not a resolved reference.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use super::{CodeQuery, Outcome, Probe, Roots, ToolSpec, run_tool, tool_env};
use crate::answer::{Item, Source};
use crate::error::{KbError, Result};
use crate::model::tools::Capability;
use crate::process::{Captured, Limits, capture};

/// One repository text search.
#[derive(Debug, Clone)]
pub struct FindQuery {
    pub text: String,
    pub regex: bool,
    /// Path globs to restrict the search (`!glob` excludes).
    pub globs: Vec<String>,
}

/// A match as `path`, 1-based `line` and the matched line's text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hit {
    pub path: String,
    pub line: u32,
    pub text: String,
}

const MAX_PER_FILE: &str = "20";

/// Parse NUL-delimited output: git grep `path\0line\0text`, ripgrep `path\0line:text`.
pub fn parse_hits(stdout: &[u8]) -> Vec<Hit> {
    let text = String::from_utf8_lossy(stdout);
    let mut hits = Vec::new();
    for raw in text.lines() {
        let Some((path, rest)) = raw.split_once('\0') else {
            continue;
        };
        let (line, body) = match rest.split_once('\0') {
            Some(parts) => parts,
            None => match rest.split_once(':') {
                Some(parts) => parts,
                None => continue,
            },
        };
        let Ok(line) = line.parse() else { continue };
        hits.push(Hit {
            path: path.trim_start_matches("./").to_string(),
            line,
            text: body.trim().to_string(),
        });
    }
    hits.sort();
    hits.dedup();
    hits
}

fn source(tool: &ToolSpec, probe: &Probe, adapter: &str, state: &str) -> Source {
    Source {
        id: tool.id.clone(),
        adapter: adapter.into(),
        version: probe.version.clone(),
        state: state.into(),
    }
}

/// Search the code root's text.
pub fn find(tool: &ToolSpec, probe: &Probe, roots: &Roots, q: &FindQuery) -> Result<Outcome> {
    let (adapter, out) = match tool.adapter {
        crate::model::tools::AdapterKind::Ripgrep => {
            let program = Path::new(probe.program.as_deref().unwrap_or("rg"));
            let mut args: Vec<String> = [
                "--null",
                "--line-number",
                "--no-heading",
                "--with-filename",
                "--color",
                "never",
                "--smart-case",
                "--sort",
                "path",
                "--max-columns",
                "300",
                "--max-columns-preview",
                "--max-count",
                MAX_PER_FILE,
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            if !q.regex {
                args.push("--fixed-strings".into());
            }
            if let Some(mount) = &roots.kb_mount {
                args.extend(["--glob".into(), format!("!{mount}/**")]);
            }
            for g in &q.globs {
                args.extend(["--glob".into(), g.clone()]);
            }
            args.extend(["-e".into(), q.text.clone(), "--".into(), ".".into()]);
            ("ripgrep", run_tool(program, &args, roots, tool.timeout())?)
        }
        _ => {
            let mut args: Vec<String> = [
                "grep",
                "-z",
                "-n",
                "-I",
                "--untracked",
                "--max-count",
                MAX_PER_FILE,
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            if !q.text.chars().any(char::is_uppercase) {
                args.push("-i".into());
            }
            args.push(if q.regex { "-E" } else { "-F" }.into());
            args.extend(["-e".into(), q.text.clone(), "--".into()]);
            args.extend(q.globs.iter().map(|g| match g.strip_prefix('!') {
                Some(rest) => format!(":(exclude,glob){rest}"),
                None => format!(":(glob){g}"),
            }));
            ("git-grep", git(roots, &args, tool.timeout())?)
        }
    };
    // Both tools exit 1 for "no match"; 2 and above are errors.
    if out.exit_code > 1 {
        return Err(KbError::invalid_input(format!(
            "{adapter} exited {}: {}",
            out.exit_code,
            crate::git::redact(String::from_utf8_lossy(&out.stderr).trim())
        )));
    }
    let mut outcome = Outcome::new(source(tool, probe, adapter, "working tree"));
    let hits = parse_hits(&out.stdout);
    outcome.found = !hits.is_empty();
    let files: BTreeSet<_> = hits.iter().map(|h| h.path.clone()).collect();
    if files.len() > 1 {
        outcome
            .notes
            .push(format!("{} match(es) in {} files", hits.len(), files.len()));
    }
    outcome.items = hits
        .into_iter()
        .map(|h| {
            Item::new("match", &tool.id)
                .at(h.path, Some(h.line))
                .detail(h.text)
        })
        .collect();
    Ok(outcome)
}

/// `git grep` in the code root, bounded like every tool run.
fn git(roots: &Roots, args: &[String], timeout: u64) -> Result<Captured> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(&roots.code)
        .args([
            "-c",
            "core.quotepath=false",
            "-c",
            "color.ui=false",
            "-c",
            "submodule.recurse=false",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    tool_env(&mut cmd);
    capture(
        &mut cmd,
        &[],
        Limits {
            timeout: Duration::from_secs(timeout),
            stdout: 16 * 1024 * 1024,
            stderr: 1024 * 1024,
        },
    )
}

/// Definition keywords of common languages, for the lexical symbol fallback.
const DEFINERS: &str = "fn|def|class|struct|enum|trait|interface|func|function|fun|type|object|protocol|module|impl|val|var|let|const|typealias|record";

fn escape_ere(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if "\\^$.|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The identifier part of a possibly qualified name (`Type::name` → `name`).
pub fn leaf(name: &str) -> &str {
    name.rsplit([':', '.', '#', '/'])
        .find(|s| !s.is_empty())
        .unwrap_or(name)
}

fn definition_pattern(name: &str) -> String {
    format!(
        "(^|[^[:alnum:]_])({DEFINERS})[[:space:]]+([[:alnum:]_<>,: ]*[[:space:]])?{}([^[:alnum:]_]|$)",
        escape_ere(leaf(name))
    )
}

/// Lexical answers for `sym`, `callers` and `impact` from `git grep`.
pub fn lexical(tool: &ToolSpec, probe: &Probe, roots: &Roots, q: &CodeQuery) -> Result<Outcome> {
    let name = leaf(&q.name).to_string();
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return Err(KbError::invalid_input("a symbol name has no whitespace"));
    }
    let mut outcome = Outcome::new(source(tool, probe, "git-grep", "working tree, lexical"));
    let run = |args: Vec<String>| -> Result<Vec<Hit>> {
        let out = git(roots, &args, tool.timeout())?;
        if out.exit_code > 1 {
            return Err(KbError::invalid_input(format!(
                "git grep exited {}: {}",
                out.exit_code,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let mut hits = parse_hits(&out.stdout);
        if let Some(f) = &q.file {
            hits.retain(|h| h.path.contains(f.as_str()));
        }
        Ok(hits)
    };
    let base = |pattern: Vec<&str>| -> Vec<String> {
        let mut v: Vec<String> = [
            "grep",
            "-z",
            "-n",
            "-I",
            "--untracked",
            "--max-count",
            MAX_PER_FILE,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        v.extend(pattern.into_iter().map(str::to_string));
        v
    };
    let pattern = definition_pattern(&name);
    let defs = run(base(vec!["-E", "-e", &pattern, "--"]))?;
    let def_at: BTreeSet<(String, u32)> = defs.iter().map(|h| (h.path.clone(), h.line)).collect();
    match q.capability {
        Capability::Sym => {
            outcome.found = !defs.is_empty();
            outcome.items = defs
                .into_iter()
                .map(|h| {
                    Item::new("definition", &tool.id)
                        .name(name.clone())
                        .at(h.path, Some(h.line))
                        .detail(h.text)
                        .confidence("lexical")
                })
                .collect();
        }
        Capability::Callers | Capability::Impact => {
            let uses = run(base(vec!["-w", "-F", "-e", &name, "--"]))?;
            let uses: Vec<Hit> = uses
                .into_iter()
                .filter(|h| !def_at.contains(&(h.path.clone(), h.line)))
                .collect();
            outcome.found = !defs.is_empty() || !uses.is_empty();
            if q.capability == Capability::Callers {
                outcome.items = uses
                    .into_iter()
                    .map(|h| {
                        Item::new("caller", &tool.id)
                            .at(h.path, Some(h.line))
                            .detail(h.text)
                            .confidence("lexical")
                    })
                    .collect();
            } else {
                let files: BTreeSet<String> = uses
                    .iter()
                    .map(|h| h.path.clone())
                    .chain(defs.iter().map(|h| h.path.clone()))
                    .collect();
                let mut per_file: std::collections::BTreeMap<String, (u32, usize)> =
                    Default::default();
                for h in &uses {
                    let e = per_file.entry(h.path.clone()).or_insert((h.line, 0));
                    e.1 += 1;
                }
                outcome.items = per_file
                    .into_iter()
                    .map(|(path, (first, n))| {
                        Item::new("affected", &tool.id)
                            .at(path, Some(first))
                            .detail(format!("{n} reference(s) to `{name}`"))
                            .confidence("lexical")
                    })
                    .collect();
                outcome.files = files.into_iter().collect();
                outcome
                    .notes
                    .push("lexical impact covers direct textual references only (depth 1)".into());
            }
        }
        _ => return Err(KbError::internal("git grep cannot answer this capability")),
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_parse_from_both_tools() {
        let out = b"b.rs\x003\x00fn x() {}\na.rs\x0012:let y = x();\nbroken line\n";
        let hits = parse_hits(out);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "a.rs");
        assert_eq!(hits[0].line, 12);
        assert_eq!(hits[1].text, "fn x() {}");
    }

    #[test]
    fn definition_pattern_matches_declarations_only() {
        let re = regex::Regex::new(
            &definition_pattern("TokenRefresher::refresh")
                .replace("[:alnum:]", "a-zA-Z0-9")
                .replace("[:space:]", " \t"),
        )
        .unwrap();
        assert!(re.is_match("    suspend fun refresh(): Token {"));
        assert!(re.is_match("pub fn refresh(&self)"));
        assert!(re.is_match("def refresh(self):"));
        assert!(!re.is_match("val t = refresher.refresh()"));
        assert!(!re.is_match("fn refreshAll()"));
        assert_eq!(leaf("a::b.c"), "c");
        assert_eq!(escape_ere("a.b(c)"), "a\\.b\\(c\\)");
    }
}
