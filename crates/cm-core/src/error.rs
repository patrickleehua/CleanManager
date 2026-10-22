//! Typed error codes and stable exit classifications (CLI-05).
//!
//! Callers branch on `ErrorCode`, never on the localized message.

use serde::{Deserialize, Serialize};

/// Internal invariant violations raised by core constructors. They map to protocol codes at
/// the service boundary; they are never silently corrected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    #[error("byte arithmetic overflowed u64")]
    ByteOverflow,
    #[error("invalid state: {0}")]
    InvalidState(&'static str),
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

impl CoreError {
    pub fn code(&self) -> ErrorCode {
        match self {
            CoreError::ByteOverflow | CoreError::InvalidState(_) => ErrorCode::InternalError,
            CoreError::InvalidInput(_) => ErrorCode::InvalidArgument,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidArgument,
    SchemaMismatch,
    NeedsApproval,
    PolicyDenied,
    PermissionRequired,
    PlanStale,
    CursorStale,
    Conflict,
    Unsupported,
    IoError,
    InternalError,
    Cancelled,
    ProtectionUnavailable,
    /// The rebuildable scan index (`index.db`) is damaged or of an unsupported schema. History, quarantine and
    /// recovery do not depend on it; `required_action` names the repair (`doctor --repair-index`), after which
    /// scans must be run again. Never retryable as is.
    IndexUnavailable,
}

/// Process exit status. `Partial` and `ReconciliationRequired` are not errors: they accompany
/// a full result record (scan_result / operation_report).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitClass {
    Success,
    Partial,
    ReconciliationRequired,
    Error(ErrorCode),
}

impl ExitClass {
    pub fn code(self) -> i32 {
        match self {
            ExitClass::Success => 0,
            ExitClass::Partial => 5,
            ExitClass::ReconciliationRequired => 8,
            ExitClass::Error(code) => code.exit_code(),
        }
    }
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 14] = [
        ErrorCode::InvalidArgument,
        ErrorCode::SchemaMismatch,
        ErrorCode::NeedsApproval,
        ErrorCode::PolicyDenied,
        ErrorCode::PermissionRequired,
        ErrorCode::PlanStale,
        ErrorCode::CursorStale,
        ErrorCode::Conflict,
        ErrorCode::Unsupported,
        ErrorCode::IoError,
        ErrorCode::InternalError,
        ErrorCode::Cancelled,
        ErrorCode::ProtectionUnavailable,
        ErrorCode::IndexUnavailable,
    ];

    /// The protocol word (the serde name), for human-readable text that names a code.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidArgument => "invalid_argument",
            ErrorCode::SchemaMismatch => "schema_mismatch",
            ErrorCode::NeedsApproval => "needs_approval",
            ErrorCode::PolicyDenied => "policy_denied",
            ErrorCode::PermissionRequired => "permission_required",
            ErrorCode::PlanStale => "plan_stale",
            ErrorCode::CursorStale => "cursor_stale",
            ErrorCode::Conflict => "conflict",
            ErrorCode::Unsupported => "unsupported",
            ErrorCode::IoError => "io_error",
            ErrorCode::InternalError => "internal_error",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::ProtectionUnavailable => "protection_unavailable",
            ErrorCode::IndexUnavailable => "index_unavailable",
        }
    }

    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::InvalidArgument | ErrorCode::SchemaMismatch => 2,
            ErrorCode::NeedsApproval | ErrorCode::PolicyDenied => 3,
            ErrorCode::PermissionRequired => 4,
            ErrorCode::PlanStale | ErrorCode::CursorStale | ErrorCode::Conflict => 6,
            ErrorCode::Unsupported => 7,
            ErrorCode::IoError | ErrorCode::InternalError | ErrorCode::ProtectionUnavailable | ErrorCode::IndexUnavailable => 8,
            ErrorCode::Cancelled => 130,
        }
    }

    /// Whether blindly repeating the same request can succeed. Unknown side effects never retry.
    pub fn retryable(self) -> bool {
        matches!(self, ErrorCode::IoError | ErrorCode::CursorStale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_protocol_table() {
        assert_eq!(ErrorCode::InvalidArgument.exit_code(), 2);
        assert_eq!(ErrorCode::NeedsApproval.exit_code(), 3);
        assert_eq!(ErrorCode::PermissionRequired.exit_code(), 4);
        assert_eq!(ExitClass::Partial.code(), 5);
        assert_eq!(ErrorCode::PlanStale.exit_code(), 6);
        assert_eq!(ErrorCode::Unsupported.exit_code(), 7);
        assert_eq!(ExitClass::ReconciliationRequired.code(), 8);
        assert_eq!(ErrorCode::ProtectionUnavailable.exit_code(), 8);
        assert_eq!(ErrorCode::IndexUnavailable.exit_code(), 8);
        assert!(!ErrorCode::IndexUnavailable.retryable(), "a damaged index needs a repair, not a retry");
        assert_eq!(ErrorCode::Cancelled.exit_code(), 130);
    }

    /// The Rust mapping, the machine-readable table and the protocol schema must agree.
    #[test]
    fn exit_table_and_schema_agree_with_rust() {
        let table: serde_json::Value = serde_json::from_str(include_str!("../../../schemas/v1/exit-codes.json")).unwrap();
        let schema: serde_json::Value = serde_json::from_str(include_str!("../../../schemas/v1/protocol.schema.json")).unwrap();
        let errors = table["errors"].as_object().unwrap();
        assert_eq!(errors.len(), ErrorCode::ALL.len());
        for code in ErrorCode::ALL {
            let name = serde_json::to_value(code).unwrap().as_str().unwrap().to_owned();
            assert_eq!(errors[&name].as_i64(), Some(i64::from(code.exit_code())), "{name}");
        }
        let schema_codes: Vec<&str> = schema["$defs"]["error"]["properties"]["code"]["enum"]
            .as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        let mut rust_codes: Vec<String> =
            ErrorCode::ALL.iter().map(|c| serde_json::to_value(c).unwrap().as_str().unwrap().to_owned()).collect();
        let mut schema_sorted: Vec<String> = schema_codes.iter().map(|s| s.to_string()).collect();
        rust_codes.sort();
        schema_sorted.sort();
        assert_eq!(rust_codes, schema_sorted);
        assert_eq!(table["results"]["success"], ExitClass::Success.code());
        assert_eq!(table["results"]["partial"], ExitClass::Partial.code());
        assert_eq!(table["results"]["reconciliation_required"], ExitClass::ReconciliationRequired.code());
    }

    #[test]
    fn words_are_the_protocol_names() {
        for code in ErrorCode::ALL {
            assert_eq!(serde_json::to_value(code).unwrap(), code.as_str(), "{code:?}");
        }
    }

    #[test]
    fn approval_errors_are_not_retryable() {
        assert!(!ErrorCode::NeedsApproval.retryable());
        assert!(!ErrorCode::PolicyDenied.retryable());
    }
}
