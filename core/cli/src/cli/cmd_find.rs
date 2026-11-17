//! `kb find "<text>"`: search the code root's text through the `find` route (ripgrep, else
//! the built-in `git grep`). For bare identifiers, Kotlin and Java body matches come
//! before imports and packages, with one match per file in each round.
//!
//! Two answers carry what would otherwise be the next call: a text with a few matches
//! comes with the lines around them, and a declaration-shaped text that misses (`sealed
//! interface Name`) is answered for the name itself: where it is declared after all, or
//! that the checkout only uses it, or that it does not occur.

use super::Ctx;
use super::agent;
use super::args::FindArgs;
use super::cmd_code::{self, Sighting};
use crate::answer::{Answer, Item};
use crate::error::{KbError, Result};
use crate::model::tools::Capability;
use crate::output::CommandOutput;
use crate::tools::{self, grep};

pub fn run(ctx: &Ctx, args: &FindArgs) -> Result<CommandOutput> {
    if args.text.is_empty() || args.text.contains('\n') || args.text.contains('\0') {
        return Err(KbError::invalid_input(
            "search text must be one non-empty line",
        ));
    }
    for g in &args.globs {
        crate::glob::RepoGlob::parse(g.strip_prefix('!').unwrap_or(g))
            .map_err(KbError::invalid_input)?;
    }
    let ws = agent::workspace(ctx)?;
    let lines = agent::lines(args.answer.lines);
    let (mut query, mut outcome, mut searched, mut notes) =
        search(&ws, args, &args.globs, &args.text, args.regex, lines)?;
    let mut globs = args.globs.clone();
    // Globs that select no file are usually right about the name and wrong about its
    // case, or name a directory: look there instead of answering only that they were wrong.
    if !outcome.found && searched == Some(0) {
        for (candidate, what) in glob_fallbacks(&args.globs) {
            let (q, o, n, mut more) = search(&ws, args, &candidate, &args.text, args.regex, lines)?;
            if o.found || n.is_some_and(|n| n > 0) {
                more.push(format!(
                    "the glob(s) select no file as written; searched {what}"
                ));
                (query, outcome, searched, notes) = (q, o, n, more);
                globs = candidate;
                break;
            }
        }
    }
    // A name or phrase that occurs nowhere as written is often a guess at how the code
    // words a thing: the lines that hold all of its words are the nearest answer.
    let mut loose = None;
    if !outcome.found
        && !args.regex
        && declared_name(args).is_none()
        && let Some(words) = loose_words(&args.text)
    {
        let (q, mut o, _, _) = search(&ws, args, &globs, &all_words(&words), true, lines)?;
        if o.found {
            o.notes.clear();
            o.items.truncate(NEAR_MATCHES);
            (query, outcome) = (q, o);
            loose = Some(words);
        }
    }
    // A miss inside one file: its declarations say where to read instead.
    let mut outline = Vec::new();
    let mut outline_cut = false;
    if !outcome.found
        && searched == Some(1)
        && !globs.is_empty()
        && let Ok((_, o, _, _)) = search(&ws, args, &globs, OUTLINE, true, lines)
    {
        // The search returns at most twenty lines of a file.
        outline_cut = o.items.len() >= 20;
        // The search ignores case and file type; the outline does not: a heading is a
        // line of a Markdown file, a declaration opens with the keyword as written.
        let keyword_led = regex::Regex::new(OUTLINE_CODE).ok();
        outline = o
            .items
            .into_iter()
            .filter(|item| {
                let text = item.detail.as_deref().unwrap_or_default();
                let markdown = item.path.as_deref().is_some_and(|path| {
                    [".md", ".markdown", ".mdx"]
                        .iter()
                        .any(|ext| path.to_ascii_lowercase().ends_with(ext))
                });
                if markdown {
                    text.starts_with('#')
                } else {
                    keyword_led.as_ref().is_some_and(|re| re.is_match(text))
                }
            })
            .collect();
    }
    let mut answer = Answer::new("find", format!("{:?}", args.text));
    answer.prioritize_code_matches =
        matches!(outcome.source.adapter.as_str(), "ripgrep" | "git-grep")
            && query.prioritize_code_matches(
                outcome.items.iter().filter_map(|item| item.path.as_deref()),
            );
    answer.source(outcome.source.clone());
    for n in notes.into_iter().chain(outcome.notes) {
        answer.note(n);
    }
    answer.items = outcome.items;
    if let Some(words) = &loose {
        answer.notes.insert(
            0,
            format!(
                "no line reads {:?}; these lines hold its words ({})",
                args.text,
                words.join(", ")
            ),
        );
    }
    let failure = if outcome.found {
        None
    } else {
        match declared_name(args) {
            Some(name) => declaration_miss(&ws, args, name, searched, &mut answer),
            None => Some(miss(args, searched)),
        }
    };
    if failure.is_some() && answer.items.is_empty() && !outline.is_empty() {
        answer.note(format!(
            "no line of the file reads {:?}; its declarations follow",
            args.text
        ));
        if outline_cut {
            answer.note("the outline was made from the first 20 matching lines of the file");
        }
        answer.items = outline
            .into_iter()
            .map(|mut item| {
                item.kind = "declaration".into();
                item
            })
            .collect();
    }
    if failure.is_none() {
        surroundings(&ws, &mut answer);
    }
    Ok(agent::output(answer, args.answer.lines, failure))
}

/// One pass over the `find` route with the given globs: the query, the first tool's
/// outcome, the number of files it covered when it missed, and the hand-over notes.
fn search(
    ws: &agent::Workspace,
    args: &FindArgs,
    globs: &[String],
    text: &str,
    regex: bool,
    lines: usize,
) -> Result<(grep::FindQuery, tools::Outcome, Option<usize>, Vec<String>)> {
    let query = grep::FindQuery {
        text: text.to_owned(),
        regex,
        globs: globs.to_vec(),
    };
    let mut notes = Vec::new();
    let mut result = None;
    let mut searched = None;
    for tool in tools::route_for(
        &ws.registry,
        Capability::Find.as_str(),
        args.answer.tool.as_deref(),
    )? {
        let probe = tools::probe(tool, &ws.roots, true);
        if !probe.available {
            notes.push(format!(
                "{} skipped: {}",
                tool.id,
                probe.reason.unwrap_or_default()
            ));
            continue;
        }
        let found = if tool.adapter == crate::model::tools::AdapterKind::Exec {
            if regex {
                notes.push(format!(
                    "{} receives the text as given; --regex is up to the tool",
                    tool.id
                ));
            }
            tools::exec::query(
                tool,
                &probe,
                &ws.roots,
                &tools::exec::ExecRequest {
                    capability: "find".into(),
                    subject: text.to_owned(),
                    globs: globs.to_vec(),
                    limit: lines,
                    ..tools::exec::ExecRequest::default()
                },
            )
        } else {
            grep::find(tool, &probe, &ws.roots, &query)
        };
        match found {
            Ok(outcome) => {
                if !outcome.found {
                    searched = grep::searched_files(tool, &probe, &ws.roots, globs);
                }
                result = Some(outcome);
                break;
            }
            Err(e) => notes.push(format!("{} failed: {}", tool.id, e.message)),
        }
    }
    let outcome =
        result.ok_or_else(|| tools::unavailable(Capability::Find.as_str(), notes.clone()))?;
    Ok((query, outcome, searched, notes))
}

/// Lines kept of a loose match: enough to see how the code words the thing.
const NEAR_MATCHES: usize = 10;

/// Lines that open a declaration with one of the listed keywords, and Markdown headings,
/// for the outline of a file whose text missed. Lower case, so that both search tools
/// match it without regard to case. Declarations written without such a keyword (a
/// constant, a method in a language of the C family) are not part of the outline.
const OUTLINE: &str = "^[[:space:]]*((public|private|internal|protected|override|suspend|inline|data|sealed|abstract|open|export|pub|pub\\([a-z ]*\\)|static|final|async|actual|expect|annotation|value|companion)[[:space:]]+)*(fn|def|class|struct|enum|trait|interface|func|function|fun|object|protocol|module|impl|typealias|record)[[:space:]]|^#{1,6}[[:space:]]";

/// The keyword-led part of [`OUTLINE`], matched with regard to case against a line that
/// the search returned (already trimmed).
const OUTLINE_CODE: &str = "^((public|private|internal|protected|override|suspend|inline|data|sealed|abstract|open|export|pub|pub\\([a-z ]*\\)|static|final|async|actual|expect|annotation|value|companion)\\s+)*(fn|def|class|struct|enum|trait|interface|func|function|fun|object|protocol|module|impl|typealias|record)\\s";

/// Other readings of globs that select no file, most literal first: the same globs
/// without regard to case, then every pathless glob as "below the directories of that
/// name" (`*tokenStore*` becomes `**/*[tT][oO]...*/**`).
fn glob_fallbacks(globs: &[String]) -> Vec<(Vec<String>, &'static str)> {
    let caseless = |glob: &String| -> String {
        if glob.starts_with('!') {
            return glob.clone();
        }
        let mut out = String::new();
        let mut class = false;
        for c in glob.chars() {
            match c {
                '[' => class = true,
                ']' => class = false,
                _ => {}
            }
            if c.is_ascii_alphabetic() && !class {
                out.push('[');
                out.push(c.to_ascii_lowercase());
                out.push(c.to_ascii_uppercase());
                out.push(']');
            } else {
                out.push(c);
            }
        }
        out
    };
    let any_case: Vec<String> = globs.iter().map(caseless).collect();
    let mut pathless = false;
    let below: Vec<String> = any_case
        .iter()
        .map(|glob| {
            if glob.starts_with('!') || glob.contains('/') {
                glob.clone()
            } else {
                pathless = true;
                format!("**/{glob}/**")
            }
        })
        .collect();
    let mut fallbacks = Vec::new();
    if any_case != globs {
        fallbacks.push((any_case, "without regard to case"));
    }
    if pathless {
        fallbacks.push((below, "below the directories of that name"));
    }
    fallbacks
}

/// The words of a text that is worth a loose search: a name of two or three words
/// (`refreshDelay`), or a phrase of two or three (`rotation window`). Longer names and
/// anything with punctuation are searched as written only.
fn loose_words(text: &str) -> Option<Vec<String>> {
    if !text
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == ' ')
    {
        return None;
    }
    let words: Vec<String> = crate::context::search::name_words(text)
        .into_iter()
        .filter(|word| word.chars().count() >= 3 && word.chars().all(char::is_alphabetic))
        .collect();
    let all = crate::context::search::name_words(text).len();
    ((2..=3).contains(&words.len()) && words.len() == all).then_some(words)
}

/// A pattern for the lines that hold every word, in any order.
fn all_words(words: &[String]) -> String {
    let mut orders: Vec<Vec<usize>> = vec![Vec::new()];
    for _ in words {
        orders = orders
            .into_iter()
            .flat_map(|order| {
                (0..words.len())
                    .filter(|i| !order.contains(i))
                    .map(|i| [order.as_slice(), &[i]].concat())
                    .collect::<Vec<_>>()
            })
            .collect();
    }
    orders
        .iter()
        .map(|order| {
            order
                .iter()
                .map(|&i| words[i].as_str())
                .collect::<Vec<_>>()
                .join(".*")
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// Lines after a single match that are shown with it.
const TRAIL_LINES: u32 = 20;

/// The few places a text occurs are what gets read next, so show them with the matches:
/// a single match with more of what follows it, several with the lines around each.
fn surroundings(ws: &agent::Workspace, answer: &mut Answer) {
    match answer.items.len() {
        1 => cmd_code::attach_places(ws, answer, TRAIL_LINES),
        n if n <= cmd_code::FEW_PLACES => {
            cmd_code::attach_places(ws, answer, cmd_code::TRAIL_LINES)
        }
        _ => {}
    }
}

/// The identifier a declaration-shaped text names: `sealed interface Name`, `fun name(`.
/// Such a text misses on any difference in keywords, so its miss says little by itself.
fn declared_name(args: &FindArgs) -> Option<&str> {
    const WORDS: &[&str] = &[
        "abstract",
        "actual",
        "annotation",
        "async",
        "class",
        "companion",
        "const",
        "data",
        "def",
        "enum",
        "expect",
        "export",
        "final",
        "fn",
        "fun",
        "func",
        "function",
        "impl",
        "inline",
        "interface",
        "internal",
        "lateinit",
        "let",
        "module",
        "namespace",
        "object",
        "open",
        "override",
        "private",
        "protected",
        "protocol",
        "pub",
        "public",
        "record",
        "sealed",
        "static",
        "struct",
        "suspend",
        "trait",
        "type",
        "typealias",
        "val",
        "value",
        "var",
    ];
    // The words that introduce a declaration; a modifier alone (`private`, `open`) does not.
    const DEFINERS: &[&str] = &[
        "class",
        "const",
        "def",
        "enum",
        "fn",
        "fun",
        "func",
        "function",
        "impl",
        "interface",
        "namespace",
        "protocol",
        "struct",
        "trait",
        "typealias",
        "val",
        "var",
    ];
    // Further definers, kept apart only because they are everyday words as well.
    const EVERYDAY: &[&str] = &["let", "module", "object", "record", "type"];
    if args.regex {
        return None;
    }
    let mut tokens = args.text.split_whitespace();
    let last = tokens.next_back()?;
    let end = last
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(last.len());
    let (name, rest) = last.split_at(end);
    let code_like = name
        .chars()
        .any(|c| c.is_uppercase() || c.is_numeric() || c == '_');
    let mut keywords = 0;
    let mut definer = false;
    for token in tokens {
        if !WORDS.contains(&token) {
            return None;
        }
        definer |= DEFINERS.contains(&token) || EVERYDAY.contains(&token);
        keywords += 1;
    }
    // Declaration punctuation after the name settles it; without any, the name has to be
    // written like code: `class action` and `open file` are prose.
    (keywords > 0
        && !WORDS.contains(&name)
        && ((definer && code_like) || !rest.is_empty())
        && name.chars().next().is_some_and(|c| !c.is_numeric())
        && rest.chars().all(|c| "(<:{=".contains(c)))
    .then_some(name)
}

/// Answer the miss of a declaration-shaped text for the name: `None` when the name is
/// declared after all, with its declaration lines as the answer.
fn declaration_miss(
    ws: &agent::Workspace,
    args: &FindArgs,
    name: &str,
    searched: Option<usize>,
    answer: &mut Answer,
) -> Option<KbError> {
    // The name is looked up in the whole checkout; the text was only searched in the
    // files the globs select, and the note says no more than that.
    let scope = if args.globs.is_empty() {
        "no line"
    } else {
        "no line of the files the glob(s) select"
    };
    let (hits, confidence, verb) = match cmd_code::sighting(ws, name) {
        Sighting::Unknown => return Some(miss(args, searched)),
        Sighting::Absent(files) => return Some(cmd_code::absent(ws, name, files, answer)),
        Sighting::Used(found) => return Some(cmd_code::used(ws, name, &found, answer)),
        Sighting::Declared(hits) => (hits, "lexical", "is declared as shown"),
        Sighting::Possible(hits) => (hits, "possible", "may be declared as shown"),
    };
    answer.source(cmd_code::lexical_source());
    answer.note(format!("{scope} reads {:?}; `{name}` {verb}", args.text));
    answer.items = hits
        .into_iter()
        .map(|hit| {
            Item::new("definition", cmd_code::LEXICAL)
                .name(name)
                .at(hit.path, Some(hit.line))
                .detail(grep::hit_detail(&hit.text))
                .confidence(confidence)
        })
        .collect();
    None
}

/// A miss names what was searched, so an absent text is not asked for again in other words:
/// the whole checkout, the files the globs select, or globs that select nothing.
fn miss(args: &FindArgs, searched: Option<usize>) -> KbError {
    let text = &args.text;
    let directory_glob = args
        .globs
        .iter()
        .any(|g| !g.starts_with('!') && !g.contains(['/', '.']));
    let error = match searched {
        Some(0) if !args.globs.is_empty() => {
            agent::not_found(format!("no match for {text:?}: the glob(s) select no file"))
        }
        Some(n) if args.globs.is_empty() => agent::not_found(format!(
            "no match for {text:?} in the {n} file(s) of this checkout"
        )),
        Some(n) => agent::not_found(format!(
            "no match for {text:?} in the {n} file(s) the glob(s) select"
        )),
        None => agent::not_found(format!("no match for {text:?}")),
    };
    if directory_glob {
        error.with_hint("to search below a directory, use a glob such as `--glob '**/DIR/**'`")
    } else {
        error
    }
}
