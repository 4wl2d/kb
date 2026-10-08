use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use super::drift::{self, State};
use crate::freshness::ReferenceDate;
use crate::knowledge::RecordEntry;
use crate::model::Status;
use crate::provenance::HostRoots;
use crate::util::FieldHasher;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Support {
    Supported,
    Stale,
    Unverifiable,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatementEvidence {
    pub id: String,
    pub record: String,
    pub record_status: Status,
    pub owner: String,
    pub support: Support,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditSample {
    pub id: String,
    pub owner: String,
    pub path: String,
    pub anchor_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Ledger {
    pub at: String,
    pub reference_date: Option<ReferenceDate>,
    pub statements: Vec<StatementEvidence>,
    pub accepted_counts: BTreeMap<&'static str, usize>,
    pub draft_audit: Vec<AuditSample>,
    pub audit_seed: String,
    pub audit_questions: Vec<&'static str>,
    pub note: &'static str,
}

pub struct Options<'a> {
    pub at: &'a str,
    pub on: Option<ReferenceDate>,
    pub max_age: Option<u32>,
    pub sample: usize,
    pub seed: &'a str,
}

pub fn report(
    kb_root: &Path,
    roots: &HostRoots,
    records: &[RecordEntry],
    options: &Options<'_>,
) -> Ledger {
    let mut statements = Vec::new();
    let mut accepted_counts = BTreeMap::from([("supported", 0), ("stale", 0), ("unverifiable", 0)]);
    let mut drafts = Vec::new();
    for entry in records {
        let record = &entry.parsed.record;
        let drift = drift::record(kb_root, roots, record, "verified", options.at);
        let freshness = crate::freshness::warnings(record, options.on.as_ref(), options.max_age);
        let future = freshness.iter().any(|d| {
            matches!(
                d.code.as_str(),
                "VERIFIED_AT_FUTURE" | "VERIFIED_AT_MISSING" | "FRESHNESS_DATE_UNKNOWN"
            )
        });
        let mut support = match drift.state {
            State::Unchanged => Support::Supported,
            State::Changed => Support::Stale,
            State::Unverifiable => Support::Unverifiable,
        };
        // Proven drift stays stale; an unknown calendar date cannot weaken it.
        if !freshness.is_empty() && support != Support::Stale {
            support = if future {
                Support::Unverifiable
            } else {
                Support::Stale
            };
        }
        if record.status() != Status::Accepted {
            support = Support::Unverifiable;
        }
        let mut reasons: Vec<_> = freshness
            .iter()
            .map(|d| format!("{}: {}", d.code, d.message))
            .collect();
        reasons.extend(
            drift
                .anchors
                .iter()
                .filter(|a| a.state != State::Unchanged)
                .map(|a| format!("anchor {}: {}", a.index, a.detail)),
        );
        if drift.anchors.is_empty() {
            reasons.push("no provenance anchors".into());
        }
        if record.status() != Status::Accepted {
            reasons.push("record is not accepted on the selected source".into());
        }
        reasons.sort();
        reasons.dedup();
        for statement in record.normative() {
            if record.status() == Status::Accepted {
                let key = match support {
                    Support::Supported => "supported",
                    Support::Stale => "stale",
                    Support::Unverifiable => "unverifiable",
                };
                *accepted_counts.entry(key).or_default() += 1;
            }
            statements.push(StatementEvidence {
                id: format!("{}#{}", record.id(), statement.id),
                record: record.id().into(),
                record_status: record.status(),
                owner: record.common().owner.into(),
                support,
                reasons: reasons.clone(),
            });
        }
        if record.status() == Status::Draft {
            let mut rank = FieldHasher::new();
            rank.field("kb-ledger-audit/1")
                .field(options.seed)
                .field(record.id());
            drafts.push((
                rank.finish_hex(),
                AuditSample {
                    id: record.id().into(),
                    owner: record.common().owner.into(),
                    path: entry.path.clone(),
                    anchor_count: record.common().anchors.len(),
                },
            ));
        }
    }
    statements.sort_by(|a, b| a.id.cmp(&b.id));
    drafts.sort_by(|a, b| (&a.0, &a.1.id).cmp(&(&b.0, &b.1.id)));
    drafts.truncate(options.sample);
    Ledger {
        at: options.at.into(),
        reference_date: options.on.clone(),
        statements,
        accepted_counts,
        draft_audit: drafts.into_iter().map(|(_, r)| r).collect(),
        audit_seed: options.seed.into(),
        audit_questions: vec![
            "Does the anchor support this exact statement and its exceptions?",
            "For confirmed behavior, is there a merged fix with a regression test or an explicit review decision?",
            "Is a reviewer named in the change, and is reported test execution independently evidenced?",
            "Should this remain descriptive orientation or a gap instead of a normative rule?",
        ],
        note: "Supported means declared Git evidence is available and unchanged under these checks. It does not prove semantic truth, test execution, human review or acceptance outside the selected approved ref. Draft samples require a human audit; no status is changed.",
    }
}
