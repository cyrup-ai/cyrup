//! Redacted provider/runtime diagnostics (1:1 port of Pi `ai/src/utils/diagnostics.ts`).
//!
//! Pi records *redacted* provider/runtime failures and recoveries (retries, transport fallbacks,
//! partial-JSON cleanups) without aborting the turn by attaching an
//! [`AssistantMessageDiagnostic`] array to an [`crate::AssistantMessage`]. This module ports the
//! struct shapes (`DiagnosticErrorInfo` / `AssistantMessageDiagnostic`) and the
//! extract/create/append helpers so cyrup-provider and cyrup-agent can attach the same records.
//!
//! Serde follows arch-00 §4: `rename_all = "camelCase"` for Pi-interop (R-00-013).

/// A diagnostic error `code` (Pi `code?: string | number`, diagnostics.ts:5). Modeled untagged so
/// a JSON string or number round-trips to the matching variant.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum DiagnosticCode {
    Str(String),
    Num(i64),
}

/// Redacted info extracted from a thrown value (Pi `DiagnosticErrorInfo`, diagnostics.ts:1-6).
///
/// Rust `std::error::Error` has no JS `Error.name`/`.stack`/`.code`, so [`extract_diagnostic_error`]
/// fills only `message` from `Display` and leaves `name`/`stack`/`code` `None`; callers that know a
/// name/code (e.g. an HTTP status, an `io::Error` os code) may set them via the builders.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticErrorInfo {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub stack: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub code: Option<DiagnosticCode>,
}

impl DiagnosticErrorInfo {
    /// A bare info record with just a `message` (Pi `{ message }`).
    pub fn from_message(message: impl Into<String>) -> Self {
        Self {
            name: None,
            message: message.into(),
            stack: None,
            code: None,
        }
    }

    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    #[must_use]
    pub fn with_stack(mut self, stack: impl Into<String>) -> Self {
        self.stack = Some(stack.into());
        self
    }

    #[must_use]
    pub fn with_code(mut self, code: DiagnosticCode) -> Self {
        self.code = Some(code);
        self
    }
}

/// One redacted diagnostic record (Pi `AssistantMessageDiagnostic`, diagnostics.ts:8-13).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessageDiagnostic {
    /// A free-form classification tag (Pi `type`), e.g. `"retry"`, `"transport_fallback"`.
    pub r#type: String,
    /// Unix timestamp in milliseconds (Pi `Date.now()`).
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<DiagnosticErrorInfo>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub details: Option<serde_json::Value>,
}

/// Format an arbitrary error value to a message string (Pi `formatThrownValue`, diagnostics.ts:15-19).
/// Rust analog: the value's `Display`, falling back to the empty-message case the caller handles.
pub fn format_thrown_value(value: &dyn std::error::Error) -> String {
    value.to_string()
}

/// Extract redacted `{name, message, stack, code}` from an error (Pi `extractDiagnosticError`,
/// diagnostics.ts:21-30). Rust errors expose only a `Display` message; `name`/`stack`/`code` stay
/// `None` (set them via the [`DiagnosticErrorInfo`] builders when known).
pub fn extract_diagnostic_error(error: &dyn std::error::Error) -> DiagnosticErrorInfo {
    DiagnosticErrorInfo::from_message(format_thrown_value(error))
}

/// Build a timestamped diagnostic from a thrown error (Pi `createAssistantMessageDiagnostic`,
/// diagnostics.ts:32-38).
pub fn create_assistant_message_diagnostic(
    r#type: impl Into<String>,
    error: &dyn std::error::Error,
    details: Option<serde_json::Value>,
) -> AssistantMessageDiagnostic {
    AssistantMessageDiagnostic {
        r#type: r#type.into(),
        timestamp: now_millis(),
        error: Some(extract_diagnostic_error(error)),
        details,
    }
}

/// Build a timestamped diagnostic from an already-extracted [`DiagnosticErrorInfo`] (the no-`Error`
/// caller path; mirrors `createAssistantMessageDiagnostic` with a pre-extracted error).
pub fn create_assistant_message_diagnostic_from(
    r#type: impl Into<String>,
    error: Option<DiagnosticErrorInfo>,
    details: Option<serde_json::Value>,
) -> AssistantMessageDiagnostic {
    AssistantMessageDiagnostic {
        r#type: r#type.into(),
        timestamp: now_millis(),
        error,
        details,
    }
}

/// Append a diagnostic to any message that carries a `diagnostics` vector (Pi
/// `appendAssistantMessageDiagnostic`, diagnostics.ts:40-45). Provided as a free function for
/// parity; [`crate::AssistantMessage::append_diagnostic`] is the ergonomic method form.
pub fn append_assistant_message_diagnostic(
    diagnostics: &mut Option<Vec<AssistantMessageDiagnostic>>,
    diagnostic: AssistantMessageDiagnostic,
) {
    diagnostics.get_or_insert_with(Vec::new).push(diagnostic);
}

/// Current unix time in milliseconds (0 on a clock error — never panics).
pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_serializes_camelcase_and_skips_none() {
        let d = AssistantMessageDiagnostic {
            r#type: "retry".into(),
            timestamp: 123,
            error: Some(DiagnosticErrorInfo::from_message("boom")),
            details: None,
        };
        let v = serde_json::to_value(&d).expect("serialize");
        assert_eq!(v["type"], "retry");
        assert_eq!(v["timestamp"], 123);
        assert_eq!(v["error"]["message"], "boom");
        assert!(v.get("details").is_none());
        assert!(v["error"].get("name").is_none());
    }

    #[test]
    fn code_roundtrips_string_and_number() {
        let s = DiagnosticErrorInfo::from_message("m").with_code(DiagnosticCode::Str("E_X".into()));
        let n = DiagnosticErrorInfo::from_message("m").with_code(DiagnosticCode::Num(503));
        let sv = serde_json::to_value(&s).expect("ser");
        let nv = serde_json::to_value(&n).expect("ser");
        assert_eq!(sv["code"], "E_X");
        assert_eq!(nv["code"], 503);
        let sb: DiagnosticErrorInfo = serde_json::from_value(sv).expect("de");
        let nb: DiagnosticErrorInfo = serde_json::from_value(nv).expect("de");
        assert_eq!(sb.code, Some(DiagnosticCode::Str("E_X".into())));
        assert_eq!(nb.code, Some(DiagnosticCode::Num(503)));
    }

    #[test]
    fn extract_from_error_fills_message() {
        let err = std::io::Error::other("disk gone");
        let info = extract_diagnostic_error(&err);
        assert_eq!(info.message, "disk gone");
        assert!(info.name.is_none());
    }

    #[test]
    fn append_accumulates() {
        let mut diags: Option<Vec<AssistantMessageDiagnostic>> = None;
        append_assistant_message_diagnostic(
            &mut diags,
            create_assistant_message_diagnostic_from("a", None, None),
        );
        append_assistant_message_diagnostic(
            &mut diags,
            create_assistant_message_diagnostic_from("b", None, None),
        );
        assert_eq!(diags.as_ref().map(Vec::len), Some(2));
    }
}
