//! Proposal overlay: local knowledge changes indexed next to (never instead of) the
//! accepted snapshot. Each changed record file becomes one classified entry:
//! `new`, `modifies <id>`, `removes <id>` or `invalid`. Changes outside the knowledge roots
//! (registry, profile config) are never applied and produce `PROPOSAL_NOT_APPLIED`. A file
//! byte-identical to the accepted file at the same path is skipped (compared by content,
//! whatever the id scheme of the accepted snapshot).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use rusqlite::params;

use super::build::{DocWriter, Member, PROPOSAL, attach};
use crate::corpus::is_record_path;
use crate::diag::{self, Diagnostic};
use crate::error::Result;
use crate::knowledge::{Overlay, OverlayFile, OverlayStatus, ProposalChange};
use crate::model::{ProfileConfig, RecordMeta, Registry};
use crate::util::sha256_hex;
use crate::validate::{MetaInput, validate_metas};

/// Outcome of applying an overlay to a snapshot.
pub struct Applied {
    pub entries: usize,
    /// Proposal files whose parsed document already existed.
    pub reused_docs: usize,
    /// Snapshot-level warnings (changes that cannot be proposals).
    pub warnings: Vec<Diagnostic>,
}

struct Entry {
    path: String,
    change: ProposalChange,
    record_id: Option<String>,
    /// Document shown for the entry: the proposal (new/modifies/invalid) or the accepted
    /// record being removed.
    doc: Option<i64>,
    /// Proposed metadata (new/modifies only), used to validate the proposed state.
    proposed: Option<Arc<RecordMeta>>,
    stale: bool,
    diagnostics: Vec<Diagnostic>,
}

fn not_applied(path: &str, why: &str) -> Diagnostic {
    Diagnostic::warning("PROPOSAL_NOT_APPLIED", why).at_path(path.to_string())
}

fn under_roots(path: &str, roots: &[String]) -> bool {
    roots.iter().any(|r| {
        path.strip_prefix(r.as_str())
            .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// Files inside knowledge roots that are documentation, not records.
fn is_documentation(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|f| f.eq_ignore_ascii_case("README.md") || f == ".gitkeep")
}

/// Is the overlay content (`bytes`, identified by `content_id`) the accepted member's
/// content? Ids of one scheme compare directly. Across schemes (a Git snapshot's `git:`
/// blob ids against the overlay's `sha256:` ids) the accepted document's stored text is
/// compared; that text is exact unless the original was not valid UTF-8 (decoded with
/// U+FFFD), in which case the contents are treated as different.
fn same_content(
    docs: &DocWriter<'_>,
    accepted: &Member,
    content_id: &str,
    bytes: &[u8],
) -> Result<bool> {
    fn scheme(id: &str) -> Option<&str> {
        id.split_once(':').map(|(s, _)| s)
    }
    if accepted.content_id == content_id {
        return Ok(true);
    }
    if scheme(&accepted.content_id) == scheme(content_id) {
        return Ok(false);
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(false);
    };
    let raw = docs.raw(accepted.doc)?;
    Ok(raw == text && !raw.contains(char::REPLACEMENT_CHARACTER))
}

/// Classify and store the overlay's proposals for `snapshot`.
pub fn apply(
    docs: &mut DocWriter<'_>,
    snapshot: i64,
    roots: &[String],
    overlay: &Overlay,
    accepted: &[Member],
    config: &ProfileConfig,
    registry: &Registry,
) -> Result<Applied> {
    let by_path: HashMap<&str, &Member> = accepted.iter().map(|m| (m.path.as_str(), m)).collect();
    let accepted_ids: HashSet<&str> = accepted
        .iter()
        .filter_map(|m| m.record_id.as_deref())
        .collect();
    let mut files: Vec<&OverlayFile> = overlay.files.iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let mut warnings = Vec::new();
    let mut entries: Vec<Entry> = Vec::new();
    let mut touched: HashSet<&str> = HashSet::new();
    let mut reused_docs = 0;
    let mut deletions = Vec::new();
    for f in files {
        if !under_roots(&f.path, roots) {
            warnings.push(not_applied(
                &f.path,
                "local change outside the knowledge roots is not applied; registry and \
                 configuration changes take effect only after review and merge",
            ));
            continue;
        }
        if !is_record_path(&f.path) {
            if !is_documentation(&f.path) {
                warnings.push(not_applied(
                    &f.path,
                    "only `.md` record files under knowledge roots can be proposed",
                ));
            }
            continue;
        }
        if f.status == OverlayStatus::Deleted {
            deletions.push(f);
            continue;
        }
        let Some(bytes) = &f.content else {
            touched.insert(&f.path);
            entries.push(Entry {
                path: f.path.clone(),
                change: ProposalChange::Invalid,
                record_id: None,
                doc: None,
                proposed: None,
                stale: f.stale,
                diagnostics: vec![
                    Diagnostic::error("PROPOSAL_UNREADABLE", "changed file content is missing")
                        .at_path(f.path.clone()),
                ],
            });
            continue;
        };
        let content_id = f
            .content_id
            .clone()
            .unwrap_or_else(|| format!("sha256:{}", sha256_hex(bytes)));
        let current = by_path.get(f.path.as_str());
        if let Some(m) = current
            && same_content(docs, m, &content_id, bytes)?
        {
            continue; // identical to the accepted file: nothing is proposed
        }
        touched.insert(&f.path);
        let (doc, reused) = docs.get_or_insert(&content_id, &f.path, bytes)?;
        reused_docs += usize::from(reused);
        docs.add_member(snapshot, PROPOSAL, &f.path, doc)?;
        let stored = docs.load(doc)?;
        let (record_id, meta) = (stored.record_id, stored.meta);
        let mut diagnostics = attach(stored.diagnostics, &f.path);
        let change = match &record_id {
            None => ProposalChange::Invalid,
            Some(id) if accepted_ids.contains(id.as_str()) => {
                ProposalChange::Modifies { id: id.clone() }
            }
            Some(id) => ProposalChange::New { id: id.clone() },
        };
        if let (Some(id), Some(Some(old))) = (&record_id, current.map(|m| &m.record_id))
            && id != old
        {
            diagnostics.push(
                Diagnostic::warning(
                    "PROPOSAL_REPLACES_RECORD",
                    format!("this change replaces accepted record `{old}` at the same path"),
                )
                .at_path(f.path.clone())
                .for_record(id.clone()),
            );
        }
        entries.push(Entry {
            path: f.path.clone(),
            change,
            record_id,
            doc: Some(doc),
            proposed: meta,
            stale: f.stale,
            diagnostics,
        });
    }

    // Deleting a file whose record reappears elsewhere in the overlay is a move, not a removal.
    let proposed_ids: HashSet<String> =
        entries.iter().filter_map(|e| e.record_id.clone()).collect();
    for f in deletions {
        touched.insert(&f.path);
        let Some(m) = by_path.get(f.path.as_str()) else {
            continue;
        };
        let Some(id) = &m.record_id else {
            continue;
        };
        if proposed_ids.contains(id) {
            continue;
        }
        entries.push(Entry {
            path: f.path.clone(),
            change: ProposalChange::Removes { id: id.clone() },
            record_id: Some(id.clone()),
            doc: Some(m.doc),
            proposed: None,
            stale: f.stale,
            diagnostics: Vec::new(),
        });
    }

    validate_proposed_state(&mut entries, accepted, &touched, config, registry);

    for e in &mut entries {
        diag::normalize(&mut e.diagnostics);
        docs.conn()
            .prepare_cached(
                "INSERT INTO snapshot_proposals
                   (snapshot, path, record_id, change, doc, stale, diagnostics)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?
            .execute(params![
                snapshot,
                e.path,
                e.record_id,
                serde_json::to_string(&e.change)?,
                e.doc,
                e.stale,
                serde_json::to_string(&e.diagnostics)?
            ])?;
    }
    Ok(Applied {
        entries: entries.len(),
        reused_docs,
        warnings,
    })
}

/// Validate the knowledge base as it would be with the proposals merged (accepted files
/// at touched paths replaced) and attach findings on proposal paths to their entries.
fn validate_proposed_state(
    entries: &mut [Entry],
    accepted: &[Member],
    touched: &HashSet<&str>,
    config: &ProfileConfig,
    registry: &Registry,
) {
    if entries.iter().all(|e| e.proposed.is_none()) {
        return;
    }
    let mut merged: Vec<MetaInput> = accepted
        .iter()
        .filter(|m| !touched.contains(m.path.as_str()))
        .filter_map(|m| {
            Some(MetaInput {
                path: m.path.clone(),
                meta: m.meta.clone()?,
            })
        })
        .collect();
    merged.extend(entries.iter().filter_map(|e| {
        Some(MetaInput {
            path: e.path.clone(),
            meta: e.proposed.clone()?,
        })
    }));
    let mut by_path: BTreeMap<String, Vec<Diagnostic>> = BTreeMap::new();
    for d in validate_metas(config, registry, &merged) {
        if let Some(p) = &d.path {
            by_path.entry(p.clone()).or_default().push(d);
        }
    }
    for e in entries.iter_mut().filter(|e| e.proposed.is_some()) {
        if let Some(found) = by_path.remove(&e.path) {
            e.diagnostics.extend(found);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_membership_is_segment_aware() {
        let roots = vec!["project/knowledge".to_string()];
        assert!(under_roots("project/knowledge/a.md", &roots));
        assert!(under_roots("project/knowledge/x/b.md", &roots));
        assert!(!under_roots("project/knowledge-old/a.md", &roots));
        assert!(!under_roots("project/registry/owners.toml", &roots));
        assert!(!under_roots("project/knowledge", &roots));
        assert!(is_documentation("project/knowledge/README.md"));
        assert!(!is_documentation("project/knowledge/a.md"));
    }
}
