//! `kb show`: one full typed record, one named section, or the authoritative file bytes.
//!
//! Records of every status stay addressable (historical ids never disappear); superseded
//! and deprecated records are shown with their status and successors. Text is emitted
//! verbatim, without rewriting or truncation.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::error::{ErrorCode, KbError, Result};
use crate::knowledge::{KnowledgeView, Origin, ProposalChange};
use crate::model::ids::check_record_id;
use crate::model::{Kind, ParsedRecord, Section, Status};
use crate::output::Format;

use super::present::{record_full, safe_text};

/// A record that supersedes the shown one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Successor {
    pub id: String,
    pub title: String,
    pub status: Status,
}

/// What the checked-out code says about an anchor's target. The handler fills it in for
/// anchors of the repository it runs in; the knowledge itself is unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorTarget {
    /// The anchored file is not in this checkout.
    Missing,
    /// First line of the file that declares, else names, the anchored symbol.
    Line(u32),
    /// An anchored document: its length, and the first line of each section the anchor
    /// names and of each code name of the record that the document mentions.
    Document {
        lines: u32,
        mentions: Vec<(String, u32)>,
    },
}

/// Places named for one anchored document.
pub const MAX_MENTIONS: usize = 5;

/// The code names a record's text puts in backticks, in order of first appearance:
/// `openSession<T>()` gives `openSession`. Only names written like code count (a case
/// change or an underscore inside, at least five characters), so that plain words in
/// backticks do not send a reader to an arbitrary line.
pub fn code_names(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for span in text.split('`').skip(1).step_by(2) {
        for word in span.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
            let chars: Vec<char> = word.chars().collect();
            let code_like = chars.len() >= 5
                && !chars[0].is_numeric()
                && chars.windows(2).any(|w| {
                    (w[0].is_lowercase() && w[1].is_uppercase())
                        || (w[0] == '_' && w[1].is_alphanumeric())
                });
            if code_like && !names.iter().any(|n| n == word) {
                names.push(word.to_string());
            }
        }
    }
    names
}

/// The sections an anchor note names: `§ Usage, § Pitfalls` gives `Usage` and `Pitfalls`.
fn sections(note: &str) -> Vec<&str> {
    note.split('§')
        .skip(1)
        .filter_map(|part| {
            // A name ends at the punctuation after it; an `and` that only joins it to the
            // next section is not part of it, an `and` inside the name is.
            let name = part.split([';', ',', '(']).next()?.trim();
            let name = name.strip_suffix(" and").unwrap_or(name).trim_end();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

/// Where a document says what a record was written from: the headings its anchor note
/// names (`§ Heading`), then the first whole-word mention of each code name. Lexical
/// locators for `kb read`, at most [`MAX_MENTIONS`].
pub fn document_mentions(
    contents: &str,
    note: Option<&str>,
    names: &[String],
) -> Vec<(String, u32)> {
    let mut mentions: Vec<(String, u32)> = Vec::new();
    // Markdown headings with their text in lower case: one to six `#` and a space, at most
    // three spaces of indentation, outside fenced code. A fence closes with at least as
    // many of the character that opened it.
    let mut fence: Option<(char, usize)> = None;
    let headings: Vec<(usize, String)> = contents
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let indent = line.len() - line.trim_start_matches(' ').len();
            let text = line.trim_start_matches(' ');
            let marker = text.chars().next().filter(|c| matches!(c, '`' | '~'));
            let run = marker.map_or(0, |m| text.chars().take_while(|c| *c == m).count());
            match (fence, marker) {
                (Some((open, least)), Some(m)) if m == open && run >= least && indent <= 3 => {
                    fence = None;
                    return None;
                }
                (None, Some(m)) if run >= 3 && indent <= 3 => {
                    fence = Some((m, run));
                    return None;
                }
                _ => {}
            }
            let level = text.chars().take_while(|c| *c == '#').count();
            let heading = &text[level..];
            (fence.is_none()
                && indent <= 3
                && (1..=6).contains(&level)
                && heading.starts_with([' ', '\t']))
            .then(|| {
                (
                    index,
                    heading.trim().trim_end_matches('#').trim().to_lowercase(),
                )
            })
        })
        .collect();
    for section in note.map(sections).unwrap_or_default() {
        let wanted = section.to_lowercase();
        // The heading that is the section's name, else the first that begins with it as
        // a whole word.
        let line = headings
            .iter()
            .find(|(_, heading)| *heading == wanted)
            .or_else(|| {
                headings.iter().find(|(_, heading)| {
                    heading.strip_prefix(&wanted).is_some_and(|rest| {
                        !rest
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_alphanumeric() || c == '_')
                    })
                })
            });
        if let Some((index, _)) = line {
            mentions.push((format!("§ {section}"), *index as u32 + 1));
        }
    }
    for name in names {
        if mentions.len() >= MAX_MENTIONS {
            break;
        }
        let line = contents.lines().position(|line| {
            line.match_indices(name.as_str()).any(|(at, _)| {
                let word = |c: char| c.is_alphanumeric() || c == '_';
                !line[..at].chars().next_back().is_some_and(word)
                    && !line[at + name.len()..].chars().next().is_some_and(word)
            })
        });
        if let Some(index) = line {
            mentions.push((name.clone(), index as u32 + 1));
        }
    }
    mentions.truncate(MAX_MENTIONS);
    mentions
}

/// What `show` found.
#[derive(Debug, Clone)]
pub struct ShowResult {
    pub id: String,
    pub origin: Origin,
    /// KB-root-relative source path.
    pub path: String,
    pub record: Arc<ParsedRecord>,
    /// The requested section (section lookups only).
    pub section: Option<Section>,
    /// Authoritative file text (`--raw` only).
    pub raw: Option<Arc<str>>,
    /// Records whose `supersedes` names this id.
    pub successors: Vec<Successor>,
    /// Local proposals changing this accepted record: (path, `modifies`|`removes`).
    pub proposals: Vec<(String, String)>,
    /// One entry per anchor of the record, or empty when no checkout was consulted.
    pub targets: Vec<Option<AnchorTarget>>,
}

/// All deferred Markdown, preserving section text and explicitly naming status.
pub fn render_sections(result: &ShowResult) -> String {
    let mut text = format!(
        "{} [{}] — deferred sections\n",
        result.id,
        result.record.record.status().as_str()
    );
    for section in &result.record.sections {
        text.push_str(&format!("\n## {}\n{}\n", section.heading, section.markdown));
    }
    safe_text(text)
}

/// Look up `target` (`id` or `id#section`). Errors: `INVALID_INPUT` for malformed ids or
/// conflicting options, `NOT_FOUND` for unknown records or sections.
pub fn show(
    view: &dyn KnowledgeView,
    target: &str,
    section: Option<&str>,
    raw: bool,
    include_proposals: bool,
) -> Result<ShowResult> {
    let (id, from_target) = match target.split_once('#') {
        Some((id, s)) => (id, Some(s)),
        None => (target, None),
    };
    let section = match (from_target, section) {
        (Some(a), Some(b)) if a != b => {
            return Err(KbError::invalid_input(format!(
                "section `{a}` in the id conflicts with --section `{b}`"
            )));
        }
        (a, b) => a.or(b),
    };
    check_record_id(id).map_err(|e| KbError::invalid_input(format!("`{id}`: {e}")))?;
    if raw && section.is_some() {
        return Err(KbError::invalid_input(
            "--raw prints the whole authoritative file; drop the section",
        ));
    }
    let ids = [id.to_string()];
    let mut origin = Origin::Accepted;
    let mut entry = view.records(&ids, Origin::Accepted)?.into_iter().next();
    if entry.is_none() && include_proposals {
        origin = Origin::Proposal;
        entry = view.records(&ids, Origin::Proposal)?.into_iter().next();
    }
    let Some(entry) = entry else {
        return Err(KbError::new(
            ErrorCode::NotFound,
            format!("record `{id}` does not exist in the selected snapshot"),
        )
        .with_details(json!({"id": id}))
        .with_hint("use `kb search <terms>` to find ids; local drafts need --include-proposals"));
    };
    let parsed = entry.parsed.clone();

    let section = match section {
        Some(s) => match parsed.section(s) {
            Some(sec) => Some(sec.clone()),
            None => {
                let available: Vec<&str> = parsed.sections.iter().map(|x| x.id.as_str()).collect();
                return Err(KbError::new(
                    ErrorCode::NotFound,
                    format!("record `{id}` has no section `{s}`"),
                )
                .with_details(json!({"id": id, "section": s, "available": available})));
            }
        },
        None => None,
    };
    let raw = if raw {
        match view.raw(id, origin)? {
            Some(r) => Some(r.text),
            None => {
                return Err(KbError::new(
                    ErrorCode::NotFound,
                    format!("raw bytes of `{id}` are not available in this view"),
                ));
            }
        }
    } else {
        None
    };

    let status = parsed.record.status();
    let mut successors = Vec::new();
    if matches!(status, Status::Superseded | Status::Deprecated) {
        for e in view.metas_by_kind(&Kind::ALL, Origin::Accepted)? {
            if e.meta.links.supersedes.iter().any(|s| s == id) {
                successors.push(Successor {
                    id: e.meta.id.clone(),
                    title: e.meta.title.clone(),
                    status: e.meta.status,
                });
            }
        }
        successors.sort_by(|a, b| a.id.cmp(&b.id));
    }
    let mut proposals = Vec::new();
    if include_proposals && origin == Origin::Accepted {
        for p in view.proposals()? {
            match &p.change {
                ProposalChange::Modifies { id: pid } if pid == id => {
                    proposals.push((p.path.clone(), "modifies".to_string()))
                }
                ProposalChange::Removes { id: pid } if pid == id => {
                    proposals.push((p.path.clone(), "removes".to_string()))
                }
                _ => {}
            }
        }
        proposals.sort();
    }
    Ok(ShowResult {
        id: id.to_string(),
        origin,
        path: entry.path.clone(),
        record: parsed,
        section,
        raw,
        successors,
        proposals,
        targets: Vec::new(),
    })
}

fn status_note(r: &ShowResult) -> Option<String> {
    let status = r.record.record.status();
    let successors: Vec<&str> = r.successors.iter().map(|s| s.id.as_str()).collect();
    let succ = if successors.is_empty() {
        String::new()
    } else {
        format!("; superseded by {}", successors.join(", "))
    };
    let mut note = match status {
        Status::Accepted => None,
        Status::Draft => Some("draft: not accepted knowledge".to_string()),
        Status::Deprecated => Some(format!("deprecated: kept for history{succ}")),
        Status::Superseded => Some(format!(
            "superseded: kept addressable for history, not current knowledge{succ}"
        )),
    };
    if r.origin == Origin::Proposal {
        let p = "local proposal: not reviewed, not accepted";
        note = Some(match note {
            Some(n) => format!("{p}; {n}"),
            None => p.to_string(),
        });
    }
    for (path, change) in &r.proposals {
        let line = format!("a local proposal {change} this record ({path}); it is not reviewed");
        note = Some(match note {
            Some(n) => format!("{n}; {line}"),
            None => line,
        });
    }
    note
}

/// Render compact or human text (JSON: pretty-printed [`to_json`]). `--raw` output is the
/// file text verbatim.
pub fn render(r: &ShowResult, format: Format) -> String {
    if format == Format::Json {
        let mut s = serde_json::to_string_pretty(&to_json(r)).unwrap_or_default();
        s.push('\n');
        return s;
    }
    if let Some(raw) = &r.raw {
        return raw.to_string();
    }
    let mut o = String::new();
    if let Some(n) = status_note(r) {
        o.push_str(&format!("note: {n}\n"));
    }
    match &r.section {
        Some(s) => {
            let heading = if s.heading.is_empty() {
                "(intro)"
            } else {
                s.heading.as_str()
            };
            o.push_str(&format!("# {}#{}: {heading}\n", r.id, s.id));
            o.push_str(&format!("source: {}\n", r.path));
            o.push_str(&s.markdown);
            o.push('\n');
        }
        None => o.push_str(&record_full(
            &r.record,
            &r.path,
            r.origin,
            format == Format::Human,
            &r.targets,
        )),
    }
    safe_text(o)
}

/// Deterministic JSON result.
pub fn to_json(r: &ShowResult) -> Value {
    let c = r.record.record.common();
    let mut v = json!({
        "id": r.id,
        "kind": c.kind,
        "status": c.status,
        "title": c.title,
        "origin": r.origin,
        "path": r.path,
        "note": status_note(r),
        "successors": r.successors.iter().map(|s| json!({
            "id": s.id, "title": s.title, "status": s.status,
        })).collect::<Vec<_>>(),
        "proposals": r.proposals.iter().map(|(p, c)| json!({"path": p, "change": c})).collect::<Vec<_>>(),
    });
    if let Some(raw) = &r.raw {
        v["raw"] = json!(raw.as_ref());
    } else if let Some(s) = &r.section {
        v["section"] = json!({"id": s.id, "heading": s.heading, "markdown": s.markdown});
    } else {
        v["record"] = serde_json::to_value(&r.record.record).unwrap_or(Value::Null);
        if r.targets.iter().any(Option::is_some) {
            v["anchor_targets"] = json!(
                r.targets
                    .iter()
                    .map(|t| match t {
                        Some(AnchorTarget::Missing) => json!({"state": "missing"}),
                        Some(AnchorTarget::Line(n)) => json!({"state": "found", "line": n}),
                        Some(AnchorTarget::Document { lines, mentions }) => json!({
                            "state": "found",
                            "lines": lines,
                            "mentions": mentions
                                .iter()
                                .map(|(text, line)| json!({"text": text, "line": line}))
                                .collect::<Vec<_>>(),
                        }),
                        None => Value::Null,
                    })
                    .collect::<Vec<_>>()
            );
        }
        v["sections"] = json!(
            r.record
                .sections
                .iter()
                .map(|s| json!({"id": s.id, "heading": s.heading, "markdown": s.markdown}))
                .collect::<Vec<_>>()
        );
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_names_are_the_backticked_names_written_like_code() {
        let text = "Create it with `openSession<SomeView>()`, never `rawSocketOf` or `plain words`;                     see `MAX_BODY_BYTES` and `openSession` again.\n```kotlin\nval s = EncryptedStore(k)\n```";
        assert_eq!(
            code_names(text),
            [
                "openSession",
                "SomeView",
                "rawSocketOf",
                "MAX_BODY_BYTES",
                "EncryptedStore"
            ]
        );
        assert!(code_names("no code here").is_empty());
    }

    #[test]
    fn a_document_is_located_by_named_sections_then_by_code_names() {
        let doc = "# Guide\n\nIntro mentions openSessions only.\n\n## Usage\n\nUse openSession here.\n\n### Pitfalls here\n\nNo rawSocketOf.\n";
        let names = vec![
            "openSession".to_string(),
            "absentName".to_string(),
            "rawSocketOf".to_string(),
        ];
        assert_eq!(
            document_mentions(
                doc,
                Some("Written from § Usage, § Pitfalls and § Background; see also"),
                &names
            ),
            [
                ("§ Usage".to_string(), 5),
                ("§ Pitfalls".to_string(), 9),
                ("openSession".to_string(), 7),
                ("rawSocketOf".to_string(), 11),
            ]
        );
        // A section is a heading that carries its name as a whole word, outside fenced
        // code: `§ Use` is not `## Usage`, and a comment in a code block is no heading.
        let fenced = "```sh\n# Setup step\n```\n\n## Setup\n\n## Usage\n";
        assert_eq!(
            document_mentions(fenced, Some("§ Setup, § Use"), &[]),
            [("§ Setup".to_string(), 5)]
        );
        // The heading that is the name wins over an earlier one that begins with it; an
        // `and` inside a name belongs to it; a longer fence is not closed by a shorter
        // one, and `#include` or an indented code line is no heading.
        let doc = "## Errors in detail\n\n## Errors\n\n## Errors and recovery\n\n````md\n```\n# Limits\n````\n\n    # Limits\n#include <limits>\n## Limits\n";
        assert_eq!(
            document_mentions(
                doc,
                Some("§ Errors and § Errors and recovery; § Limits"),
                &[]
            ),
            [
                ("§ Errors".to_string(), 3),
                ("§ Errors and recovery".to_string(), 5),
                ("§ Limits".to_string(), 14),
            ]
        );
        let many: Vec<String> = (0..9).map(|n| format!("name{n}Value")).collect();
        let text: String = many.iter().map(|n| format!("{n}\n")).collect();
        assert_eq!(document_mentions(&text, None, &many).len(), MAX_MENTIONS);
    }
}
