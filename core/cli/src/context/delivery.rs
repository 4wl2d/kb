//! Pure delivery reuse. Applicability and dependencies are recomputed before reuse.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Unit, UnitBody, canonical_json};
use crate::knowledge::Origin;
use crate::model::{Delivery, Record};
use crate::util::sha256_hex;

#[derive(Debug, Clone, Default)]
pub struct DeliveryState {
    pub since_receipt: Option<String>,
    pub core_receipt: Option<String>,
    pub previous: BTreeMap<String, String>,
    pub core: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reuse {
    Unchanged,
    Core,
}

impl Reuse {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Core => "core",
        }
    }
}

pub fn record_digest(record: &Record) -> String {
    sha256_hex(canonical_json(&json!({"record":record})).as_bytes())
}

pub fn unit_digest(unit: &Unit) -> String {
    let value = match &unit.body {
        UnitBody::Record(p) => return record_digest(&p.record),
        UnitBody::Section(s) => json!({"section":s}),
        UnitBody::Code(c) => json!({"code":c}),
        UnitBody::Removal => json!({"removed":unit.record_id}),
    };
    sha256_hex(canonical_json(&value).as_bytes())
}

pub fn annotate(unit: &mut Unit, state: &DeliveryState, versioned: bool) {
    if !versioned && state.since_receipt.is_none() && state.core_receipt.is_none() {
        return;
    }
    let digest = unit_digest(unit);
    let always =
        matches!(&unit.body, UnitBody::Record(p) if p.record.delivery() == Delivery::Always);
    unit.reuse =
        if always && unit.origin == Origin::Accepted && state.core.get(&unit.id) == Some(&digest) {
            Some(Reuse::Core)
        } else if state.previous.get(&unit.id) == Some(&digest) {
            Some(Reuse::Unchanged)
        } else {
            None
        };
    unit.content_digest = Some(digest);
}
