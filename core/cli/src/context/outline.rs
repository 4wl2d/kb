//! Bounded inventory, never a delivery receipt or a substitute for mandatory content.
use serde_json::{Value, json};

use super::{ContextResult, UnitBody, estimate_tokens, measure, present};
use crate::error::{ErrorCode, KbError, Result};
use crate::model::BudgetUnit;
use crate::output::Format;

/// Terse inventory: mandatory units share a handful of reasons, so each reason and kind is
/// written once with its ids; optional units keep their own ranking reason. Every id of the
/// table form is present, which keeps several outlines in one session affordable.
fn grouped(rows: &[Value]) -> String {
    let text = |row: &Value, key: &str| row[key].as_str().unwrap_or_default().to_string();
    let (mandatory, optional): (Vec<&Value>, Vec<&Value>) =
        rows.iter().partition(|row| row["mandatory"] == true);
    let mut out = String::new();
    if !mandatory.is_empty() {
        let tokens: u64 = mandatory.iter().filter_map(|r| r["tokens"].as_u64()).sum();
        out.push_str(&format!(
            "mandatory: {} unit(s), {tokens} tokens-est; reason | kind | ids\n",
            mandatory.len()
        ));
        let mut groups: Vec<((String, String), Vec<String>)> = Vec::new();
        for row in mandatory {
            let key = (text(row, "reason"), text(row, "kind"));
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, ids)) => ids.push(text(row, "id")),
                None => groups.push((key, vec![text(row, "id")])),
            }
        }
        for ((reason, kind), ids) in groups {
            out.push_str(&format!("{reason} | {kind} | {}\n", ids.join(", ")));
        }
    }
    if !optional.is_empty() {
        out.push_str(&format!(
            "optional: {} unit(s); id | kind | tokens-est | reason\n",
            optional.len()
        ));
        for row in optional {
            out.push_str(&format!(
                "{} | {} | {} | {}\n",
                text(row, "id"),
                text(row, "kind"),
                row["tokens"],
                text(row, "reason")
            ));
        }
    }
    out
}

pub fn render(
    result: &ContextResult,
    format: Format,
    limit: u64,
    unit: BudgetUnit,
) -> Result<(Value, String)> {
    let mut rows: Vec<Value> = result.units.iter().map(|u| {
        let mut full = u.clone(); full.reuse = None;
        json!({"id":u.id,"kind":if matches!(u.body,UnitBody::Code(_)){"code"}else{u.kind.as_str()},
            "mandatory":u.tier.is_required(),"tokens":estimate_tokens(&present::unit(&full,Format::Terse)),"reason":u.why})
    }).collect();
    let total = rows.len();
    let mut used = 0;
    loop {
        let value = json!({"protocol":"kb.outline.v1","completeness":result.header.completeness.as_str(),
            "reasons":result.header.reasons.iter().map(|r| &r.code).collect::<Vec<_>>(),
            "snapshot_content_digest":result.header.snapshot.content_digest,
            "units":rows,"omitted_optional":total-rows.len(),
            "budget":{"used":used,"limit":limit,"unit":unit.as_str(),"hard_max_tokens_est":2000},
            "note":"Inventory only. Request context before editing; this output does not prove delivery of the listed obligations."});
        let mut text = format!("kb outline: {}\n", result.header.completeness.as_str());
        if format == Format::Terse {
            text.push_str(&grouped(&rows));
        } else {
            for row in &rows {
                text.push_str(&format!(
                    "{} | {} | {} | {} tokens-est | {}\n",
                    row["id"].as_str().unwrap_or_default(),
                    row["kind"].as_str().unwrap_or_default(),
                    if row["mandatory"] == true {
                        "mandatory"
                    } else {
                        "optional"
                    },
                    row["tokens"],
                    row["reason"].as_str().unwrap_or_default()
                ));
            }
        }
        text.push_str(&format!("omitted optional: {}; budget {used}/{limit} {} (hard max 2000 tokens-est)\nInventory only; run context before editing.\n", total-rows.len(), unit.as_str()));
        let text = present::safe_text(text);
        let measured = if format == Format::Json {
            serde_json::to_string_pretty(&value)?.replace('\n', "\n  ")
        } else {
            text.clone()
        };
        let size = measure(&measured, unit);
        if size <= limit && estimate_tokens(&measured) <= 2000 {
            if size == used {
                return Ok((value, text));
            }
            used = size;
            continue;
        }
        if let Some(index) = rows.iter().rposition(|r| r["mandatory"] == false) {
            rows.remove(index);
            used = 0;
        } else {
            return Err(KbError::new(
                ErrorCode::ContextBudgetExceeded,
                "mandatory outline inventory exceeds the requested budget or 2000-token ceiling",
            )
            .with_hint(
                "narrow --path, --module or --repo; mandatory ids are never silently dropped",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_sharing_a_reason_and_kind_take_one_line() {
        let row = |id: &str, kind: &str, mandatory: bool, reason: &str| json!({"id": id, "kind": kind, "mandatory": mandatory, "tokens": 20, "reason": reason});
        let rows = [
            row("a.one", "policy", true, "applies (modules)"),
            row("a.two", "policy", true, "applies (repos)"),
            row("a.three", "policy", true, "applies (modules)"),
            row("a.four", "invariant", true, "applies (modules)"),
            row("a.five", "reference", false, "score 5: related to a.one"),
        ];
        assert_eq!(
            grouped(&rows),
            "mandatory: 4 unit(s), 80 tokens-est; reason | kind | ids\n\
             applies (modules) | policy | a.one, a.three\n\
             applies (repos) | policy | a.two\n\
             applies (modules) | invariant | a.four\n\
             optional: 1 unit(s); id | kind | tokens-est | reason\n\
             a.five | reference | 20 | score 5: related to a.one\n"
        );
        assert_eq!(grouped(&[]), "");
    }
}
