//! `details.error` — the machine-readable vocabulary (MCP-169).
//!
//! See [`crate::proxy`] for the module overview.


use serde::{Deserialize, Serialize};
use serde_json::Value;



// ==================================================================================================
// 1 · `details.error` — the machine-readable vocabulary (MCP-169)
// ==================================================================================================

/// Every `details.error` code the gateway can emit.
///
/// **Thirty-two survive the scope cuts** (upstream has 35; `timeout`, `script_error` and
/// `invalid_tool_path` were `mcpScript`-only). Of the 32, [`McpErrorCode::UnsafePattern`] has no
/// producer once the `recheck` gate goes — 31 are reachable. It is kept in the enum as a documented
/// no-producer variant so a future engine change does not have to reintroduce vocabulary.
///
/// `error-signal.ts:13` `toolErrorOverride` maps **exactly** [`McpErrorCode::ToolError`] and
/// [`McpErrorCode::CallFailed`] to `{isError: true}` — not `auth_required`, not `approval_denied`,
/// not `connect_failed`. Its own comment states why: pi never reads a result-level `isError`, so
/// without the override a failed MCP call is recorded as a **success**. See
/// [`McpErrorCode::is_tool_error_override`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpErrorCode {
    /// Dispatch preamble — the 30 s init wait elapsed. Carries **no `mode` key**.
    InitTimeout,
    /// Dispatch preamble — the init future rejected with a non-abort error. **No `mode` key**.
    InitFailed,
    /// Dispatch preamble — no state and no live init. **No `mode` key**.
    NotInitialized,
    /// `auth-start` / `auth-complete` reached without `server`.
    MissingServer,
    /// `auth-complete` reached without a usable `redirectUrl` / `code` / `input`.
    MissingInput,
    /// Every mode — the named server is `disabled: true`.
    ServerDisabled,
    /// `auth-start`, `auth-complete`, `list`, `instructions`, `connect` — no such `mcpServers` key.
    NotFound,
    /// `auth-start` — no URL, or `supportsOAuth` is false.
    OauthNotSupported,
    /// `auth-start` — `startAuth` threw.
    AuthStartFailed,
    /// `auth-complete` — the flow settled at a status other than `authenticated`.
    NotAuthenticated,
    /// `auth-complete` — `completeAuthFromInput` threw.
    AuthCompleteFailed,
    /// `describe`, `call` — the name matches more than one enabled server. **Fails closed.**
    AmbiguousTool,
    /// `describe`, `call` — nothing matched anywhere.
    ToolNotFound,
    /// `search` — a `regex` query longer than [`crate::proxy::MAX_REGEX_SEARCH_QUERY_LENGTH`].
    QueryTooLong,
    /// `search` — the pattern did not compile (or exceeded [`crate::proxy::constants::REGEX_SIZE_LIMIT`]).
    InvalidPattern,
    /// `search` — a blank query with no `server` filter.
    EmptyQuery,
    /// `search` — **no producer post-cut**. Upstream's `recheck` ReDoS verdict; Rust's `regex` is a
    /// finite automaton with a linear-time matching guarantee, so the analysis has nothing to
    /// prevent. Retained as vocabulary (MCP-159, MCP-169).
    UnsafePattern,
    /// `list`, `instructions` — configured but never connected and nothing cached.
    NotConnected,
    /// `instructions` — connected, and the server declared none.
    NoInstructions,
    /// `connect`, `call` — the connection is `needs-auth` and auto-auth did not rescue it.
    AuthRequired,
    /// `connect`, `call` — the connect attempt threw.
    ConnectFailed,
    /// `connect`, `call` — the owner or the caller's token fired.
    Aborted,
    /// `call` — the `server` hint names no configured server.
    ServerNotFound,
    /// `call` — connected (or reconnected) and the tool still is not there.
    ToolNotFoundAfterReconnect,
    /// `call` — a failure was recorded inside the 60-second backoff window.
    ServerBackoff,
    /// `call` — no connection and no definition to build one from.
    ServerNotConnected,
    /// `call` — the name is a host tool, not an MCP tool. Call it directly (MCP-199).
    NativeTool,
    /// `call` — the user declined the approval prompt.
    ApprovalDenied,
    /// `call` — approval is required and no interactive session exists to ask in.
    ApprovalRequired,
    /// `call` — the **server** returned `isError: true`. One of the two `isError` overrides.
    ToolError,
    /// `call` — the server demanded a URL interaction before the tool could run.
    UrlElicitationRequired,
    /// `call` — the request itself threw. The other `isError` override.
    CallFailed,
}

impl McpErrorCode {
    /// Every code, in 13d §13 table order. The conformance test asserts the serialised set.
    pub const ALL: [McpErrorCode; 32] = [
        McpErrorCode::InitTimeout,
        McpErrorCode::InitFailed,
        McpErrorCode::NotInitialized,
        McpErrorCode::MissingServer,
        McpErrorCode::MissingInput,
        McpErrorCode::ServerDisabled,
        McpErrorCode::NotFound,
        McpErrorCode::OauthNotSupported,
        McpErrorCode::AuthStartFailed,
        McpErrorCode::NotAuthenticated,
        McpErrorCode::AuthCompleteFailed,
        McpErrorCode::AmbiguousTool,
        McpErrorCode::ToolNotFound,
        McpErrorCode::QueryTooLong,
        McpErrorCode::InvalidPattern,
        McpErrorCode::EmptyQuery,
        McpErrorCode::UnsafePattern,
        McpErrorCode::NotConnected,
        McpErrorCode::NoInstructions,
        McpErrorCode::AuthRequired,
        McpErrorCode::ConnectFailed,
        McpErrorCode::Aborted,
        McpErrorCode::ServerNotFound,
        McpErrorCode::ToolNotFoundAfterReconnect,
        McpErrorCode::ServerBackoff,
        McpErrorCode::ServerNotConnected,
        McpErrorCode::NativeTool,
        McpErrorCode::ApprovalDenied,
        McpErrorCode::ApprovalRequired,
        McpErrorCode::ToolError,
        McpErrorCode::UrlElicitationRequired,
        McpErrorCode::CallFailed,
    ];

    /// The wire spelling — what lands in `details.error`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            McpErrorCode::InitTimeout => "init_timeout",
            McpErrorCode::InitFailed => "init_failed",
            McpErrorCode::NotInitialized => "not_initialized",
            McpErrorCode::MissingServer => "missing_server",
            McpErrorCode::MissingInput => "missing_input",
            McpErrorCode::ServerDisabled => "server_disabled",
            McpErrorCode::NotFound => "not_found",
            McpErrorCode::OauthNotSupported => "oauth_not_supported",
            McpErrorCode::AuthStartFailed => "auth_start_failed",
            McpErrorCode::NotAuthenticated => "not_authenticated",
            McpErrorCode::AuthCompleteFailed => "auth_complete_failed",
            McpErrorCode::AmbiguousTool => "ambiguous_tool",
            McpErrorCode::ToolNotFound => "tool_not_found",
            McpErrorCode::QueryTooLong => "query_too_long",
            McpErrorCode::InvalidPattern => "invalid_pattern",
            McpErrorCode::EmptyQuery => "empty_query",
            McpErrorCode::UnsafePattern => "unsafe_pattern",
            McpErrorCode::NotConnected => "not_connected",
            McpErrorCode::NoInstructions => "no_instructions",
            McpErrorCode::AuthRequired => "auth_required",
            McpErrorCode::ConnectFailed => "connect_failed",
            McpErrorCode::Aborted => "aborted",
            McpErrorCode::ServerNotFound => "server_not_found",
            McpErrorCode::ToolNotFoundAfterReconnect => "tool_not_found_after_reconnect",
            McpErrorCode::ServerBackoff => "server_backoff",
            McpErrorCode::ServerNotConnected => "server_not_connected",
            McpErrorCode::NativeTool => "native_tool",
            McpErrorCode::ApprovalDenied => "approval_denied",
            McpErrorCode::ApprovalRequired => "approval_required",
            McpErrorCode::ToolError => "tool_error",
            McpErrorCode::UrlElicitationRequired => "url_elicitation_required",
            McpErrorCode::CallFailed => "call_failed",
        }
    }

    /// `error-signal.ts:13` `toolErrorOverride` — **exactly two** codes re-flag the result as an
    /// error. Everything else (`auth_required`, connection states, search feedback, …) is not a
    /// failed tool call and gets no override.
    #[must_use]
    pub const fn is_tool_error_override(self) -> bool {
        matches!(self, McpErrorCode::ToolError | McpErrorCode::CallFailed)
    }
}

/// `error-signal.ts:13` `toolErrorOverride(details)`.
///
/// Returns `Some(true)` when the finished result must be re-flagged. In cyrup that lands as an
/// `EventPatch::ToolResult` with only `is_error` set, merged field-by-field, delivered under
/// `EventKind::ToolResult` — the same "flip the flag, keep `content` and `details`" shape as pi's
/// field-by-field merge.
#[must_use]
pub fn tool_error_override(details: Option<&Value>) -> Option<bool> {
    let code = details?.get("error")?.as_str()?;
    McpErrorCode::ALL
        .iter()
        .find(|candidate| candidate.as_str() == code)
        .filter(|candidate| candidate.is_tool_error_override())
        .map(|_| true)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- MCP-169 · the `details.error` vocabulary --------------------------------------------------

    #[test]
    fn error_vocabulary_is_thirty_two_codes_and_two_overrides() {
        let expected = [
            "init_timeout",
            "init_failed",
            "not_initialized",
            "missing_server",
            "missing_input",
            "server_disabled",
            "not_found",
            "oauth_not_supported",
            "auth_start_failed",
            "not_authenticated",
            "auth_complete_failed",
            "ambiguous_tool",
            "tool_not_found",
            "query_too_long",
            "invalid_pattern",
            "empty_query",
            "unsafe_pattern",
            "not_connected",
            "no_instructions",
            "auth_required",
            "connect_failed",
            "aborted",
            "server_not_found",
            "tool_not_found_after_reconnect",
            "server_backoff",
            "server_not_connected",
            "native_tool",
            "approval_denied",
            "approval_required",
            "tool_error",
            "url_elicitation_required",
            "call_failed",
        ];
        assert_eq!(McpErrorCode::ALL.len(), 32);
        let actual: Vec<&str> = McpErrorCode::ALL.iter().map(|code| code.as_str()).collect();
        assert_eq!(actual, expected);
        // The serde spelling must agree with `as_str`, since both reach `details.error`.
        for code in McpErrorCode::ALL {
            assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
        }
        // Exactly two codes re-flag the result as an error.
        let overrides: Vec<&str> = McpErrorCode::ALL
            .iter()
            .filter(|code| code.is_tool_error_override())
            .map(|code| code.as_str())
            .collect();
        assert_eq!(overrides, vec!["tool_error", "call_failed"]);
        assert_eq!(tool_error_override(Some(&json!({"error": "tool_error"}))), Some(true));
        assert_eq!(tool_error_override(Some(&json!({"error": "call_failed"}))), Some(true));
        assert_eq!(tool_error_override(Some(&json!({"error": "auth_required"}))), None);
        assert_eq!(tool_error_override(Some(&json!({"mode": "status"}))), None);
        assert_eq!(tool_error_override(None), None);
    }

}
