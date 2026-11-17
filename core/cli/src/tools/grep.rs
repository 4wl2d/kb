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

impl FindQuery {
    pub(crate) fn prioritize_code_matches<'a>(
        &self,
        mut paths: impl Iterator<Item = &'a str>,
    ) -> bool {
        let mut chars = self.text.chars();
        !self.regex
            && !matches!(self.text.as_str(), "import" | "package")
            && chars
                .next()
                .is_some_and(|c| c.is_alphabetic() || matches!(c, '_' | '$'))
            && chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | '$'))
            && paths.any(is_jvm_source)
    }
}

/// A match as `path`, 1-based `line` and the matched line's text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hit {
    pub path: String,
    pub line: u32,
    pub text: String,
}

const MAX_PER_FILE: &str = "20";
/// Lines per file read by the scan behind a final miss: enough that a declaration below
/// many uses of its name in one file is still examined.
const SCAN_PER_FILE: &str = "200";
/// Characters an item keeps of a matched line: one line of a minified file must not make
/// an answer megabytes large, in JSON either. Decisions are made on the whole line.
const MAX_HIT_TEXT: usize = 500;

/// The text of a matched line as an item carries it.
pub fn hit_detail(text: &str) -> String {
    crate::answer::clip(text, MAX_HIT_TEXT)
}

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

fn filename_rank(path: &str, needle: &str) -> u8 {
    let path = Path::new(path);
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if filename.to_lowercase() == needle {
        return 0;
    }
    let stem = stem.to_lowercase();
    if stem == needle {
        0
    } else if stem.starts_with(needle) {
        1
    } else if stem.contains(needle) {
        2
    } else {
        3
    }
}

/// Put one matching line per file before deeper hits. The input is already in
/// path and line order, which breaks filename-rank ties deterministically.
fn rank_files<'a>(hits: &'a [Hit], q: &FindQuery) -> Vec<&'a [Hit]> {
    let needle = (!q.regex
        && q.text
            .chars()
            .all(|c| c.is_alphanumeric() || "_-.".contains(c)))
    .then(|| q.text.to_lowercase());
    let mut files: Vec<_> = hits.chunk_by(|a, b| a.path == b.path).collect();
    if let Some(needle) = needle.as_deref() {
        files.sort_by_cached_key(|rows| filename_rank(&rows[0].path, needle));
    }
    files
}

/// An import or package line: it names a symbol without using it.
pub fn is_import(line: &str) -> bool {
    let mut words = line.split_whitespace();
    match words.next() {
        Some("import" | "package" | "use" | "using" | "#include" | "#import") => true,
        Some("pub" | "export") => matches!(words.next(), Some("use" | "import")),
        Some(first) if first.starts_with("pub(") => words.next() == Some("use"),
        Some("from") => words.any(|word| word == "import"),
        _ => false,
    }
}

fn is_jvm_source(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|ext| ext.to_str()),
        Some("kt" | "kts" | "java")
    )
}

fn rank_hits<'a>(hits: &'a [Hit], q: &FindQuery) -> Vec<&'a Hit> {
    let files = rank_files(hits, q);
    if !q.prioritize_code_matches(files.iter().map(|rows| rows[0].path.as_str())) {
        return files
            .iter()
            .map(|rows| &rows[0])
            .chain(files.iter().flat_map(|rows| &rows[1..]))
            .collect();
    }

    let mut ranked = Vec::with_capacity(hits.len());
    for rows in files {
        let mut per_tier = [0; 2];
        let is_jvm = is_jvm_source(&rows[0].path);
        for hit in rows {
            let tier = usize::from(
                is_jvm
                    && matches!(
                        hit.text.split_whitespace().next(),
                        Some("import" | "package")
                    ),
            );
            ranked.push((tier, per_tier[tier], hit));
            per_tier[tier] += 1;
        }
    }
    // Keep each tier in rounds of one hit per file. Stable sorting preserves the
    // filename ranking and path tie-break within each round.
    ranked.sort_by_key(|&(tier, round, _)| (tier, round));
    ranked.into_iter().map(|(_, _, hit)| hit).collect()
}

fn git_glob(glob: &str) -> String {
    let (magic, path) = glob
        .strip_prefix('!')
        .map_or(("glob", glob), |path| ("exclude,glob", path));
    let prefix = if path.contains('/') { "" } else { "**/" };
    format!(":({magic}){prefix}{path}")
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
                "--hidden",
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
            // Hidden files are searched like any other, Git's own directory never: the
            // exclusion comes last so that no glob of the request overrides it.
            args.extend(["--glob".into(), "!.git".into()]);
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
            args.extend(q.globs.iter().map(|g| git_glob(g)));
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
    let file_count = hits.chunk_by(|a, b| a.path == b.path).count();
    if file_count > 1 {
        outcome
            .notes
            .push(format!("{} match(es) in {file_count} files", hits.len()));
    }
    outcome.items = rank_hits(&hits, q)
        .into_iter()
        .map(|h| {
            Item::new("match", &tool.id)
                .at(h.path.clone(), Some(h.line))
                .detail(hit_detail(&h.text))
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

const LIST_FILES: [&str; 5] = [
    "ls-files",
    "--cached",
    "--others",
    "--exclude-standard",
    "-z",
];

fn count_files(out: Captured, separator: u8) -> Option<usize> {
    (out.exit_code == 0).then(|| {
        out.stdout
            .split(|&b| b == separator)
            .filter(|path| !path.is_empty())
            .count()
    })
}

/// How many files a text search with these globs covers: what a miss was checked against.
/// `None` when the tool cannot list its files (a project `exec` tool, no Git work tree).
pub fn searched_files(
    tool: &ToolSpec,
    probe: &Probe,
    roots: &Roots,
    globs: &[String],
) -> Option<usize> {
    use crate::model::tools::AdapterKind;
    match tool.adapter {
        AdapterKind::Ripgrep => {
            let program = Path::new(probe.program.as_deref().unwrap_or("rg"));
            let mut args: Vec<String> = vec!["--files".into(), "--null".into(), "--hidden".into()];
            if let Some(mount) = &roots.kb_mount {
                args.extend(["--glob".into(), format!("!{mount}/**")]);
            }
            for g in globs {
                args.extend(["--glob".into(), g.clone()]);
            }
            args.extend(["--glob".into(), "!.git".into()]);
            args.extend(["--".into(), ".".into()]);
            let out = run_tool(program, &args, roots, tool.timeout()).ok()?;
            // ripgrep exits 1 when the globs select no file.
            if out.exit_code == 1 && out.stdout.is_empty() {
                return Some(0);
            }
            count_files(out, 0)
        }
        AdapterKind::GitGrep => {
            let mut args: Vec<String> = LIST_FILES.iter().map(|s| s.to_string()).collect();
            args.push("--".into());
            args.extend(globs.iter().map(|g| git_glob(g)));
            count_files(git(roots, &args, tool.timeout()).ok()?, 0)
        }
        _ => None,
    }
}

/// Every textual use of a symbol in the code root, for the answer to a symbol that no
/// tool can define: whether the name occurs at all, and how many files were checked.
#[derive(Debug, Clone, Default)]
pub struct Occurrences {
    /// Whole-word matches of the name as source writes it, at most 200 per file.
    pub hits: Vec<Hit>,
    /// Lines that read as a keyword-led declaration of the name, from a scan of their own.
    pub declarations: Vec<Hit>,
    /// Tracked and untracked, non-ignored files of the code root.
    pub searched: usize,
}

impl Occurrences {
    pub fn files(&self) -> usize {
        self.hits.chunk_by(|a, b| a.path == b.path).count()
    }
}

/// The text a use of `name` is written as: a dotted name (`Type.member`, `pkg.Type`)
/// appears in source as given, a `Type::member`, `Type#member` or path-qualified name only
/// by its last segment.
pub fn written_as(name: &str) -> &str {
    if name.contains("::") || name.contains(['#', '/']) {
        leaf(name)
    } else {
        name
    }
}

/// Built-in `git grep` for whole-word uses of `name`; an error when the code root is not
/// a Git work tree.
pub fn occurrences(roots: &Roots, name: &str, timeout: u64) -> Result<Occurrences> {
    let name = written_as(name);
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return Err(KbError::invalid_input("a symbol name has no whitespace"));
    }
    let args: Vec<String> = [
        "grep",
        "-z",
        "-n",
        "-I",
        "--untracked",
        "--max-count",
        SCAN_PER_FILE,
        "-w",
        "-F",
        "-e",
        name,
        "--",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let out = git(roots, &args, timeout)?;
    if out.exit_code > 1 {
        return Err(KbError::invalid_input(format!(
            "git grep exited {}",
            out.exit_code
        )));
    }
    let hits = parse_hits(&out.stdout);
    let declares = declaration_matcher(name);
    let declarations = if name == leaf(name) {
        let pattern = definition_pattern(name);
        let args: Vec<String> = [
            "grep",
            "-z",
            "-n",
            "-I",
            "--untracked",
            "--max-count",
            SCAN_PER_FILE,
            "-E",
            "-e",
            &pattern,
            "--",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let out = git(roots, &args, timeout)?;
        if out.exit_code > 1 {
            return Err(KbError::invalid_input(format!(
                "git grep exited {}",
                out.exit_code
            )));
        }
        parse_hits(&out.stdout)
            .into_iter()
            .filter(|hit| declares(&hit.text))
            .collect()
    } else {
        // A dotted name is searched as written: its declaration is not a line that
        // writes `Type.member`, so only the hits themselves are examined.
        hits.iter()
            .filter(|hit| declares(&hit.text))
            .cloned()
            .collect()
    };
    let list: Vec<String> = LIST_FILES.iter().map(|s| s.to_string()).collect();
    let searched = count_files(git(roots, &list, timeout)?, 0)
        .ok_or_else(|| KbError::invalid_input("git ls-files failed"))?;
    Ok(Occurrences {
        hits,
        declarations,
        searched,
    })
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
        "(^|[^[:alnum:]_])({DEFINERS})[[:space:]]+([[:alnum:]_<>,: ]*[[:space:]])?([[:alnum:]_<>?,]*\\.)?{}([^[:alnum:]_.]|$)",
        escape_ere(leaf(name))
    )
}

/// Does `line` read as a declaration of `name` by the lexical definition pattern?
pub fn declares(line: &str, name: &str) -> bool {
    declaration_matcher(name)(line)
}

/// [`declares`] for many lines of one name: the pattern is compiled once.
pub fn declaration_matcher(name: &str) -> impl Fn(&str) -> bool {
    let re = regex::Regex::new(&definition_pattern(name)).ok();
    move |line| re.as_ref().is_some_and(|re| reads_as_declaration(re, line))
}

/// A definition-pattern match that declares the name rather than using it as a type: in
/// `val action: Name` the keyword declares `action`, in `class Sub extends Name` it
/// declares `Sub`, and in `fun Name.other()` it declares `other`.
fn reads_as_declaration(re: &regex::Regex, line: &str) -> bool {
    // Every keyword of the line is tried: in `class A(val a: T, val name: U)` the first
    // `val` declares `a`, the second `name`.
    // No declaration line has more keywords than this before its name; a generated line
    // of thousands is not searched keyword by keyword.
    const KEYWORDS_TRIED: usize = 32;
    let mut from = 0;
    for _ in 0..KEYWORDS_TRIED {
        let Some(caps) = re.captures_at(line, from) else {
            return false;
        };
        let between = caps.get(3).map_or("", |m| m.as_str());
        if introduces(between) {
            return true;
        }
        match caps.get(2) {
            Some(keyword) => from = keyword.start() + 1,
            None => return false,
        }
    }
    false
}

/// Does the text between a definition keyword and a name leave the name as the thing
/// declared? Not after a `:` (a type), after `extends` or `implements` (a supertype), or
/// inside an unclosed `<` (a bound).
fn introduces(between: &str) -> bool {
    if between
        .split_whitespace()
        .any(|word| matches!(word, "extends" | "implements"))
    {
        return false;
    }
    let mut depth = 0usize;
    for c in between.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => return false,
            _ => {}
        }
    }
    depth == 0
}

/// Words that precede a call, never the name a declaration introduces.
const NOT_A_TYPE: &[&str] = &[
    "return",
    "new",
    "throw",
    "else",
    "await",
    "yield",
    "case",
    "in",
    "of",
    "typeof",
    "delete",
    "if",
    "while",
    "for",
    "do",
    "goto",
    "sizeof",
    "co_await",
    "co_return",
    "co_yield",
    "and",
    "or",
    "not",
];

/// Could `line` of the file `path` declare `name` without any definition keyword? Languages
/// of the C family and Java write a function as `Type name(`, Go a method as
/// `func (r T) name(`, JavaScript and TypeScript a method as `name(...) {`, Rust a static
/// as `static NAME`. The shapes are loose and decided by the file's extension: the answer
/// only keeps kb from saying that a name has no declaration, or marks a line as a possible
/// one; it is never reported as a resolved definition.
pub fn may_declare(path: &str, line: &str, name: &str) -> bool {
    let name = leaf(name);
    let extension = path.rsplit_once('.').map_or("", |(_, e)| e);
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let Some(at) = line.match_indices(name).map(|(at, _)| at).find(|&at| {
        !line[..at].chars().next_back().is_some_and(word)
            && !line[at + name.len()..].chars().next().is_some_and(word)
    }) else {
        return false;
    };
    // A declared name stands after its type, never after a `.`, `->` or `::` of a use.
    let spaced = line[..at].ends_with(|c: char| c.is_whitespace() || c == '*' || c == '&');
    let before = line[..at].trim();
    let after = line[at + name.len()..].trim_start();
    match extension {
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "m" | "mm" | "java" | "cs"
        | "dart" | "groovy" => {
            // One or more type or modifier words, then the name and its parameter list.
            // Every word is shaped like a type, a modifier or an annotation; an operator, a
            // comma or a `?` before the name makes the line an expression.
            let before = before.trim_end_matches(['*', '&']).trim_end();
            let type_like = |token: &str| {
                let token = token.trim_start_matches('@').trim_end_matches(['*', '&']);
                token
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphabetic() || c == '_')
                    && token.chars().all(|c| word(c) || "<>[].:".contains(c))
                    && !token.ends_with([':', '.'])
                    && !NOT_A_TYPE.contains(&token)
            };
            after.starts_with('(')
                && spaced
                && !before.is_empty()
                && before.split_whitespace().all(type_like)
        }
        "go" => {
            after.starts_with(['(', '[']) && before.starts_with("func") && before.ends_with(')')
        }
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => {
            // A method: only modifiers before the name, a parameter list that closes on the
            // line, and a body that opens after it. A call that takes a callback or an
            // object leaves its list open, or has an arrow or `function` inside.
            let closes = || {
                let list = after.trim_start_matches(|c: char| c != '(');
                let mut depth = 0usize;
                for (at, c) in list.char_indices() {
                    match c {
                        '(' => depth += 1,
                        ')' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                let rest = list[at + 1..].trim();
                                return rest == "{"
                                    || (rest.starts_with(':') && rest.ends_with('{'));
                            }
                        }
                        _ => {}
                    }
                }
                false
            };
            after.starts_with(['(', '<'])
                && !line.contains("=>")
                && !line.contains("function")
                && closes()
                && before.split_whitespace().all(|w| {
                    matches!(
                        w.trim_start_matches(['*', '#']),
                        "" | "public"
                            | "private"
                            | "protected"
                            | "static"
                            | "async"
                            | "readonly"
                            | "override"
                            | "abstract"
                            | "get"
                            | "set"
                            | "export"
                            | "default"
                    )
                })
        }
        "rs" => {
            before
                .split_whitespace()
                .next_back()
                .is_some_and(|last| matches!(last, "static" | "mut"))
                && before.split_whitespace().any(|w| w == "static")
        }
        _ => false,
    }
}

/// The line of `contents` that locates `name`: the first declaration by the lexical
/// definition pattern, else the first use outside an import, else the first use.
pub fn declaration_line(contents: &str, name: &str) -> Option<u32> {
    let name = leaf(name);
    if name.is_empty() {
        return None;
    }
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let uses = |line: &str| {
        line.match_indices(name).any(|(at, _)| {
            !line[..at].chars().next_back().is_some_and(word)
                && !line[at + name.len()..].chars().next().is_some_and(word)
        })
    };
    let number = |index: usize| u32::try_from(index + 1).ok();
    let declares = regex::Regex::new(&definition_pattern(name)).ok();
    let mut first_use = None;
    let mut first_import = None;
    for (index, line) in contents.lines().enumerate() {
        if !uses(line) {
            continue;
        }
        if declares
            .as_ref()
            .is_some_and(|re| reads_as_declaration(re, line))
        {
            return number(index);
        }
        if is_import(line) {
            first_import = first_import.or(Some(index));
        } else {
            first_use = first_use.or(Some(index));
        }
    }
    first_use.or(first_import).and_then(number)
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
    let declares = declaration_matcher(&name);
    let defs: Vec<Hit> = run(base(vec!["-E", "-e", &pattern, "--"]))?
        .into_iter()
        .filter(|hit| declares(&hit.text))
        .collect();
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
                        .detail(hit_detail(&h.text))
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
                            .detail(hit_detail(&h.text))
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
    fn a_typed_use_is_not_a_declaration() {
        assert!(declares("sealed interface Clock {", "Clock"));
        assert!(declares("    val Clock = SystemClock()", "Clock"));
        assert!(declares("pub fn <T: Ord> sort(items: &mut [T])", "sort"));
        assert!(!declares("    val started: Clock = Clock.now()", "Clock"));
        assert!(!declares("fun tick(clock: Clock)", "Clock"));
        assert_eq!(
            declaration_line("val started: Clock = now()\nclass Clock\n", "Clock"),
            Some(2)
        );
    }

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
    fn filename_matches_rank_first_and_ties_use_path_order() {
        let hits = parse_hits(
            b"b/Row.kt\x001:Row\na/Row.kt\x001:Row\nz/RowDriver.kt\x009:Row\na/DriverRow.kt\x001:Row\na/Unrelated.kt\x001:Row\na/Row.kt\x003:Row\n",
        );
        let q = FindQuery {
            text: "Row".into(),
            regex: false,
            globs: Vec::new(),
        };
        let files = rank_files(&hits, &q);
        let names: Vec<_> = files.iter().map(|rows| rows[0].path.as_str()).collect();
        assert_eq!(
            names,
            [
                "a/Row.kt",
                "b/Row.kt",
                "z/RowDriver.kt",
                "a/DriverRow.kt",
                "a/Unrelated.kt"
            ]
        );
        assert_eq!(files[0][1].line, 3);
        let q = FindQuery { regex: true, ..q };
        assert_eq!(rank_files(&hits, &q)[0][0].path, "a/DriverRow.kt");
    }

    #[test]
    fn declarations_are_told_from_types_receivers_and_bounds() {
        assert!(declares("fun TokenStore.renew() {", "renew"));
        assert!(!declares("fun TokenStore.renew() {", "TokenStore"));
        assert!(declares(
            "class Pair(val left: Int, val right: String)",
            "right"
        ));
        assert!(!declares("val store: TokenStore = make()", "TokenStore"));
        assert!(!declares("fun <T : Limit> run()", "Limit"));
        assert!(!declares("class Sub extends Base {", "Base"));
        assert!(declares("class Sub extends Base {", "Sub"));
    }

    #[test]
    fn keyword_less_declarations_are_recognised_by_language() {
        for (path, line, name, expected) in [
            (
                "a.java",
                "    public Token rotate(String id) {",
                "rotate",
                true,
            ),
            ("a.java", "        return s.rotate(\"a\");", "rotate", false),
            ("a.java", "        return rotate(1);", "rotate", false),
            ("a.c", "static int rotate(struct buf *b) {", "rotate", true),
            ("a.c", "    x = ok ? a : rotate(b);", "rotate", false),
            ("a.c", "        first, rotate(b));", "rotate", false),
            ("a.ts", "  rotate(id: string): Token {", "rotate", true),
            ("a.ts", "  async rotate(id) {", "rotate", true),
            ("a.ts", "  rotate(id, () => {", "rotate", false),
            ("a.js", "rotate({", "rotate", false),
            (
                "a.go",
                "func (s *Store) Rotate(id string) error {",
                "Rotate",
                true,
            ),
            ("a.rs", "pub static ROTATION: u32 = 1;", "ROTATION", true),
            ("a.kt", "    rotate(id) {", "rotate", false),
            ("a.java", "    größe wert(int größe) {", "wert", true),
        ] {
            assert_eq!(may_declare(path, line, name), expected, "{path}: {line}");
        }
    }

    #[test]
    fn imports_of_several_languages_are_imports() {
        for line in [
            "import lib.Name",
            "from lib import Name",
            "pub use crate::lib::Name;",
            "pub(crate) use lib::Name;",
            "#include <name.h>",
            "using Lib.Name;",
        ] {
            assert!(is_import(line), "{line}");
        }
        assert!(!is_import("useful(Name)"));
        assert!(!is_import("fromage = Name()"));
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

    #[test]
    fn uses_are_searched_as_source_writes_the_name() {
        assert_eq!(written_as("Command.BeginQuery"), "Command.BeginQuery");
        assert_eq!(written_as("Screen.kt"), "Screen.kt");
        assert_eq!(written_as("TokenRefresher::refresh"), "refresh");
        assert_eq!(written_as("Type#member"), "member");
    }

    #[test]
    fn a_symbol_is_located_at_its_declaration_then_its_first_real_use() {
        let source =
            "import lib.Clock\n// Clock drives expiry\nval limit = Clock.now()\nclass Clock\n";
        assert_eq!(declaration_line(source, "lib.Clock"), Some(4));
        let uses = "import lib.Clock\nval started = Clock.now()\n";
        assert_eq!(declaration_line(uses, "Clock"), Some(2));
        assert_eq!(declaration_line("import lib.Clock\n", "Clock"), Some(1));
        assert_eq!(
            declaration_line("val clockwork = Clocks.now()\n", "Clock"),
            None
        );
    }
}
