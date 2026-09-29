//! Diagnostics: non-fatal or aggregated findings with stable codes.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// A single finding. `code` is a stable SCREAMING_SNAKE identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub message: String,
}

impl Diagnostic {
    pub fn new(severity: Severity, code: &str, message: impl Into<String>) -> Self {
        Diagnostic {
            severity,
            code: code.to_string(),
            path: None,
            record: None,
            line: None,
            message: message.into(),
        }
    }

    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Diagnostic::new(Severity::Error, code, message)
    }

    pub fn warning(code: &str, message: impl Into<String>) -> Self {
        Diagnostic::new(Severity::Warning, code, message)
    }

    pub fn info(code: &str, message: impl Into<String>) -> Self {
        Diagnostic::new(Severity::Info, code, message)
    }

    pub fn at_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn for_record(mut self, id: impl Into<String>) -> Self {
        self.record = Some(id.into());
        self
    }

    pub fn at_line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Sort diagnostics deterministically (severity, path, record, line, code, message) and
/// remove exact duplicates.
pub fn normalize(diags: &mut Vec<Diagnostic>) {
    diags.sort_by(|a, b| {
        (a.severity, &a.path, &a.record, a.line, &a.code, &a.message)
            .cmp(&(b.severity, &b.path, &b.record, b.line, &b.code, &b.message))
    });
    diags.dedup();
}

pub fn has_errors(diags: &[Diagnostic]) -> bool {
    diags.iter().any(Diagnostic::is_error)
}
