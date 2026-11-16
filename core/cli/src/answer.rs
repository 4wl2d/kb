//! The common answer of the agent commands (`sym`, `callers`, `impact <symbol>`, `doc`,
//! `find`, `check`, `tools`): one structured, bounded result that names the source of every
//! item. Pure: rendering and bounding only.
//!
//! Text output is at most `lines` lines (default 30) of at most [`MAX_COLUMNS`] characters:
//! a header naming the sources, the items that fit, up to [`MAX_NOTES`] notes, an optional
//! `next:` line and a footer with the shown/total counts. JSON (`kb.answer.v1` inside the
//! `kb.cli.v1` envelope) carries the same shown items, the total and the sources.

use serde::Serialize;
use serde_json::{Value, json};

pub const ANSWER_ID: &str = "kb.answer.v1";
pub const DEFAULT_LINES: usize = 30;
pub const MIN_LINES: usize = 5;
pub const MAX_LINES: usize = 200;
pub const MAX_COLUMNS: usize = 200;
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
    /// Items found before bounding.
    pub total: usize,
    pub truncated: bool,
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
            total: 0,
            truncated: false,
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
        json!({
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
        })
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
        out.push(format!(
            "kb {}{subject} · {}",
            self.command,
            if sources.is_empty() {
                "no source".to_string()
            } else {
                sources.join("; ")
            }
        ));
        let tag_sources = self.sources.len() > 1;
        for item in &self.items {
            out.push(render_item(item, tag_sources));
        }
        for n in &self.notes {
            out.push(format!("note: {n}"));
        }
        if !self.next.is_empty() {
            out.push(format!("next: {}", self.next.join(" · ")));
        }
        let mut footer = format!("-- {} of {}", self.items.len(), self.total);
        if self.truncated {
            footer.push_str(" (truncated; --lines widens the bound)");
        }
        out.push(footer);
        let mut text = out
            .into_iter()
            .map(|l| clip(&l, MAX_COLUMNS))
            .collect::<Vec<_>>()
            .join("\n");
        text.push('\n');
        text
    }
}

fn render_item(item: &Item, tag_source: bool) -> String {
    let mut parts = vec![item.kind.clone()];
    if let Some(n) = &item.name {
        parts.push(n.clone());
    }
    if let Some(p) = &item.path {
        let loc = match (item.line, item.end_line) {
            (Some(l), Some(e)) => format!("{p}:{l}-{e}"),
            (Some(l), None) => format!("{p}:{l}"),
            _ => p.clone(),
        };
        parts.push(loc);
    }
    if let Some(d) = &item.detail {
        parts.push(single_line(d));
    }
    if let Some(c) = &item.confidence {
        parts.push(format!("[{c}]"));
    }
    if tag_source {
        parts.push(format!("<{}>", item.source));
    }
    parts.join("  ")
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
    fn clip_is_char_safe() {
        assert_eq!(clip("абвгд", 3), "аб…");
        assert_eq!(clip("abc", 3), "abc");
    }
}
