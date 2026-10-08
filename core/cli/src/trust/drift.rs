use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::error::{KbError, Result};
use crate::git::Git;
use crate::knowledge::RecordEntry;
use crate::model::{Anchor, AnchorKind, Record, date_days};
use crate::provenance::{self, AnchorStatus, HostRoots};
use crate::util::sha256_hex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Unchanged,
    Changed,
    Unverifiable,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnchorDrift {
    pub index: usize,
    pub anchor: Anchor,
    pub from: Option<String>,
    pub at: Option<String>,
    pub state: State,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordDrift {
    pub id: String,
    pub owner: String,
    pub state: State,
    pub anchors: Vec<AnchorDrift>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriftReport {
    pub since: String,
    pub at: String,
    pub checked_records: usize,
    pub changed_records: usize,
    pub unverifiable_records: usize,
    pub by_owner: BTreeMap<String, Vec<RecordDrift>>,
    pub note: &'static str,
}

pub fn record(
    kb_root: &Path,
    roots: &HostRoots,
    record: &Record,
    since: &str,
    at: &str,
) -> RecordDrift {
    let anchors: Vec<_> = record
        .common()
        .anchors
        .iter()
        .enumerate()
        .map(|(index, anchor)| anchor_drift(kb_root, roots, record, index, anchor, since, at))
        .collect();
    let state = if anchors.iter().any(|a| a.state == State::Changed) {
        State::Changed
    } else if anchors.is_empty() || anchors.iter().any(|a| a.state == State::Unverifiable) {
        State::Unverifiable
    } else {
        State::Unchanged
    };
    RecordDrift {
        id: record.id().into(),
        owner: record.common().owner.into(),
        state,
        anchors,
    }
}

pub fn report(
    kb_root: &Path,
    roots: &HostRoots,
    records: &[RecordEntry],
    since: &str,
    at: &str,
) -> DriftReport {
    let mut changed_records = 0;
    let mut unverifiable_records = 0;
    let mut by_owner: BTreeMap<String, Vec<RecordDrift>> = BTreeMap::new();
    for entry in records {
        let drift = record(kb_root, roots, &entry.parsed.record, since, at);
        if drift.state == State::Changed {
            changed_records += 1;
        }
        if drift.state == State::Unverifiable {
            unverifiable_records += 1;
        }
        if drift.state != State::Unchanged {
            by_owner.entry(drift.owner.clone()).or_default().push(drift);
        }
    }
    for group in by_owner.values_mut() {
        group.sort_by(|a, b| a.id.cmp(&b.id));
    }
    DriftReport {
        since: since.into(),
        at: at.into(),
        checked_records: records.len(),
        changed_records,
        unverifiable_records,
        by_owner,
        note: "Review queue only. A changed anchor does not prove its statement is false; unchanged bytes do not prove semantic correctness. No source is read from the mutable work tree.",
    }
}

fn anchor_drift(
    kb_root: &Path,
    roots: &HostRoots,
    record: &Record,
    index: usize,
    anchor: &Anchor,
    since: &str,
    at: &str,
) -> AnchorDrift {
    let mut out = AnchorDrift {
        index,
        anchor: anchor.clone(),
        from: None,
        at: None,
        state: State::Unverifiable,
        detail: String::new(),
    };
    let checked = (|| -> Result<()> {
        let root = provenance::anchor_root(kb_root, roots, anchor)?;
        let git = Git::new(root);
        if anchor.kind == AnchorKind::Change && anchor.path.is_none() {
            let checked = provenance::inspect(kb_root, roots, record.id(), index, anchor, None);
            out.from = checked.resolved_commit;
            out.state = if checked.status == AnchorStatus::Verified {
                State::Unchanged
            } else {
                State::Unverifiable
            };
            out.detail = checked.detail;
            return Ok(());
        }
        let path = anchor
            .path
            .as_deref()
            .ok_or_else(|| KbError::invalid_input("anchor has no path"))?;
        let at_commit = git
            .resolve_commit(at)?
            .ok_or_else(|| KbError::invalid_input(format!("revision {at} is unavailable")))?;
        let baseline = if since == "verified" {
            anchor
                .stamp
                .as_ref()
                .map(|s| s.commit.as_str())
                .or(anchor.commit.as_deref())
                .or(record.common().verified_at)
                .ok_or_else(|| {
                    KbError::invalid_input("no stamp, anchor commit or verified_at baseline")
                })?
        } else {
            since
        };
        let from = if date_days(baseline).is_some() {
            crate::host::facts::resolve_as_of(Some(root), baseline, &[])?
                .host_revision
                .ok_or_else(|| {
                    KbError::invalid_input("no host commit exists at the verification date")
                })?
        } else {
            git.resolve_commit(baseline)?.ok_or_else(|| {
                KbError::invalid_input(format!("baseline {baseline} is unavailable"))
            })?
        };
        out.from = Some(from.clone());
        out.at = Some(at_commit.clone());
        if since == "verified" && !git.is_ancestor(&from, &at_commit)? {
            return Err(KbError::invalid_input(
                "verification baseline is not an ancestor of the selected revision (or history is incomplete)",
            ));
        }
        let before = crate::host::facts::blob_at(root, &from, path, provenance::MAX_ANCHOR_BYTES)?
            .ok_or_else(|| KbError::invalid_input("anchored path did not exist at the baseline"))?;
        let after =
            crate::host::facts::blob_at(root, &at_commit, path, provenance::MAX_ANCHOR_BYTES)?;
        if since == "verified"
            && let Some(stamp) = &anchor.stamp
        {
            let baseline_bytes = provenance::line_span(&before, stamp.start_line, stamp.end_line)?;
            if sha256_hex(baseline_bytes) != stamp.sha256 {
                return Err(KbError::invalid_input(
                    "stored stamp does not match its own baseline",
                ));
            }
            let same = after
                .as_ref()
                .and_then(|b| provenance::line_span(b, stamp.start_line, stamp.end_line).ok())
                .is_some_and(|bytes| sha256_hex(bytes) == stamp.sha256);
            out.state = if same {
                State::Unchanged
            } else {
                State::Changed
            };
            out.detail = if same {
                "stamped span is unchanged"
            } else {
                "stamped span changed, moved or disappeared; review it before restamping"
            }
            .into();
        } else {
            if let Some(symbol) = &anchor.symbol
                && !provenance::symbol_spelling(
                    std::str::from_utf8(&before)
                        .map_err(|_| KbError::invalid_input("symbol baseline is not UTF-8"))?,
                    symbol,
                )
            {
                return Err(KbError::invalid_input(
                    "symbol spelling was absent at the baseline; qualified definitions need provider evidence",
                ));
            }
            out.state = if after.as_deref() == Some(before.as_slice()) {
                State::Unchanged
            } else {
                State::Changed
            };
            out.detail =
                "whole-file comparison; a provider definition stamp can narrow later drift checks"
                    .into();
        }
        Ok(())
    })();
    if let Err(error) = checked {
        out.detail = error.message;
    }
    out
}
