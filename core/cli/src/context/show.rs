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
