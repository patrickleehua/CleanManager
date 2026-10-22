//! The `permission_cause` vocabulary (docs/platforms/macos.md §6): which blocking layer stands behind a
//! `permission_required` report, as far as it can be proven.
//!
//! Platform code writes the cause into its message as `permission_cause=<cause>` (the text a person reads keeps
//! it); the machine records carry it again as the optional, structured `permission_cause` field of an issue or an
//! unknown size, derived here from that text with [`cause_in`] so every record that shows the message also shows the
//! field. A message without the marker has no cause field: nothing is guessed.

use serde::{Deserialize, Serialize};

/// Why a location could not be read, as far as this build can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionCause {
    /// The blocking layer cannot be proven: POSIX permissions, an ACL, privacy protection or another policy layer all
    /// answer the same `EPERM`/`EACCES`; or a first access did not return in time where a privacy prompt was never
    /// measured (a slow or unresponsive volume answers the same way).
    Undetermined,
    /// The first access to a privacy-protected location whose prompt behaviour was measured did not return in time:
    /// macOS is most likely showing its "would like to access…" prompt. Unknown, never empty.
    PrivacyPromptPending,
}

/// Every cause, in wire order.
pub const ALL: [PermissionCause; 2] = [PermissionCause::Undetermined, PermissionCause::PrivacyPromptPending];

/// The marker of [`PermissionCause::Undetermined`] in a message.
pub const UNDETERMINED_MARK: &str = "permission_cause=undetermined";
/// The marker of [`PermissionCause::PrivacyPromptPending`] in a message.
pub const PROMPT_PENDING_MARK: &str = "permission_cause=privacy_prompt_pending";

impl PermissionCause {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionCause::Undetermined => "undetermined",
            PermissionCause::PrivacyPromptPending => "privacy_prompt_pending",
        }
    }

    /// `permission_cause=<cause>`, as written into messages.
    pub fn mark(self) -> &'static str {
        match self {
            PermissionCause::Undetermined => UNDETERMINED_MARK,
            PermissionCause::PrivacyPromptPending => PROMPT_PENDING_MARK,
        }
    }

    pub fn parse(s: &str) -> Option<PermissionCause> {
        ALL.into_iter().find(|c| c.as_str() == s)
    }
}

/// The cause a message names (`permission_cause=<cause>`); the first marker wins, an unknown spelling is none.
pub fn cause_in(text: &str) -> Option<PermissionCause> {
    const KEY: &str = "permission_cause=";
    let at = text.find(KEY)? + KEY.len();
    let word: String = text[at..].chars().take_while(|c| c.is_ascii_lowercase() || *c == '_').collect();
    PermissionCause::parse(&word)
}

/// Add the structured `permission_cause` to one wire issue (`{code, scope_display, message}`) when its message names
/// one; an issue that already carries the field, or whose message names none, is left as it is.
pub fn annotate_issue(issue: &mut serde_json::Value) {
    let Some(obj) = issue.as_object_mut() else { return };
    if obj.contains_key("permission_cause") {
        return;
    }
    if let Some(cause) = obj.get("message").and_then(serde_json::Value::as_str).and_then(cause_in) {
        obj.insert("permission_cause".into(), cause.as_str().into());
    }
}

/// [`annotate_issue`] for every issue of a list.
pub fn annotate_issues(issues: &mut [serde_json::Value]) {
    issues.iter_mut().for_each(annotate_issue);
}

/// The structured cause of an unknown size from its note (the `size_note` of an insight or card), as a wire value.
pub fn cause_value(note: Option<&str>) -> serde_json::Value {
    note.and_then(cause_in).map_or(serde_json::Value::Null, |c| c.as_str().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_cause_is_read_from_the_marker_only() {
        assert_eq!(cause_in("permission_required (permission_cause=privacy_prompt_pending): macOS did not answer"), Some(PermissionCause::PrivacyPromptPending));
        assert_eq!(cause_in("x (blocked by privacy protection; permission_cause=undetermined). Grant"), Some(PermissionCause::Undetermined));
        assert_eq!(cause_in("permission_cause=undetermined_but_longer"), None, "a different word is not a cause");
        assert_eq!(cause_in("permission denied"), None);
        assert_eq!(cause_in("Size unavailable: permission_required (permission_cause=undetermined): a; permission_cause=privacy_prompt_pending"), Some(PermissionCause::Undetermined));
        for c in ALL {
            assert_eq!(PermissionCause::parse(c.as_str()), Some(c));
            assert_eq!(cause_in(c.mark()), Some(c));
            assert_eq!(serde_json::to_value(c).unwrap(), json!(c.as_str()));
        }
    }

    #[test]
    fn issues_gain_the_field_only_when_their_message_names_a_cause() {
        let mut issues = vec![
            json!({"code": "permission_required", "scope_display": "/Users/a/Downloads", "message": "permission_required (permission_cause=privacy_prompt_pending): x"}),
            json!({"code": "io_error", "scope_display": "/x", "message": "Input/output error"}),
            json!({"code": "permission_required", "scope_display": "/y", "message": "m", "permission_cause": "undetermined"}),
        ];
        annotate_issues(&mut issues);
        assert_eq!(issues[0]["permission_cause"], "privacy_prompt_pending");
        assert!(issues[1].get("permission_cause").is_none(), "no marker, no field");
        assert_eq!(issues[2]["permission_cause"], "undetermined", "an explicit field is kept");
        assert_eq!(cause_value(Some("Size unavailable: permission_required (permission_cause=undetermined): denied")), json!("undetermined"));
        assert_eq!(cause_value(None), serde_json::Value::Null);
    }
}
