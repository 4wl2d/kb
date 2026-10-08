//! Versioned out-of-process code-intelligence protocol. Facts stay separate from knowledge.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum CodeProtocol {
    #[serde(rename = "kb.code.v1")]
    V1,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum CodeOperation {
    Symbols,
    Refs,
    Dependents,
    Similar,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeRequest {
    pub protocol: CodeProtocol,
    pub operation: CodeOperation,
    pub root: String,
    pub repo: String,
    pub commit: String,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub identifiers: Vec<String>,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default = "default_depth")]
    pub depth: u32,
}

fn default_depth() -> u32 {
    2
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeTool {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CodeExtent {
    Definition,
    Line,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeSymbol {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub extent: CodeExtent,
    /// SHA-256 of the exact inclusive source span, including original line endings.
    pub sha256: String,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub test: bool,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum CodeRelation {
    Call,
    Import,
    Reference,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum CodeConfidence {
    Resolved,
    Possible,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeRef {
    pub from: String,
    pub to: String,
    pub kind: CodeRelation,
    pub confidence: CodeConfidence,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeSimilar {
    pub symbol: String,
    pub score: u32,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeResponse {
    pub protocol: CodeProtocol,
    pub repo: String,
    pub commit: String,
    pub tool: CodeTool,
    pub capabilities: Vec<CodeOperation>,
    /// Complete within the declared static query scope, not runtime reachability.
    pub complete: bool,
    #[serde(default)]
    pub limitations: Vec<String>,
    pub symbols: Vec<CodeSymbol>,
    pub refs: Vec<CodeRef>,
    #[serde(default)]
    pub similar: Vec<CodeSimilar>,
}
