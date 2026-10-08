//! Git-backed knowledge maintenance. Evidence checks never establish semantic truth or
//! change a record's acceptance status. All writes are explicit, previewable metadata edits.
pub mod drift;
pub mod ledger;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;
use toml_edit::{DocumentMut, InlineTable, Item, TableLike, Value};

use crate::code::Provider;
use crate::corpus::Corpus;
use crate::error::{ErrorCode, KbError, Result};
use crate::git::Git;
use crate::knowledge::{KnowledgeView, Origin, RecordEntry};
use crate::model::*;
use crate::provenance::{self, AnchorEvidence, AnchorStatus, HostRoots};
use crate::util::{atomic_write, read_file_limited, safe_join, sha256_hex};

pub fn select(
    view: &dyn KnowledgeView,
    ids: &[String],
    include_drafts: bool,
) -> Result<Vec<RecordEntry>> {
    check_ids(ids)?;
    let mut records = if ids.is_empty() {
        view.all_records(Origin::Accepted)?
    } else {
        view.records(ids, Origin::Accepted)?
    };
    if !ids.is_empty() {
        let found: BTreeSet<_> = records.iter().map(|r| r.parsed.record.id()).collect();
        if let Some(missing) = ids.iter().find(|id| !found.contains(id.as_str())) {
            return Err(KbError::new(
                ErrorCode::NotFound,
                format!("record {missing} is absent from this snapshot"),
            ));
        }
    } else {
        records.retain(|r| {
            r.parsed.record.status() == Status::Accepted
                || (include_drafts && r.parsed.record.status() == Status::Draft)
        });
    }
    records.sort_by(|a, b| a.parsed.record.id().cmp(b.parsed.record.id()));
    records.dedup_by(|a, b| a.parsed.record.id() == b.parsed.record.id());
    Ok(records)
}

fn check_ids(ids: &[String]) -> Result<()> {
    if ids.len() > 512 {
        return Err(KbError::invalid_input(
            "at most 512 record ids per maintenance operation",
        ));
    }
    for id in ids {
        crate::model::ids::check_record_id(id).map_err(KbError::invalid_input)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct AnchorReport {
    pub at: String,
    pub checked_records: usize,
    pub anchors: Vec<AnchorEvidence>,
    pub without_anchors: Vec<String>,
    pub verified: usize,
    pub changed: usize,
    pub missing: usize,
    pub unverifiable: usize,
    pub note: &'static str,
}

pub fn check_anchors(
    kb_root: &Path,
    roots: &HostRoots,
    records: &[RecordEntry],
    at: &str,
) -> AnchorReport {
    let mut anchors = Vec::new();
    let mut without_anchors = Vec::new();
    for entry in records {
        let record = &entry.parsed.record;
        if record.common().anchors.is_empty() {
            without_anchors.push(record.id().into());
        }
        for (index, anchor) in record.common().anchors.iter().enumerate() {
            anchors.push(provenance::inspect(
                kb_root,
                roots,
                record.id(),
                index,
                anchor,
                Some(at),
            ));
        }
    }
    let count = |status| anchors.iter().filter(|a| a.status == status).count();
    AnchorReport {
        at: at.into(),
        checked_records: records.len(),
        verified: count(AnchorStatus::Verified),
        changed: count(AnchorStatus::Changed),
        missing: count(AnchorStatus::Missing),
        unverifiable: count(AnchorStatus::Unverifiable),
        anchors,
        without_anchors,
        note: "Git file/commit/stamp checks only. An unstamped file or lexical symbol match does not prove a claim, review decision or executed test.",
    }
}

pub struct StampOptions<'a> {
    pub ids: &'a [String],
    pub at: &'a str,
    pub verified_at: Option<&'a str>,
    pub review_by: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StampedAnchor {
    pub index: usize,
    pub stamp: AnchorStamp,
    pub extent: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct StampChange {
    pub id: String,
    pub path: String,
    pub before_sha256: String,
    pub after_sha256: String,
    pub anchors: Vec<StampedAnchor>,
    pub diff: String,
    #[serde(skip)]
    before: Vec<u8>,
    #[serde(skip)]
    after: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StampPlan {
    pub changes: Vec<StampChange>,
    pub skipped: Vec<String>,
    pub note: &'static str,
}

pub fn plan_stamps(
    kb_root: &Path,
    corpus: &Corpus,
    roots: &HostRoots,
    provider: &Provider,
    options: &StampOptions<'_>,
) -> Result<StampPlan> {
    check_ids(options.ids)?;
    if options.ids.is_empty() {
        return Err(KbError::invalid_input(
            "anchors stamp requires explicit --id targets",
        ));
    }
    for date in [options.verified_at, options.review_by]
        .into_iter()
        .flatten()
    {
        if date_days(date).is_none() {
            return Err(KbError::invalid_input(
                "review metadata must use YYYY-MM-DD",
            ));
        }
    }
    let ids: BTreeSet<_> = options.ids.iter().map(String::as_str).collect();
    let selected: BTreeMap<_, _> = corpus
        .records()
        .filter(|(_, p)| ids.contains(p.record.id()))
        .map(|(e, p)| (p.record.id(), (e, p)))
        .collect();
    for id in ids {
        if !selected.contains_key(id) {
            return Err(KbError::new(
                ErrorCode::NotFound,
                format!("record {id} is missing or invalid"),
            ));
        }
    }
    let mut graphs: BTreeMap<(String, String), CodeResponse> = BTreeMap::new();
    let mut changes = Vec::new();
    let mut skipped = Vec::new();
    for (id, (entry, parsed)) in selected {
        if parsed.record.common().schema != 2 {
            return Err(KbError::invalid_input(format!(
                "{id} needs schema 2; run kbw migrate before stamping"
            )));
        }
        let text = entry.raw.as_ref();
        let (front, _, _) =
            crate::parse::split_front_matter(text).map_err(KbError::invalid_input)?;
        let start = front.as_ptr() as usize - text.as_ptr() as usize;
        let end = start + front.len();
        let mut doc = front
            .parse::<DocumentMut>()
            .map_err(|e| KbError::invalid_input(e.to_string()))?;
        let mut stamped = Vec::new();
        for (index, anchor) in parsed.record.common().anchors.iter().enumerate() {
            let Some(path) = &anchor.path else {
                skipped.push(format!(
                    "{id} anchor {index}: immutable change reference has no source span"
                ));
                continue;
            };
            let root = provenance::anchor_root(kb_root, roots, anchor)?;
            if root == kb_root && path == &entry.path {
                return Err(KbError::invalid_input(
                    "an anchor cannot stamp the knowledge file that contains its own stamp",
                ));
            }
            let commit = Git::new(root).resolve_commit(options.at)?.ok_or_else(|| {
                KbError::invalid_input(format!(
                    "host revision {} is unavailable for {id}",
                    options.at
                ))
            })?;
            // A change anchor's own commit names the reviewed change: it is never rewritten,
            // and a stamp from another commit would contradict it.
            let reviewed = anchor
                .commit
                .as_deref()
                .filter(|_| anchor.kind == AnchorKind::Change);
            if let Some(reviewed) = reviewed
                && !commit.starts_with(&reviewed.to_ascii_lowercase())
            {
                skipped.push(format!(
                    "{id} anchor {index}: change anchor keeps its reviewed commit {reviewed}; stamp it with --at {reviewed}"
                ));
                continue;
            }
            let bytes =
                crate::host::facts::blob_at(root, &commit, path, provenance::MAX_ANCHOR_BYTES)?
                    .ok_or_else(|| {
                        KbError::new(ErrorCode::NotFound, format!("{path} is absent at {commit}"))
                    })?;
            let mut stamp = provenance::whole_file_stamp(commit.clone(), &bytes)?;
            let mut extent = "whole-file";
            if let Some(symbol) = &anchor.symbol {
                let mut resolved = false;
                if provider.configured()
                    && let Some(repo) = &anchor.repo
                {
                    let key = (repo.clone(), commit.clone());
                    if !graphs.contains_key(&key) {
                        if graphs.len() >= 32 {
                            return Err(KbError::invalid_input(
                                "stamping needs over 32 code graphs; split the requested ids",
                            ));
                        }
                        let request =
                            crate::code::request(root, repo, &commit, CodeOperation::Symbols)?;
                        graphs.insert(key.clone(), provider.load(&request)?);
                    }
                    let found: Vec<_> = graphs[&key]
                        .symbols
                        .iter()
                        .filter(|s| {
                            s.path == *path
                                && s.name == *symbol
                                && s.extent == CodeExtent::Definition
                        })
                        .collect();
                    if let [definition] = found.as_slice() {
                        stamp.start_line = definition.start_line;
                        stamp.end_line = definition.end_line;
                        stamp.sha256 = definition.sha256.clone();
                        extent = "provider-definition";
                        resolved = true;
                    }
                }
                if !resolved
                    && !provenance::symbol_spelling(
                        std::str::from_utf8(&bytes)
                            .map_err(|_| KbError::invalid_input("symbol source is not UTF-8"))?,
                        symbol,
                    )
                {
                    return Err(KbError::invalid_input(format!(
                        "symbol {symbol:?} has no exact provider definition or lexical spelling in {path}"
                    )));
                }
            }
            let anchors = doc
                .get_mut("anchors")
                .ok_or_else(|| KbError::internal("parsed anchors are absent from TOML"))?;
            let table: &mut dyn TableLike = if let Item::ArrayOfTables(array) = anchors {
                array
                    .get_mut(index)
                    .ok_or_else(|| KbError::internal("anchor index"))?
            } else {
                anchors
                    .as_array_mut()
                    .and_then(|array| array.get_mut(index))
                    .and_then(Value::as_inline_table_mut)
                    .ok_or_else(|| KbError::invalid_input("anchors must be tables"))?
            };
            if anchor.kind != AnchorKind::Change {
                put(table, "commit", Value::from(commit));
            }
            if table.get("stamp").is_none() {
                table.insert("stamp", Item::Value(Value::InlineTable(InlineTable::new())));
            }
            let target = table
                .get_mut("stamp")
                .and_then(Item::as_table_like_mut)
                .ok_or_else(|| KbError::invalid_input("stamp must be a table"))?;
            put(target, "commit", Value::from(stamp.commit.clone()));
            put(
                target,
                "start_line",
                Value::from(i64::from(stamp.start_line)),
            );
            put(target, "end_line", Value::from(i64::from(stamp.end_line)));
            put(target, "sha256", Value::from(stamp.sha256.clone()));
            stamped.push(StampedAnchor {
                index,
                stamp,
                extent,
            });
        }
        if let Some(date) = options.verified_at {
            put(doc.as_table_mut(), "verified_at", Value::from(date));
        }
        if let Some(date) = options.review_by {
            put(doc.as_table_mut(), "review_by", Value::from(date));
        }
        let after = format!(
            "{}{}{}",
            &text[..start],
            keep_line_endings(front, &doc.to_string()),
            &text[end..]
        );
        crate::parse::parse_record(&entry.path, after.as_bytes()).map_err(|d| {
            KbError::new(ErrorCode::ValidationFailed, "stamped record is invalid")
                .with_diagnostics(d)
        })?;
        changes.push(StampChange {
            id: id.into(),
            path: entry.path.clone(),
            before_sha256: sha256_hex(text.as_bytes()),
            after_sha256: sha256_hex(after.as_bytes()),
            anchors: stamped,
            diff: crate::migrate::unified_diff(&entry.path, text, &after),
            before: text.as_bytes().to_vec(),
            after: after.into_bytes(),
        });
    }
    Ok(StampPlan {
        changes,
        skipped,
        note: "Metadata only; no status is accepted. verified_at/review_by are caller-reported review metadata, never inferred from an anchor or a test command. Without a unique provider definition, symbol anchors conservatively stamp the whole file.",
    })
}

/// `toml_edit` re-emits untouched values byte for byte but writes `\n` line breaks. A line
/// that differs from an original line only in its break keeps the original bytes, so a CRLF
/// line, or a bare LF inside a multi-line string, is not rewritten. Only inserted or edited
/// lines take the front matter's predominant line ending.
fn keep_line_endings(original: &str, edited: &str) -> String {
    use crate::migrate::Op;
    // A line without its break, and whether it has one.
    fn key(line: &str) -> (&str, bool) {
        match line.strip_suffix('\n') {
            Some(l) => (l.strip_suffix('\r').unwrap_or(l), true),
            None => (line, false),
        }
    }
    let eol = if original.matches("\r\n").count() * 2 > original.matches('\n').count() {
        "\r\n"
    } else {
        "\n"
    };
    let a: Vec<&str> = original.split_inclusive('\n').collect();
    let b: Vec<(&str, bool)> = edited.split_inclusive('\n').map(key).collect();
    let a_keys: Vec<(&str, bool)> = a.iter().map(|l| key(l)).collect();
    let mut out = String::with_capacity(edited.len());
    for op in crate::migrate::diff_ops(&a_keys, &b) {
        match op {
            Op::Equal(x, _) => out.push_str(a[x]),
            Op::Insert(y) => {
                out.push_str(b[y].0);
                if b[y].1 {
                    out.push_str(eol);
                }
            }
            Op::Delete(_) => {}
        }
    }
    out
}

fn put(table: &mut dyn TableLike, key: &str, mut value: Value) {
    if let Some(previous) = table.get(key).and_then(Item::as_value) {
        *value.decor_mut() = previous.decor().clone();
    }
    table.insert(key, Item::Value(value));
}

pub fn apply_stamps(kb_root: &Path, plan: &StampPlan) -> Result<usize> {
    // Check the entire plan before the first write. Each write is atomic; concurrent edits
    // to any target detected here are preserved instead of silently replacing them.
    for change in &plan.changes {
        let path = safe_join(kb_root, &change.path)?;
        if read_file_limited(&path, crate::parse::MAX_RECORD_BYTES as u64)? != change.before {
            return Err(KbError::new(
                ErrorCode::Conflict,
                format!("{} changed after stamp planning", change.path),
            ));
        }
    }
    let mut written = 0;
    for change in &plan.changes {
        if change.before != change.after {
            atomic_write(&safe_join(kb_root, &change.path)?, &change.after)?;
            written += 1;
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::keep_line_endings;
    use toml_edit::{DocumentMut, Value};

    /// Insert `c = 3` with `toml_edit`, as stamping does, and restore the line endings.
    fn stamp(original: &str) -> String {
        let mut doc = original.parse::<DocumentMut>().unwrap();
        doc.insert("c", toml_edit::value(Value::from(3)));
        keep_line_endings(original, &doc.to_string())
    }

    #[test]
    fn stamped_front_matter_keeps_its_predominant_line_ending() {
        assert_eq!(stamp("a = 1\r\nb = 2\r\n"), "a = 1\r\nb = 2\r\nc = 3\r\n");
        assert_eq!(stamp("a = 1\nb = 2\n"), "a = 1\nb = 2\nc = 3\n");
        // Unchanged lines keep their own break; only the inserted line takes the majority's.
        assert_eq!(
            stamp("a = 1\r\nb = 2\nd = 4\n"),
            "a = 1\r\nb = 2\nd = 4\nc = 3\n"
        );
        // A bare LF inside a multi-line string is part of the value and is not rewritten.
        let original = "a = 1\r\ntext = \"\"\"\r\nPersist synthetic\nstate.\"\"\"\r\nb = 2\r\n";
        let stamped = stamp(original);
        assert_eq!(stamped, format!("{original}c = 3\r\n"));
        let parsed = stamped.parse::<DocumentMut>().unwrap();
        assert_eq!(parsed["text"].as_str(), Some("Persist synthetic\nstate."));
    }
}
