//! Draft submission adapter. It never accepts records, commits changes or modifies refs.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use crate::context::{canonical_json, lexical};
use crate::corpus::load_corpus;
use crate::error::{ErrorCode, KbError, Result};
use crate::knowledge::{KnowledgeView, Origin, RecordEntry};
use crate::model::{ParsedRecord, ProfileLocation, Status};
use crate::parse::{MAX_RECORD_BYTES, parse_record};
use crate::provenance::{AnchorEvidence, AnchorStatus, HostRoots};
use crate::source::WorkingTreeSource;
use crate::util::{atomic_write, read_file_limited, safe_join, sha256_hex};
use crate::validate::{MetaInput, validate_metas};

pub struct DraftContext<'a> {
    pub kb_root: &'a Path,
    pub loc: &'a ProfileLocation,
    pub view: &'a dyn KnowledgeView,
    pub hosts: &'a HostRoots,
}

#[derive(Debug, Clone, Serialize)]
pub struct DuplicateCandidate {
    pub id: String,
    pub origin: Origin,
    pub status: Status,
    pub similarity_per_mille: u64,
    pub exact: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DraftPlan {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumer_evidence: Option<crate::context::code::CodeInfo>,
    pub id: String,
    pub path: String,
    pub action: &'static str,
    pub status: &'static str,
    pub sha256: String,
    pub duplicates: Vec<DuplicateCandidate>,
    pub anchors: Vec<AnchorEvidence>,
    pub diff: String,
    pub note: &'static str,
    #[serde(skip)]
    before: Option<Vec<u8>>,
    #[serde(skip)]
    after: Vec<u8>,
}

fn content_key(parsed: &ParsedRecord) -> Result<String> {
    let mut value = serde_json::to_value(parsed)?;
    if let Some(record) = value.get_mut("record").and_then(Value::as_object_mut) {
        for key in [
            "schema",
            "id",
            "title",
            "owner",
            "status",
            "anchors",
            "verified_at",
            "review_by",
        ] {
            record.remove(key);
        }
    }
    Ok(sha256_hex(canonical_json(&value).as_bytes()))
}

fn words(parsed: &ParsedRecord) -> BTreeSet<String> {
    let text = lexical::fts_columns(parsed);
    crate::normalize::tokens(&format!("{} {}", text.title, text.normative))
        .into_iter()
        .filter(|s| s.chars().count() >= 3)
        .collect()
}

pub fn duplicate_candidates(
    parsed: &ParsedRecord,
    records: &[RecordEntry],
) -> Result<Vec<DuplicateCandidate>> {
    let key = content_key(parsed)?;
    let query = words(parsed);
    let mut candidates = Vec::new();
    for entry in records {
        let r = &entry.parsed;
        if !matches!(r.record.status(), Status::Accepted | Status::Draft) {
            continue;
        }
        if r.record.id() == parsed.record.id() || r.record.kind() != parsed.record.kind() {
            continue;
        }
        let exact = content_key(r)? == key;
        let other = words(r);
        let intersection = query.intersection(&other).count();
        let union = query.union(&other).count();
        let score = if union == 0 {
            0
        } else {
            (intersection as u64 * 1000) / union as u64
        };
        if exact || (intersection >= 3 && score >= 450) {
            candidates.push(DuplicateCandidate {
                id: r.record.id().into(),
                origin: entry.origin,
                status: r.record.status(),
                similarity_per_mille: if exact { 1000 } else { score },
                exact,
            });
        }
    }
    candidates.sort_by(|a, b| {
        b.exact
            .cmp(&a.exact)
            .then_with(|| b.similarity_per_mille.cmp(&a.similarity_per_mille))
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.origin.cmp(&b.origin))
    });
    candidates.dedup_by(|a, b| a.id == b.id && a.exact == b.exact);
    candidates.truncate(12);
    Ok(candidates)
}

fn current_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(_) => read_file_limited(path, MAX_RECORD_BYTES as u64).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(KbError::io(path.display(), e)),
    }
}

/// Whether the KB checkout's HEAD commit has a file at `path`.
fn head_has(kb_root: &Path, path: &str) -> Result<bool> {
    let Some(head) = crate::git::Git::new(kb_root).resolve_commit("HEAD")? else {
        return Ok(false);
    };
    Ok(crate::host::facts::blob_at(kb_root, &head, path, MAX_RECORD_BYTES as u64)?.is_some())
}

const SNAPSHOT_HINT: &str = "to revise a draft committed on a KB proposal branch, retry from that branch with --snapshot working-tree --offline; otherwise update the KB checkout to the selected snapshot";

pub fn prepare(ctx: &DraftContext<'_>, text: &str) -> Result<DraftPlan> {
    let parsed = parse_record("submission", text.as_bytes()).map_err(|d| {
        KbError::new(
            ErrorCode::ValidationFailed,
            "draft does not satisfy the document schema",
        )
        .with_details(serde_json::json!({"diagnostics": d}))
    })?;
    if parsed.record.status() != Status::Draft {
        return Err(KbError::new(
            ErrorCode::ValidationFailed,
            "submissions must have status = draft; only review and merge can accept knowledge",
        ));
    }
    let id = parsed.record.id();
    let existing = ctx.view.raw(id, Origin::Accepted)?;
    // Identity also covers local records that the selected snapshot does not contain yet,
    // such as unapproved drafts on a proposal branch.
    let local = load_corpus(&WorkingTreeSource::new(ctx.kb_root), ctx.loc)?;
    let same_id: Vec<_> = local
        .records()
        .filter(|(_, r)| r.record.id() == id)
        .collect();
    if ctx
        .view
        .metas_by_ids(&[id.into()], Origin::Accepted)?
        .iter()
        .any(|e| e.meta.kind != parsed.record.kind())
        || same_id
            .iter()
            .any(|(_, r)| r.record.kind() != parsed.record.kind())
    {
        return Err(KbError::new(
            ErrorCode::ValidationFailed,
            "a stable record id cannot change its kind",
        ));
    }
    let local_path = match same_id.as_slice() {
        [] => None,
        [(entry, _)] => Some(entry.path.clone()),
        _ => {
            return Err(KbError::new(
                ErrorCode::Conflict,
                format!("{id} already exists at several local paths; resolve that duplicate first"),
            ));
        }
    };
    let path = match (&existing, &local_path) {
        (Some(existing), Some(local)) if existing.path != *local => {
            return Err(KbError::new(
                ErrorCode::Conflict,
                format!(
                    "{id} is at {local} locally but at {} in the selected snapshot; submission will not create a second copy",
                    existing.path
                ),
            ));
        }
        (Some(existing), _) => existing.path.clone(),
        (None, Some(local)) => local.clone(),
        (None, None) => format!(
            "{}/drafts/{id}.md",
            ctx.loc
                .knowledge_roots(ctx.view.config())
                .first()
                .ok_or_else(|| KbError::invalid_input("profile has no knowledge root"))?
        ),
    };
    let target = safe_join(ctx.kb_root, &path)?;
    let before = current_bytes(&target)?;
    if before.as_deref() != Some(text.as_bytes()) {
        let committed = if let Some(bytes) = &before
            && let Some(head) = crate::git::Git::new(ctx.kb_root).resolve_commit("HEAD")?
        {
            if crate::host::facts::blob_at(ctx.kb_root, &head, &path, MAX_RECORD_BYTES as u64)?
                .as_ref()
                != Some(bytes)
            {
                return Err(KbError::new(
                    ErrorCode::Conflict,
                    format!(
                        "{path} has uncommitted changes; edit/validate that draft directly instead of overwriting it"
                    ),
                ));
            }
            true
        } else {
            false
        };
        match (&before, &existing) {
            (Some(bytes), Some(accepted)) if bytes.as_slice() == accepted.text.as_bytes() => {}
            // A committed local record with this id that the snapshot lacks (an unapproved
            // draft on a proposal branch) may be revised in place; Git keeps its old bytes.
            (Some(_), None) if committed && local_path.as_deref() == Some(path.as_str()) => {}
            (None, None) => {}
            // The file matches HEAD: it is a revision committed on this branch, or the
            // checkout is not at the selected snapshot.
            (Some(_), Some(_)) if committed => {
                return Err(KbError::new(
                    ErrorCode::Conflict,
                    format!(
                        "{path} is committed with text that differs from the approved record in the selected snapshot; submission will not overwrite it"
                    ),
                )
                .with_hint(SNAPSHOT_HINT));
            }
            // Neither the working tree nor HEAD has the snapshot's record: the checkout is
            // behind it, such as a proposal branch created before the record was accepted.
            // A deletion that is not committed yet is a local change.
            (None, Some(_)) if !head_has(ctx.kb_root, &path)? => {
                return Err(KbError::new(
                    ErrorCode::Conflict,
                    format!(
                        "the KB checkout does not contain the approved record {path} from the selected snapshot; submission will not write it"
                    ),
                )
                .with_hint(SNAPSHOT_HINT));
            }
            _ => {
                return Err(KbError::new(
                    ErrorCode::Conflict,
                    format!("{path} has local changes; submission will not overwrite them"),
                ));
            }
        }
    }
    let mut metas: Vec<_> = ctx
        .view
        .metas_by_kind(&crate::model::Kind::ALL, Origin::Accepted)?
        .into_iter()
        .filter(|e| e.meta.id != id)
        .map(|e| MetaInput {
            path: e.path,
            meta: e.meta,
        })
        .collect();
    metas.push(MetaInput {
        path: path.clone(),
        meta: Arc::new(parsed.meta()),
    });
    let problems: Vec<_> = validate_metas(ctx.view.config(), ctx.view.registry(), &metas)
        .into_iter()
        .filter(|d| d.is_error() && (d.record.as_deref() == Some(id) || d.code.ends_with("CYCLE")))
        .collect();
    if !problems.is_empty() {
        return Err(KbError::new(
            ErrorCode::ValidationFailed,
            "draft has invalid ownership, scope or references",
        )
        .with_details(serde_json::json!({"diagnostics": problems})));
    }
    let anchors: Vec<_> = parsed
        .record
        .common()
        .anchors
        .iter()
        .enumerate()
        .map(|(i, a)| crate::provenance::inspect(ctx.kb_root, ctx.hosts, id, i, a, None))
        .collect();
    if anchors.iter().any(|a| a.status != AnchorStatus::Verified) {
        return Err(KbError::new(
            ErrorCode::ValidationFailed,
            "one or more draft anchors could not be verified",
        )
        .with_details(serde_json::json!({"anchors": anchors})));
    }
    let mut records = ctx.view.all_records(Origin::Accepted)?;
    for (entry, record) in local.records() {
        records.push(RecordEntry {
            path: entry.path.clone(),
            origin: Origin::Proposal,
            parsed: record.clone(),
        });
    }
    let duplicates = duplicate_candidates(&parsed, &records)?;
    if duplicates.iter().any(|c| c.exact) {
        return Err(KbError::new(
            ErrorCode::Conflict,
            "identical knowledge already exists under another id; revise the existing record",
        )
        .with_details(serde_json::json!({"duplicates": duplicates})));
    }
    let action = if before.as_deref() == Some(text.as_bytes()) {
        "unchanged"
    } else if before.is_some() {
        "modify"
    } else {
        "create"
    };
    Ok(DraftPlan {
        consumer_evidence: None,
        id: id.into(),
        path: path.clone(),
        action,
        status: "draft",
        sha256: sha256_hex(text.as_bytes()),
        duplicates,
        anchors,
        diff: crate::migrate::unified_diff(
            &path,
            before
                .as_ref()
                .and_then(|b| std::str::from_utf8(b).ok())
                .unwrap_or_default(),
            text,
        ),
        note: "Draft only. Anchor checks verify available Git evidence, not semantic truth or test execution. Reviewers finalize status/links before merging into the approved ref.",
        before,
        after: text.as_bytes().to_vec(),
    })
}

pub fn apply(kb_root: &Path, plan: &DraftPlan) -> Result<bool> {
    let path = safe_join(kb_root, &plan.path)?;
    if current_bytes(&path)? != plan.before {
        return Err(KbError::new(
            ErrorCode::Conflict,
            format!(
                "{} changed after validation; retry without overwriting it",
                plan.path
            ),
        ));
    }
    if plan.action == "unchanged" {
        return Ok(false);
    }
    atomic_write(&path, &plan.after)?;
    Ok(true)
}

/// Append consumer candidates without reserializing the author's other TOML or Markdown.
pub fn add_consumers(text: &str, candidates: &[crate::model::Consumer]) -> Result<String> {
    use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};
    let parsed = parse_record("consumer-input", text.as_bytes()).map_err(|d| {
        KbError::new(ErrorCode::ValidationFailed, "invalid consumer input").with_diagnostics(d)
    })?;
    let crate::model::Record::Contract(contract) = &parsed.record else {
        return Err(KbError::invalid_input(
            "--fill-consumers applies only to contract drafts",
        ));
    };
    let mut existing: BTreeSet<_> = contract
        .consumers
        .iter()
        .map(|c| (c.repo.clone(), c.path.clone(), c.symbol.clone()))
        .collect();
    let (front, _, _) = crate::parse::split_front_matter(text).map_err(KbError::invalid_input)?;
    let start = front.as_ptr() as usize - text.as_ptr() as usize;
    let end = start + front.len();
    let mut doc = front
        .parse::<DocumentMut>()
        .map_err(|e| KbError::invalid_input(e.to_string()))?;
    for c in candidates {
        if !existing.insert((c.repo.clone(), c.path.clone(), c.symbol.clone())) {
            continue;
        }
        if doc.get("consumers").is_none() {
            doc["consumers"] = Item::ArrayOfTables(ArrayOfTables::new());
        }
        let fields = std::iter::once(("repo", &c.repo))
            .chain(std::iter::once(("path", &c.path)))
            .chain(c.symbol.iter().map(|symbol| ("symbol", symbol)));
        let slot = doc.get_mut("consumers").unwrap();
        if let Some(array) = slot.as_array_mut() {
            let mut table = InlineTable::new();
            for (key, value) in fields {
                table.insert(key, Value::from(value.as_str()));
            }
            array.push(Value::InlineTable(table));
        } else if let Some(array) = slot.as_array_of_tables_mut() {
            let mut table = Table::new();
            for (key, value) in fields {
                table.insert(key, toml_edit::value(value.as_str()));
            }
            array.push(table);
        } else {
            return Err(KbError::invalid_input(
                "consumers must be an array of tables",
            ));
        }
    }
    Ok(format!("{}{}{}", &text[..start], doc, &text[end..]))
}
