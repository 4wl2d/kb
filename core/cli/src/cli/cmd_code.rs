//! `kb sym`, `kb callers` and the symbol mode of `kb impact`: code requests routed through
//! the tool registry (docs/agent-cli.md). The answer names the tool and the state of what
//! it read; a symbol no tool knows is `NOT_FOUND` with the result still printed.

use super::Ctx;
use super::agent::{self, Detail};
use super::args::{CallersArgs, ImpactArgs, SymArgs};
use super::cmd_read;
use crate::answer::{Answer, EXCERPT_LINES, Item, Source};
use crate::error::{KbError, Result};
use crate::model::tools::Capability;
use crate::output::CommandOutput;
use crate::tools::{self, CodeQuery, grep};

pub(super) fn shell_word(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "_-.:/#".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

pub(super) fn read_around(path: &str, line: u32) -> String {
    let first = line.saturating_sub(2).max(1);
    let last = line.saturating_add(8);
    format!("kb read {} {first} {last}", shell_word(path))
}

/// The `kb read` that shows a definition: its span when the tool reports one, as far as
/// one excerpt reaches, so the body needs no second, overlapping read.
pub(super) fn read_definition(path: &str, line: u32, end_line: Option<u32>) -> String {
    let Some(end) = end_line.filter(|end| *end > line) else {
        return read_around(path, line);
    };
    let first = line.saturating_sub(2).max(1);
    let last = end
        .max(line.saturating_add(8))
        .min(first.saturating_add(super::cmd_read::MAX_LINES as u32 - 1));
    format!("kb read {} {first} {last}", shell_word(path))
}

/// Lines above a located line shown with it: annotations, the end of a doc comment.
pub(super) const LEAD_LINES: u32 = 3;
/// Lines below a located line shown with it when several places are shown together.
pub(super) const TRAIL_LINES: u32 = 8;
/// Lines shown of a definition whose end the tool did not report.
const DEFINITION_LINES: u32 = 15;
/// Lines shown at most of each definition when a name has a few of them.
const BRIEF_DEFINITION_LINES: u32 = 25;
/// Definitions, call sites or matches few enough to be shown with their lines.
pub(super) const FEW_PLACES: usize = 4;

/// Put the located definition into the answer, so that no read has to follow: its span as
/// far as one excerpt reaches, then the `kb read` that continues it.
fn attach_definition(
    ws: &agent::Workspace,
    answer: &mut Answer,
    path: &str,
    line: u32,
    end_line: Option<u32>,
) {
    let first = line.saturating_sub(LEAD_LINES).max(1);
    let end = end_line
        .filter(|end| *end > line)
        .unwrap_or(line.saturating_add(DEFINITION_LINES - 1));
    match cmd_read::attach(&ws.roots.code, path, first, end, EXCERPT_LINES) {
        Some(excerpt) => {
            if let Some(next) = excerpt.next_line {
                let last = end.min(next.saturating_add(cmd_read::MAX_LINES as u32 - 1));
                answer.next(format!("kb read {} {next} {last}", shell_word(path)));
            }
            answer.excerpts.push(excerpt);
        }
        None => answer.next(read_definition(path, line, end_line)),
    }
}

/// A name with a few definitions (overloads, an interface and its implementation): show
/// the head of each, so that choosing between them needs no read.
fn attach_definitions(ws: &agent::Workspace, answer: &mut Answer, items: &[Item]) {
    // Every head gets its share of the excerpt lines, at most BRIEF_DEFINITION_LINES.
    let definitions = items.iter().filter(|i| i.kind == "definition").count() as u32;
    let head = (EXCERPT_LINES as u32 / definitions.max(1))
        .saturating_sub(LEAD_LINES)
        .clamp(1, BRIEF_DEFINITION_LINES);
    let spans: Vec<(String, u32, u32)> = items
        .iter()
        .filter(|item| item.kind == "definition")
        .filter_map(|item| {
            let (path, line) = (item.path.as_ref()?, item.line?);
            let end = item
                .end_line
                .filter(|end| *end > line)
                .unwrap_or(line.saturating_add(DEFINITION_LINES - 1))
                .min(line.saturating_add(head - 1));
            Some((path.clone(), line.saturating_sub(LEAD_LINES).max(1), end))
        })
        .collect();
    answer.excerpts = cmd_read::attach_spans(&ws.roots.code, &spans);
}

/// Show a few located places (call sites, matches) with the lines around them. A place
/// that is an import line is named by its item and not shown: what surrounds it is other
/// imports.
pub(super) fn attach_places(ws: &agent::Workspace, answer: &mut Answer, trail: u32) {
    let spans: Vec<(String, u32, u32)> = answer
        .items
        .iter()
        .filter_map(|item| {
            let (path, line) = (item.path.as_ref()?, item.line?);
            let text = cmd_read::attach(&ws.roots.code, path, line, line, 1)?;
            if text.lines.first().is_some_and(|l| grep::is_import(l)) {
                return None;
            }
            Some((
                path.clone(),
                line.saturating_sub(LEAD_LINES).max(1),
                line.saturating_add(trail),
            ))
        })
        .collect();
    answer.excerpts = cmd_read::attach_spans(&ws.roots.code, &spans);
}

/// Uses listed for a symbol that is not declared in the checkout: one per file.
const MAX_REFERENCES: usize = 5;
/// Similar names kept for a symbol that does not occur at all.
const MAX_SIMILAR: usize = 3;
/// Source id of the built-in `git grep` scan behind those uses.
pub(super) const LEXICAL: &str = "git-grep";

pub(super) fn lexical_source() -> Source {
    Source {
        id: LEXICAL.into(),
        adapter: LEXICAL.into(),
        version: None,
        state: "working tree, lexical".into(),
    }
}

/// What the text of the checkout holds of a name that no tool defined.
pub(super) enum Sighting {
    /// Not established: no Git work tree, or the scan failed.
    Unknown,
    /// Lines that read as its declaration.
    Declared(Vec<grep::Hit>),
    /// Lines that could declare it in a language that needs no keyword for that.
    Possible(Vec<grep::Hit>),
    /// Used, and no line reads as its declaration.
    Used(grep::Occurrences),
    /// In none of this many files.
    Absent(usize),
}

pub(super) fn sighting(ws: &agent::Workspace, name: &str) -> Sighting {
    let timeout = ws
        .registry
        .config
        .route_named(Capability::Sym.as_str())
        .first()
        .map_or(60, |tool| tool.timeout());
    let Ok(found) = grep::occurrences(&ws.roots, name, timeout) else {
        return Sighting::Unknown;
    };
    if !found.declarations.is_empty() {
        return Sighting::Declared(found.declarations);
    }
    let possible: Vec<grep::Hit> = found
        .hits
        .iter()
        .filter(|hit| grep::may_declare(&hit.path, &hit.text, name))
        .cloned()
        .collect();
    if !possible.is_empty() {
        Sighting::Possible(possible)
    } else if found.hits.is_empty() {
        Sighting::Absent(found.searched)
    } else {
        Sighting::Used(found)
    }
}

/// The lexical scan does not enter submodules: say so where a final answer rests on it.
fn submodules_note(ws: &agent::Workspace, answer: &mut Answer) {
    let Ok(modules) = std::fs::read_to_string(ws.roots.code.join(".gitmodules")) else {
        return;
    };
    // The mounted knowledge base is a submodule of most hosts and holds no host code.
    let other = modules
        .lines()
        .filter_map(|line| line.trim().strip_prefix("path"))
        .filter_map(|rest| rest.trim_start().strip_prefix('='))
        .map(str::trim)
        .any(|path| ws.roots.kb_mount.as_deref() != Some(path));
    if other {
        let note = "submodules of this checkout were not scanned".to_string();
        if !answer.notes.contains(&note) {
            // Second, after the conclusion it qualifies: later notes may be cut.
            answer.notes.insert(1.min(answer.notes.len()), note);
        }
    }
}

/// The hand-over note of a tool that did not know the symbol: a final answer replaces it.
fn drop_hand_over(answer: &mut Answer, symbol: &str) {
    let handed_over = format!("did not find `{symbol}`");
    answer.notes.retain(|note| !note.ends_with(&handed_over));
}

/// A dotted name that no line writes, while its last segment occurs: the member may be
/// declared inside its type or called on an instance, so nothing final is said. The answer
/// points at the lookups by the member's own name.
fn not_written(name: &str, member: &str, answer: &mut Answer) -> KbError {
    answer.items.clear();
    answer.notes.insert(
        0,
        format!("no line of this checkout writes `{name}`; `{member}` occurs in it"),
    );
    answer.next(format!("kb sym {}", shell_word(member)));
    answer.next(format!("kb callers {}", shell_word(member)));
    agent::not_found(format!("no line writes `{name}`")).with_hint(
        "a member is declared and often called without its type's name; look it up by its own name",
    )
}

/// The member of a written dotted name that has a declaration of its own in a checkout
/// that declares its type: `Type.member()` is used, and `member` is declared inside
/// `Type` without the dot. The final "used, no declaration" answer would be wrong there.
fn declared_member<'a>(ws: &agent::Workspace, name: &'a str) -> Option<&'a str> {
    let member = grep::leaf(name);
    (member != grep::written_as(name)
        && owner(name).is_some()
        && !declared_elsewhere(ws, name)
        && matches!(
            sighting(ws, member),
            Sighting::Declared(_) | Sighting::Possible(_)
        ))
    .then_some(member)
}

/// The answer for [`declared_member`]: not final, it names the lookup that finds it.
fn by_member(name: &str, member: &str, answer: &mut Answer) -> KbError {
    answer.items.clear();
    answer.notes.insert(
        0,
        format!("no line declares `{name}` under that name; `{member}` has a declaration in this checkout"),
    );
    answer.next(format!("kb sym {}", shell_word(member)));
    agent::not_found(format!("no declaration is written as `{name}`"))
        .with_hint("a member is declared without its type's name; look it up by its own name")
}

/// The member of a dotted name that is worth a lookup of its own: the last segment, when
/// the name as written occurs nowhere but that segment does.
fn written_member<'a>(ws: &agent::Workspace, name: &'a str) -> Option<&'a str> {
    let member = grep::leaf(name);
    (member != grep::written_as(name)
        && !matches!(
            sighting(ws, member),
            Sighting::Absent(_) | Sighting::Unknown
        ))
    .then_some(member)
}

/// The type before the dot of `Type.member`: when it is not declared here, the member is
/// not either. A lower-case head is a package or a value and proves nothing.
fn owner(name: &str) -> Option<&str> {
    let (head, _) = name.split_once('.')?;
    (head.chars().next()?.is_uppercase() && head.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .then_some(head)
}

/// Is `name` declared outside this checkout, going by the type it belongs to?
fn declared_elsewhere(ws: &agent::Workspace, name: &str) -> bool {
    let subject = owner(name).unwrap_or_else(|| grep::leaf(name));
    matches!(
        sighting(ws, subject),
        Sighting::Used(_) | Sighting::Absent(_)
    )
}

/// The final answer for a name that occurs nowhere: there is nothing else to look up.
pub(super) fn absent(
    ws: &agent::Workspace,
    name: &str,
    searched: usize,
    answer: &mut Answer,
) -> KbError {
    answer.items.truncate(MAX_SIMILAR);
    answer.notes.insert(
        0,
        format!("`{name}` does not occur in the {searched} file(s) of this checkout"),
    );
    submodules_note(ws, answer);
    agent::not_found(format!("`{name}` does not occur in this checkout")).with_hint(
        "check the spelling: the name was looked for as a whole word, as written, in the files `kbw find` and `kbw read` cover",
    )
}

/// The final answer for a name that is used while no line reads as its declaration: its
/// uses are the evidence this checkout holds. The scan is textual, so the answer says what
/// it saw and does not claim more: the declaration is in a dependency, in generated code,
/// in a part of the repository that is not checked out, or written in a form the scan does
/// not recognise.
pub(super) fn used(
    ws: &agent::Workspace,
    name: &str,
    found: &grep::Occurrences,
    answer: &mut Answer,
) -> KbError {
    let files = found.files();
    let searched = found.searched;
    answer
        .notes
        .retain(|note| !note.contains("similar symbols follow"));
    answer.notes.insert(
        0,
        format!(
            "`{name}` is used in {files} file(s) and no line of the {searched} file(s) of this checkout reads as its declaration"
        ),
    );
    submodules_note(ws, answer);
    answer.source(lexical_source());
    answer.items = found
        .hits
        .chunk_by(|a, b| a.path == b.path)
        .take(MAX_REFERENCES)
        .map(|rows| {
            // An import says less about the use than the code below it.
            let hit = rows
                .iter()
                .find(|hit| !grep::is_import(&hit.text))
                .unwrap_or(&rows[0]);
            Item::new("reference", LEXICAL)
                .at(hit.path.clone(), Some(hit.line))
                .detail(grep::hit_detail(&hit.text))
                .confidence("lexical")
        })
        .collect();
    if files > MAX_REFERENCES {
        answer.next(format!("kb callers {}", shell_word(name)));
    }
    agent::not_found(format!("no declaration of `{name}` was found in this checkout")).with_hint(
        "it is declared in a dependency, in generated code or in a form the text scan does not recognise; the uses above are what the files `kbw find` and `kbw read` cover hold of it, so cite them",
    )
}

/// Turn "no tool found a definition" into an answer the agent can stop on: the name is
/// used while nothing reads as its declaration, or it does not occur at all, or a line
/// could be its declaration in a language that writes one without a keyword (`None`: the
/// answer then holds that line as a possible definition). The plain miss is kept where no
/// such conclusion is established: a lookup narrowed by `--file` or `--kind`, no Git work
/// tree, or a line that reads as a declaration the route did not report.
fn undeclared(ws: &agent::Workspace, args: &SymArgs, answer: &mut Answer) -> Option<KbError> {
    let name = &args.name;
    let plain = |answer: &mut Answer, hint: &str| {
        answer.items.truncate(MAX_SIMILAR);
        agent::not_found(format!("no definition of `{name}` was found")).with_hint(hint)
    };
    if args.file.is_some() || args.kind.is_some() {
        return Some(plain(
            answer,
            "the lookup was narrowed by --file or --kind; without them it covers the whole checkout",
        ));
    }
    let handed_over = format!("did not find `{name}`");
    match sighting(ws, name) {
        Sighting::Unknown | Sighting::Declared(_) => Some(plain(
            answer,
            "check the spelling, try `kbw find <text>`, or see `kbw tools` for the connected tools",
        )),
        Sighting::Possible(hits) => {
            answer.notes.retain(|note| {
                !note.ends_with(&handed_over) && !note.contains("similar symbols follow")
            });
            answer.notes.insert(
                0,
                format!("no tool resolved `{name}`; each line shown could be its declaration"),
            );
            answer.source(lexical_source());
            answer.items = hits
                .into_iter()
                .map(|hit| {
                    Item::new("definition", LEXICAL)
                        .name(grep::leaf(name))
                        .at(hit.path, Some(hit.line))
                        .detail(grep::hit_detail(&hit.text))
                        .confidence("possible")
                })
                .collect();
            if let [item] = answer.items.as_slice()
                && let (Some(path), Some(line)) = (item.path.clone(), item.line)
            {
                attach_definition(ws, answer, &path, line, None);
            }
            answer.next(format!("kb callers {}", shell_word(name)));
            None
        }
        Sighting::Absent(searched) => {
            answer.notes.retain(|note| !note.ends_with(&handed_over));
            Some(match written_member(ws, name) {
                Some(member) => not_written(name, member, answer),
                None => absent(ws, name, searched, answer),
            })
        }
        Sighting::Used(found) => {
            answer.notes.retain(|note| !note.ends_with(&handed_over));
            Some(match declared_member(ws, name) {
                Some(member) => by_member(name, member, answer),
                None => used(ws, name, &found, answer),
            })
        }
    }
}

/// The conclusive miss of `callers` and `impact`, where the checkout's text settles it.
fn unknown_symbol(ws: &agent::Workspace, symbol: &str, answer: &mut Answer) -> KbError {
    match sighting(ws, symbol) {
        Sighting::Absent(searched) => {
            drop_hand_over(answer, symbol);
            match written_member(ws, symbol) {
                Some(member) => not_written(symbol, member, answer),
                None => absent(ws, symbol, searched, answer),
            }
        }
        Sighting::Used(found) => {
            drop_hand_over(answer, symbol);
            used(ws, symbol, &found, answer)
        }
        _ => agent::not_found(format!("`{symbol}` was not found")),
    }
}

pub fn sym(ctx: &Ctx, args: &SymArgs) -> Result<CommandOutput> {
    let ws = agent::workspace(ctx)?;
    let lines = agent::lines(args.answer.lines);
    let q = CodeQuery {
        capability: Capability::Sym,
        name: args.name.clone(),
        file: args.file.clone(),
        kind: args.kind.clone(),
        depth: 1,
        limit: lines,
    };
    let outcome = tools::answer_code(&ws.registry, &ws.roots, &q, args.answer.tool.as_deref())?;
    let mut answer = Answer::new("sym", shell_word(&args.name));
    answer.source(outcome.source.clone());
    for n in &outcome.notes {
        answer.note(n.clone());
    }
    // A dotted name answered from text matched only its last segment: when its type is
    // not declared here, the same-named member of another type is not the answer.
    let misattributed = outcome.found
        && outcome.source.adapter == LEXICAL
        && args.file.is_none()
        && args.kind.is_none()
        && owner(&args.name).is_some()
        // `fun Type.member` declares the member here, whatever is known of the type.
        && !outcome.items.iter().any(|item| {
            item.detail
                .as_deref()
                .is_some_and(|d| d.contains(grep::written_as(&args.name)))
        })
        && declared_elsewhere(&ws, &args.name);
    let found = outcome.found && !misattributed;
    if found {
        let definitions = outcome
            .items
            .iter()
            .filter(|item| item.kind == "definition")
            .count();
        if let Some(item) = outcome.items.first()
            && let (Some(path), Some(line)) = (&item.path, item.line)
        {
            if definitions == 1 {
                attach_definition(&ws, &mut answer, path, line, item.end_line);
            } else if (2..=FEW_PLACES).contains(&definitions) {
                attach_definitions(&ws, &mut answer, &outcome.items);
            }
            if definitions != 1 && answer.excerpts.is_empty() {
                answer.next(read_definition(path, line, item.end_line));
            }
        }
        answer.next(format!("kb callers {}", shell_word(&args.name)));
        answer.items = outcome.items;
    } else if !misattributed {
        answer.items = outcome.items;
    }
    let failure = if found {
        None
    } else {
        undeclared(&ws, args, &mut answer)
    };
    Ok(agent::output(answer, args.answer.lines, failure))
}

pub fn callers(ctx: &Ctx, args: &CallersArgs) -> Result<CommandOutput> {
    let ws = agent::workspace(ctx)?;
    let lines = agent::lines(args.answer.lines);
    let q = CodeQuery {
        capability: Capability::Callers,
        name: args.symbol.clone(),
        file: args.file.clone(),
        kind: None,
        depth: 1,
        limit: lines,
    };
    let outcome = tools::answer_code(&ws.registry, &ws.roots, &q, args.answer.tool.as_deref())?;
    let mut answer = Answer::new("callers", shell_word(&args.symbol));
    answer.source(outcome.source.clone());
    for n in &outcome.notes {
        answer.note(n.clone());
    }
    if outcome.found && outcome.items.is_empty() {
        answer.note("no callers were found");
    }
    // Text matches for a symbol whose type has no declaration here are what this checkout
    // holds of it: say so, and do not point at further lookups of the same symbol.
    let textual_only = outcome.found
        && outcome.source.adapter == LEXICAL
        && args.file.is_none()
        && declared_elsewhere(&ws, &args.symbol);
    let mut found = outcome.found;
    let mut items = outcome.items;
    // The member is used, but no use writes it with this type's name.
    let mut member_only = false;
    if textual_only && owner(&args.symbol).is_some() {
        // A member of a type that is not declared here: only the lines that write the
        // dotted name are its uses, not the same-named member of another type.
        let written = grep::written_as(&args.symbol);
        items.retain(|item| item.detail.as_deref().is_some_and(|d| d.contains(written)));
        found = !items.is_empty();
        member_only = !found;
    }
    let textual_only = textual_only && found;
    if textual_only {
        let subject = owner(&args.symbol).unwrap_or_else(|| grep::leaf(&args.symbol));
        // With a type before the dot the lines write the dotted name; otherwise they are
        // the uses of its last segment.
        let listed = if owner(&args.symbol).is_some() {
            args.symbol.as_str()
        } else {
            grep::leaf(&args.symbol)
        };
        drop_hand_over(&mut answer, &args.symbol);
        answer.notes.insert(
            0,
            format!(
                "no line of this checkout reads as a declaration of `{subject}`; these lines are textual uses of `{listed}` in it"
            ),
        );
    }
    answer.items = items;
    // A few call sites are what gets read next: show them with the answer.
    if found && (1..=FEW_PLACES).contains(&answer.items.len()) {
        attach_places(&ws, &mut answer, TRAIL_LINES);
    }
    if found && !textual_only {
        if let Some((path, line)) = &outcome.call_site
            && answer.excerpts.is_empty()
        {
            answer.next(read_around(path, *line));
        }
        answer.next(format!("kb impact {}", shell_word(&args.symbol)));
    }
    let failure = if member_only {
        drop_hand_over(&mut answer, &args.symbol);
        Some(not_written(
            &args.symbol,
            grep::leaf(&args.symbol),
            &mut answer,
        ))
    } else {
        (!found).then(|| unknown_symbol(&ws, &args.symbol, &mut answer))
    };
    Ok(agent::output(answer, args.answer.lines, failure))
}

/// `kb impact <symbol>`: the code that depends on the symbol, then the features and the
/// knowledge records that govern the affected files.
pub fn impact(ctx: &Ctx, args: &ImpactArgs, symbol: &str) -> Result<CommandOutput> {
    if super::code_options::provider(ctx, &args.code).configured() {
        return Err(KbError::invalid_input(
            "provider options belong to diff mode with --deep; symbol mode uses the tool registry",
        ));
    }
    if !(1..=4).contains(&args.depth) {
        return Err(KbError::invalid_input("impact depth must be 1..4"));
    }
    let ws = agent::workspace(ctx)?;
    let lines = agent::lines(args.answer.lines);
    let q = CodeQuery {
        capability: Capability::Impact,
        name: symbol.to_string(),
        file: args.file.clone(),
        kind: None,
        depth: args.depth,
        limit: lines,
    };
    let outcome = tools::answer_code(&ws.registry, &ws.roots, &q, args.answer.tool.as_deref())?;
    let mut answer = Answer::new("impact", shell_word(symbol));
    answer.source(outcome.source.clone());
    for n in &outcome.notes {
        answer.note(n.clone());
    }
    if !outcome.found {
        let failure = unknown_symbol(&ws, symbol, &mut answer);
        return Ok(agent::output(answer, args.answer.lines, Some(failure)));
    }
    if !outcome.files.is_empty() {
        let mut knowledge = Answer::new("impact", "");
        match agent::knowledge(ctx, &ws, &outcome.files, Detail::Impact, &mut knowledge) {
            Ok(()) => {
                for s in knowledge.sources {
                    answer.source(s);
                }
                answer.items.extend(knowledge.items);
                for n in knowledge.notes {
                    answer.note(n);
                }
            }
            Err(e) => answer.note(format!("knowledge not consulted: {}", e.message)),
        }
    }
    let affected = outcome.items.len();
    if outcome.source.adapter == LEXICAL && args.file.is_none() && declared_elsewhere(&ws, symbol) {
        let subject = owner(symbol).unwrap_or_else(|| grep::leaf(symbol));
        answer.notes.insert(
            0,
            format!(
                "no line of this checkout reads as a declaration of `{subject}`; the affected files hold textual uses of `{}`",
                grep::leaf(symbol)
            ),
        );
    }
    answer.items.extend(outcome.items);
    if affected == 0 {
        answer.note("no dependent code was found");
    }
    answer.note(format!(
        "{affected} dependent symbol(s) in {} file(s); static analysis, not runtime reachability",
        outcome.files.len()
    ));
    answer.next(format!("kb callers {}", shell_word(symbol)));
    if let Some(path) = outcome.files.first() {
        answer.next(format!("kb doc {}", shell_word(path)));
    }
    Ok(agent::output(answer, args.answer.lines, None))
}

#[cfg(test)]
mod tests {
    #[test]
    fn shell_words_quote_only_when_needed() {
        assert_eq!(super::shell_word("Type::name"), "Type::name");
        assert_eq!(super::shell_word("a b"), "'a b'");
        assert_eq!(super::shell_word("it's"), "'it'\\''s'");
    }
}
