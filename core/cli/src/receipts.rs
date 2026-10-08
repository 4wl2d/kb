//! Private local receipt storage. No clock, network, session inference or automatic reuse.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::{ContextResult, canonical_json, receipt_id_of, to_json};
use crate::env::Env;
use crate::error::{ErrorCode, KbError, Result};
use crate::util::{FieldHasher, atomic_write, read_file_limited, safe_join};

const MAX_RECEIPT_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    schema: u32,
    subject: String,
    result: Value,
}

pub fn subject(env: &Env, host: Option<&Path>, profile: &str) -> String {
    let canonical = |p: &Path| {
        p.canonicalize()
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .into_owned()
    };
    let mut h = FieldHasher::new();
    h.field("kb-receipt-subject/1")
        .field(canonical(&env.kb_root))
        .field(profile)
        .field(host.map(canonical).unwrap_or_default());
    h.finish_hex()
}

fn hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn path(cache: &Path, id: &str, subject: &str) -> Result<PathBuf> {
    if !hash(subject) {
        return Err(KbError::invalid_input("invalid receipt subject"));
    }
    let hex = id
        .strip_prefix("sha256:")
        .filter(|s| hash(s))
        .ok_or_else(|| {
            KbError::invalid_input("receipt id must be sha256:<64 lowercase hex digits>")
        })?;
    safe_join(cache, &format!("context-receipts/{subject}/{hex}.json"))
}

pub fn load(cache: &Path, id: &str, subject: &str) -> Result<BTreeMap<String, String>> {
    let path = path(cache, id, subject)?;
    if !path.is_file() {
        return Err(KbError::new(ErrorCode::NotFound, "previous receipt is not available in this local cache")
            .with_hint("request full context without --since-receipt; never omit knowledge based on an unavailable receipt"));
    }
    let stored: Stored = serde_json::from_slice(&read_file_limited(&path, MAX_RECEIPT_BYTES)?)
        .map_err(|e| KbError::invalid_input(format!("invalid stored receipt: {e}")))?;
    if stored.schema != 1
        || stored.subject != subject
        || stored.result["receipt"]["id"].as_str() != Some(id)
        || receipt_id_of(&stored.result) != id
        || stored.result["receipt"]["protocol"] != "kb.receipt.v2"
        || stored.result["receipt"]["snapshot_content_digest"]
            != stored.result["snapshot"]["content_digest"]
    {
        return Err(KbError::invalid_input(
            "receipt is corrupt, unsupported, or belongs to a different KB/host/profile",
        )
        .with_hint("request full context without --since-receipt"));
    }
    let units = stored.result["units"]
        .as_array()
        .ok_or_else(|| KbError::invalid_input("receipt has no unit list"))?;
    let mut result = BTreeMap::new();
    for unit in units {
        let id = unit["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| KbError::invalid_input("invalid receipt unit id"))?;
        let digest = unit["content_sha256"]
            .as_str()
            .filter(|s| hash(s))
            .ok_or_else(|| KbError::invalid_input("receipt unit has no valid content hash"))?;
        if result.insert(id.to_string(), digest.to_string()).is_some() {
            return Err(KbError::invalid_input("duplicate receipt unit id"));
        }
    }
    Ok(result)
}

pub fn save(cache: &Path, subject: &str, result: &ContextResult) -> Result<()> {
    if result.footer.snapshot_digest.is_none() {
        return Ok(());
    }
    let stored = Stored {
        schema: 1,
        subject: subject.into(),
        result: to_json(result, false),
    };
    let bytes = (canonical_json(&serde_json::to_value(stored)?) + "\n").into_bytes();
    if bytes.len() as u64 > MAX_RECEIPT_BYTES {
        return Err(KbError::invalid_input(
            "receipt exceeds the 32 MiB storage limit",
        ));
    }
    let path = path(cache, result.receipt_id(), subject)?;
    if path.exists() {
        if read_file_limited(&path, MAX_RECEIPT_BYTES)? == bytes {
            return Ok(());
        }
        return Err(KbError::new(
            ErrorCode::Conflict,
            "existing receipt differs; preserving it without overwriting",
        ));
    }
    atomic_write(&path, &bytes)
}
