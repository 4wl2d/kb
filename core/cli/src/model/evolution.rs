//! Additive schema-2 values. These are data; no provider, Git or probe is run here.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::ids::{COMMIT_PATTERN, LOCAL_ID_PATTERN, MAX_LOCAL_ID, NON_BLANK_PATTERN};
use crate::parse::{MAX_LIST_LEN, MAX_TEXT_BYTES};

pub const DATE_PATTERN: &str = r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$";
pub const TEMPORAL_PATTERN: &str = r"^([0-9]{4}-[0-9]{2}-[0-9]{2}|[0-9a-fA-F]{7,64})$";

pub fn is_commit(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Strict Gregorian calendar date, returned as days relative to 1970-01-01.
/// The result depends only on the input, never on the process clock or timezone.
pub fn date_days(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return None;
    }
    let year = value[..4].parse::<i64>().ok()?;
    let month = value[5..7].parse::<usize>().ok()?;
    let day = value[8..].parse::<i64>().ok()?;
    if year == 0 || !(1..=12).contains(&month) {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let months = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > months[month - 1] {
        return None;
    }
    let previous = year - 1;
    Some(
        previous * 365 + previous / 4 - previous / 100
            + previous / 400
            + months[..month - 1].iter().sum::<i64>()
            + day
            - 719_163,
    )
}

/// Inverse of `date_days` within the supported four-digit calendar range.
pub fn date_from_days(days: i64) -> Option<String> {
    if !(date_days("0001-01-01")?..=date_days("9999-12-31")?).contains(&days) {
        return None;
    }
    let mut lo = 1;
    let mut hi = 10_000;
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if date_days(&format!("{mid:04}-01-01"))? <= days {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let mut month = 1;
    while month < 12 && date_days(&format!("{lo:04}-{:02}-01", month + 1))? <= days {
        month += 1;
    }
    let day = days - date_days(&format!("{lo:04}-{month:02}-01"))? + 1;
    Some(format!("{lo:04}-{month:02}-{day:02}"))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Delivery {
    Always,
    #[default]
    Scoped,
}

impl Delivery {
    pub fn is_scoped(&self) -> bool {
        *self == Self::Scoped
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    #[schemars(regex(pattern = LOCAL_ID_PATTERN), length(max = MAX_LOCAL_ID))]
    pub id: String,
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    pub given: String,
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    pub expect: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    #[schemars(regex(pattern = LOCAL_ID_PATTERN), length(max = MAX_LOCAL_ID))]
    pub id: String,
    #[schemars(regex(pattern = LOCAL_ID_PATTERN), length(max = MAX_LOCAL_ID))]
    pub from: String,
    #[schemars(regex(pattern = LOCAL_ID_PATTERN), length(max = MAX_LOCAL_ID))]
    pub to: String,
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    pub when: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Consumer {
    pub repo: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Term {
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    pub term: String,
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    pub meaning: String,
    /// A record id, reviewed change, repository path or authoritative URL.
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    pub source: String,
}

/// Hash of an exact, inclusive line span in a Git blob at an immutable host commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnchorStamp {
    #[schemars(regex(pattern = COMMIT_PATTERN))]
    pub commit: String,
    #[schemars(range(min = 1))]
    pub start_line: u32,
    #[schemars(range(min = 1))]
    pub end_line: u32,
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub sha256: String,
}

/// Declarative checks, never shell commands. Regexes use the bounded Rust regex engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum VerifyProbe {
    CommitMessage {
        pattern: String,
    },
    BranchName {
        pattern: String,
    },
    ForbiddenImport {
        #[schemars(length(min = 1, max = MAX_LIST_LEN))]
        from: Vec<String>,
        #[schemars(length(min = 1, max = MAX_LIST_LEN))]
        to: Vec<String>,
    },
    Naming {
        #[schemars(length(min = 1, max = MAX_LIST_LEN))]
        paths: Vec<String>,
        pattern: String,
    },
    BannedApi {
        #[schemars(length(min = 1, max = MAX_LIST_LEN))]
        paths: Vec<String>,
        pattern: String,
    },
}
