//! The common answer of the agent commands (`sym`, `callers`, `impact <symbol>`, `doc`,
//! `find`, `check`, `tools`): one structured, bounded result that names the source of every
//! item. Pure: rendering and bounding only.
//!
//! Text output is at most `lines` lines (default 30) of at most [`MAX_COLUMNS`] characters:
//! a header naming the sources, the items that fit, up to [`MAX_NOTES`] notes, an optional
//! `next:` line and a footer with the shown/total counts. JSON (`kb.answer.v1` inside the
//! `kb.cli.v1` envelope) carries the same shown items, the total and the sources.
//!
//! Two things may exceed those bounds, because cutting them costs the agent another call: an
//! item's code text keeps [`DETAIL_ROOM`] characters however long its path is, and an
//! answer's [`Excerpt`]s add up to [`EXCERPT_LINES`] numbered source lines after the items.

use serde::Serialize;
use serde_json::{Value, json};

pub const ANSWER_ID: &str = "kb.answer.v1";
pub const DEFAULT_LINES: usize = 30;
pub const MIN_LINES: usize = 5;
pub const MAX_LINES: usize = 200;
pub const MAX_COLUMNS: usize = 200;
/// Characters of an item's code text that a long path never squeezes out.
pub const DETAIL_ROOM: usize = 120;
/// Characters of an item's name shown next to its code text.
pub const NAME_ROOM: usize = 80;
/// Source lines an answer may carry in its excerpts: one `kb read` window.
pub const EXCERPT_LINES: usize = 80;
const EXCERPT_COLUMNS: usize = 300;
pub const MAX_NOTES: usize = 3;
pub const MAX_NEXT: usize = 3;

/// Who produced (part of) an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Source {
    /// Registry tool id, or `knowledge` for the KB itself.
    pub id: String,
    pub adapter: String,
    /// Tool name and version as the tool reports them, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Freshness and completeness of what the source read, e.g. `index synced, complete`,
    /// `lexical`, or the knowledge snapshot.
    pub state: String,
}

/// Numbered source lines an answer carries so that the call that located them need not be
/// followed by a read: the definition `sym` found, the surroundings of a few `find`
/// matches, or the call sites `callers` found. Lines are as `kb read` prints them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Excerpt {
    pub path: String,
    pub first_line: u32,
    pub lines: Vec<String>,
    /// The line after the excerpt when the definition or file goes on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_line: Option<u32>,
}

/// One fact of an answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Item {
    /// What the fact is: `definition`, `caller`, `affected`, `match`, `feature`, `purpose`,
    /// `invariant`, `contract`, `entry`, `limit`, `gap`, `check`, `output`, `tool`, `route`.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Path relative to the code root (`repo:path` for knowledge scopes of another repo).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// `resolved` (static resolution), `possible` (name-based) or `lexical` (text match).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    /// Id of the [`Source`] that produced the item.
    pub source: String,
}

impl Item {
    pub fn new(kind: &str, source: &str) -> Item {
        Item {
            kind: kind.into(),
            source: source.into(),
            ..Item::default()
        }
    }
    pub fn name(mut self, name: impl Into<String>) -> Item {
        self.name = Some(name.into());
        self
    }
    pub fn at(mut self, path: impl Into<String>, line: Option<u32>) -> Item {
        self.path = Some(path.into());
        self.line = line;
        self
    }
    pub fn span(mut self, end_line: Option<u32>) -> Item {
        self.end_line = end_line.filter(|e| Some(*e) != self.line);
        self
    }
    pub fn detail(mut self, detail: impl Into<String>) -> Item {
        let d = detail.into();
        self.detail = (!d.trim().is_empty()).then_some(d);
        self
    }
    pub fn confidence(mut self, c: &str) -> Item {
        self.confidence = Some(c.into());
        self
    }
}

/// A bounded answer. Build it with every item found, then call [`Answer::bound`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Answer {
    pub command: String,
    pub subject: String,
    pub sources: Vec<Source>,
    pub items: Vec<Item>,
    pub notes: Vec<String>,
    pub next: Vec<String>,
    /// Source lines of the located code, when the command attaches them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub excerpts: Vec<Excerpt>,
    /// Items found before bounding.
    pub total: usize,
    pub truncated: bool,
    #[serde(skip)]
    pub(crate) prioritize_code_matches: bool,
}

impl Answer {
    pub fn new(command: &str, subject: impl Into<String>) -> Answer {
        Answer {
            command: command.into(),
            subject: subject.into(),
            sources: Vec::new(),
            items: Vec::new(),
            notes: Vec::new(),
            next: Vec::new(),
            excerpts: Vec::new(),
            total: 0,
            truncated: false,
            prioritize_code_matches: false,
        }
    }

    pub fn source(&mut self, s: Source) {
        if !self.sources.iter().any(|x| x.id == s.id) {
            self.sources.push(s);
        }
    }

    pub fn note(&mut self, n: impl Into<String>) {
        let n = n.into();
        if !self.notes.contains(&n) {
            self.notes.push(n);
        }
    }

    pub fn next(&mut self, command: impl Into<String>) {
        let c = command.into();
        if !self.next.contains(&c) {
            self.next.push(c);
        }
    }

    /// Keep what fits in `lines` text lines. Notes beyond [`MAX_NOTES`] and follow-ups
    /// beyond [`MAX_NEXT`] are dropped; items are kept in their given (priority) order.
    pub fn bound(mut self, lines: usize) -> Answer {
        let lines = lines.clamp(MIN_LINES, MAX_LINES);
        self.notes.truncate(MAX_NOTES);
        self.next.truncate(MAX_NEXT);
        self.total = self.items.len();
        let fixed = 2 + self.notes.len() + usize::from(!self.next.is_empty());
        let room = lines.saturating_sub(fixed);
        self.truncated = self.items.len() > room;
        self.items.truncate(room);
        self
    }

    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "answer": ANSWER_ID,
            "command": self.command,
            "subject": self.subject,
            "sources": self.sources,
            "items": self.items,
            "shown": self.items.len(),
            "total": self.total,
            "truncated": self.truncated,
            "notes": self.notes,
            "next": self.next,
        });
        if !self.excerpts.is_empty() {
            value["excerpts"] = json!(self.excerpts);
        }
        value
    }

    pub fn render(&self) -> String {
        let mut out = Vec::new();
        let sources: Vec<String> = self
            .sources
            .iter()
            .map(|s| {
                let name = s.version.clone().unwrap_or_else(|| s.id.clone());
                format!("{name} ({})", s.state)
            })
            .collect();
        let subject = if self.subject.is_empty() {
            String::new()
        } else {
            format!(" {}", self.subject)
        };
        out.push(clip(
            &format!(
                "kb {}{subject} · {}",
                self.command,
                if sources.is_empty() {
                    "no source".to_string()
                } else {
                    sources.join("; ")
                }
            ),
            MAX_COLUMNS,
        ));
        let tag_sources = self.sources.len() > 1;
        let mut items: Vec<_> = self.items.iter().collect();
        if self.command == "find" {
            // Group the hits that survived the selection budget by file, so that a path is
            // written once, without changing which hits are selected or their priority in
            // the structured answer. Ranked hits keep their files in rank order.
            if self.prioritize_code_matches {
                let mut order: Vec<Option<&str>> = Vec::new();
                for item in &items {
                    if !order.contains(&item.path.as_deref()) {
                        order.push(item.path.as_deref());
                    }
                }
                items.sort_by_key(|item| {
                    (
                        order.iter().position(|path| *path == item.path.as_deref()),
                        item.line,
                    )
                });
            } else {
                items.sort_by_key(|item| (item.path.as_deref(), item.line));
            }
        }
        let mut previous = None;
        for item in items {
            let location = item
                .path
                .as_deref()
                .map(|path| (path, item.source.as_str()));
            let repeat_path = self.command == "find" && location.is_some() && location == previous;
            out.push(render_item(item, tag_sources, repeat_path));
            previous = location;
        }
        // An excerpt of the one file the items name belongs to them; otherwise excerpts are
        // told apart by their paths, and further excerpts of one file by the gap in their
        // line numbers.
        let headed = self.excerpts.len() > 1
            || self.excerpts.first().is_some_and(|excerpt| {
                let mut paths = self.items.iter().filter_map(|item| item.path.as_deref());
                let first = paths.next();
                first != Some(excerpt.path.as_str()) || paths.any(|path| path != excerpt.path)
            });
        let mut shown: Option<&str> = None;
        for excerpt in &self.excerpts {
            if headed && shown != Some(excerpt.path.as_str()) {
                out.push(excerpt.path.clone());
            }
            shown = Some(excerpt.path.as_str());
            for (line, text) in (excerpt.first_line..).zip(&excerpt.lines) {
                out.push(clip(&format!("{line}:{text}"), EXCERPT_COLUMNS));
            }
        }
        for n in &self.notes {
            out.push(clip(&format!("note: {n}"), MAX_COLUMNS));
        }
        if !self.next.is_empty() {
            out.push(clip(
                &format!("next: {}", self.next.join(" · ")),
                MAX_COLUMNS,
            ));
        }
        let mut footer = format!("-- {} of {}", self.items.len(), self.total);
        if self.truncated {
            footer.push_str(" (truncated; --lines widens the bound)");
        }
        out.push(footer);
        let mut text = out.join("\n");
        text.push('\n');
        text
    }
}

fn render_item(item: &Item, tag_source: bool, repeat_path: bool) -> String {
    let mut parts = vec![item.kind.clone()];
    if let Some(p) = &item.path {
        let span = match (item.line, item.end_line) {
            (Some(l), Some(e)) => format!(":{l}-{e}"),
            (Some(l), None) => format!(":{l}"),
            _ => String::new(),
        };
        let path = if repeat_path && !span.is_empty() {
            String::new()
        } else {
            // Prioritize the filename and line number over a long symbol name or ancestry.
            // Leave a column for the final ellipsis when trailing metadata is clipped.
            let room = MAX_COLUMNS.saturating_sub(item.kind.chars().count() + 3 + span.len());
            clip_path(p, room)
        };
        parts.push(format!("{path}{span}"));
    }
    if let Some(n) = &item.name {
        // A name is a locator, not content: with code text to show, it yields the room.
        parts.push(if item.detail.is_some() {
            clip(n, NAME_ROOM)
        } else {
            n.clone()
        });
    }
    let mut tags = Vec::new();
    if let Some(c) = &item.confidence {
        tags.push(format!("[{c}]"));
    }
    if tag_source {
        tags.push(format!("<{}>", item.source));
    }
    let Some(d) = &item.detail else {
        parts.extend(tags);
        return clip(&parts.join("  "), MAX_COLUMNS);
    };
    // A long path must not squeeze out the code text: a clipped match is asked for again.
    let around: usize = parts
        .iter()
        .chain(&tags)
        .map(|p| p.chars().count() + 2)
        .sum();
    let room = MAX_COLUMNS.saturating_sub(around).max(DETAIL_ROOM);
    parts.push(clip(&single_line(d), room));
    parts.extend(tags);
    parts.join("  ")
}

fn clip_path(path: &str, max: usize) -> String {
    let count = path.chars().count();
    if count <= max {
        return path.to_string();
    }
    let tail = path
        .char_indices()
        .nth(count - max.saturating_sub(1))
        .map_or("", |(index, _)| &path[index..]);
    format!("…{tail}")
}

fn single_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Clip to `max` characters, marking the cut with `…`.
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: usize) -> Answer {
        let mut a = Answer::new("find", "\"needle\"");
        a.source(Source {
            id: "rg".into(),
            adapter: "ripgrep".into(),
            version: Some("ripgrep 14.1.1".into()),
            state: "working tree".into(),
        });
        for i in 0..n {
            a.items.push(
                Item::new("match", "rg")
                    .at(format!("src/f{i:02}.rs"), Some(i as u32 + 1))
                    .detail("let needle = 1;"),
            );
        }
        a
    }

    #[test]
    fn text_never_exceeds_the_line_bound() {
        for (n, lines) in [(0, 30), (5, 30), (100, 30), (100, 5), (100, 1), (3, 500)] {
            let mut a = sample(n);
            a.note("one");
            a.next("kb doc src/f00.rs");
            let a = a.bound(lines);
            let text = a.render();
            let count = text.lines().count();
            assert!(
                count <= lines.clamp(MIN_LINES, MAX_LINES),
                "{n} {lines}: {count}"
            );
            assert_eq!(a.total, n);
            assert_eq!(a.truncated, a.items.len() < n);
            assert!(text.lines().all(|l| l.chars().count() <= MAX_COLUMNS));
        }
    }

    #[test]
    fn rendering_names_the_source() {
        let a = sample(2).bound(30);
        let text = a.render();
        assert!(text.starts_with("kb find \"needle\" · ripgrep 14.1.1 (working tree)\n"));
        assert!(text.contains("match  src/f01.rs:2  let needle = 1;"));
        assert!(text.ends_with("-- 2 of 2\n"));
        let v = a.to_json();
        assert_eq!(v["answer"], ANSWER_ID);
        assert_eq!(v["items"][0]["source"], "rg");
        assert_eq!(v["shown"], 2);
    }

    #[test]
    fn compact_code_locators_survive_long_metadata() {
        let path = format!("src/{}/ActualSource.kt", "parent/".repeat(40));
        let mut answer = Answer::new("sym", "update");
        answer.items.push(
            Item::new("definition", "graph")
                .name("namespace::".repeat(30) + "update")
                .at(path.clone(), Some(44))
                .span(Some(58)),
        );
        let bounded = answer.bound(30);
        assert!(bounded.render().contains("/ActualSource.kt:44-58"));
        assert_eq!(bounded.to_json()["items"][0]["path"], path);
    }

    #[test]
    fn find_groups_displayed_lines_without_reordering_json_results() {
        let mut answer = Answer::new("find", "needle");
        for (path, line) in [("src/a.kt", 3), ("src/b.kt", 2), ("src/a.kt", 8)] {
            answer.items.push(
                Item::new("match", "rg")
                    .at(path, Some(line))
                    .detail("needle"),
            );
        }
        let bounded = answer.clone().bound(30);
        let text = bounded.render();
        assert_eq!(text.matches("src/a.kt").count(), 1);
        assert!(text.contains("src/a.kt:3  needle\nmatch  :8  needle"));
        assert_eq!(bounded.to_json()["items"][1]["path"], "src/b.kt");

        answer.prioritize_code_matches = true;
        let ranked = answer.bound(30);
        assert_eq!(ranked.to_json(), bounded.to_json());
        let lines: Vec<_> = ranked
            .render()
            .lines()
            .skip(1)
            .take(3)
            .map(str::to_owned)
            .collect();
        // Ranked hits keep the rank order of their files; a file's lines stay together.
        assert!(lines[0].contains("src/a.kt:3"));
        assert!(lines[1].starts_with("match  :8"));
        assert!(lines[2].contains("src/b.kt:2"));
    }

    #[test]
    fn an_excerpt_names_its_file_unless_the_items_above_it_do() {
        let excerpt = |path: &str| Excerpt {
            path: path.into(),
            first_line: 3,
            lines: vec!["let needle = 1;".into()],
            next_line: None,
        };
        let item = |path: &str| {
            Item::new("match", "rg")
                .at(path, Some(3))
                .detail("let needle = 1;")
        };
        let render = |paths: &[&str], lines: usize| {
            let mut a = Answer::new("find", "needle");
            a.items = paths.iter().map(|p| item(p)).collect();
            a.excerpts = vec![excerpt("src/a.kt")];
            a.bound(lines).render()
        };
        // The one item names the file of the excerpt.
        assert!(!render(&["src/a.kt"], 30).contains("\nsrc/a.kt\n"));
        // Another file among the items, or no item shown at all: the excerpt says where
        // it is from.
        assert!(render(&["src/b.kt", "src/a.kt"], 30).contains("\nsrc/a.kt\n3:let needle = 1;"));
        let mut bare = Answer::new("find", "needle");
        bare.excerpts = vec![excerpt("src/a.kt")];
        assert!(
            bare.bound(30)
                .render()
                .contains("\nsrc/a.kt\n3:let needle = 1;")
        );
    }

    #[test]
    fn clip_is_char_safe() {
        assert_eq!(clip("абвгд", 3), "аб…");
        assert_eq!(clip("abc", 3), "abc");
    }
}
