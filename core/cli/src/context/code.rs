//! Non-normative, whole-unit code evidence supplied by an adapter.

use serde::Serialize;

use crate::model::{CodeSymbol, CodeTool};

#[derive(Debug, Clone, Serialize)]
pub struct CodeInfo {
    pub repo: String,
    pub commit: String,
    pub tool: CodeTool,
    pub complete: bool,
    pub limitations: Vec<String>,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodeEvidence {
    pub repo: String,
    pub commit: String,
    pub tool: CodeTool,
    pub symbol: CodeSymbol,
    pub role: String,
    pub reason: String,
    pub source: String,
}
