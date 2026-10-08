//! Pure calendar checks. The adapter supplies an explicit date or a pinned commit date.
use serde::Serialize;

use crate::diag::Diagnostic;
use crate::model::{Record, Status, date_days};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferenceDate {
    pub date: String,
    pub source: String,
    pub commit: Option<String>,
}

pub fn has_dates(record: &Record) -> bool {
    let c = record.common();
    c.verified_at.is_some() || c.review_by.is_some()
}

/// A warning does not revoke an accepted rule; it asks for a review of its evidence.
pub fn warnings(
    record: &Record,
    on: Option<&ReferenceDate>,
    max_age_days: Option<u32>,
) -> Vec<Diagnostic> {
    if record.status() != Status::Accepted || (!has_dates(record) && max_age_days.is_none()) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut warn = |code: &str, message: String| {
        out.push(Diagnostic::warning(code, message).for_record(record.id()))
    };
    let Some(on) = on.and_then(|p| date_days(&p.date).map(|day| (p, day))) else {
        warn(
            "FRESHNESS_DATE_UNKNOWN",
            "no committed reference date is available; pass --on YYYY-MM-DD to evaluate freshness"
                .into(),
        );
        return out;
    };
    let c = record.common();
    if let Some(review_by) = c.review_by.and_then(date_days)
        && review_by < on.1
    {
        warn(
            "REVIEW_OVERDUE",
            format!(
                "review_by {} is before reference date {} ({})",
                c.review_by.unwrap(),
                on.0.date,
                on.0.source
            ),
        );
    }
    match c.verified_at.and_then(date_days) {
        Some(verified) if verified > on.1 => warn(
            "VERIFIED_AT_FUTURE",
            format!(
                "verified_at {} is after reference date {}",
                c.verified_at.unwrap(),
                on.0.date
            ),
        ),
        Some(verified) if max_age_days.is_some_and(|max| on.1 - verified > i64::from(max)) => {
            warn(
                "KNOWLEDGE_STALE",
                format!(
                    "verification is {} days old at {}; allowed age is {} days",
                    on.1 - verified,
                    on.0.date,
                    max_age_days.unwrap()
                ),
            );
        }
        None if max_age_days.is_some() => warn(
            "VERIFIED_AT_MISSING",
            "accepted record has no verified_at evidence for the requested age check".into(),
        ),
        _ => {}
    }
    out
}
