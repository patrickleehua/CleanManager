//! Versioned machine records (CLI-03/CLI-04). Every record carries schema_version, kind and request_id.

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::capability::{Capability, Platform};
use crate::error::ErrorCode;
use crate::{ENGINE_VERSION, SCHEMA_VERSION};

/// Wrap record-specific fields in the common envelope.
pub fn record(kind: &str, request_id: &str, fields: Map<String, Value>) -> Value {
    let mut out = Map::new();
    out.insert("schema_version".into(), Value::from(SCHEMA_VERSION));
    out.insert("kind".into(), Value::from(kind));
    out.insert("request_id".into(), Value::from(request_id));
    out.extend(fields);
    Value::Object(out)
}

fn fields(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => unreachable!("record fields are always an object"),
    }
}

pub fn capabilities_record(request_id: &str, platform: Platform, capabilities: &[Capability]) -> Value {
    record(
        "capabilities",
        request_id,
        fields(json!({
            "platform": platform,
            "engine_version": ENGINE_VERSION,
            "supported_schema_versions": [SCHEMA_VERSION],
            "capabilities": capabilities,
        })),
    )
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ErrorDetails {
    pub plan_id: Option<String>,
    pub operation_id: Option<String>,
    pub scope_id: Option<String>,
}

pub fn error_record(
    request_id: &str,
    code: ErrorCode,
    message: &str,
    required_action: Option<&str>,
    details: ErrorDetails,
) -> Value {
    record(
        "error",
        request_id,
        fields(json!({
            "code": code,
            "message": message,
            "retryable": code.retryable(),
            "required_action": required_action,
            "details": details,
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityState;

    #[test]
    fn envelope_has_required_fields_first_class() {
        let caps = [Capability::new("scan.directory", CapabilityState::Unverified, Some("not built"))];
        let v = capabilities_record("req_1", Platform::Windows, &caps);
        assert_eq!(v["schema_version"], "1.0");
        assert_eq!(v["kind"], "capabilities");
        assert_eq!(v["request_id"], "req_1");
        assert_eq!(v["capabilities"][0]["state"], "unverified");
    }

    #[test]
    fn error_record_uses_typed_code() {
        let v = error_record("r", ErrorCode::NeedsApproval, "msg", None, ErrorDetails::default());
        assert_eq!(v["code"], "needs_approval");
        assert_eq!(v["retryable"], false);
        assert!(v["details"]["plan_id"].is_null());
    }
}
