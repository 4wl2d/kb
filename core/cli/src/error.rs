//! Stable symbolic errors and exit-code groups (see docs/architecture.md §8).

use std::fmt;

use serde::Serialize;
use serde_json::Value;

use crate::diag::Diagnostic;

/// Stable symbolic error codes. The string form and exit code are part of the CLI protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    Internal,
    Usage,
    ProjectNotInitialized,
    ConfigInvalid,
    UnsupportedSchemaVersion,
    AlreadyInitialized,
    UnknownScope,
    NotFound,
    FreshnessUnverified,
    UpdateRequired,
    SnapshotNotFound,
    SkillOutdated,
    ContextIncomplete,
    ContextBudgetExceeded,
    ValidationFailed,
    RoutingTestsFailed,
    DriftDetected,
    EngineDiverged,
    ImpactUnacknowledged,
    Conflict,
    RuntimeIncompatible,
    UpdateConflict,
    MigrationFailed,
    UpdateFailed,
    GitError,
    IndexError,
    IoError,
    UnsafePath,
    InvalidInput,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 29] = [
        ErrorCode::Internal,
        ErrorCode::Usage,
        ErrorCode::ProjectNotInitialized,
        ErrorCode::ConfigInvalid,
        ErrorCode::UnsupportedSchemaVersion,
        ErrorCode::AlreadyInitialized,
        ErrorCode::UnknownScope,
        ErrorCode::NotFound,
        ErrorCode::FreshnessUnverified,
        ErrorCode::UpdateRequired,
        ErrorCode::SnapshotNotFound,
        ErrorCode::SkillOutdated,
        ErrorCode::ContextIncomplete,
        ErrorCode::ContextBudgetExceeded,
        ErrorCode::ValidationFailed,
        ErrorCode::RoutingTestsFailed,
        ErrorCode::DriftDetected,
        ErrorCode::EngineDiverged,
        ErrorCode::ImpactUnacknowledged,
        ErrorCode::Conflict,
        ErrorCode::RuntimeIncompatible,
        ErrorCode::UpdateConflict,
        ErrorCode::MigrationFailed,
        ErrorCode::UpdateFailed,
        ErrorCode::GitError,
        ErrorCode::IndexError,
        ErrorCode::IoError,
        ErrorCode::UnsafePath,
        ErrorCode::InvalidInput,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Internal => "INTERNAL",
            ErrorCode::Usage => "USAGE",
            ErrorCode::ProjectNotInitialized => "PROJECT_NOT_INITIALIZED",
            ErrorCode::ConfigInvalid => "CONFIG_INVALID",
            ErrorCode::UnsupportedSchemaVersion => "UNSUPPORTED_SCHEMA_VERSION",
            ErrorCode::AlreadyInitialized => "ALREADY_INITIALIZED",
            ErrorCode::UnknownScope => "UNKNOWN_SCOPE",
            ErrorCode::NotFound => "NOT_FOUND",
            ErrorCode::FreshnessUnverified => "FRESHNESS_UNVERIFIED",
            ErrorCode::UpdateRequired => "UPDATE_REQUIRED",
            ErrorCode::SnapshotNotFound => "SNAPSHOT_NOT_FOUND",
            ErrorCode::SkillOutdated => "SKILL_OUTDATED",
            ErrorCode::ContextIncomplete => "CONTEXT_INCOMPLETE",
            ErrorCode::ContextBudgetExceeded => "CONTEXT_BUDGET_EXCEEDED",
            ErrorCode::ValidationFailed => "VALIDATION_FAILED",
            ErrorCode::RoutingTestsFailed => "ROUTING_TESTS_FAILED",
            ErrorCode::DriftDetected => "DRIFT_DETECTED",
            ErrorCode::EngineDiverged => "ENGINE_DIVERGED",
            ErrorCode::ImpactUnacknowledged => "IMPACT_UNACKNOWLEDGED",
            ErrorCode::Conflict => "CONFLICT",
            ErrorCode::RuntimeIncompatible => "RUNTIME_INCOMPATIBLE",
            ErrorCode::UpdateConflict => "UPDATE_CONFLICT",
            ErrorCode::MigrationFailed => "MIGRATION_FAILED",
            ErrorCode::UpdateFailed => "UPDATE_FAILED",
            ErrorCode::GitError => "GIT_ERROR",
            ErrorCode::IndexError => "INDEX_ERROR",
            ErrorCode::IoError => "IO_ERROR",
            ErrorCode::UnsafePath => "UNSAFE_PATH",
            ErrorCode::InvalidInput => "INVALID_INPUT",
        }
    }

    /// Numeric exit code. Groups: 1 internal, 2 usage, 10–19 project/config,
    /// 20–29 freshness/snapshot, 30–39 context, 40–49 checks, 50–59 runtime/update,
    /// 60–69 environment.
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::Internal => 1,
            ErrorCode::Usage => 2,
            ErrorCode::ProjectNotInitialized => 10,
            ErrorCode::ConfigInvalid => 11,
            // 12 is unused: invalid records are `validate` diagnostics and make `context`
            // incomplete (reason `SNAPSHOT_INVALID`); no error code reports them.
            ErrorCode::UnsupportedSchemaVersion => 13,
            ErrorCode::AlreadyInitialized => 14,
            ErrorCode::UnknownScope => 15,
            ErrorCode::NotFound => 16,
            ErrorCode::FreshnessUnverified => 20,
            ErrorCode::UpdateRequired => 21,
            ErrorCode::SnapshotNotFound => 22,
            ErrorCode::SkillOutdated => 23,
            ErrorCode::ContextIncomplete => 30,
            ErrorCode::ContextBudgetExceeded => 31,
            ErrorCode::ValidationFailed => 40,
            ErrorCode::RoutingTestsFailed => 41,
            ErrorCode::DriftDetected => 42,
            ErrorCode::EngineDiverged => 43,
            ErrorCode::ImpactUnacknowledged => 44,
            ErrorCode::Conflict => 45,
            ErrorCode::RuntimeIncompatible => 50,
            ErrorCode::UpdateConflict => 51,
            ErrorCode::MigrationFailed => 52,
            ErrorCode::UpdateFailed => 53,
            ErrorCode::GitError => 60,
            ErrorCode::IndexError => 61,
            ErrorCode::IoError => 62,
            ErrorCode::UnsafePath => 63,
            ErrorCode::InvalidInput => 64,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// An error with a stable code, a human message, structured details and an optional hint.
#[derive(Debug, Clone)]
pub struct KbError {
    pub code: ErrorCode,
    pub message: String,
    pub details: Value,
    pub hint: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
}

pub type Result<T, E = KbError> = std::result::Result<T, E>;

impl KbError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        KbError {
            code,
            message: message.into(),
            details: Value::Null,
            hint: None,
            diagnostics: Vec::new(),
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_diagnostics(mut self, diagnostics: Vec<Diagnostic>) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    pub fn internal(message: impl Into<String>) -> Self {
        KbError::new(ErrorCode::Internal, message)
    }

    pub fn io(context: impl fmt::Display, err: std::io::Error) -> Self {
        KbError::new(ErrorCode::IoError, format!("{context}: {err}"))
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        KbError::new(ErrorCode::InvalidInput, message)
    }

    pub fn unsafe_path(message: impl Into<String>) -> Self {
        KbError::new(ErrorCode::UnsafePath, message)
    }

    pub fn exit_code(&self) -> i32 {
        self.code.exit_code()
    }
}

impl fmt::Display for KbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for KbError {}

impl From<rusqlite::Error> for KbError {
    fn from(err: rusqlite::Error) -> Self {
        KbError::new(ErrorCode::IndexError, format!("index: {err}"))
    }
}

impl From<serde_json::Error> for KbError {
    fn from(err: serde_json::Error) -> Self {
        KbError::internal(format!("json: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn codes_and_exit_codes_are_unique() {
        let names: HashSet<_> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        let exits: HashSet<_> = ErrorCode::ALL.iter().map(|c| c.exit_code()).collect();
        assert_eq!(names.len(), ErrorCode::ALL.len());
        assert_eq!(exits.len(), ErrorCode::ALL.len());
    }

    #[test]
    fn exit_code_groups() {
        assert_eq!(ErrorCode::ProjectNotInitialized.exit_code(), 10);
        assert_eq!(ErrorCode::FreshnessUnverified.exit_code(), 20);
        assert_eq!(ErrorCode::ContextBudgetExceeded.exit_code(), 31);
        assert_eq!(ErrorCode::UpdateRequired.exit_code(), 21);
    }
}
