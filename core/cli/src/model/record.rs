//! Canonical typed record model (document schema 1). See docs/architecture.md §3.
//!
//! Every kind is a separate struct with `deny_unknown_fields`; the common fields are
//! expanded by `record_struct!` so that the strict parser, the generated JSON Schema and
//! serialization share one definition.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Constants referenced by `#[schemars(...)]` attributes so that the generated JSON Schema
// states the same limits as the strict parser (`parse::check_record`).
use crate::model::ids::{
    COMMIT_PATTERN, LOCAL_ID_PATTERN, MAX_LOCAL_ID, MAX_RECORD_ID, NON_BLANK_PATTERN,
    OVERRIDE_TARGET_PATTERN, RECORD_ID_PATTERN, SINGLE_LINE_PATTERN,
};
use crate::parse::{MAX_LIST_LEN, MAX_TEXT_BYTES, MAX_TITLE_CHARS};

// ---------------------------------------------------------------------------------------
// Enumerations
// ---------------------------------------------------------------------------------------

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Policy,
    Feature,
    Invariant,
    Contract,
    Decision,
    Procedure,
    Reference,
    Gap,
}

impl Kind {
    pub const ALL: [Kind; 8] = [
        Kind::Policy,
        Kind::Feature,
        Kind::Invariant,
        Kind::Contract,
        Kind::Decision,
        Kind::Procedure,
        Kind::Reference,
        Kind::Gap,
    ];

    /// Kinds whose applicable accepted records are mandatory context.
    pub const MANDATORY: [Kind; 4] = [Kind::Policy, Kind::Invariant, Kind::Contract, Kind::Gap];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Policy => "policy",
            Kind::Feature => "feature",
            Kind::Invariant => "invariant",
            Kind::Contract => "contract",
            Kind::Decision => "decision",
            Kind::Procedure => "procedure",
            Kind::Reference => "reference",
            Kind::Gap => "gap",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    pub fn is_mandatory(self) -> bool {
        Kind::MANDATORY.contains(&self)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Draft,
    Accepted,
    Deprecated,
    Superseded,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Draft => "draft",
            Status::Accepted => "accepted",
            Status::Deprecated => "deprecated",
            Status::Superseded => "superseded",
        }
    }
}

/// Normative strength of a statement (RFC 2119 style).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    Must,
    MustNot,
    Should,
    ShouldNot,
    May,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Must => "must",
            Level::MustNot => "must-not",
            Level::Should => "should",
            Level::ShouldNot => "should-not",
            Level::May => "may",
        }
    }
    /// Uppercase keyword used in rendered output.
    pub fn keyword(self) -> &'static str {
        match self {
            Level::Must => "MUST",
            Level::MustNot => "MUST NOT",
            Level::Should => "SHOULD",
            Level::ShouldNot => "SHOULD NOT",
            Level::May => "MAY",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Intent {
    Implement,
    Refactor,
    Debug,
    Review,
    Explain,
}

impl Intent {
    pub const ALL: [Intent; 5] = [
        Intent::Implement,
        Intent::Refactor,
        Intent::Debug,
        Intent::Review,
        Intent::Explain,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Intent::Implement => "implement",
            Intent::Refactor => "refactor",
            Intent::Debug => "debug",
            Intent::Review => "review",
            Intent::Explain => "explain",
        }
    }
    pub fn parse(s: &str) -> Option<Intent> {
        Intent::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

// ---------------------------------------------------------------------------------------
// Common structures
// ---------------------------------------------------------------------------------------

/// Mandatory applicability. AND across dimensions, OR within a dimension.
///
/// `product = true` XOR at least one non-empty dimension (an accidental empty scope is an
/// error). Registry existence and satisfiability are checked by `validate`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend(
    "if" = {"properties": {"product": {"const": true}}, "required": ["product"]},
    "then" = {"properties": {
        "repos": {"maxItems": 0},
        "modules": {"maxItems": 0},
        "features": {"maxItems": 0}
    }},
    "else" = {"anyOf": [
        {"properties": {"repos": {"minItems": 1}}, "required": ["repos"]},
        {"properties": {"modules": {"minItems": 1}}, "required": ["modules"]},
        {"properties": {"features": {"minItems": 1}}, "required": ["features"]}
    ]}
))]
pub struct Scope {
    /// Product-wide applicability. When true, all other dimensions must be empty.
    #[serde(default)]
    pub product: bool,
    /// Registry repo ids.
    #[serde(default)]
    #[schemars(extend("uniqueItems" = true), length(max = MAX_LIST_LEN))]
    pub repos: Vec<String>,
    /// Registry module ids.
    #[serde(default)]
    #[schemars(extend("uniqueItems" = true), length(max = MAX_LIST_LEN))]
    pub modules: Vec<String>,
    /// Registry feature ids.
    #[serde(default)]
    #[schemars(extend("uniqueItems" = true), length(max = MAX_LIST_LEN))]
    pub features: Vec<String>,
}

impl Scope {
    pub fn is_unconstrained(&self) -> bool {
        self.repos.is_empty() && self.modules.is_empty() && self.features.is_empty()
    }
}

/// Relevance signals. Never exclude an applicable obligation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Selectors {
    /// Repo-relative globs, optionally qualified as `repo:glob`.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Registry concept ids.
    #[serde(default)]
    #[schemars(extend("uniqueItems" = true), length(max = MAX_LIST_LEN))]
    pub concepts: Vec<String>,
    #[serde(default)]
    pub intents: Vec<Intent>,
    /// Extra query phrases (normalized per docs/architecture.md §4.1).
    #[serde(default)]
    #[schemars(extend("uniqueItems" = true), length(max = MAX_LIST_LEN))]
    pub aliases: Vec<String>,
}

impl Selectors {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
            && self.concepts.is_empty()
            && self.intents.is_empty()
            && self.aliases.is_empty()
    }
}

/// Typed relationships between records.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Links {
    /// Mandatory transitive dependencies (acyclic).
    #[serde(default)]
    #[schemars(
        extend("uniqueItems" = true),
        length(max = MAX_LIST_LEN),
        inner(regex(pattern = RECORD_ID_PATTERN), length(max = MAX_RECORD_ID))
    )]
    pub requires: Vec<String>,
    /// Explanations (usually decisions); optional context, not followed transitively.
    #[serde(default)]
    #[schemars(
        extend("uniqueItems" = true),
        length(max = MAX_LIST_LEN),
        inner(regex(pattern = RECORD_ID_PATTERN), length(max = MAX_RECORD_ID))
    )]
    pub rationale: Vec<String>,
    /// One-hop suggestions; never followed transitively.
    #[serde(default)]
    #[schemars(
        extend("uniqueItems" = true),
        length(max = MAX_LIST_LEN),
        inner(regex(pattern = RECORD_ID_PATTERN), length(max = MAX_RECORD_ID))
    )]
    pub related: Vec<String>,
    /// Records this one replaces (acyclic; targets must have status `superseded`).
    #[serde(default)]
    #[schemars(
        extend("uniqueItems" = true),
        length(max = MAX_LIST_LEN),
        inner(regex(pattern = RECORD_ID_PATTERN), length(max = MAX_RECORD_ID))
    )]
    pub supersedes: Vec<String>,
}

impl Links {
    pub fn is_empty(&self) -> bool {
        self.requires.is_empty()
            && self.rationale.is_empty()
            && self.related.is_empty()
            && self.supersedes.is_empty()
    }
}

/// Version applicability per registry repo (semver requirement strings).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Applicability {
    #[serde(default)]
    pub versions: BTreeMap<String, String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum AnchorKind {
    Source,
    Test,
    Change,
    Doc,
}

/// Provenance anchor: binds knowledge to source, tests, docs or reviewed changes.
///
/// `source`/`test` anchors need `repo` and `path`, `change` anchors need `change` or
/// `commit`, `doc` anchors need `path`. A test anchor does not prove that the test runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend("allOf" = [
    {
        "if": {"properties": {"kind": {"enum": ["source", "test"]}}},
        "then": {"required": ["repo", "path"]}
    },
    {
        "if": {"properties": {"kind": {"const": "change"}}},
        "then": {"anyOf": [{"required": ["change"]}, {"required": ["commit"]}]}
    },
    {
        "if": {"properties": {"kind": {"const": "doc"}}},
        "then": {"required": ["path"]}
    }
]))]
pub struct Anchor {
    pub kind: AnchorKind,
    /// Registry repo id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Repo-relative path (no `..`, not absolute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// Commit id, 7..=64 hex characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = COMMIT_PATTERN))]
    pub commit: Option<String>,
    /// Reviewed change reference, e.g. `"!42"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// An exception to a statement. Exceptions are part of normative content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Exception {
    /// Local id, unique within the statement.
    #[schemars(regex(pattern = LOCAL_ID_PATTERN), length(max = MAX_LOCAL_ID))]
    pub id: String,
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    pub text: String,
}

/// An atomic normative statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Statement {
    /// Local id, unique within the record.
    #[schemars(regex(pattern = LOCAL_ID_PATTERN), length(max = MAX_LOCAL_ID))]
    pub id: String,
    pub level: Level,
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    pub text: String,
    /// Conditions under which the statement applies (all must hold).
    #[serde(default)]
    #[schemars(
        length(max = MAX_LIST_LEN),
        inner(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))
    )]
    pub conditions: Vec<String>,
    #[serde(default)]
    pub exceptions: Vec<Exception>,
}

/// A contract obligation: a statement bound to a declared party.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Obligation {
    /// Local id, unique within the record.
    #[schemars(regex(pattern = LOCAL_ID_PATTERN), length(max = MAX_LOCAL_ID))]
    pub id: String,
    /// Id of a declared party of the same contract.
    pub party: String,
    pub level: Level,
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    pub text: String,
    #[serde(default)]
    #[schemars(
        length(max = MAX_LIST_LEN),
        inner(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))
    )]
    pub conditions: Vec<String>,
    #[serde(default)]
    pub exceptions: Vec<Exception>,
}

/// A simple identified text item (behaviors, boundaries, steps).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Item {
    /// Local id, unique within the record.
    #[schemars(regex(pattern = LOCAL_ID_PATTERN), length(max = MAX_LOCAL_ID))]
    pub id: String,
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    pub text: String,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum SettingType {
    Integer,
    Boolean,
    String,
    StringSet,
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum OverrideMode {
    #[default]
    Forbidden,
    Stricter,
    Any,
}

/// Direction in which a setting value becomes stricter.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Stricter {
    Lower,
    Higher,
    True,
    False,
    Superset,
    Subset,
}

/// A setting value. TOML integers, booleans, strings and string arrays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SettingValue {
    Boolean(bool),
    Integer(i64),
    String(String),
    StringSet(Vec<String>),
}

impl SettingValue {
    pub fn type_of(&self) -> SettingType {
        match self {
            SettingValue::Boolean(_) => SettingType::Boolean,
            SettingValue::Integer(_) => SettingType::Integer,
            SettingValue::String(_) => SettingType::String,
            SettingValue::StringSet(_) => SettingType::StringSet,
        }
    }

    /// Deterministic display form.
    pub fn display(&self) -> String {
        match self {
            SettingValue::Boolean(b) => b.to_string(),
            SettingValue::Integer(i) => i.to_string(),
            SettingValue::String(s) => format!("{s:?}"),
            SettingValue::StringSet(v) => {
                let mut v = v.clone();
                v.sort();
                format!(
                    "[{}]",
                    v.iter()
                        .map(|s| format!("{s:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
    }
}

/// A named, typed policy setting that may be overridable.
///
/// `value` must have the declared `type`; `stricter` is required iff
/// `override = "stricter"` and must fit the type (integer: lower/higher, boolean:
/// true/false, string-set: superset/subset; strings have no stricter direction);
/// `override_owners` is only meaningful for overridable settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend("allOf" = [
    {
        "if": {"properties": {"type": {"const": "integer"}}},
        "then": {"properties": {
            "value": {"type": "integer"},
            "stricter": {"enum": ["lower", "higher"]}
        }}
    },
    {
        "if": {"properties": {"type": {"const": "boolean"}}},
        "then": {"properties": {
            "value": {"type": "boolean"},
            "stricter": {"enum": ["true", "false"]}
        }}
    },
    {
        "if": {"properties": {"type": {"const": "string"}}},
        "then": {"properties": {"value": {"type": "string"}, "stricter": false}}
    },
    {
        "if": {"properties": {"type": {"const": "string-set"}}},
        "then": {"properties": {
            "value": {"type": "array", "uniqueItems": true, "maxItems": MAX_LIST_LEN},
            "stricter": {"enum": ["superset", "subset"]}
        }}
    },
    {
        "if": {"properties": {"override": {"const": "stricter"}}, "required": ["override"]},
        "then": {"required": ["stricter"]},
        "else": {"not": {"required": ["stricter"]}}
    },
    {
        "if": {"properties": {"override": {"enum": ["stricter", "any"]}}, "required": ["override"]},
        "else": {"properties": {"override_owners": {"maxItems": 0}}}
    }
]))]
pub struct Setting {
    /// Setting name, unique within the policy.
    #[schemars(regex(pattern = LOCAL_ID_PATTERN), length(max = MAX_LOCAL_ID))]
    pub name: String,
    #[serde(rename = "type")]
    pub value_type: SettingType,
    pub value: SettingValue,
    #[serde(default)]
    pub r#override: OverrideMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stricter: Option<Stricter>,
    /// Owner ids allowed to override (empty: any owner).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub override_owners: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// An override of another policy's named setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Override {
    /// `<policy-id>#<setting-name>`
    #[schemars(regex(pattern = OVERRIDE_TARGET_PATTERN))]
    pub target: String,
    pub value: SettingValue,
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    pub reason: String,
}

impl Override {
    /// Split `target` into (record id, setting name).
    pub fn split_target(&self) -> Option<(&str, &str)> {
        let (id, name) = self.target.split_once('#')?;
        if id.is_empty() || name.is_empty() {
            return None;
        }
        Some((id, name))
    }
}

/// A contract party: a registry repo, optionally narrowed to some of its modules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Party {
    /// Local id, unique within the contract.
    #[schemars(regex(pattern = LOCAL_ID_PATTERN), length(max = MAX_LOCAL_ID))]
    pub id: String,
    /// Registry repo id.
    pub repo: String,
    /// Registry module ids (must belong to `repo`).
    #[serde(default)]
    pub modules: Vec<String>,
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Alternative {
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    pub option: String,
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    pub rejected_because: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum GapKind {
    Missing,
    Ambiguity,
    Contradiction,
}

// ---------------------------------------------------------------------------------------
// Per-kind records
// ---------------------------------------------------------------------------------------

macro_rules! kind_literal {
    ($name:ident, $lit:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
        pub enum $name {
            #[serde(rename = $lit)]
            Value,
        }
    };
}

kind_literal!(PolicyKind, "policy");
kind_literal!(FeatureKind, "feature");
kind_literal!(InvariantKind, "invariant");
kind_literal!(ContractKind, "contract");
kind_literal!(DecisionKind, "decision");
kind_literal!(ProcedureKind, "procedure");
kind_literal!(ReferenceKind, "reference");
kind_literal!(GapKindLiteral, "gap");

// Struct-level attributes (`$m`) are emitted after the derive so that `#[schemars(...)]`
// helper attributes can be passed through.
macro_rules! record_struct {
    ($(#[$m:meta])* $name:ident, $kindty:ident { $($(#[$fattr:meta])* $field:ident : $fty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        $(#[$m])*
        pub struct $name {
            /// Document schema version (must be 1).
            #[schemars(extend("const" = 1))]
            pub schema: u32,
            /// Stable namespaced id; the first segment is the profile namespace.
            #[schemars(regex(pattern = RECORD_ID_PATTERN), length(max = MAX_RECORD_ID))]
            pub id: String,
            pub kind: $kindty,
            /// Single line of 1..=200 characters.
            #[schemars(regex(pattern = SINGLE_LINE_PATTERN), length(min = 1, max = MAX_TITLE_CHARS))]
            pub title: String,
            pub status: Status,
            /// Registry owner id.
            #[schemars(regex(pattern = NON_BLANK_PATTERN))]
            pub owner: String,
            pub scope: Scope,
            #[serde(default, skip_serializing_if = "Selectors::is_empty")]
            pub selectors: Selectors,
            #[serde(default, skip_serializing_if = "Links::is_empty")]
            pub links: Links,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub applicability: Option<Applicability>,
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            pub anchors: Vec<Anchor>,
            $($(#[$fattr])* pub $field: $fty,)*
        }
    };
}

record_struct!(
    /// Mandatory rules and/or explicitly overridable named settings (at least one of
    /// `rules`, `settings`, `overrides` is non-empty).
    #[schemars(extend("anyOf" = [
        {"properties": {"rules": {"minItems": 1}}, "required": ["rules"]},
        {"properties": {"settings": {"minItems": 1}}, "required": ["settings"]},
        {"properties": {"overrides": {"minItems": 1}}, "required": ["overrides"]}
    ]))]
    PolicyRecord, PolicyKind {
    #[serde(default)] #[schemars(length(max = MAX_LIST_LEN))] rules: Vec<Statement>,
    #[serde(default)] settings: Vec<Setting>,
    #[serde(default)] overrides: Vec<Override>,
});

record_struct!(
    /// Behavior and boundaries of a registry feature.
    FeatureRecord, FeatureKind {
    /// Registry feature id.
    feature: String,
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    summary: String,
    #[schemars(length(min = 1, max = MAX_LIST_LEN))]
    behaviors: Vec<Item>,
    #[serde(default)] #[schemars(length(max = MAX_LIST_LEN))] boundaries: Vec<Item>,
});

record_struct!(
    /// A condition preserved across changes.
    InvariantRecord, InvariantKind {
    #[schemars(length(min = 1, max = MAX_LIST_LEN))]
    statements: Vec<Statement>,
});

record_struct!(
    /// Obligations between modules or repositories.
    ContractRecord, ContractKind {
    #[schemars(length(min = 2))]
    parties: Vec<Party>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    interface: Option<String>,
    #[schemars(length(min = 1))]
    obligations: Vec<Obligation>,
});

record_struct!(
    /// A decision with reasons, alternatives and consequences.
    DecisionRecord, DecisionKind {
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    context: String,
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    decision: String,
    #[schemars(
        length(min = 1, max = MAX_LIST_LEN),
        inner(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))
    )]
    reasons: Vec<String>,
    #[serde(default)] alternatives: Vec<Alternative>,
    #[serde(default)]
    #[schemars(
        length(max = MAX_LIST_LEN),
        inner(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))
    )]
    consequences: Vec<String>,
});

record_struct!(
    /// Preconditions, steps and expected result. Never executed by kb.
    ProcedureRecord, ProcedureKind {
    #[serde(default)]
    #[schemars(
        length(max = MAX_LIST_LEN),
        inner(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))
    )]
    preconditions: Vec<String>,
    #[schemars(length(min = 1, max = MAX_LIST_LEN))]
    steps: Vec<Item>,
    #[schemars(
        length(min = 1, max = MAX_LIST_LEN),
        inner(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))
    )]
    expected: Vec<String>,
});

record_struct!(
    /// Explanatory information and sources.
    ReferenceRecord, ReferenceKind {
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    summary: String,
    #[serde(default)] sources: Vec<SourceRef>,
});

record_struct!(
    /// A known gap, ambiguity or contradiction.
    GapRecord, GapKindLiteral {
    gap: GapKind,
    #[schemars(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))]
    description: String,
    /// Ids of records affected by the gap (must exist).
    #[serde(default)]
    #[schemars(
        extend("uniqueItems" = true),
        length(max = MAX_LIST_LEN),
        inner(regex(pattern = RECORD_ID_PATTERN), length(max = MAX_RECORD_ID))
    )]
    affects: Vec<String>,
    #[serde(default)]
    #[schemars(
        length(max = MAX_LIST_LEN),
        inner(regex(pattern = NON_BLANK_PATTERN), length(max = MAX_TEXT_BYTES))
    )]
    questions: Vec<String>,
});

/// Any record. Serialized without an extra tag (each variant carries `kind`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Record {
    Policy(PolicyRecord),
    Feature(FeatureRecord),
    Invariant(InvariantRecord),
    Contract(ContractRecord),
    Decision(DecisionRecord),
    Procedure(ProcedureRecord),
    Reference(ReferenceRecord),
    Gap(GapRecord),
}

/// Borrowed view of the common fields.
#[derive(Debug, Clone, Copy)]
pub struct Common<'a> {
    pub schema: u32,
    pub id: &'a str,
    pub kind: Kind,
    pub title: &'a str,
    pub status: Status,
    pub owner: &'a str,
    pub scope: &'a Scope,
    pub selectors: &'a Selectors,
    pub links: &'a Links,
    pub applicability: Option<&'a Applicability>,
    pub anchors: &'a [Anchor],
}

macro_rules! common_of {
    ($r:expr, $kind:expr) => {
        Common {
            schema: $r.schema,
            id: &$r.id,
            kind: $kind,
            title: &$r.title,
            status: $r.status,
            owner: &$r.owner,
            scope: &$r.scope,
            selectors: &$r.selectors,
            links: &$r.links,
            applicability: $r.applicability.as_ref(),
            anchors: &$r.anchors,
        }
    };
}

impl Record {
    pub fn common(&self) -> Common<'_> {
        match self {
            Record::Policy(r) => common_of!(r, Kind::Policy),
            Record::Feature(r) => common_of!(r, Kind::Feature),
            Record::Invariant(r) => common_of!(r, Kind::Invariant),
            Record::Contract(r) => common_of!(r, Kind::Contract),
            Record::Decision(r) => common_of!(r, Kind::Decision),
            Record::Procedure(r) => common_of!(r, Kind::Procedure),
            Record::Reference(r) => common_of!(r, Kind::Reference),
            Record::Gap(r) => common_of!(r, Kind::Gap),
        }
    }

    pub fn id(&self) -> &str {
        self.common().id
    }

    pub fn kind(&self) -> Kind {
        self.common().kind
    }

    pub fn status(&self) -> Status {
        self.common().status
    }

    /// Lightweight metadata used for routing, validation and indexing.
    pub fn meta(&self, sections: Vec<String>) -> RecordMeta {
        let c = self.common();
        let mut meta = RecordMeta {
            id: c.id.to_string(),
            kind: c.kind,
            status: c.status,
            title: c.title.to_string(),
            owner: c.owner.to_string(),
            scope: c.scope.clone(),
            selectors: c.selectors.clone(),
            links: c.links.clone(),
            applicability: c.applicability.cloned(),
            anchors: c.anchors.to_vec(),
            settings: Vec::new(),
            overrides: Vec::new(),
            parties: Vec::new(),
            feature: None,
            gap_affects: Vec::new(),
            sections,
        };
        match self {
            Record::Policy(p) => {
                meta.settings = p.settings.clone();
                meta.overrides = p.overrides.clone();
            }
            Record::Contract(k) => meta.parties = k.parties.clone(),
            Record::Feature(f) => meta.feature = Some(f.feature.clone()),
            Record::Gap(g) => meta.gap_affects = g.affects.clone(),
            _ => {}
        }
        meta
    }

    /// All normative statements (rules, invariant statements, obligations) as
    /// (statement id, level, text, conditions, exceptions, party).
    pub fn normative(&self) -> Vec<NormativeRef<'_>> {
        match self {
            Record::Policy(p) => p.rules.iter().map(NormativeRef::from_statement).collect(),
            Record::Invariant(i) => i
                .statements
                .iter()
                .map(NormativeRef::from_statement)
                .collect(),
            Record::Contract(k) => k
                .obligations
                .iter()
                .map(|o| NormativeRef {
                    id: &o.id,
                    level: o.level,
                    text: &o.text,
                    conditions: &o.conditions,
                    exceptions: &o.exceptions,
                    party: Some(&o.party),
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NormativeRef<'a> {
    pub id: &'a str,
    pub level: Level,
    pub text: &'a str,
    pub conditions: &'a [String],
    pub exceptions: &'a [Exception],
    pub party: Option<&'a str>,
}

impl<'a> NormativeRef<'a> {
    fn from_statement(s: &'a Statement) -> Self {
        NormativeRef {
            id: &s.id,
            level: s.level,
            text: &s.text,
            conditions: &s.conditions,
            exceptions: &s.exceptions,
            party: None,
        }
    }
}

/// Metadata needed for routing, cross-record validation and impact analysis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordMeta {
    pub id: String,
    pub kind: Kind,
    pub status: Status,
    pub title: String,
    pub owner: String,
    pub scope: Scope,
    #[serde(default)]
    pub selectors: Selectors,
    #[serde(default)]
    pub links: Links,
    #[serde(default)]
    pub applicability: Option<Applicability>,
    #[serde(default)]
    pub anchors: Vec<Anchor>,
    #[serde(default)]
    pub settings: Vec<Setting>,
    #[serde(default)]
    pub overrides: Vec<Override>,
    #[serde(default)]
    pub parties: Vec<Party>,
    #[serde(default)]
    pub feature: Option<String>,
    #[serde(default)]
    pub gap_affects: Vec<String>,
    /// Ids of optional Markdown sections available via `show`.
    #[serde(default)]
    pub sections: Vec<String>,
}

/// An optional explanatory Markdown section of a record body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    /// Slug of the heading (`intro` for text before the first `## ` heading).
    pub id: String,
    pub heading: String,
    /// Markdown text of the section, without its heading line, trimmed of leading and
    /// trailing blank lines. Authoritative bytes; never rewritten.
    pub markdown: String,
}

/// A parsed record: typed front matter plus optional sections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedRecord {
    pub record: Record,
    pub sections: Vec<Section>,
}

impl ParsedRecord {
    pub fn meta(&self) -> RecordMeta {
        self.record
            .meta(self.sections.iter().map(|s| s.id.clone()).collect())
    }

    pub fn section(&self, id: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.id == id)
    }
}
