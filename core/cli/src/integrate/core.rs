//! Small, reviewable always-on core plus explicit verification of its installed source.
use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::context::{canonical_json, delivery::record_digest, estimate_tokens};
use crate::error::{KbError, Result};
use crate::knowledge::{KnowledgeView, Origin};
use crate::model::{Delivery, Kind, Record, Status};
use crate::util::{read_file_limited, safe_join, sha256_hex};

pub const CORE_TOKEN_CAP: u64 = 600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreBundle {
    pub protocol: String,
    pub digest: String,
    pub records: BTreeMap<String, String>,
    pub lines: Vec<String>,
}

impl CoreBundle {
    pub fn text(&self) -> String {
        format!(
            "Always-on core ({}):\n{}\n",
            self.digest,
            self.lines.join("\n")
        )
    }
}

pub fn build<'a>(records: impl IntoIterator<Item = &'a Record>) -> Result<Option<CoreBundle>> {
    let mut chosen: Vec<_> = records
        .into_iter()
        .filter(|r| r.status() == Status::Accepted && r.delivery() == Delivery::Always)
        .collect();
    chosen.sort_by(|a, b| a.id().cmp(b.id()));
    if chosen.is_empty() {
        return Ok(None);
    }
    let mut hashes = BTreeMap::new();
    let mut lines = Vec::new();
    for record in chosen {
        hashes.insert(record.id().to_string(), record_digest(record));
        for rule in record.normative() {
            let mut line = format!(
                "- {}#{} {} {}",
                record.id(),
                rule.id,
                rule.level.as_str(),
                serde_json::to_string(rule.text)?
            );
            if !rule.conditions.is_empty() {
                line.push_str(&format!(
                    "; when ALL {}",
                    canonical_json(&json!(rule.conditions))
                ));
            }
            if !rule.exceptions.is_empty() {
                line.push_str(&format!(
                    "; except {}",
                    canonical_json(&json!(rule.exceptions))
                ));
            }
            if !rule.verify.is_empty() {
                line.push_str(&format!("; verify {}", canonical_json(&json!(rule.verify))));
            }
            lines.push(line);
        }
        if let Record::Policy(policy) = record {
            if !policy.settings.is_empty() {
                lines.push(format!(
                    "- {} settings {}",
                    record.id(),
                    canonical_json(&json!(policy.settings))
                ));
            }
            if !policy.overrides.is_empty() {
                lines.push(format!(
                    "- {} overrides {}",
                    record.id(),
                    canonical_json(&json!(policy.overrides))
                ));
            }
        }
    }
    let digest = format!(
        "sha256:{}",
        sha256_hex(
            canonical_json(&json!({"protocol":"kb.core.v1","records":hashes,"lines":lines}))
                .as_bytes()
        )
    );
    let bundle = CoreBundle {
        protocol: "kb.core.v1".into(),
        digest,
        records: hashes,
        lines,
    };
    let tokens = estimate_tokens(&bundle.text());
    if tokens > CORE_TOKEN_CAP {
        return Err(KbError::invalid_input(format!("always-on core needs {tokens} estimated tokens; cap is {CORE_TOKEN_CAP}"))
            .with_hint("keep fewer stable records delivery=always; the generator never truncates statements, conditions or exceptions"));
    }
    Ok(Some(bundle))
}

pub fn from_view(view: &dyn KnowledgeView) -> Result<Option<CoreBundle>> {
    let ids: Vec<_> = view
        .metas_by_kind(&[Kind::Policy, Kind::Invariant], Origin::Accepted)?
        .into_iter()
        .map(|m| m.meta.id.clone())
        .collect();
    let records = view.records(&ids, Origin::Accepted)?;
    build(records.iter().map(|r| &r.parsed.record))
}

/// The caller passes a receipt read from a loaded instruction block. Both the installed
/// source bytes and the entire selected snapshot's always-on set must still match.
pub fn verify_installed(
    kb_root: &Path,
    loc: &crate::model::ProfileLocation,
    host: &Path,
    id: &str,
    source: &str,
    view: &dyn KnowledgeView,
) -> Result<BTreeMap<String, String>> {
    let stale = || {
        KbError::invalid_input("loaded core receipt is missing, stale or different from the selected snapshot")
        .with_hint("regenerate and reinstall the integration, then re-read it; request full context without --core-receipt in the meantime")
    };
    let expected = from_view(view)?.ok_or_else(stale)?;
    if expected.digest != id {
        return Err(stale());
    }
    let bundle = super::generate::load_bundle(kb_root, loc)?;
    if host.join(&bundle.manifest.kb_path).canonicalize().ok() != kb_root.canonicalize().ok() {
        return Err(KbError::invalid_input(
            "the installed core's launcher path does not point to this KB checkout",
        ));
    }
    if bundle.manifest.core.as_ref() != Some(&expected)
        || bundle.manifest.core_source.as_deref() != Some(source)
    {
        return Err(stale());
    }
    let lock = super::install::read_lock(host)?.ok_or_else(stale)?;
    if lock.skill_protocol != crate::versions::SKILL_PROTOCOL
        || lock.core_receipt.as_deref() != Some(id)
    {
        return Err(stale());
    }
    let bytes = read_file_limited(&safe_join(host, source)?, 4 * 1024 * 1024)?;
    super::generate::check_size(source, &bytes)?;
    let layout = bundle.layout();
    if let Some(target) = layout.blocks.iter().find(|b| b.host_file == source) {
        let span = super::blocks::find_block(&bytes, super::blocks::BLOCK_NAME)
            .map_err(|_| stale())?
            .ok_or_else(stale)?;
        let actual = span.inner(&bytes);
        let wanted = bundle.files.get(target.bundle_file).ok_or_else(stale)?;
        if actual != wanted
            || !lock.blocks.iter().any(|b| {
                b.file == source
                    && b.name == super::blocks::BLOCK_NAME
                    && b.sha256 == sha256_hex(actual)
            })
        {
            return Err(stale());
        }
    } else if let Some(target) = layout.files.iter().find(|f| f.host_file == source) {
        let wanted = bundle.files.get(target.bundle_file).ok_or_else(stale)?;
        if &bytes != wanted
            || !lock
                .files
                .iter()
                .any(|f| f.path == source && f.sha256 == sha256_hex(&bytes))
        {
            return Err(stale());
        }
    } else {
        return Err(stale());
    }
    if !String::from_utf8_lossy(&bytes).contains(&expected.text()) {
        return Err(stale());
    }
    Ok(expected.records)
}
