//! Routing fixtures (`<profile>/routing-tests/*.toml`): golden context requests with
//! expected mandatory ids and forbidden irrelevant ids.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::profile::BudgetUnit;
use crate::model::record::Intent;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingTestFile {
    /// Document schema version (must be 1).
    #[schemars(extend("const" = 1))]
    pub schema: u32,
    #[serde(default)]
    pub case: Vec<RoutingCase>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingCase {
    pub name: String,
    pub intent: Intent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default)]
    pub repos: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub modules: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub concepts: Vec<String>,
    /// `repo=version` host versions.
    #[serde(default)]
    pub host_versions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_unit: Option<BudgetUnit>,
    /// Ids that must be in the mandatory tier.
    #[serde(default)]
    pub expect_mandatory: Vec<String>,
    /// Ids that must be included in any tier.
    #[serde(default)]
    pub expect_included: Vec<String>,
    /// Ids that must not appear at all.
    #[serde(default)]
    pub forbid: Vec<String>,
    /// Expected completeness status (`complete`, `partial`, `conflict`, `incomplete`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect_status: Option<String>,
    /// Normalized phrases expected to be reported as ambiguous.
    #[serde(default)]
    pub expect_ambiguous: Vec<String>,
    /// Expect CONTEXT_BUDGET_EXCEEDED.
    #[serde(default)]
    pub expect_budget_exceeded: bool,
}
