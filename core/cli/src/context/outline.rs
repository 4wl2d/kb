//! Bounded inventory, never a delivery receipt or a substitute for mandatory content.
use serde_json::{Value, json};

use super::{ContextResult, UnitBody, estimate_tokens, measure, present};
use crate::error::{ErrorCode, KbError, Result};
use crate::model::BudgetUnit;
use crate::output::Format;

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
