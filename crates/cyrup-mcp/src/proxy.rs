//! The proxy modes — `proxy-modes.ts`, `search-ranking.ts`, and the `mcp` tool `index.ts` registers
//! (gap-analysis **13d**, units MCP-151…MCP-199).
//!
//! # One tool, nine modes
//!
//! After the four scope cuts, `pi-mcp-adapter`'s entire model-facing gateway reduces to a single
//! registered tool named **`mcp`** whose *arguments* select one of nine behaviours. Everything else
//! in the adapter — transports, OAuth, the metadata cache, lifecycle — exists to keep that one tool
//! answering. This module is the whole of it: [`McpTool`] is the [`cyrup_core::Tool`] impl,
//! [`execute_status`]…[`execute_call`] are the nine modes, and [`build_proxy_description`] is the
//! *regenerated* description the model discovers everything through.
//!
//! Three properties a reader must internalise.
//!
//! **First, the tool's description is data, not a literal.** `buildProxyDescription`
//! (`direct-tools.ts:234`) regenerates it from the current config and the on-disk metadata cache on
//! every surface sync, and `syncProxyTool` (`index.ts:995`) re-registers the whole tool whenever the
//! generated text differs. The model learns which servers exist, how many tools each has, which are
//! disabled, a 150-character snippet of each server's own instructions, and a nine-line usage
//! cheatsheet — all from that regenerated string. A port that hard-codes it ships a gateway the
//! model cannot discover anything through. See [`build_proxy_description`], and MCP-193 for the one
//! missing handle (`HA-1`) that keeps re-registration from reaching a live session.
//!
//! **Second, [`execute_call`] is not a dispatcher, it is a resolution state machine** with five
//! entry paths and five auto-auth retry points fenced by one function-scoped boolean. A bare tool
//! name can resolve against already-known metadata, against a server hint, by lazily connecting a
//! server whose prefix the name starts with, or by connecting and re-resolving after the handshake —
//! and at five of those points a `needs-auth` connection can trigger [`attempt_auto_auth`], close,
//! reconnect and resolve again. `auto_auth_attempted` latches all of them. Get the **ambiguity
//! gate** wrong and a call silently reaches the wrong server's same-named tool — that is this
//! section's only `critical` (MCP-163), and it is why [`get_single_tool_match`] returns
//! [`SingleMatch::Ambiguous`] rather than picking the first.
//!
//! **Third, `details.error` is the contract, not the text.** Every mode returns
//! `{content, details}` and `details.error` is a machine-readable code that downstream code
//! branches on: `error-signal.ts`'s `toolErrorOverride` re-flags exactly `tool_error` and
//! `call_failed` as `isError`, and nothing else. Port the prose loosely at your peril; port the
//! codes byte-exactly. [`McpErrorCode`] freezes all thirty-two (MCP-169).
//!
//! # What is cut here, deliberately
//!
//! * **`mcpScript` / the JS worker (Cut 4)** — `mcp-code.ts`'s registration, `McpSettings.scriptMode`
//!   and `McpToolApprovalOrigin::Script`. [`ApprovalOrigin`] keeps its shape and its `Proxy` default;
//!   only the `"script"` variant and its call site disappear. The description's `use mcpScript.` sentence is gone,
//!   and the `timeout` / `script_error` / `invalid_tool_path` codes with it.
//! * **MCP Apps (Cut 2)** — `executeUiMessages` and the `action: "ui-messages"` arm. The router drops
//!   from ten arms to nine with every other arm keeping its relative order, so `action:"ui-messages"`
//!   now falls through to [`execute_status`] rather than erroring, and the `action` property's
//!   description narrows to two values. [`execute_call`] loses its UI-enabled-tool result path —
//!   **three** paths remain.
//! * **The `recheck` ReDoS gate** — Rust's `regex` compiles to a finite automaton with a linear-time
//!   matching guarantee, so the attack the check exists to stop cannot occur. `unsafe_pattern`
//!   survives in [`McpErrorCode`] as a documented no-producer variant (MCP-159, MCP-169).
//! * **Legacy HTTP+SSE and raw unix sockets (Cuts 1 and 3)** — no mode in this file branches on
//!   transport, so nothing here changes beyond the set of servers that can reach `connected`.
//!
//! # The collaborator seam
//!
//! Upstream every mode takes one mutable `McpExtensionState` record and calls freely into
//! `init.ts`, `server-manager.ts`, `mcp-auth-flow.ts`, `tool-metadata.ts`, `tool-approval.ts` and
//! `mcp-output-guard.ts`. Those subsystems are owned by sections 13a/13c/13e/13g. Here they arrive
//! through [`ProxyEnv`], one trait whose methods are named 1:1 after the upstream functions they
//! stand for, bundled with the state record into [`ProxyCtx`]. That is not an architectural
//! invention: 13d's own conformance plan (MCP-196) requires "a controllable `needs-auth` connection
//! state and an injectable `authenticate`", which is exactly this trait. The call *order*, the
//! branch structure and the returned codes are the port; only the resolution of each collaborator is
//! late-bound.
//!
//! *Provenance: upstream is `pi-mcp-adapter` v2.25.0; every citation below is `file:line` at that
//! tag.*

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map as JsonMap, Value};

use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolRenderKind, ToolResult, ToolUpdateSink};

use crate::abort::{combine, is_abort_error, throw_if_aborted};
use crate::config::{
    locale_compare, BoolOrList, McpConfig, McpSettings, OAuthGrantType, OAuthSetting, ServerEntry,
    ToolPrefix, ToolResultRendering,
};
use crate::errors::{McpError, McpResult};
use crate::owner::McpRuntimeOwner;
use crate::state::McpState;

// ==================================================================================================
// 0 · Constants
// ==================================================================================================

/// `index.ts:39` `INIT_WAIT_TIMEOUT_MS = 30_000` — the bounded wait the dispatch preamble races the
/// live init future against.
pub const INIT_WAIT_TIMEOUT_MS: u64 = 30_000;

/// `proxy-modes.ts:27` `MAX_REGEX_SEARCH_QUERY_LENGTH = 256`.
///
/// JS measures `query.length` in UTF-16 code units. This port measures `chars().count()` (Unicode
/// scalar values), which diverges only for astral-plane text in a regex query — a query that would
/// be rejected at 128 astral characters upstream and 256 here. Stated rather than silently changed.
pub const MAX_REGEX_SEARCH_QUERY_LENGTH: usize = 256;

/// `proxy-modes.ts:28` `INSTRUCTIONS_PREVIEW_LENGTH = 300` — [`execute_list`]'s inline preview.
pub const INSTRUCTIONS_PREVIEW_LENGTH: usize = 300;

/// `direct-tools.ts:27` `INSTRUCTIONS_SNIPPET_LENGTH = 150` — [`build_proxy_description`]'s snippet.
pub const INSTRUCTIONS_SNIPPET_LENGTH: usize = 150;

/// `tool-approval.ts:176 @v2.26.1` — the approval dialog's argument preview budget, in UTF-16 code units.
///
/// Not a named constant upstream (the literal `500` is inline); named here because
/// [`approval_argument_preview`] and its conformance test both have to agree on it.
pub const APPROVAL_PREVIEW_LENGTH: usize = 500;

/// `tool-approval.ts:182 @v2.26.1` — the approval dialog's first option. **A wire contract**: the answer is
/// compared against this exact string, so a reworded label silently becomes a denial.
pub const APPROVE_ONCE_OPTION: &str = "Allow once";

/// `tool-approval.ts:182 @v2.26.1` — the second option, the one that writes the session cache.
pub const APPROVE_FOR_SESSION_OPTION: &str = "Allow for session";

/// `tool-approval.ts:182 @v2.26.1` — the third option. Never compared against: it is the `_` arm's label,
/// and every answer that is not one of the two `Allow …` strings denies. Named so the option list
/// reads as three deliberate strings rather than two and a loose literal.
pub const DENY_OPTION: &str = "Deny";

/// `tool-approval.ts:182 @v2.26.1` `["Allow once", "Allow for session", "Deny"]`, in upstream's order — the
/// order the human sees, and therefore the one a muscle-memory `Enter` picks.
pub const APPROVAL_OPTIONS: [&str; 3] =
    [APPROVE_ONCE_OPTION, APPROVE_FOR_SESSION_OPTION, DENY_OPTION];

/// `search-ranking.ts:9` — shortest field token allowed to stem-match a longer query token.
///
/// Real descriptions tokenize possessives into single letters (`"project's"` → `["project", "s"]`),
/// which would otherwise make every query starting with that letter a match.
pub const MIN_STEM_LENGTH: usize = 4;

/// `search-ranking.ts:11` `FIELD_WEIGHTS.name`.
const WEIGHT_NAME: i64 = 12;
/// `search-ranking.ts:11` `FIELD_WEIGHTS.originalName`.
const WEIGHT_ORIGINAL_NAME: i64 = 10;
/// `search-ranking.ts:11` `FIELD_WEIGHTS.server`.
const WEIGHT_SERVER: i64 = 8;
/// `search-ranking.ts:11` `FIELD_WEIGHTS.description`.
const WEIGHT_DESCRIPTION: i64 = 5;
/// `search-ranking.ts:11` `FIELD_WEIGHTS.keywords`.
const WEIGHT_KEYWORDS: i64 = 5;

/// The regex engine's compiled-program ceiling, set **explicitly** rather than left to the crate
/// default (MCP-159): a model-supplied pattern must not be able to trade a 256-character query for
/// an unbounded DFA. A size-limit failure surfaces as [`McpErrorCode::InvalidPattern`], exactly as a
/// syntax error does.
const REGEX_SIZE_LIMIT: usize = 1 << 20;
/// Companion ceiling for the lazy DFA cache; see [`REGEX_SIZE_LIMIT`].
const REGEX_DFA_SIZE_LIMIT: usize = 1 << 20;

/// The tool's registered name. **Fixed by cross-crate contract** (13d §13.1):
/// `cyrup_permission_system::manager` branches on `normalized == "mcp"` and its `BUILT_IN_TOOL_NAMES`
/// does not contain it, so it must arrive from the extension registry under exactly this name;
/// `gate.rs` addresses it by name in the no-UI denial text.
pub const MCP_TOOL_NAME: &str = "mcp";

/// `index.ts:824` `label: "MCP"`.
pub const MCP_TOOL_LABEL: &str = "MCP";

/// `index.ts:825` `promptSnippet`.
pub const MCP_TOOL_PROMPT_SNIPPET: &str =
    "MCP gateway — status, search, describe, auth, and single MCP tool calls";

/// The guideline bullet `cyrup_permission_system::sanitize::tools` keys its MCP rule on.
///
/// 13d §13.3: `normalize_guideline_text` is bullet-strip + whitespace-collapse + lowercase, so this
/// source string need not itself be lowercase — but it must normalise to
/// `use mcp for mcp discovery first: search by capability, describe one exact tool name, then call
/// it.` **The failure mode is inverted from the obvious guess**: `should_keep_guideline` is
/// `guideline_keep_rule(...).unwrap_or(true)`, so a bullet matching no rule is *always kept*. A
/// mismatched string therefore does not delete guidance — it silently disables the gating and leaves
/// "use mcp …" in the system prompt after the `mcp` tool has been taken away (MCP-192).
pub const MCP_TOOL_GUIDELINE: &str =
    "Use mcp for MCP discovery first: search by capability, describe one exact tool name, then call it.";

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
    /// `search` — a `regex` query longer than [`MAX_REGEX_SEARCH_QUERY_LENGTH`].
    QueryTooLong,
    /// `search` — the pattern did not compile (or exceeded [`REGEX_SIZE_LIMIT`]).
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

// ==================================================================================================
// 2 · `ToolMetadata` and the tool-name grammar
//
// INTEGRATION NOTE (13e / MCP-200…MCP-207): `ToolMetadata` and the six naming functions below are
// `types.ts`' and are owned by section 13e, which lands them in `crate::renderers`. They are ported
// here byte-faithfully so 13d compiles and is testable standalone; when 13e lands, delete this
// block and replace it with `pub use crate::renderers::{…};` — the shapes are upstream's, so the
// swap is a delete, not a rewrite. `is_tool_allowed` here is the *simple* candidate-set form
// (`otherCurrentCandidates: Set<string>`); 13e owns the `ToolSelectorCandidateIndex` memoising form.
// ==================================================================================================

/// `types.ts:575` `ToolMetadata` — one model-visible MCP tool or resource tool.
///
/// `uiResourceUri` and `uiStreamMode` are **Cut 2** (MCP Apps); `uiVisibility` survives the cut
/// because `buildProxyDescription`'s counts use it to hide tools the server explicitly marked
/// app-only (13d §2, MCP-208).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolMetadata {
    /// Prefixed, model-visible name (e.g. `xcodebuild_list_sims`).
    pub name: String,
    /// The name the MCP server knows (e.g. `list_sims`) — what `tools/call` is sent.
    pub original_name: String,
    /// `tool.description ?? ""`.
    pub description: String,
    /// Resource tools only: the URI `resources/read` is issued against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_uri: Option<String>,
    /// `_meta.ui.visibility`, when the server declared one. `None` == visible to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_visibility: Option<Vec<String>>,
    /// The raw JSON Schema, stored for `describe` and for the `Expected parameters:` error suffix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
}

impl ToolMetadata {
    /// A plain tool, for tests and for callers that only need the three required fields.
    #[must_use]
    pub fn new(name: impl Into<String>, original_name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            original_name: original_name.into(),
            description: description.into(),
            resource_uri: None,
            ui_visibility: None,
            input_schema: None,
        }
    }
}

/// `ui-tool-visibility.ts` `isUiToolVisibleToModel(v)` = `v === undefined || v.includes("model")`.
///
/// **Kept from the cut file** (13d §2): dropping it would expose to the model tools the server
/// explicitly marked app-only.
#[must_use]
pub fn is_ui_tool_visible_to_model(visibility: Option<&[String]>) -> bool {
    match visibility {
        None => true,
        Some(list) => list.iter().any(|entry| entry == "model"),
    }
}

/// `types.ts:651` `sanitizeServerPrefix(serverName, preserveProviderValid = true)`.
///
/// With the default `preserveProviderValid`, `[A-Za-z0-9_-]` is kept **verbatim** — so `-` survives —
/// and anything else is hex-escaped as `_<codepoint-hex>_`. This is the four-mode, hyphen-preserving
/// grammar; `cyrup_ext_subagents::exec::mcp_direct_tools::get_server_prefix` is a three-mode,
/// hyphen-*replacing* one ported from `pi-subagents`, and the two disagree for every hyphenated
/// server name. **MCP-178/MCP-205 open decision**, recommendation (b): upgrade `mcp_direct_tools` so
/// one tool-name grammar governs the process. This file implements the adapter's rule because
/// `rankSuggestions` and [`execute_call`]'s phase 4 both test `toolName.starts_with(prefix + "_")`,
/// and under the other rule every hyphenated server silently stops matching.
#[must_use]
pub fn sanitize_server_prefix(server_name: &str) -> String {
    let mut out = String::with_capacity(server_name.len());
    for ch in server_name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
            out.push_str(&format!("{:x}", ch as u32));
            out.push('_');
        }
    }
    out
}

/// `types.ts:677` `getServerPrefix(serverName, mode)`.
///
/// `short` strips a trailing `-?mcp` (case-insensitively) and falls back to the literal `mcp` when
/// that empties the name; `mcp` mode produces `mcp__<sanitized>`; `none` yields the empty string,
/// which every caller treats as "no prefix".
#[must_use]
pub fn get_server_prefix(server_name: &str, mode: ToolPrefix) -> String {
    match mode {
        ToolPrefix::None => String::new(),
        ToolPrefix::Short => {
            let stripped = strip_mcp_suffix(server_name);
            let short = sanitize_server_prefix(stripped);
            if short.is_empty() { "mcp".to_string() } else { short }
        }
        ToolPrefix::Mcp => format!("mcp__{}", sanitize_server_prefix(server_name)),
        ToolPrefix::Server => sanitize_server_prefix(server_name),
    }
}

/// `serverName.replace(/-?mcp$/i, "")` — the `short` mode's suffix strip, anchored at the end and
/// case-insensitive, with an optional single leading hyphen.
fn strip_mcp_suffix(server_name: &str) -> &str {
    let bytes = server_name.as_bytes();
    let len = bytes.len();
    if len < 3 {
        return server_name;
    }
    let tail = server_name.get(len - 3..).unwrap_or_default();
    if !tail.eq_ignore_ascii_case("mcp") {
        return server_name;
    }
    if len >= 4 && bytes.get(len - 4) == Some(&b'-') {
        return server_name.get(..len - 4).unwrap_or_default();
    }
    server_name.get(..len - 3).unwrap_or_default()
}

/// `types.ts:694` `formatToolName(toolName, serverName, prefix)` — note the `.` → `_` sanitisation
/// of the *tool* name, which `cyrup_ext_subagents`' copy omits (MCP-205 row 4).
#[must_use]
pub fn format_tool_name(tool_name: &str, server_name: &str, prefix: ToolPrefix) -> String {
    let server_prefix = get_server_prefix(server_name, prefix);
    let sanitized = tool_name.replace('.', "_");
    if server_prefix.is_empty() { sanitized } else { format!("{server_prefix}_{sanitized}") }
}

/// `types.ts:770` `formatLegacyToolName` — the pre-2.x spelling, which sanitised `-` as well as `.`
/// and used the non-`preserveProviderValid` prefix.
fn format_legacy_tool_name(tool_name: &str, server_name: &str, prefix: ToolPrefix) -> String {
    let server_prefix = match prefix {
        ToolPrefix::None => String::new(),
        _ => {
            let base = get_server_prefix(server_name, prefix);
            // `getLegacyServerPrefix` re-sanitises with `preserveProviderValid = false`, i.e. only
            // `[A-Za-z0-9]` survives verbatim.
            let mut out = String::with_capacity(base.len());
            for ch in base.chars() {
                if ch.is_ascii_alphanumeric() {
                    out.push(ch);
                } else {
                    out.push('_');
                    out.push_str(&format!("{:x}", ch as u32));
                    out.push('_');
                }
            }
            out
        }
    };
    let sanitized: String = tool_name.chars().map(|c| if c == '.' || c == '-' { '_' } else { c }).collect();
    if server_prefix.is_empty() { sanitized } else { format!("{server_prefix}_{sanitized}") }
}

/// `types.ts:704` `resolveToolPrefix(definition, globalPrefix)` — the per-server override wins.
#[must_use]
pub fn resolve_tool_prefix(definition: Option<&ServerEntry>, global_prefix: ToolPrefix) -> ToolPrefix {
    definition.and_then(|entry| entry.tool_prefix).unwrap_or(global_prefix)
}

/// `types.ts:779` `getToolNameCandidates(toolName, serverName, prefix, includeLegacy = true)`.
///
/// The set every `includeTools` / `excludeTools` / `searchKeywords` pattern is matched against: the
/// bare name, the name under the effective prefix and under all three concrete modes, and — when
/// `include_legacy` — the same five under `-`→`_` normalisation plus the four legacy spellings.
/// An [`IndexSet`] rather than a `HashSet` so the insertion order upstream's `Set` preserves is
/// preserved here too; only membership is read today, but the order is free.
#[must_use]
pub fn get_tool_name_candidates(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    include_legacy: bool,
) -> IndexSet<String> {
    let mut candidates = IndexSet::new();
    candidates.insert(tool_name.to_string());
    candidates.insert(format_tool_name(tool_name, server_name, prefix));
    candidates.insert(format_tool_name(tool_name, server_name, ToolPrefix::Server));
    candidates.insert(format_tool_name(tool_name, server_name, ToolPrefix::Short));
    candidates.insert(format_tool_name(tool_name, server_name, ToolPrefix::Mcp));
    if include_legacy {
        let legacy_tool_name = tool_name.replace('-', "_");
        candidates.insert(legacy_tool_name.clone());
        candidates.insert(format_tool_name(&legacy_tool_name, server_name, prefix));
        candidates.insert(format_tool_name(&legacy_tool_name, server_name, ToolPrefix::Server));
        candidates.insert(format_tool_name(&legacy_tool_name, server_name, ToolPrefix::Short));
        candidates.insert(format_tool_name(&legacy_tool_name, server_name, ToolPrefix::Mcp));
        candidates.insert(format_legacy_tool_name(tool_name, server_name, prefix));
        candidates.insert(format_legacy_tool_name(tool_name, server_name, ToolPrefix::Server));
        candidates.insert(format_legacy_tool_name(tool_name, server_name, ToolPrefix::Short));
        candidates.insert(format_legacy_tool_name(tool_name, server_name, ToolPrefix::Mcp));
        candidates.insert(format_tool_name(tool_name, server_name, prefix).replace('-', "_"));
        candidates.insert(format_tool_name(tool_name, server_name, ToolPrefix::Server).replace('-', "_"));
        candidates.insert(format_tool_name(tool_name, server_name, ToolPrefix::Short).replace('-', "_"));
        candidates.insert(format_tool_name(tool_name, server_name, ToolPrefix::Mcp).replace('-', "_"));
    }
    candidates
}

/// `types.ts:814` `globToRegExp(pattern)` — `*` → `.*`, `?` → `.`, everything in
/// `[.+^${}()|[\]\\]` escaped, anchored at both ends.
///
/// Compiled with the same explicit ceilings as the search path; a pattern that will not compile
/// simply matches nothing, which is upstream's behaviour for a pattern that matches nothing.
fn glob_to_regex(pattern: &str) -> Option<regex::Regex> {
    let mut out = String::with_capacity(pattern.len() * 2 + 2);
    out.push('^');
    for ch in pattern.chars() {
        match ch {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '.' | '+' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out.push('$');
    regex::RegexBuilder::new(&out)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
        .build()
        .ok()
}

/// `types.ts:830` `matchesToolPattern(candidates, patterns)`.
///
/// An empty or absent pattern list is `false`. A pattern containing neither `*` nor `?` is an exact
/// membership test; a glob is compiled and tested against every candidate.
#[must_use]
pub fn matches_tool_pattern(candidates: &IndexSet<String>, patterns: Option<&[String]>) -> bool {
    let Some(patterns) = patterns else { return false };
    if patterns.is_empty() {
        return false;
    }
    for pattern in patterns {
        let is_glob = pattern.contains('*') || pattern.contains('?');
        if !is_glob && candidates.contains(pattern) {
            return true;
        }
        if is_glob
            && let Some(matcher) = glob_to_regex(pattern)
            && candidates.iter().any(|candidate| matcher.is_match(candidate))
        {
            return true;
        }
    }
    false
}

/// `types.ts:857` `indexHasOtherCurrentMatch(index, toolName, currentCandidates, pattern)`.
///
/// "Does this selector reach a tool other than mine?" A non-glob pattern collides when it is some
/// *other* server's candidate and not one of mine. A glob collides when it matches **more**
/// candidates overall than it matches of mine — which is why the caller's set spans every server
/// including this one, and why a naive `any(matches)` would report a self-match as a collision.
///
/// **Not memoised, unlike upstream post-`14c0e6c`.** Upstream hoists `matcherByPattern` and
/// `matchingCountByPattern` onto a `ToolSelectorCandidateIndex` built once per filtered server
/// (`types.ts:811-829`); this free function takes a bare `&IndexSet` and so recompiles the glob and
/// re-walks the whole candidate set once per **(tool, pattern)** pair — not, as an earlier revision
/// of this comment claimed, once per pattern. Only reached when a pattern matches a legacy-only
/// candidate, so it is a cost, never a behaviour difference; the planned close is to delete this
/// function in favour of [`crate::registration::CandidateIndex`], which is the memoising port, when
/// MCP-207 collapses the two selector paths (see MCP-249). `additionalCurrentCandidatesByToolName`
/// has no caller on this path and is not modelled.
fn index_has_other_current_match(
    all_candidates: &IndexSet<String>,
    current_candidates: &IndexSet<String>,
    pattern: &str,
) -> bool {
    let is_glob = pattern.contains('*') || pattern.contains('?');
    if !is_glob {
        return all_candidates.contains(pattern) && !current_candidates.contains(pattern);
    }
    let Some(matcher) = glob_to_regex(pattern) else { return false };
    let total = all_candidates.iter().filter(|candidate| matcher.is_match(candidate)).count();
    if total == 0 {
        return false;
    }
    let mine = current_candidates
        .iter()
        .filter(|candidate| all_candidates.contains(*candidate) && matcher.is_match(candidate))
        .count();
    total > mine
}

/// `types.ts:891` `matchesToolSelector(toolName, serverName, prefix, patterns,
/// otherCurrentCandidates)`.
///
/// **Two tiers, and the second is the whole point.** A pattern that matches one of the tool's
/// *current* names selects it outright. Only when that misses does the **legacy** residue — the
/// pre-2.x spellings, minus everything already in the current set — get a look, and then only for a
/// pattern that does not also reach some other configured tool. That is what stops a legacy
/// selector written for one server from silently capturing another server's tool after a rename.
fn matches_tool_selector(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    patterns: Option<&[String]>,
    all_current_candidates: Option<&IndexSet<String>>,
) -> bool {
    let Some(patterns) = patterns else { return false };
    if patterns.is_empty() {
        return false;
    }
    let current = get_tool_name_candidates(tool_name, server_name, prefix, false);
    if matches_tool_pattern(&current, Some(patterns)) {
        return true;
    }
    let Some(all_candidates) = all_current_candidates else {
        // No collision context: fall back to the full legacy-inclusive set.
        let legacy = get_tool_name_candidates(tool_name, server_name, prefix, true);
        return matches_tool_pattern(&legacy, Some(patterns));
    };
    let mut legacy = get_tool_name_candidates(tool_name, server_name, prefix, true);
    for candidate in &current {
        legacy.shift_remove(candidate);
    }
    patterns.iter().any(|pattern| {
        matches_tool_pattern(&legacy, Some(std::slice::from_ref(pattern)))
            && !index_has_other_current_match(all_candidates, &current, pattern)
    })
}

/// `types.ts:912` `isToolIncluded` — an absent or empty `includeTools` includes everything.
fn is_tool_included(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    include_tools: Option<&[String]>,
    all_current_candidates: Option<&IndexSet<String>>,
) -> bool {
    match include_tools {
        // `!Array.isArray(includeTools) || includeTools.length === 0` — both spellings of "no
        // allowlist" include everything.
        None | Some([]) => true,
        Some(patterns) => {
            matches_tool_selector(tool_name, server_name, prefix, Some(patterns), all_current_candidates)
        }
    }
}

/// `types.ts:940` `isToolAllowed(toolName, serverName, prefix, includeTools, excludeTools,
/// otherCurrentCandidates)` — the allowlist first, then the denylist.
///
/// `all_current_candidates` is the cross-server collision set (MCP-198), spanning **every**
/// cache-valid enabled server including this one.
#[must_use]
pub fn is_tool_allowed(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    include_tools: Option<&[String]>,
    exclude_tools: Option<&[String]>,
    all_current_candidates: Option<&IndexSet<String>>,
) -> bool {
    is_tool_included(tool_name, server_name, prefix, include_tools, all_current_candidates)
        && !matches_tool_selector(
            tool_name,
            server_name,
            prefix,
            exclude_tools,
            all_current_candidates,
        )
}

// **De-duplicated at integration (MCP-203).** `resourceNameToToolName` lives in
// [`crate::registration`] with the rest of the naming grammar, and the copy that stood here had
// drifted: upstream's `result = "resource" + (result ? "_" + result : "")`
// (`resource-tools.ts:13`) yields `"resource"` for an all-punctuation name, where the copy yielded
// `"resource_"`. Every `read_<name>` resource tool that a server names `"///"` would have been
// registered under a name the cache could never match.
pub use crate::registration::resource_name_to_tool_name;

/// `tool-metadata.ts:154` `findToolByName(metadata, toolName)` — exact `name` match first,
/// otherwise compare with `-` globally replaced by `_` on **both** sides.
#[must_use]
pub fn find_tool_by_name<'a>(metadata: &'a [ToolMetadata], tool_name: &str) -> Option<&'a ToolMetadata> {
    if let Some(exact) = metadata.iter().find(|tool| tool.name == tool_name) {
        return Some(exact);
    }
    let normalized = tool_name.replace('-', "_");
    metadata.iter().find(|tool| tool.name.replace('-', "_") == normalized)
}

// **De-duplicated at integration (MCP-206).** `truncateAtWord` lives in
// [`crate::registration`], and the copy that stood here had drifted on units: JS `.length` and
// `.slice` count **UTF-16 code units** (`utils.ts:265-267`), which is what `registration.rs`
// measures; the copy counted Unicode scalar values, so an emoji-bearing description was cut at a
// different point than the description the cache and the subagent bridge compute.
pub use crate::registration::truncate_at_word;

// ==================================================================================================
// 3 · `search-ranking.ts` — 206 lines of allocation-free integer scoring, no I/O
//     (MCP-172, MCP-173, MCP-174, MCP-175, MCP-176, MCP-177, MCP-178)
// ==================================================================================================

/// `search-ranking.ts:54` `normalizeSearchText(value)` — three steps, **in this order**.
///
/// 1. `replace(/([a-z0-9])([A-Z])/g, "$1 $2")` — camelCase split, **before** lowercasing, so `ID`
///    does not split (the pattern needs a lowercase or digit *before* the uppercase). The JS global
///    replace is non-overlapping: it consumes both characters of a match, so `"aBcD"` becomes
///    `"a Bc D"`, and this scanner reproduces that by advancing two characters on a hit.
/// 2. `replace(/[_./:-]+/g, " ")` — the class is exactly `_ . / : -`, runs collapsed to one space.
/// 3. `toLowerCase()`.
///
/// Hand-written rather than `regex` (MCP-172): both patterns are trivial and a scanner keeps the
/// ranking path allocation-light.
#[must_use]
pub fn normalize_search_text(value: &str) -> String {
    // Step 1 — camelCase split.
    let chars: Vec<char> = value.chars().collect();
    let mut split = String::with_capacity(value.len() + 8);
    let mut index = 0usize;
    while index < chars.len() {
        let current = chars.get(index).copied().unwrap_or('\0');
        let next = chars.get(index + 1).copied();
        let boundary = matches!(next, Some(n) if n.is_ascii_uppercase())
            && (current.is_ascii_lowercase() || current.is_ascii_digit());
        if boundary {
            split.push(current);
            split.push(' ');
            if let Some(n) = next {
                split.push(n);
            }
            index += 2;
        } else {
            split.push(current);
            index += 1;
        }
    }

    // Step 2 — separator runs to a single space.
    let mut collapsed = String::with_capacity(split.len());
    let mut in_run = false;
    for ch in split.chars() {
        if matches!(ch, '_' | '.' | '/' | ':' | '-') {
            if !in_run {
                collapsed.push(' ');
            }
            in_run = true;
        } else {
            collapsed.push(ch);
            in_run = false;
        }
    }

    // Step 3.
    collapsed.to_lowercase()
}

/// `search-ranking.ts:62` `tokenize(value)` =
/// `normalizeSearchText(value).split(/[^a-z0-9]+/).filter(Boolean)`.
///
/// ASCII-only by construction: any non-`[a-z0-9]` byte is a separator, so a non-ASCII identifier
/// tokenizes to nothing. That is upstream's behaviour and is load-bearing for the coverage gate.
#[must_use]
pub fn tokenize(value: &str) -> Vec<String> {
    let normalized = normalize_search_text(value);
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in normalized.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// One scored `(server, tool)` pair — `search-ranking.ts:20` `RankedToolMatch`.
#[derive(Debug, Clone, PartialEq)]
pub struct RankedToolMatch {
    /// The `mcpServers` key the tool came from.
    pub server: String,
    /// The tool's metadata record.
    pub tool: ToolMetadata,
    /// The integer score. **Always `0` on the regex path**, which is never sorted.
    pub score: i64,
}

/// The three-tier token ladder, shared by the field loop and the keyword loop.
///
/// Exactly one tier fires per `(field, query token)` pair, first match wins:
/// * the token is a field token ⇒ `weight * 4`;
/// * some field token prefixes the query token, **or** the query token prefixes a field token of at
///   least [`MIN_STEM_LENGTH`] characters ⇒ `weight * 2`;
/// * the raw (normalised) value contains the token ⇒ `weight * 1`.
///
/// **The stem rule is deliberately asymmetric.** `field.starts_with(token)` matches at any length,
/// but `token.starts_with(field)` only when the field token is ≥ 4 characters, because real
/// descriptions tokenize possessives into single letters.
fn token_bonus(weight: i64, field_tokens: &[String], raw_contains: bool, token: &str) -> Option<i64> {
    if field_tokens.iter().any(|field_token| field_token == token) {
        return Some(weight * 4);
    }
    let stemmed = field_tokens.iter().any(|field_token| {
        field_token.starts_with(token)
            || (field_token.chars().count() >= MIN_STEM_LENGTH && token.starts_with(field_token.as_str()))
    });
    if stemmed {
        return Some(weight * 2);
    }
    if raw_contains {
        return Some(weight);
    }
    None
}

/// The phrase ladder for one field: exact ⇒ `×14` (also sets `whole_field_exact`), prefix ⇒ `×9`,
/// substring ⇒ `×6`. First match wins; a miss contributes nothing and does not set `phrase_matched`.
fn phrase_bonus(weight: i64, value: &str, normalized_query: &str) -> Option<(i64, bool)> {
    if value == normalized_query {
        return Some((weight * 14, true));
    }
    if value.starts_with(normalized_query) {
        return Some((weight * 9, false));
    }
    if value.contains(normalized_query) {
        return Some((weight * 6, false));
    }
    None
}

/// `search-ranking.ts:65` `scoreToolMatch(tool, server, query, keywords?)`.
///
/// `None` is "this tool does not match at all" — the coverage gate's verdict, not a zero score.
///
/// Steps, in order (13d §7):
/// 1. normalise and tokenize the query; **empty tokens ⇒ `None`**;
/// 2. four fields — `name`, `originalName`, `server`, `description` — in that exact order, each
///    normalised but **not trimmed** (a leading space in a description defeats `starts_with`);
/// 3. one phrase bonus per field;
/// 4. one token bonus per (field, query token);
/// 5. keywords, only when `Some` and non-empty — the phrase bonus is a **max over phrases** added
///    **once**, deliberately, so a query spanning two unrelated keywords cannot collect it twice;
/// 6. the coverage gate;
/// 7. the final bonuses.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn score_tool_match(
    tool: &ToolMetadata,
    server: &str,
    query: &str,
    keywords: Option<&[String]>,
) -> Option<i64> {
    let normalized_query = normalize_search_text(query).trim().to_string();
    let query_tokens = tokenize(query);
    if query_tokens.is_empty() {
        return None;
    }

    // Step 2 — the field order is the JS object literal's insertion order, and it matters because
    // the first phrase hit per field is the only one that scores.
    let fields: [(i64, String); 4] = [
        (WEIGHT_NAME, normalize_search_text(&tool.name)),
        (WEIGHT_ORIGINAL_NAME, normalize_search_text(&tool.original_name)),
        (WEIGHT_SERVER, normalize_search_text(server)),
        (WEIGHT_DESCRIPTION, normalize_search_text(&tool.description)),
    ];

    let mut score: i64 = 0;
    let mut phrase_matched = false;
    let mut whole_field_exact = false;
    let mut matched_tokens: IndexSet<String> = IndexSet::new();

    for (weight, value) in &fields {
        let field_tokens = tokenize(value);
        if let Some((bonus, exact)) = phrase_bonus(*weight, value, &normalized_query) {
            score += bonus;
            phrase_matched = true;
            whole_field_exact |= exact;
        }
        for token in &query_tokens {
            if let Some(bonus) = token_bonus(*weight, &field_tokens, value.contains(token.as_str()), token) {
                score += bonus;
                matched_tokens.insert(token.clone());
            }
        }
    }

    // Step 5 — configured keywords are discrete phrases, so the phrase-level bonus is computed per
    // phrase (best match wins) rather than on a joined string.
    if let Some(keywords) = keywords
        && !keywords.is_empty()
    {
        let weight = WEIGHT_KEYWORDS;
        let phrases: Vec<String> = keywords
            .iter()
            .map(|keyword| normalize_search_text(keyword).trim().to_string())
            .filter(|phrase| !phrase.is_empty())
            .collect();
        let mut phrase_score: i64 = 0;
        for phrase in &phrases {
            if let Some((bonus, exact)) = phrase_bonus(weight, phrase, &normalized_query) {
                phrase_score = phrase_score.max(bonus);
                phrase_matched = true;
                whole_field_exact |= exact;
            }
        }
        score += phrase_score;

        let keyword_tokens: Vec<String> = phrases.iter().flat_map(|phrase| tokenize(phrase)).collect();
        for token in &query_tokens {
            let raw_contains = phrases.iter().any(|phrase| phrase.contains(token.as_str()));
            if let Some(bonus) = token_bonus(weight, &keyword_tokens, raw_contains, token) {
                score += bonus;
                matched_tokens.insert(token.clone());
            }
        }
    }

    // Step 6 — the coverage gate. Without a phrase match, a 1-2 token query must match **all** its
    // tokens and a longer query must reach 0.6.
    let matched = matched_tokens.len();
    let total = query_tokens.len();
    let full_coverage = matched == total; // integer comparison, never a float equality
    let coverage = matched as f64 / total as f64;
    if !phrase_matched && (if total <= 2 { !full_coverage } else { coverage < 0.6 }) {
        return None;
    }

    // Step 7 — the final bonuses. `Math.round` on a positive value is Rust's `f64::round`.
    score += if full_coverage { 25 } else { (coverage * 10.0).round() as i64 };
    if let Some(first) = query_tokens.first()
        && tokenize(&fields.first().map(|(_, value)| value.clone()).unwrap_or_default())
            .iter()
            .any(|token| token == first)
    {
        score += 8;
    }
    if whole_field_exact {
        score += 20;
    }
    Some(score)
}

/// `search-ranking.ts:30` `resolveSearchKeywords(definition, toolOriginalName, serverName,
/// globalPrefix)`.
///
/// Keys match by original name, prefixed name, and glob — the same candidate set
/// `includeTools`/`excludeTools` use — and **all** matching entries are unioned, deduped, in key
/// order. A missing / non-object / array map yields `[]`; non-string and blank values are dropped.
///
/// **Two divergences forced by `ServerEntry::search_keywords`'s current type**
/// (`Option<BTreeMap<String, Vec<String>>>`, `config.rs`), both reported for integration:
/// * a `BTreeMap` **sorts** its keys, where `Object.entries` yields insertion order — so the union
///   order can differ from upstream's when two glob keys both match. `IndexMap` is the fix.
/// * `lenient` rejects the whole field when *any* value is not a `string[]`, where upstream drops
///   only the offending key (and only the offending element).
///
/// Configured keywords are searchable by ranked query **and** by regex, but never appear in
/// schemas, `describe` output, or the metadata cache.
#[must_use]
pub fn resolve_search_keywords(
    definition: Option<&ServerEntry>,
    tool_original_name: &str,
    server_name: &str,
    global_prefix: ToolPrefix,
) -> Vec<String> {
    let Some(map) = definition.and_then(|entry| entry.search_keywords.as_ref()) else {
        return Vec::new();
    };
    let candidates = get_tool_name_candidates(
        tool_original_name,
        server_name,
        resolve_tool_prefix(definition, global_prefix),
        true,
    );
    let mut keywords: Vec<String> = Vec::new();
    let mut seen: IndexSet<String> = IndexSet::new();
    for (pattern, values) in map {
        if !matches_tool_pattern(&candidates, Some(std::slice::from_ref(pattern))) {
            continue;
        }
        for value in values {
            let trimmed = value.trim();
            if trimmed.is_empty() || seen.contains(trimmed) {
                continue;
            }
            seen.insert(trimmed.to_string());
            keywords.push(trimmed.to_string());
        }
    }
    keywords
}

/// The rank tie-break — `String.prototype.localeCompare` with no locale (ICU root collation).
///
/// **MCP-171** offered three options; this takes the exact one by reusing
/// [`crate::config::locale_compare`], the `feruca` UCA collator already proven against Node in
/// `cyrup-tools/src/tools/ls.rs` and `cyrup-config/src/model.rs`. A fourth hand-rolled ASCII
/// approximation is exactly the drift those two exist to prevent, and the collator is only ever
/// asked to order equal-score results and a hint list — never to decide which tools match.
#[must_use]
pub fn rank_collate(left: &str, right: &str) -> std::cmp::Ordering {
    locale_compare(left, right)
}

/// `search-ranking.ts:152` `rankToolMatches(state, query, server?, includeKeywords = true)`.
///
/// Walks `state.toolMetadata` in **insertion order** (MCP-170), skips disabled servers and — when
/// `server` is set — non-matching ones, and sorts by score descending then [`rank_collate`]
/// ascending.
///
/// `has_keywords` is `includeKeywords && definition?.searchKeywords !== undefined`: **an empty
/// object still counts as present**, which changes whether `keywords` is `Some(&[])` or `None`.
/// `Some(&[])` is a no-op by [`score_tool_match`]'s non-empty guard — reproduced, not simplified,
/// because the distinction is what the "does not change scoring when the keyword list is empty"
/// conformance case pins.
#[must_use]
pub fn rank_tool_matches(
    config: &McpConfig,
    tool_metadata: &IndexMap<String, Vec<ToolMetadata>>,
    query: &str,
    server: Option<&str>,
    include_keywords: bool,
) -> Vec<RankedToolMatch> {
    let global_prefix = config.tool_prefix();
    let mut matches: Vec<RankedToolMatch> = Vec::new();
    for (server_name, metadata) in tool_metadata {
        if let Some(filter) = server
            && server_name != filter
        {
            continue;
        }
        let definition = config.mcp_servers.get(server_name);
        if definition.is_some_and(ServerEntry::is_disabled) {
            continue;
        }
        let has_keywords =
            include_keywords && definition.is_some_and(|entry| entry.search_keywords.is_some());
        for tool in metadata {
            let keywords = if has_keywords {
                Some(resolve_search_keywords(definition, &tool.original_name, server_name, global_prefix))
            } else {
                None
            };
            if let Some(score) =
                score_tool_match(tool, server_name, query, keywords.as_deref())
            {
                matches.push(RankedToolMatch {
                    server: server_name.clone(),
                    tool: tool.clone(),
                    score,
                });
            }
        }
    }
    matches.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| rank_collate(&a.tool.name, &b.tool.name)));
    matches
}

/// The result of [`paginate`] — `search-ranking.ts:176`'s return object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// The slice actually returned.
    pub items: Vec<T>,
    /// `items.length` of the **unpaginated** input.
    pub total: usize,
    /// Whether `next_offset` is `Some`.
    pub has_more: bool,
    /// `safeOffset + page.length`, but only when that is still inside the list.
    pub next_offset: Option<usize>,
}

/// `search-ranking.ts:176` `paginate(items, offset, limit)`.
///
/// `offset` and `limit` arrive from JSON as numbers, so both are `f64` here:
/// `safeOffset = Number.isFinite(offset) ? Math.max(0, Math.trunc(offset)) : 0`;
/// `safeLimit = Number.isFinite(limit) ? Math.max(1, Math.trunc(limit)) : 1`.
/// JS `slice` clamps both ends and never throws — Rust must clamp explicitly.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
pub fn paginate<T: Clone>(items: &[T], offset: f64, limit: f64) -> Page<T> {
    let safe_offset: usize = if offset.is_finite() { offset.trunc().max(0.0) as usize } else { 0 };
    let safe_limit: usize = if limit.is_finite() { limit.trunc().max(1.0) as usize } else { 1 };
    let total = items.len();
    let start = safe_offset.min(total);
    let end = start.saturating_add(safe_limit).min(total);
    let page: Vec<T> = items.get(start..end).unwrap_or_default().to_vec();
    // Upstream computes `safeOffset + page.length`, NOT `start + page.length`; for an offset past
    // the end the page is empty, so the two agree.
    let next_offset_raw = safe_offset.saturating_add(page.len());
    let has_more = next_offset_raw < total;
    Page {
        items: page,
        total,
        has_more,
        next_offset: if has_more { Some(next_offset_raw) } else { None },
    }
}

/// `search-ranking.ts:194` `rankSuggestions(state, name, limit)` — the "Did you mean:" list.
///
/// Strips the **longest** matching server prefix — probing modes `server`, `short` and `mcp`
/// regardless of the configured mode, with `none` deliberately excluded because it yields an empty
/// prefix — and re-ranks the remainder with **keywords disabled**, so a suggestion never comes from
/// a configured alias.
#[must_use]
pub fn rank_suggestions(
    config: &McpConfig,
    tool_metadata: &IndexMap<String, Vec<ToolMetadata>>,
    name: &str,
    limit: usize,
) -> Vec<String> {
    let mut stripped: Vec<String> = Vec::new();
    for server in config.mcp_servers.keys() {
        for mode in [ToolPrefix::Server, ToolPrefix::Short, ToolPrefix::Mcp] {
            let candidate = get_server_prefix(server, mode);
            if candidate.is_empty() || !name.starts_with(&format!("{candidate}_")) {
                continue;
            }
            stripped.push(candidate);
        }
    }
    // `sort((a, b) => b.length - a.length)` — descending prefix length, stably, so ties keep
    // configuration order.
    stripped.sort_by_key(|candidate| std::cmp::Reverse(candidate.len()));
    let query = stripped
        .first()
        .and_then(|candidate| name.get(candidate.len() + 1..).map(str::to_string))
        .unwrap_or_else(|| name.to_string());
    rank_tool_matches(config, tool_metadata, &query, None, false)
        .into_iter()
        .take(limit)
        .map(|entry| entry.tool.name)
        .collect()
}

// ==================================================================================================
// 4 · The collaborator seam — `ProxyEnv`, and the context every mode takes
// ==================================================================================================

/// `types.ts:138` `McpConnection["status"]` — the three states a connection can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Handshake completed; `tools/call` will be accepted.
    Connected,
    /// The transport closed, or was never opened.
    Closed,
    /// The server answered `401`/`WWW-Authenticate`; an OAuth flow must run first.
    NeedsAuth,
}

/// What `manager.connect` / `manager.reconnect` hand back to [`execute_connect`].
///
/// The `buildToolMetadata(connection.tools, connection.resources, …)` step is applied by the
/// implementor rather than here: that builder is `tool-metadata.ts`'s and is owned by 13e
/// (MCP-207). Everything downstream of it — where the metadata is stored, when instructions are
/// **deleted** rather than set, and the eight-step commit order — stays in this file, because that
/// order is the port.
#[derive(Debug, Clone, Default)]
pub struct ConnectOutcome {
    /// `connection.status`.
    pub status: Option<ConnectionStatus>,
    /// `buildToolMetadata(...).metadata` for this server.
    pub metadata: Vec<ToolMetadata>,
    /// `connection.instructions` — `None` means **delete** the cached entry, not "leave it".
    pub instructions: Option<String>,
    /// `connection.promptDiscoveryFailed` — when false, prompt metadata is reconstructed and the
    /// server joins `promptMetadataLive`.
    pub prompt_discovery_failed: bool,
}

impl ConnectOutcome {
    /// `connection.status === "connected"`.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.status == Some(ConnectionStatus::Connected)
    }

    /// `connection.status === "needs-auth"`.
    #[must_use]
    pub fn needs_auth(&self) -> bool {
        self.status == Some(ConnectionStatus::NeedsAuth)
    }
}

/// The already-transformed payload of one `tools/call`.
///
/// `content` has been through `transformMcpContent` / `resolveMcpResultContent`
/// (`tool-registrar.ts`, owned by 13e) before it reaches this file; `raw` is the untouched
/// `CallToolResult` the output guard stores as `rawMcpResult`.
#[derive(Debug, Clone, Default)]
pub struct CallToolOutcome {
    /// The transformed content blocks.
    pub content: Vec<Content>,
    /// `result.isError` — the discriminator for the `tool_error` path.
    pub is_error: bool,
    /// The raw MCP result, for `guardedMcpDetails`' `mcpResult` key.
    pub raw: Option<Value>,
}

/// The three failures [`execute_call`]'s catch block distinguishes (13d §10, MCP-165).
#[derive(Debug)]
pub enum ProxyCallError {
    /// `session-recovery.ts`'s `SessionRecoveryAuthRequiredError` — a mid-request `needs-auth` that
    /// [`attempt_auto_auth`] could not rescue. `auth_message` is the error's own text when it
    /// carried one.
    SessionRecoveryAuthRequired {
        /// The server the recovery was attempted against.
        server: String,
        /// `error.authMessage`, when present.
        auth_message: Option<String>,
    },
    /// rmcp's `UrlElicitationRequiredError` — the server wants a URL interaction first.
    UrlElicitationRequired {
        /// Opaque detail handed straight back to `manager.handleUrlElicitationRequired`.
        detail: String,
    },
    /// Everything else, including aborts.
    Other(McpError),
}

/// `manager.handleUrlElicitationRequired`'s verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlElicitationAction {
    /// The user opened the URL; the tool did not run and must be retried.
    Accept,
    /// The user refused.
    Decline,
    /// The interaction was cancelled.
    Cancel,
}

impl UrlElicitationAction {
    /// The `details.action` spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            UrlElicitationAction::Accept => "accept",
            UrlElicitationAction::Decline => "decline",
            UrlElicitationAction::Cancel => "cancel",
        }
    }
}

/// `types.ts:477 @v2.26.1` `McpToolApprovalOrigin` — which surface is asking.
///
/// Upstream has five: `"proxy" | "direct" | "script" | "resource" | "iframe"`. Two are cut and
/// neither leaves a hole: `"script"` is **Cut 4** (`mcpScript` / the JS worker) and `"iframe"` is
/// **Cut 2** (MCP Apps, `ui-server.ts:474`). The three that survive are the three surfaces that can
/// still reach a tool.
///
/// The value reaches only `requestBrokerApproval` upstream, which is MCP-233's cut — so it is
/// carried and not yet read. See [`ensure_tool_call_approved`] for why it stays in the signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOrigin {
    /// `proxy-modes.ts:1145` — a `mcp({tool})` call through the gateway.
    Proxy,
    /// `direct-tools.ts:440 @v2.26.1` — a registered per-tool direct tool.
    Direct,
    /// A resource tool, derived when `toolMeta.resourceUri` is set. Reached from **both** call
    /// sites, which is why the derivation is a constructor rather than a literal.
    Resource,
}

impl ApprovalOrigin {
    /// `proxy-modes.ts:1145` `origin ?? (toolMeta.resourceUri ? "resource" : "proxy")` — the
    /// gateway's derivation, applied only when the caller passed no explicit origin.
    #[must_use]
    pub const fn for_proxy_call(resource_uri: Option<&String>) -> Self {
        match resource_uri {
            Some(_) => ApprovalOrigin::Resource,
            None => ApprovalOrigin::Proxy,
        }
    }

    /// `direct-tools.ts:440 @v2.26.1` `spec.resourceUri ? "resource" : "direct"` — the direct-tool
    /// derivation.
    ///
    /// **The two derivations differ in their fallback and only there**, which is the whole reason
    /// both are written out: a resource tool reports `resource` whichever surface invoked it, while
    /// a plain tool reports the surface. Collapsing them into one helper would make every direct
    /// tool call claim it came through the gateway.
    #[must_use]
    pub const fn for_direct_tool(resource_uri: Option<&String>) -> Self {
        match resource_uri {
            Some(_) => ApprovalOrigin::Resource,
            None => ApprovalOrigin::Direct,
        }
    }

    /// The `details.origin` spelling — `types.ts:477 @v2.26.1`'s own strings.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ApprovalOrigin::Proxy => "proxy",
            ApprovalOrigin::Direct => "direct",
            ApprovalOrigin::Resource => "resource",
        }
    }
}

/// `tool-approval.ts`'s `ToolCallApprovalResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// `{ok: true}`.
    Approved,
    /// `{ok: false, reason: "denied"}` — the user said no.
    Denied,
    /// `{ok: false, reason: …}` — approval is required and there is no interactive session.
    NoInteractiveSession,
}

/// The per-call-site half of `guardMcpOutput`'s options.
///
/// The limits themselves come from `resolveMcpOutputGuardOptions(config.settings)` — already
/// available as [`crate::config::McpSettings::output_guard`] — and are read by the implementor;
/// only these four vary between the three call sites.
#[derive(Debug, Clone, Default)]
pub struct OutputGuardOptions {
    /// `"Error: "` on the tool-error path, `"Failed to call tool: "` in the catch, else empty.
    pub prefix: String,
    /// `"\n\nExpected parameters:\n<formatSchema>"` when an input schema exists.
    pub suffix: String,
    /// `"Tool execution failed"` on the tool-error path.
    pub empty_text_fallback: Option<String>,
    /// The untouched MCP result, stored as `details.mcpResult` when it survives the size cap.
    pub raw_mcp_result: Option<Value>,
}

/// `mcp-output-guard.ts`'s `GuardedMcpOutput`.
#[derive(Debug, Clone, Default)]
pub struct GuardedOutput {
    /// The bounded content actually returned to the model.
    pub content: Vec<Content>,
    /// `details.mcpResult`, when the raw result fit under `detailsMaxBytes`.
    pub mcp_result: Option<Value>,
    /// `details.outputGuard`, when text was truncated or spilled to a file.
    pub output_guard: Option<Value>,
}

impl GuardedOutput {
    /// `mcp-output-guard.ts:78` `guardedMcpDetails(guarded)` — each key present **only** when set.
    fn write_details(&self, details: &mut JsonMap<String, Value>) {
        if let Some(result) = &self.mcp_result {
            details.insert("mcpResult".to_string(), result.clone());
        }
        if let Some(guard) = &self.output_guard {
            details.insert("outputGuard".to_string(), guard.clone());
        }
    }
}

/// The subsystems the proxy modes call into, late-bound.
///
/// Each method names the upstream function it stands for. Implementing this trait is the whole of
/// integrating 13d with 13a/13c/13e/13g; the call order and branch structure live in this file.
#[async_trait::async_trait]
pub trait ProxyEnv: Send + Sync {
    // --- server-manager.ts -----------------------------------------------------------------------
    /// `state.manager.getConnection(server)?.status`.
    fn get_connection(&self, server: &str) -> Option<ConnectionStatus>;
    /// `state.manager.isConnecting(server)` — drives [`execute_search`]'s zero-result hint.
    fn is_connecting(&self, server: &str) -> bool;
    /// `state.manager.connect(server, definition, signal)`.
    async fn connect(&self, server: &str, cancel: &CancelToken) -> McpResult<ConnectOutcome>;
    /// `state.manager.reconnect(server, definition, currentConnection, signal)`.
    async fn reconnect(&self, server: &str, cancel: &CancelToken) -> McpResult<ConnectOutcome>;
    /// `init.ts`'s `lazyConnect(state, server, signal)` — `true` iff the server ended `connected`.
    async fn lazy_connect(&self, server: &str, cancel: &CancelToken) -> bool;
    /// `state.manager.close(server)`.
    async fn close(&self, server: &str);
    /// `state.manager.touch(server)`.
    fn touch(&self, server: &str);
    /// `state.manager.incrementInFlight(server)`.
    fn increment_in_flight(&self, server: &str);
    /// `state.manager.decrementInFlight(server)`.
    fn decrement_in_flight(&self, server: &str);
    /// `withSessionRecovery(..., conn => abortable(conn.client.callTool({name, arguments}), signal))`.
    ///
    /// The cancellation wrapper belongs on **this** side: rmcp's shape is
    /// `Peer::send_request_with_option(...)` → `RequestHandle`, with a task calling
    /// `RequestHandle::cancel(reason)` when `cancel` fires.
    ///
    /// `recovery` is the `onNeedsAuth` callback — call [`AuthRecovery::recover`] from inside the
    /// recovery loop rather than re-deriving the ladder, so the single-shot latch is honoured
    /// (MCP-162).
    async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: JsonMap<String, Value>,
        recovery: &AuthRecovery<'_>,
        cancel: &CancelToken,
    ) -> Result<CallToolOutcome, ProxyCallError>;
    /// `withSessionRecovery(..., conn => conn.client.readResource({uri}, requestOptions))`.
    ///
    /// **Deliberately not wrapped in `abortable`** — upstream's asymmetry, reproduced rather than
    /// "fixed": a resource read is cancellable only through the request options' own signal
    /// (13d §10). `recovery` is the same `onNeedsAuth` callback [`ProxyEnv::call_tool`] takes.
    async fn read_resource(
        &self,
        server: &str,
        uri: &str,
        recovery: &AuthRecovery<'_>,
        cancel: &CancelToken,
    ) -> Result<Vec<Content>, ProxyCallError>;
    /// `state.manager.handleUrlElicitationRequired(server, error)`.
    async fn handle_url_elicitation_required(&self, server: &str, detail: &str) -> UrlElicitationAction;

    // --- init.ts ---------------------------------------------------------------------------------
    /// `getFailureAgeSeconds(state, server)` — `None` outside the 60-second backoff window.
    fn failure_age_seconds(&self, server: &str) -> Option<u64>;
    /// `recordFailure(state, server, message)`. Never called for an abort.
    fn record_failure(&self, server: &str, message: &str);
    /// `clearFailure(state, server)`.
    fn clear_failure(&self, server: &str);
    /// `updateStatusBar(state)`.
    fn update_status_bar(&self);
    /// `updateServerMetadata(state, server)` — refresh `state.toolMetadata` from a live connection.
    fn update_server_metadata(&self, server: &str);
    /// `updateMetadataCache(state, server)` — write `<agent_dir>/mcp-cache.json`.
    fn update_metadata_cache(&self, server: &str);
    /// `markKeepAliveAfterConnect(state, server)`.
    fn mark_keep_alive_after_connect(&self, server: &str);
    /// `state.promptMetadata.set(...)` + `state.promptMetadataLive.add(...)` via
    /// `reconstructPromptMetadata` — run only when `!connection.promptDiscoveryFailed`.
    fn commit_prompt_metadata(&self, server: &str);
    /// `syncToolSurface(ctx)` — re-derive direct tools and the `mcp` description after a connect
    /// (dispatch arm 4). A no-op until `HA-1` lands (MCP-193).
    fn sync_tool_surface(&self);

    // --- mcp-auth-flow.ts ------------------------------------------------------------------------
    /// `supportsOAuth(definition)`.
    fn supports_oauth(&self, definition: &ServerEntry) -> bool;
    /// `utils.ts:167` `resolveServerUrl(definition)`.
    ///
    /// `Ok(None)` is a falsy URL (a stdio server); `Err` is the **throw** — a missing `${VAR}` or a
    /// URL that will not parse after interpolation. [`attempt_auto_auth`] treats those differently
    /// and the distinction is load-bearing.
    fn resolve_server_url(&self, definition: &ServerEntry) -> McpResult<Option<String>>;
    /// `authenticate(server, url, definition, {authStorageOptions?, signal?, runtime})`.
    async fn authenticate(
        &self,
        server: &str,
        server_url: &str,
        definition: &ServerEntry,
        cancel: &CancelToken,
    ) -> McpResult<()>;
    /// `startAuth(...)` → `{authorizationUrl}`. `Ok(None)` means the flow completed synchronously
    /// (client-credentials).
    async fn start_auth(
        &self,
        server: &str,
        server_url: &str,
        definition: &ServerEntry,
        cancel: &CancelToken,
    ) -> McpResult<Option<String>>;
    /// `completeAuthFromInput(server, input, opts)` → the resulting status string.
    async fn complete_auth_from_input(
        &self,
        server: &str,
        input: &str,
        cancel: &CancelToken,
    ) -> McpResult<String>;

    // --- tool-metadata.ts / ts-shape.ts -----------------------------------------------------------
    /// `formatSchema(schema, indent)`. Note [`execute_describe`] passes the **default** `"  "` while
    /// [`execute_search`] passes `"    "`.
    fn format_schema(&self, schema: &Value, indent: &str) -> String;
    /// `renderTsShape(schema)` — `None` is upstream's `null`, which forks to `Parameters:`.
    fn render_ts_shape(&self, schema: &Value) -> Option<String>;

    // --- tool-approval.ts ------------------------------------------------------------------------
    /// `isToolCallApprovalRequired(config, server, toolMeta, state.toolMetadata)` — the
    /// `" (requires approval)"` marker in `describe` and `search`.
    fn is_tool_call_approval_required(&self, server: &str, tool: &ToolMetadata) -> bool;
    /// `ensureToolCallApproved(state, server, toolMeta, args, signal, origin)`.
    async fn ensure_tool_call_approved(
        &self,
        server: &str,
        tool: &ToolMetadata,
        arguments: &Value,
        origin: ApprovalOrigin,
        cancel: &CancelToken,
    ) -> ApprovalOutcome;

    // --- mcp-output-guard.ts ---------------------------------------------------------------------
    /// `guardMcpOutput(content, {...resolveMcpOutputGuardOptions(settings), ...options})`.
    async fn guard_mcp_output(&self, content: Vec<Content>, options: OutputGuardOptions) -> GuardedOutput;

    // --- pi.getAllTools() ------------------------------------------------------------------------
    /// `getPiTools?.()` — `HostServices::all_tool_names()`.
    ///
    /// **`None` is not a defect**: upstream's `getPiTools` is an *optional* parameter invoked as
    /// `getPiTools?.()`, and `None` reproduces that branch exactly — skip the native-tool check and
    /// fall through to `tool_not_found` (MCP-199). Do **not** synthesise a built-in name list as a
    /// floor: that would answer `native_tool` for a built-in the session actually disabled, which pi
    /// never does.
    fn all_tool_names(&self) -> Option<Vec<String>>;
}

/// One generation's proxy-mode context — upstream's `state: McpExtensionState` parameter.
///
/// Everything the modes read that already exists on [`McpState`] is read through `state`;
/// [`ProxyCtx::tool_metadata`] is held here because `crate::state::ServerToolMetadata` is still 13e's
/// forward declaration and cannot yet carry a `Vec<ToolMetadata>`. **Integration:** once
/// MCP-207 lands, delete this field and project `McpState::tool_metadata` instead — every read site
/// below goes through [`ProxyCtx::with_metadata`], so the swap is one function.
pub struct ProxyCtx {
    /// The generation's runtime record: config, owner, UI handle, `serverInstructions`.
    pub state: Arc<McpState>,
    /// `state.toolMetadata: Map<string, ToolMetadata[]>`, **insertion-ordered** (MCP-170).
    ///
    /// Insertion order decides which server wins a fuzzy tool-name match, which disabled server is
    /// named in an error, and the output order of the unsorted regex search path. A `BTreeMap` here
    /// would change observable behaviour whenever server names are not already alphabetical.
    pub tool_metadata: Mutex<IndexMap<String, Vec<ToolMetadata>>>,
    /// The late-bound collaborators.
    pub env: Arc<dyn ProxyEnv>,
}

impl ProxyCtx {
    /// Build a context over a live state and a collaborator implementation.
    #[must_use]
    pub fn new(state: Arc<McpState>, env: Arc<dyn ProxyEnv>) -> Self {
        Self { state, tool_metadata: Mutex::new(IndexMap::new()), env }
    }

    /// The one read path onto `state.toolMetadata`. A poisoned lock degrades to "no metadata",
    /// never to a panic (the crate denies `clippy::panic` and `init` must not fail).
    fn with_metadata<R>(&self, f: impl FnOnce(&IndexMap<String, Vec<ToolMetadata>>) -> R) -> R {
        match self.tool_metadata.lock() {
            Ok(guard) => f(&guard),
            Err(_) => f(&IndexMap::new()),
        }
    }

    /// `isToolCallApprovalRequired(state.config, server, toolMeta, state.toolMetadata)` over this
    /// context (MCP-231) — the body a production [`ProxyEnv::is_tool_call_approval_required`] has.
    ///
    /// The metadata is read **under the lock, without cloning**, because this runs once per row in
    /// `describe` and `search`.
    #[must_use]
    pub fn approval_required(&self, server: &str, tool: &ToolMetadata) -> bool {
        self.with_metadata(|metadata| {
            is_tool_call_approval_required(&self.state.config, server, tool, Some(metadata))
        })
    }

    /// `ensureToolCallApproved(state, server, toolMeta, args, signal, origin)` over this context
    /// (MCP-232) — the body a production [`ProxyEnv::ensure_tool_call_approved`] has, and the one
    /// place `state` and `state.toolMetadata` are joined for it.
    ///
    /// The metadata **is** cloned here, unlike in [`Self::approval_required`]: the gate awaits a
    /// human, and a `std::sync::MutexGuard` cannot be held across an await. The cost is one map
    /// clone per MCP tool invocation, against a dialog that may sit on screen for minutes.
    ///
    /// **Integration note:** [`execute_call`] deliberately keeps calling through
    /// [`ProxyCtx::env`] rather than this method — the trait is the seam MCP-196's conformance
    /// suite scripts a denial through. This is what the production implementor forwards to.
    pub async fn ensure_tool_call_approved(
        &self,
        server: &str,
        tool: &ToolMetadata,
        args: &Value,
        origin: ApprovalOrigin,
        cancel: &CancelToken,
    ) -> ApprovalOutcome {
        let metadata = self.with_metadata(Clone::clone);
        ensure_tool_call_approved(&self.state, server, tool, args, origin, cancel, &metadata).await
    }

    /// The one write path onto `state.toolMetadata`.
    fn with_metadata_mut<R>(&self, f: impl FnOnce(&mut IndexMap<String, Vec<ToolMetadata>>) -> R) -> Option<R> {
        self.tool_metadata.lock().ok().map(|mut guard| f(&mut guard))
    }

    /// The resolved configuration this generation is running.
    fn config(&self) -> &McpConfig {
        &self.state.config
    }

    /// `state.config.settings`, or an all-defaults block.
    fn settings(&self) -> &McpSettings {
        self.state.config.settings_or_default()
    }

    /// `state.owner` — the generation's ownership token.
    fn owner(&self) -> &Arc<McpRuntimeOwner> {
        &self.state.owner
    }

    /// `combineAbortSignals(state.owner?.signal, signal)`.
    fn owned_signal(&self, cancel: &CancelToken) -> CancelToken {
        combine(&self.owner().token(), Some(cancel))
    }

    /// `state.ui` — `None` in a headless build, which is upstream's `if (state.ui)` guard.
    fn has_ui(&self) -> bool {
        self.state.ui.is_some()
    }

    /// `state.ui.setStatus("mcp", formatMcpStatus(config, message))`.
    ///
    /// `HostServices::set_status(key, Option<&str>)` is a keyed footer segment cleared with `None`;
    /// its default impl is a no-op, which degrades exactly the way upstream's `if (state.ui)` guard
    /// does — no gap.
    fn set_status(&self, message: &str) {
        let Some(ui) = self.state.ui.as_ref() else { return };
        let text = format_mcp_status(self.config(), message);
        cyrup_ext::HostServices::set_status(ui.as_ref(), "mcp", text.as_deref());
    }

    /// `getToolNames(state, serverName)` (`tool-metadata.ts:142`).
    fn tool_names(&self, server: &str) -> Vec<String> {
        self.with_metadata(|metadata| {
            metadata
                .get(server)
                .map(|tools| tools.iter().map(|tool| tool.name.clone()).collect())
                .unwrap_or_default()
        })
    }

    /// `state.serverInstructions.get(server)`.
    fn server_instructions(&self, server: &str) -> Option<String> {
        self.state.server_instructions.lock().ok().and_then(|map| map.get(server).cloned())
    }

    /// `isServerDisabled(state.config.mcpServers[server])` — **only** the literal boolean `true`
    /// disables a server, and an *unknown* server is not disabled.
    fn is_disabled(&self, server: &str) -> bool {
        self.config().mcp_servers.get(server).is_some_and(ServerEntry::is_disabled)
    }

    /// `rankSuggestions(state, name, limit)` against this context.
    fn suggestions(&self, name: &str, limit: usize) -> Vec<String> {
        self.with_metadata(|metadata| rank_suggestions(self.config(), metadata, name, limit))
    }
}

// **De-duplicated at integration.** `utils.ts:339` `formatMcpStatus(config, message)` had landed
// twice: here against `&McpSettings`, and in `ui.rs` against `&McpConfig`. 13h owns the footer
// (`init.ts` `updateStatusBar` is `footer_status_text`'s only other caller), and upstream's
// parameter is `Pick<McpConfig, "settings">` — the config, not the settings — so the `ui.rs` one is
// both the owner's and the literal signature. This is its re-export.
pub use crate::ui::format_mcp_status;

/// `utils.ts:330` `formatAuthRequiredMessage(config, serverName, defaultMessage)` — a configured
/// `settings.authRequiredMessage` template wins, with `${server}` replaced everywhere.
#[must_use]
pub fn format_auth_required_message(
    settings: &McpSettings,
    server_name: &str,
    default_message: &str,
) -> String {
    match settings.auth_required_message() {
        Some(template) => template.replace("${server}", server_name),
        None => default_message.to_string(),
    }
}

// ==================================================================================================
// 5 · Shared result helpers (13d §12)
// ==================================================================================================

/// `{content: [{type:"text", text}], details}` — the envelope every mode returns.
fn text_result(text: impl Into<String>, details: JsonMap<String, Value>) -> ToolResult {
    ToolResult {
        content: vec![Content::Text { text: text.into(), text_signature: None }],
        details: Some(Value::Object(details)),
        ..Default::default()
    }
}

/// A `details` builder seeded with `{mode}`.
fn details(mode: &str) -> JsonMap<String, Value> {
    let mut map = JsonMap::new();
    map.insert("mode".to_string(), Value::String(mode.to_string()));
    map
}

/// A `details` builder seeded with `{mode, error}`.
fn details_err(mode: &str, code: McpErrorCode) -> JsonMap<String, Value> {
    let mut map = details(mode);
    map.insert("error".to_string(), Value::String(code.as_str().to_string()));
    map
}

/// `proxy-modes.ts:61` `ambiguousToolResult(mode, toolName)`.
///
/// The **fail-closed** answer: a bare name matching more than one enabled server is refused rather
/// than guessed. `getSingleToolMatch` returning the `"ambiguous"` sentinel instead of `matches[0]`
/// is what upstream's conformance suite calls "fails closed for duplicate unqualified proxy names",
/// and it is why MCP-163 is this section's only `critical`.
#[must_use]
pub fn ambiguous_tool_result(mode: &str, tool_name: &str) -> ToolResult {
    let message = format!("Tool \"{tool_name}\" matches multiple servers. Specify a server.");
    let mut map = details_err(mode, McpErrorCode::AmbiguousTool);
    map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
    map.insert("message".to_string(), Value::String(message.clone()));
    text_result(message, map)
}

/// `proxy-modes.ts:69` `disabledResult(mode, serverName)` — shared by every mode.
#[must_use]
pub fn disabled_result(mode: &str, server_name: &str) -> ToolResult {
    let message = format!(
        "Server \"{server_name}\" is disabled. Run /mcp enable {server_name} and /reload to enable it."
    );
    let mut map = details_err(mode, McpErrorCode::ServerDisabled);
    map.insert("server".to_string(), Value::String(server_name.to_string()));
    map.insert("message".to_string(), Value::String(message.clone()));
    text_result(message, map)
}

/// `Server "<s>" not found. Use mcp({}) to see available servers.` — `auth-start`, `auth-complete`,
/// `list`, `instructions` and `connect` all share the text and the `not_found` code.
fn not_found_result(mode: &str, server_name: &str) -> ToolResult {
    let mut map = details_err(mode, McpErrorCode::NotFound);
    map.insert("server".to_string(), Value::String(server_name.to_string()));
    text_result(
        format!("Server \"{server_name}\" not found. Use mcp({{}}) to see available servers."),
        map,
    )
}

/// `proxy-modes.ts:77` `getAuthRequiredMessage(state, serverName, defaultMessage?)`.
///
/// The default names both escape hatches; a configured `settings.authRequiredMessage` still wins,
/// which is why the caller-supplied default in [`attempt_auto_auth`] step 4 also routes through
/// here rather than being returned directly.
#[must_use]
pub fn get_auth_required_message(settings: &McpSettings, server_name: &str) -> String {
    format_auth_required_message(settings, server_name, &default_auth_required_message(server_name))
}

/// The literal default `getAuthRequiredMessage` is declared with.
fn default_auth_required_message(server_name: &str) -> String {
    format!(
        "Server \"{server_name}\" requires OAuth authentication. Run mcp({{ action: \"auth-start\", server: \"{server_name}\" }}) to get a browser URL, or /mcp-auth {server_name} in an interactive local session."
    )
}

/// `proxy-modes.ts:85` `getAuthFailedMessage(state, serverName, message)`.
///
/// The two arms differ: with a configured template the guidance is *appended* via
/// [`get_auth_required_message`]; without one the default guidance is inlined literally. Both spell
/// the same sentence, but the template arm renders the user's text.
#[must_use]
pub fn get_auth_failed_message(settings: &McpSettings, server_name: &str, message: &str) -> String {
    if settings.auth_required_message().is_some() {
        format!(
            "OAuth authentication failed for \"{server_name}\": {message}. {}",
            get_auth_required_message(settings, server_name)
        )
    } else {
        format!(
            "OAuth authentication failed for \"{server_name}\": {message}. Run mcp({{ action: \"auth-start\", server: \"{server_name}\" }}) to get a browser URL, or /mcp-auth {server_name} in an interactive local session."
        )
    }
}

/// `proxy-modes.ts:39` `getToolMatches(metadata, toolName, exact)`.
///
/// `exact` compares `tool.name` verbatim; the fuzzy form compares with all `-` replaced by `_` on
/// **both** sides.
fn get_tool_matches<'a>(metadata: &'a [ToolMetadata], tool_name: &str, exact: bool) -> Vec<&'a ToolMetadata> {
    if exact {
        return metadata.iter().filter(|tool| tool.name == tool_name).collect();
    }
    let normalized = tool_name.replace('-', "_");
    metadata.iter().filter(|tool| tool.name.replace('-', "_") == normalized).collect()
}

/// `proxy-modes.ts:46` `getEnabledToolMatches(state, toolName, exact)` — flat-mapped over
/// non-disabled servers in `state.toolMetadata` **insertion order**.
fn get_enabled_tool_matches(
    config: &McpConfig,
    metadata: &IndexMap<String, Vec<ToolMetadata>>,
    tool_name: &str,
    exact: bool,
) -> Vec<(String, ToolMetadata)> {
    let mut matches = Vec::new();
    for (server, tools) in metadata {
        if config.mcp_servers.get(server).is_some_and(ServerEntry::is_disabled) {
            continue;
        }
        for tool in get_tool_matches(tools, tool_name, exact) {
            matches.push((server.clone(), tool.clone()));
        }
    }
    matches
}

/// `proxy-modes.ts:55` `getSingleToolMatch(metadata, toolName)`'s three-valued return.
#[derive(Debug, Clone, PartialEq)]
pub enum SingleMatch {
    /// Exactly one match — exact if any exact matches existed, else the single fuzzy one.
    One(ToolMetadata),
    /// **More than one.** The sentinel that fails the call closed rather than routing it to
    /// whichever server happened to be first in the map.
    Ambiguous,
    /// Nothing matched.
    None,
}

/// `proxy-modes.ts:55` `getSingleToolMatch(metadata, toolName)`.
///
/// Exact matches win outright when there are any; only when there are none does the fuzzy set get a
/// look. `>1` in whichever set was consulted is [`SingleMatch::Ambiguous`].
#[must_use]
pub fn get_single_tool_match(metadata: Option<&Vec<ToolMetadata>>, tool_name: &str) -> SingleMatch {
    let Some(metadata) = metadata else { return SingleMatch::None };
    let exact = get_tool_matches(metadata, tool_name, true);
    let matches = if exact.is_empty() { get_tool_matches(metadata, tool_name, false) } else { exact };
    if matches.len() > 1 {
        return SingleMatch::Ambiguous;
    }
    matches.first().map_or(SingleMatch::None, |tool| SingleMatch::One((*tool).clone()))
}

// ==================================================================================================
// 6 · Discovery modes — `status`, `list`, `instructions` (MCP-154, MCP-155, MCP-156)
// ==================================================================================================

/// `proxy-modes.ts:277` `executeStatus(state)`.
///
/// Per server key of `config.mcpServers`, **in insertion order**, the status is computed by this
/// six-rung ladder: `disabled` → `connected` → `needs-auth` → `failed` (when the failure age is
/// non-null) → `cached` (metadata present) → `not connected`. `metadata` and `connection` are forced
/// absent and `failedAgo` to `null` for a disabled server.
///
/// The header counts **enabled servers only**; the glyphs `⊘ ✓ ⚠ ○ ✗` are literal and must not be
/// substituted. `details.servers[i].disabled` is present **only when true**.
#[must_use]
pub fn execute_status(ctx: &ProxyCtx) -> ToolResult {
    #[derive(Clone)]
    struct Row {
        name: String,
        status: &'static str,
        tool_count: usize,
        failed_ago: Option<u64>,
        disabled: bool,
    }

    let mut servers: Vec<Row> = Vec::new();
    for name in ctx.config().mcp_servers.keys() {
        let disabled = ctx.is_disabled(name);
        let connection = if disabled { None } else { ctx.env.get_connection(name) };
        let tool_count = if disabled {
            0
        } else {
            ctx.with_metadata(|metadata| metadata.get(name).map(Vec::len)).unwrap_or(0)
        };
        let has_metadata = !disabled && ctx.with_metadata(|metadata| metadata.contains_key(name));
        let failed_ago = if disabled { None } else { ctx.env.failure_age_seconds(name) };

        let status = if disabled {
            "disabled"
        } else if connection == Some(ConnectionStatus::Connected) {
            "connected"
        } else if connection == Some(ConnectionStatus::NeedsAuth) {
            "needs-auth"
        } else if failed_ago.is_some() {
            "failed"
        } else if has_metadata {
            "cached"
        } else {
            "not connected"
        };

        servers.push(Row { name: name.clone(), status, tool_count, failed_ago, disabled });
    }

    let disabled_count = servers.iter().filter(|row| row.disabled).count();
    let enabled: Vec<&Row> = servers.iter().filter(|row| !row.disabled).collect();
    let total_tools: usize = enabled.iter().map(|row| row.tool_count).sum();
    let connected_count = enabled.iter().filter(|row| row.status == "connected").count();

    let mut text = format!("MCP: {connected_count}/{} servers, {total_tools} tools", enabled.len());
    if disabled_count > 0 {
        text.push_str(&format!(" ({disabled_count} disabled)"));
    }
    text.push_str("\n\n");
    for row in &servers {
        let name = &row.name;
        if row.disabled {
            text.push_str(&format!("⊘ {name} (disabled)\n"));
            continue;
        }
        match row.status {
            "connected" => text.push_str(&format!("✓ {name} ({} tools)\n", row.tool_count)),
            "needs-auth" => text.push_str(&format!("⚠ {name} (needs auth)\n")),
            "cached" => text.push_str(&format!("○ {name} ({} tools, cached)\n", row.tool_count)),
            "failed" => {
                text.push_str(&format!("✗ {name} (failed {}s ago)\n", row.failed_ago.unwrap_or(0)));
            }
            _ => text.push_str(&format!("○ {name} (not connected)\n")),
        }
    }
    if !servers.is_empty() {
        text.push_str("\nmcp({ server: \"name\" }) to list tools, mcp({ search: \"...\" }) to search");
    }

    let rows: Vec<Value> = servers
        .iter()
        .map(|row| {
            let mut entry = JsonMap::new();
            entry.insert("name".to_string(), Value::String(row.name.clone()));
            entry.insert("status".to_string(), Value::String(row.status.to_string()));
            entry.insert("toolCount".to_string(), json!(row.tool_count));
            entry.insert(
                "failedAgo".to_string(),
                row.failed_ago.map_or(Value::Null, |seconds| json!(seconds)),
            );
            if row.disabled {
                entry.insert("disabled".to_string(), Value::Bool(true));
            }
            Value::Object(entry)
        })
        .collect();

    let mut map = details("status");
    map.insert("servers".to_string(), Value::Array(rows));
    map.insert("totalTools".to_string(), json!(total_tools));
    map.insert("connectedCount".to_string(), json!(connected_count));
    map.insert("disabledCount".to_string(), json!(disabled_count));
    text_result(text.trim().to_string(), map)
}

/// `proxy-modes.ts:633` `executeList(state, server)`.
///
/// Five outcomes, three of them for the zero-tool case, each with a distinct `details` shape. The
/// `Use mcp({ instructions: … }) for the full text.` pointer appears **only when the 300-character
/// preview actually truncated**.
#[must_use]
pub fn execute_list(ctx: &ProxyCtx, server: &str) -> ToolResult {
    if !ctx.config().mcp_servers.contains_key(server) {
        let mut map = details_err("list", McpErrorCode::NotFound);
        map.insert("server".to_string(), Value::String(server.to_string()));
        map.insert("tools".to_string(), Value::Array(Vec::new()));
        map.insert("count".to_string(), json!(0));
        return text_result(
            format!("Server \"{server}\" not found. Use mcp({{}}) to see available servers."),
            map,
        );
    }
    if ctx.is_disabled(server) {
        return disabled_result("list", server);
    }

    let metadata: Option<Vec<ToolMetadata>> =
        ctx.with_metadata(|map| map.get(server).cloned());
    let tool_names: Vec<String> =
        metadata.as_ref().map(|tools| tools.iter().map(|tool| tool.name.clone()).collect()).unwrap_or_default();
    let connection = ctx.env.get_connection(server);
    // `Boolean(instructions)` — an empty string is falsy upstream, so it neither renders the
    // preview block nor sets `hasInstructions`.
    let instructions = ctx.server_instructions(server).filter(|text| !text.is_empty());

    let mut instructions_text = String::new();
    if let Some(instructions) = instructions.as_ref() {
        let preview = truncate_at_word(instructions, INSTRUCTIONS_PREVIEW_LENGTH);
        instructions_text = format!("\n\nServer instructions:\n{preview}");
        if &preview != instructions {
            instructions_text.push_str(&format!("\nUse mcp({{ instructions: \"{server}\" }}) for the full text."));
        }
    }
    let has_instructions = instructions.is_some();

    if tool_names.is_empty() {
        if connection == Some(ConnectionStatus::Connected) {
            let mut map = details("list");
            map.insert("server".to_string(), Value::String(server.to_string()));
            map.insert("tools".to_string(), Value::Array(Vec::new()));
            map.insert("count".to_string(), json!(0));
            map.insert("hasInstructions".to_string(), Value::Bool(has_instructions));
            return text_result(format!("Server \"{server}\" has no tools.{instructions_text}"), map);
        }
        if metadata.is_some() {
            let mut map = details("list");
            map.insert("server".to_string(), Value::String(server.to_string()));
            map.insert("tools".to_string(), Value::Array(Vec::new()));
            map.insert("count".to_string(), json!(0));
            map.insert("cached".to_string(), Value::Bool(true));
            map.insert("hasInstructions".to_string(), Value::Bool(has_instructions));
            return text_result(
                format!("Server \"{server}\" has no cached tools (not connected).{instructions_text}"),
                map,
            );
        }
        let mut map = details_err("list", McpErrorCode::NotConnected);
        map.insert("server".to_string(), Value::String(server.to_string()));
        map.insert("tools".to_string(), Value::Array(Vec::new()));
        map.insert("count".to_string(), json!(0));
        map.insert("hasInstructions".to_string(), Value::Bool(has_instructions));
        return text_result(
            format!("Server \"{server}\" is configured but not connected. Use mcp({{ connect: \"{server}\" }}) or /mcp reconnect {server} to retry.{instructions_text}"),
            map,
        );
    }

    let cached_note = if connection == Some(ConnectionStatus::Connected) { "" } else { " (not connected, cached)" };
    let mut text = format!("{server} ({} tools{cached_note}):\n\n", tool_names.len());
    let descriptions: BTreeMap<String, String> = metadata
        .as_ref()
        .map(|tools| tools.iter().map(|tool| (tool.name.clone(), tool.description.clone())).collect())
        .unwrap_or_default();
    for tool in &tool_names {
        let description = descriptions.get(tool).map(String::as_str).unwrap_or_default();
        let truncated = truncate_at_word(description, 50);
        text.push_str(&format!("- {tool}"));
        if !truncated.is_empty() {
            text.push_str(&format!(" - {truncated}"));
        }
        text.push('\n');
    }
    text.push_str(&instructions_text);

    let mut map = details("list");
    map.insert("server".to_string(), Value::String(server.to_string()));
    map.insert(
        "tools".to_string(),
        Value::Array(tool_names.iter().map(|name| Value::String(name.clone())).collect()),
    );
    map.insert("count".to_string(), json!(tool_names.len()));
    map.insert("hasInstructions".to_string(), Value::Bool(has_instructions));
    text_result(text.trim().to_string(), map)
}

/// `proxy-modes.ts:700` `executeInstructions(state, server)`.
///
/// Five outcomes checked in this order: `not_found`, `server_disabled`, cached instructions,
/// `no_instructions` (connected and the server declared none), `not_connected`.
/// **Cached instructions win even for a disconnected server** — the connection is only consulted
/// once the cache has already missed.
#[must_use]
pub fn execute_instructions(ctx: &ProxyCtx, server: &str) -> ToolResult {
    if !ctx.config().mcp_servers.contains_key(server) {
        return not_found_result("instructions", server);
    }
    if ctx.is_disabled(server) {
        return disabled_result("instructions", server);
    }

    if let Some(instructions) = ctx.server_instructions(server).filter(|text| !text.is_empty()) {
        let mut map = details("instructions");
        map.insert("server".to_string(), Value::String(server.to_string()));
        // JS `.length` is UTF-16 code units; `chars().count()` is the closest honest analogue and
        // this value is diagnostic only.
        map.insert("length".to_string(), json!(instructions.chars().count()));
        return text_result(format!("{server} instructions:\n\n{instructions}"), map);
    }

    if ctx.env.get_connection(server) == Some(ConnectionStatus::Connected) {
        let mut map = details_err("instructions", McpErrorCode::NoInstructions);
        map.insert("server".to_string(), Value::String(server.to_string()));
        return text_result(format!("Server \"{server}\" does not provide instructions."), map);
    }

    let mut map = details_err("instructions", McpErrorCode::NotConnected);
    map.insert("server".to_string(), Value::String(server.to_string()));
    text_result(
        format!("No instructions cached for \"{server}\". Use mcp({{ connect: \"{server}\" }}) to connect and refresh."),
        map,
    )
}

// ==================================================================================================
// 7 · `executeDescribe` (MCP-157)
// ==================================================================================================

/// `proxy-modes.ts:434` `executeDescribe(state, toolName)`.
///
/// 1. **Ambiguity first**, before any resolution: `>1` exact enabled match is ambiguous; otherwise
///    `0` exact and `>1` fuzzy is ambiguous.
/// 2. The single exact match; if none, walk `state.toolMetadata` in insertion order with
///    `findToolByName`, remembering the **first** disabled hit (`??=`) and breaking on the first
///    enabled hit.
/// 3. No hit: a disabled server that matched is reported as `server_disabled` rather than
///    `tool_not_found`; otherwise ranked suggestions.
/// 4. Render. Note `formatSchema` is called here with the **default** indent, unlike
///    [`execute_search`]'s `"    "`.
#[must_use]
pub fn execute_describe(ctx: &ProxyCtx, tool_name: &str) -> ToolResult {
    let resolved = ctx.with_metadata(|metadata| {
        let exact = get_enabled_tool_matches(ctx.config(), metadata, tool_name, true);
        if exact.len() > 1 {
            return Err(true);
        }
        if exact.is_empty()
            && get_enabled_tool_matches(ctx.config(), metadata, tool_name, false).len() > 1
        {
            return Err(true);
        }

        let mut server_name = exact.first().map(|(server, _)| server.clone());
        let mut tool_meta = exact.first().map(|(_, tool)| tool.clone());
        let mut disabled_match: Option<String> = None;

        if tool_meta.is_none() {
            for (server, tools) in metadata {
                let Some(found) = find_tool_by_name(tools, tool_name) else { continue };
                if ctx.config().mcp_servers.get(server).is_some_and(ServerEntry::is_disabled) {
                    // `??=` — the FIRST disabled hit is remembered and the scan continues.
                    disabled_match.get_or_insert_with(|| server.clone());
                    continue;
                }
                server_name = Some(server.clone());
                tool_meta = Some(found.clone());
                break;
            }
        }
        Ok((server_name, tool_meta, disabled_match))
    });

    let (server_name, tool_meta, disabled_match) = match resolved {
        Err(_) => return ambiguous_tool_result("describe", tool_name),
        Ok(triple) => triple,
    };

    let (Some(server_name), Some(tool_meta)) = (server_name, tool_meta) else {
        if let Some(disabled) = disabled_match {
            return disabled_result("describe", &disabled);
        }
        let suggestions = ctx.suggestions(tool_name, 5);
        let suggestion_text = if suggestions.is_empty() {
            String::new()
        } else {
            format!(" Did you mean: {}", suggestions.join(", "))
        };
        let mut map = details_err("describe", McpErrorCode::ToolNotFound);
        map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
        map.insert(
            "suggestions".to_string(),
            Value::Array(suggestions.iter().map(|name| Value::String(name.clone())).collect()),
        );
        return text_result(
            format!("Tool \"{tool_name}\" not found. Use mcp({{ search: \"...\" }}) to search.{suggestion_text}"),
            map,
        );
    };

    let approval_marker = if ctx.env.is_tool_call_approval_required(&server_name, &tool_meta) {
        " (requires approval)"
    } else {
        ""
    };
    let mut text = format!("{}{approval_marker}\n", tool_meta.name);
    text.push_str(&format!("Server: {server_name}\n"));
    if let Some(uri) = tool_meta.resource_uri.as_ref() {
        text.push_str(&format!("Type: Resource (reads from {uri})\n"));
    }
    let description =
        if tool_meta.description.is_empty() { "(no description)" } else { tool_meta.description.as_str() };
    text.push_str(&format!("\n{description}\n"));

    match (tool_meta.input_schema.as_ref(), tool_meta.resource_uri.as_ref()) {
        (Some(schema), None) => match ctx.env.render_ts_shape(schema) {
            // `renderTsShape` returning null is the fork to the long-form printer.
            None => text.push_str(&format!("\nParameters:\n{}", ctx.env.format_schema(schema, "  "))),
            Some(shape) => text.push_str(&format!("\nShape:\n{shape}")),
        },
        (_, Some(_)) => text.push_str("\nNo parameters required (resource tool)."),
        (None, None) => text.push_str("\nNo parameters defined."),
    }

    let mut map = details("describe");
    map.insert(
        "tool".to_string(),
        serde_json::to_value(&tool_meta).unwrap_or(Value::Null),
    );
    map.insert("server".to_string(), Value::String(server_name));
    text_result(text.trim().to_string(), map)
}

// ==================================================================================================
// 8 · `executeSearch` (MCP-158, MCP-159, MCP-160, MCP-177)
// ==================================================================================================

/// `proxy-modes.ts:492` `executeSearch(state, query, regex?, server?, includeSchemas?, limit = 12,
/// offset = 0)`.
///
/// Three **mutually exclusive** selection paths, then one rendering path.
///
/// * `regex` truthy — length cap, compile, scan. **Every match gets `score: 0` and the list is never
///   sorted**, so the output order is server-insertion order then per-server metadata order, and it
///   is observable in `details.matches`.
/// * blank query — with no `server` that is `empty_query`; with one, all of that server's metadata
///   at `score: 0`, sorted by [`rank_collate`].
/// * otherwise — [`rank_tool_matches`].
///
/// A `server` filter naming a disabled server short-circuits to [`disabled_result`] before any of
/// them.
#[must_use]
pub fn execute_search(
    ctx: &ProxyCtx,
    query: &str,
    regex: Option<bool>,
    server: Option<&str>,
    include_schemas: Option<bool>,
    limit: Option<f64>,
    offset: Option<f64>,
) -> ToolResult {
    // `includeSchemas !== false`, so `undefined` ⇒ true.
    let show_schemas = include_schemas != Some(false);
    let limit = limit.unwrap_or(12.0);
    let offset = offset.unwrap_or(0.0);

    if let Some(server) = server
        && ctx.is_disabled(server)
    {
        return disabled_result("search", server);
    }

    let global_prefix = ctx.config().tool_prefix();
    let matches: Vec<RankedToolMatch> = if regex == Some(true) {
        // (a) The regex path, in this exact order.
        if query.chars().count() > MAX_REGEX_SEARCH_QUERY_LENGTH {
            let mut map = details_err("search", McpErrorCode::QueryTooLong);
            map.insert("query".to_string(), Value::String(query.to_string()));
            map.insert("maxLength".to_string(), json!(MAX_REGEX_SEARCH_QUERY_LENGTH));
            return text_result(
                format!("Regex query is too long; maximum length is {MAX_REGEX_SEARCH_QUERY_LENGTH} characters."),
                map,
            );
        }
        // Compiled case-insensitively with EXPLICIT ceilings (MCP-159). Upstream's `recheck` ReDoS
        // gate has no port: `regex` is a finite automaton with a linear-time matching guarantee, so
        // catastrophic backtracking is structurally impossible. The named residual is that JS-only
        // syntax — backreferences, lookaround — becomes `invalid_pattern` here where upstream
        // compiled it.
        let pattern = match regex::RegexBuilder::new(query)
            .case_insensitive(true)
            .size_limit(REGEX_SIZE_LIMIT)
            .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
            .build()
        {
            Ok(pattern) => pattern,
            Err(_) => {
                let mut map = details_err("search", McpErrorCode::InvalidPattern);
                map.insert("query".to_string(), Value::String(query.to_string()));
                return text_result(format!("Invalid regex: {query}"), map);
            }
        };

        ctx.with_metadata(|metadata| {
            let mut matches = Vec::new();
            for (server_name, tools) in metadata {
                let definition = ctx.config().mcp_servers.get(server_name);
                if definition.is_some_and(ServerEntry::is_disabled) {
                    continue;
                }
                if let Some(filter) = server
                    && server_name != filter
                {
                    continue;
                }
                for tool in tools {
                    // MCP-177: configured keywords are searchable by regex too, resolved against
                    // the GLOBAL prefix — the per-server override is applied inside
                    // `resolveSearchKeywords` via `resolveToolPrefix`.
                    let matched = pattern.is_match(&tool.name)
                        || pattern.is_match(&tool.description)
                        || resolve_search_keywords(definition, &tool.original_name, server_name, global_prefix)
                            .iter()
                            .any(|keyword| pattern.is_match(keyword));
                    if matched {
                        matches.push(RankedToolMatch {
                            server: server_name.clone(),
                            tool: tool.clone(),
                            score: 0,
                        });
                    }
                }
            }
            matches
        })
    } else if query.trim().is_empty() {
        // (b) The blank-query path.
        let Some(server) = server else {
            let map = details_err("search", McpErrorCode::EmptyQuery);
            return text_result("Search query cannot be empty", map);
        };
        ctx.with_metadata(|metadata| {
            let mut matches: Vec<RankedToolMatch> = metadata
                .get(server)
                .map(|tools| {
                    tools
                        .iter()
                        .map(|tool| RankedToolMatch {
                            server: server.to_string(),
                            tool: tool.clone(),
                            score: 0,
                        })
                        .collect()
                })
                .unwrap_or_default();
            matches.sort_by(|a, b| rank_collate(&a.tool.name, &b.tool.name));
            matches
        })
    } else {
        // (c) The ranked path.
        ctx.with_metadata(|metadata| rank_tool_matches(ctx.config(), metadata, query, server, true))
    };

    let page = paginate(&matches, offset, limit);

    if page.total == 0 {
        // The "still connecting" hint: with a `server` filter, that server iff it is configured AND
        // connecting; otherwise every configured, non-disabled, connecting server, sorted.
        let connecting: Vec<String> = match server {
            Some(server) => {
                if ctx.config().mcp_servers.contains_key(server) && ctx.env.is_connecting(server) {
                    vec![server.to_string()]
                } else {
                    Vec::new()
                }
            }
            None => {
                let mut names: Vec<String> = ctx
                    .config()
                    .mcp_servers
                    .keys()
                    .filter(|name| !ctx.is_disabled(name) && ctx.env.is_connecting(name))
                    .cloned()
                    .collect();
                names.sort_by(|a, b| rank_collate(a, b));
                names
            }
        };
        let base = match server {
            Some(server) => format!("No tools matching \"{query}\" in \"{server}\""),
            None => format!("No tools matching \"{query}\""),
        };
        let hint = match connecting.len() {
            0 => String::new(),
            1 => format!(
                " Server \"{}\" is still connecting; retry in a moment.",
                connecting.first().map(String::as_str).unwrap_or_default()
            ),
            _ => format!(
                " Servers {} are still connecting; retry in a moment.",
                connecting.iter().map(|name| format!("\"{name}\"")).collect::<Vec<_>>().join(", ")
            ),
        };
        let mut map = details("search");
        map.insert("matches".to_string(), Value::Array(Vec::new()));
        map.insert("count".to_string(), json!(0));
        map.insert("hasMore".to_string(), Value::Bool(false));
        map.insert("nextOffset".to_string(), Value::Null);
        map.insert("query".to_string(), Value::String(query.to_string()));
        if !connecting.is_empty() {
            map.insert(
                "connectingServers".to_string(),
                Value::Array(connecting.iter().map(|name| Value::String(name.clone())).collect()),
            );
        }
        return text_result(format!("{base}{hint}"), map);
    }

    let plural = if page.total == 1 { "" } else { "s" };
    let mut text = format!("Found {} tool{plural} matching \"{query}\":\n\n", page.total);
    for entry in &page.items {
        let approval_marker = if ctx.env.is_tool_call_approval_required(&entry.server, &entry.tool) {
            " (requires approval)"
        } else {
            ""
        };
        if show_schemas {
            text.push_str(&format!("{}{approval_marker}\n", entry.tool.name));
            let description = if entry.tool.description.is_empty() {
                "(no description)"
            } else {
                entry.tool.description.as_str()
            };
            text.push_str(&format!("  {description}\n"));
            match (entry.tool.input_schema.as_ref(), entry.tool.resource_uri.as_ref()) {
                (Some(schema), None) => match ctx.env.render_ts_shape(schema) {
                    None => text.push_str(&format!(
                        "\n  Parameters:\n{}\n",
                        ctx.env.format_schema(schema, "    ")
                    )),
                    Some(shape) => {
                        let indented = shape
                            .split('\n')
                            .map(|line| format!("    {line}"))
                            .collect::<Vec<_>>()
                            .join("\n");
                        text.push_str(&format!("\n  Shape:\n{indented}\n"));
                    }
                },
                (_, Some(_)) => text.push_str("  No parameters (resource tool).\n"),
                (None, None) => {}
            }
            text.push('\n');
        } else {
            text.push_str(&format!("- {}{approval_marker}", entry.tool.name));
            if !entry.tool.description.is_empty() {
                text.push_str(&format!(" - {}", truncate_at_word(&entry.tool.description, 50)));
            }
            text.push('\n');
        }
    }
    if page.has_more {
        // Em-dash, not a hyphen — this string is model-visible.
        text.push_str(&format!(
            "\n{} of {} — offset: {} for more\n",
            page.items.len(),
            page.total,
            page.next_offset.unwrap_or(0)
        ));
    }

    let rows: Vec<Value> = page
        .items
        .iter()
        .map(|entry| {
            let mut row = JsonMap::new();
            row.insert("server".to_string(), Value::String(entry.server.clone()));
            row.insert("tool".to_string(), Value::String(entry.tool.name.clone()));
            row.insert("score".to_string(), json!(entry.score));
            Value::Object(row)
        })
        .collect();
    let mut map = details("search");
    map.insert("matches".to_string(), Value::Array(rows));
    map.insert("count".to_string(), json!(page.total));
    map.insert("hasMore".to_string(), Value::Bool(page.has_more));
    map.insert(
        "nextOffset".to_string(),
        page.next_offset.map_or(Value::Null, |offset| json!(offset)),
    );
    map.insert("query".to_string(), Value::String(query.to_string()));
    text_result(text.trim().to_string(), map)
}

// ==================================================================================================
// 9 · Manual OAuth modes (MCP-167, MCP-168)
// ==================================================================================================

/// `proxy-modes.ts:92` `getRedirectPort(authorizationUrl)`.
///
/// Parse the `redirect_uri` query parameter, then that URI's port, accepting only an integer. Any
/// parse failure yields nothing. `url::Url::port()` normalises a scheme's default port away exactly
/// as `new URL(...).port` does, so `http://localhost/cb` and `http://localhost:80/cb` both yield
/// nothing on both sides.
#[must_use]
pub fn get_redirect_port(authorization_url: &str) -> Option<u16> {
    let parsed = url::Url::parse(authorization_url).ok()?;
    let redirect_uri = parsed
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .map(|(_, value)| value.into_owned())?;
    if redirect_uri.is_empty() {
        return None;
    }
    url::Url::parse(&redirect_uri).ok()?.port()
}

/// `proxy-modes.ts:104` `formatManualAuthInstructions(serverName, authorizationUrl)`.
///
/// An array of literals joined by `\n`, with the empty strings preserved and the final `portNote`
/// dropped when empty via `.filter(Boolean)`. **`portNote` itself begins with `\n`**, so when a port
/// is parseable the rendered text has a **blank line before it** — that is the byte-exact shape, not
/// an accident.
#[must_use]
pub fn format_manual_auth_instructions(server_name: &str, authorization_url: &str) -> String {
    let port_note = get_redirect_port(authorization_url).map(|port| {
        format!("\nThe redirect URL will use local port {port}. On a remote server it is expected for that localhost page to fail locally; copy the address bar URL anyway.")
    });
    let mut lines: Vec<String> = vec![
        format!("MCP OAuth required for \"{server_name}\"."),
        String::new(),
        "Open this URL in your local browser:".to_string(),
        String::new(),
        authorization_url.to_string(),
        String::new(),
        "After approving, copy the full redirected localhost URL from your browser address bar and send it back with:".to_string(),
        format!("mcp({{ action: \"auth-complete\", server: \"{server_name}\", args: {{ redirectUrl: \"PASTE_REDIRECT_URL_HERE\" }} }})"),
        String::new(),
        "You can also pass just the `code` query parameter as `args: { code: \"PASTE_CODE_HERE\" }`. JSON-string args remain supported.".to_string(),
    ];
    if let Some(note) = port_note {
        // `.trimEnd()` upstream, then `.filter(Boolean)` — a note that trims to empty disappears.
        let trimmed = note.trim_end().to_string();
        if !trimmed.is_empty() {
            lines.push(trimmed);
        }
    }
    // `.filter(Boolean)` drops the empty separators too — upstream relies on that NOT happening for
    // the middle blanks because they are `""`… which IS falsy. Reproduced literally: JS filters
    // every empty string out, so the joined text has no blank lines from the literals; the blank
    // lines a reader sees come from `portNote`'s leading `\n` and from the multi-line strings.
    lines.retain(|line| !line.is_empty());
    lines.join("\n")
}

/// `proxy-modes.ts:344` `executeAuthStart(state, serverName, signal)`.
///
/// The manual-OAuth block is a copy-paste protocol for remote/headless sessions: the model prints an
/// authorization URL and instructs the human to paste back the redirect URL. `startAuth` returning
/// no `authorizationUrl` means the flow completed synchronously (client-credentials).
pub async fn execute_auth_start(ctx: &ProxyCtx, server_name: &str, cancel: &CancelToken) -> McpResult<ToolResult> {
    let owned = ctx.owned_signal(cancel);
    throw_if_aborted(&owned, ctx.owner().stop_reason().as_deref().map(String::as_str))?;

    let Some(definition) = ctx.config().mcp_servers.get(server_name).cloned() else {
        return Ok(not_found_result("auth-start", server_name));
    };
    if definition.is_disabled() {
        return Ok(disabled_result("auth-start", server_name));
    }

    let started = async {
        let server_url = ctx.env.resolve_server_url(&definition)?;
        let Some(server_url) = server_url.filter(|url| !url.is_empty()) else {
            return Ok(None);
        };
        if !ctx.env.supports_oauth(&definition) {
            return Ok(None);
        }
        ctx.env.start_auth(server_name, &server_url, &definition, &owned).await.map(Some)
    }
    .await;

    match started {
        Err(error) => {
            let message = error.to_string();
            let mut map = details_err("auth-start", McpErrorCode::AuthStartFailed);
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            map.insert("message".to_string(), Value::String(message.clone()));
            Ok(text_result(format!("Failed to start OAuth for \"{server_name}\": {message}"), map))
        }
        // A falsy URL or `!supportsOAuth(definition)` — one message, one code.
        Ok(None) => {
            let mut map = details_err("auth-start", McpErrorCode::OauthNotSupported);
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            Ok(text_result(format!("Server \"{server_name}\" is not configured for OAuth over HTTP."), map))
        }
        Ok(Some(None)) => {
            let mut map = details("auth-start");
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            map.insert("authenticated".to_string(), Value::Bool(true));
            Ok(text_result(format!("OAuth authentication successful for \"{server_name}\"."), map))
        }
        Ok(Some(Some(authorization_url))) => {
            let mut map = details("auth-start");
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            map.insert("authorizationUrl".to_string(), Value::String(authorization_url.clone()));
            Ok(text_result(format_manual_auth_instructions(server_name, &authorization_url), map))
        }
    }
}

/// `proxy-modes.ts:388` `executeAuthComplete(state, serverName, input, signal)`.
///
/// On success the connection is **closed** and the failure record cleared, so the next `connect`
/// uses the new token rather than the stale session that produced the `401`.
pub async fn execute_auth_complete(
    ctx: &ProxyCtx,
    server_name: &str,
    input: &str,
    cancel: &CancelToken,
) -> McpResult<ToolResult> {
    let owned = ctx.owned_signal(cancel);
    throw_if_aborted(&owned, ctx.owner().stop_reason().as_deref().map(String::as_str))?;

    let Some(definition) = ctx.config().mcp_servers.get(server_name) else {
        return Ok(not_found_result("auth-complete", server_name));
    };
    if definition.is_disabled() {
        return Ok(disabled_result("auth-complete", server_name));
    }

    match ctx.env.complete_auth_from_input(server_name, input, &owned).await {
        Err(error) => {
            let message = error.to_string();
            let mut map = details_err("auth-complete", McpErrorCode::AuthCompleteFailed);
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            map.insert("message".to_string(), Value::String(message.clone()));
            Ok(text_result(format!("Failed to complete OAuth for \"{server_name}\": {message}"), map))
        }
        Ok(status) if status != "authenticated" => {
            let mut map = details_err("auth-complete", McpErrorCode::NotAuthenticated);
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            map.insert("status".to_string(), Value::String(status));
            Ok(text_result(format!("OAuth authentication did not complete for \"{server_name}\"."), map))
        }
        Ok(_) => {
            ctx.env.close(server_name).await;
            ctx.env.clear_failure(server_name);
            ctx.env.update_status_bar();
            let mut map = details("auth-complete");
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            map.insert("authenticated".to_string(), Value::Bool(true));
            Ok(text_result(
                format!("OAuth authentication successful for \"{server_name}\". Run mcp({{ connect: \"{server_name}\" }}) to connect with the new token."),
                map,
            ))
        }
    }
}

// ==================================================================================================
// 10 · `attemptAutoAuth` and the single-shot latch (MCP-162)
// ==================================================================================================

/// `proxy-modes.ts:122` `attemptAutoAuth`'s three-valued return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoAuthResult {
    /// Not attempted — `autoAuth` is off, the definition is missing/disabled/non-OAuth, or there is
    /// no URL to authenticate against.
    Skipped,
    /// A token was obtained; the caller closes and reconnects.
    Success,
    /// The attempt was made (or refused for a *reason the model must see*) and failed.
    Failed(String),
}

/// `proxy-modes.ts:122` `attemptAutoAuth(state, serverName, signal)`.
///
/// 1. `settings.autoAuth !== true` ⇒ **skipped**. Opt-in, not opt-out.
/// 2. Missing / disabled / non-OAuth definition ⇒ skipped.
/// 3. `resolveServerUrl(definition)` **throwing** (a missing `${VAR}`, an invalid URL after
///    interpolation) ⇒ **failed**, not skipped. A falsy URL ⇒ skipped. Reproduce that split.
/// 4. `grantType = definition.oauth?.grantType ?? "authorization_code"`. **No interactive UI and a
///    grant type other than `client_credentials`** ⇒ failed, with the message routed through
///    [`get_auth_required_message`] so a configured `settings.authRequiredMessage` still wins.
/// 5. `authenticate(...)`. Upstream's four-way branch exists only to avoid passing `undefined`
///    keys; Rust builds one options struct, so the seam takes one call.
/// 6. Abort errors **rethrow**; anything else ⇒ failed with [`get_auth_failed_message`].
///
/// The browser opening this may trigger is `opener::open` called directly by the native crate —
/// there is no host verb for it and none is wanted, matching upstream's direct `open` dependency.
pub async fn attempt_auto_auth(
    ctx: &ProxyCtx,
    server_name: &str,
    cancel: &CancelToken,
) -> McpResult<AutoAuthResult> {
    if !ctx.settings().auto_auth() {
        return Ok(AutoAuthResult::Skipped);
    }
    let Some(definition) = ctx.config().mcp_servers.get(server_name).cloned() else {
        return Ok(AutoAuthResult::Skipped);
    };
    if definition.is_disabled() || !ctx.env.supports_oauth(&definition) {
        return Ok(AutoAuthResult::Skipped);
    }

    let server_url = match ctx.env.resolve_server_url(&definition) {
        Err(error) => {
            return Ok(AutoAuthResult::Failed(get_auth_failed_message(
                ctx.settings(),
                server_name,
                &error.to_string(),
            )));
        }
        Ok(url) => url,
    };
    let Some(server_url) = server_url.filter(|url| !url.is_empty()) else {
        return Ok(AutoAuthResult::Skipped);
    };

    let grant_type = match definition.oauth.as_ref() {
        Some(OAuthSetting::Config(config)) => {
            config.grant_type.unwrap_or(OAuthGrantType::AuthorizationCode)
        }
        _ => OAuthGrantType::AuthorizationCode,
    };
    if !ctx.has_ui() && grant_type != OAuthGrantType::ClientCredentials {
        return Ok(AutoAuthResult::Failed(get_auth_required_message(ctx.settings(), server_name)));
    }

    match ctx.env.authenticate(server_name, &server_url, &definition, cancel).await {
        Ok(()) => Ok(AutoAuthResult::Success),
        Err(error) => {
            if is_abort_error(&error, Some(cancel)) {
                return Err(error);
            }
            Ok(AutoAuthResult::Failed(get_auth_failed_message(
                ctx.settings(),
                server_name,
                &error.to_string(),
            )))
        }
    }
}

// ==================================================================================================
// 11 · `executeConnect` (MCP-161)
// ==================================================================================================

/// `proxy-modes.ts:730` `executeConnect(state, serverName, signal)`.
///
/// Reconnect if already connected, else connect; one auto-auth retry on `needs-auth`; then an
/// **eight-step metadata commit in this order** — store metadata, prompts *iff* discovery succeeded,
/// instructions set-or-**delete**, cache write, notify with reason `"proxy-connect"`, keep-alive
/// mark, clear failure, status bar — and finally **[`execute_list`]'s output**, so a successful
/// connect reports `details.mode === "list"`.
pub async fn execute_connect(ctx: &ProxyCtx, server_name: &str, cancel: &CancelToken) -> McpResult<ToolResult> {
    let owned = ctx.owned_signal(cancel);
    throw_if_aborted(&owned, ctx.owner().stop_reason().as_deref().map(String::as_str))?;

    if !ctx.config().mcp_servers.contains_key(server_name) {
        let mut map = details_err("connect", McpErrorCode::NotFound);
        map.insert("server".to_string(), Value::String(server_name.to_string()));
        return Ok(text_result(
            format!("Server \"{server_name}\" not found. Use mcp({{}}) to see available servers."),
            map,
        ));
    }
    if ctx.is_disabled(server_name) {
        return Ok(disabled_result("connect", server_name));
    }

    let outcome: McpResult<ConnectOutcome> = async {
        ctx.set_status(&format!("connecting to {server_name}..."));
        let current = ctx.env.get_connection(server_name);
        let mut connection = if current == Some(ConnectionStatus::Connected) {
            ctx.env.reconnect(server_name, &owned).await?
        } else {
            ctx.env.connect(server_name, &owned).await?
        };
        if connection.needs_auth() {
            match attempt_auto_auth(ctx, server_name, &owned).await? {
                AutoAuthResult::Failed(message) => {
                    return Err(McpError::Other(format!("\u{0}auth_required\u{0}{message}")));
                }
                AutoAuthResult::Success => {
                    ctx.env.close(server_name).await;
                    throw_if_aborted(&owned, ctx.owner().stop_reason().as_deref().map(String::as_str))?;
                    connection = ctx.env.connect(server_name, &owned).await?;
                }
                AutoAuthResult::Skipped => {}
            }
            if connection.needs_auth() {
                let message = get_auth_required_message(ctx.settings(), server_name);
                return Err(McpError::Other(format!("\u{0}auth_required\u{0}{message}")));
            }
        }
        Ok(connection)
    }
    .await;

    let connection = match outcome {
        Ok(connection) => connection,
        Err(error) => {
            // The `auth_required` arms are not connect failures: they return before `recordFailure`
            // and carry their own code. The NUL-delimited marker keeps that control flow inside one
            // `?`-friendly future without inventing a second error type for a two-branch catch.
            if let Some(message) = strip_auth_required_marker(&error) {
                let mut map = details_err("connect", McpErrorCode::AuthRequired);
                map.insert("server".to_string(), Value::String(server_name.to_string()));
                map.insert("message".to_string(), Value::String(message.clone()));
                return Ok(text_result(message, map));
            }
            // Upstream's catch does NOT rethrow an abort: it reports it as `error: "aborted"` and
            // skips `recordFailure`, because misclassifying a user cancellation as a connection
            // failure would poison the next minute of that server's availability. The only throw on
            // this path is the `throwIfAborted` at the top.
            let message = error.to_string();
            let aborted = is_abort_error(&error, Some(&owned));
            if !aborted {
                ctx.env.record_failure(server_name, &message);
            }
            ctx.env.update_status_bar();
            let code = if aborted { McpErrorCode::Aborted } else { McpErrorCode::ConnectFailed };
            let mut map = details_err("connect", code);
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            map.insert("message".to_string(), Value::String(message.clone()));
            return Ok(text_result(format!("Failed to connect to \"{server_name}\": {message}"), map));
        }
    };

    // The eight-step commit, in order.
    ctx.with_metadata_mut(|metadata| {
        metadata.insert(server_name.to_string(), connection.metadata.clone());
    });
    if !connection.prompt_discovery_failed {
        ctx.env.commit_prompt_metadata(server_name);
    }
    if let Ok(mut instructions) = ctx.state.server_instructions.lock() {
        match connection.instructions.as_ref().filter(|text| !text.is_empty()) {
            // `state.serverInstructions.set(...)` / `.delete(...)` — the delete arm is what keeps a
            // server that dropped its instructions from showing yesterday's.
            Some(text) => {
                instructions.insert(server_name.to_string(), text.clone());
            }
            None => {
                instructions.shift_remove(server_name);
            }
        }
    }
    ctx.env.update_metadata_cache(server_name);
    ctx.state.notify_tool_metadata_updated(server_name, "proxy-connect");
    ctx.env.mark_keep_alive_after_connect(server_name);
    ctx.env.clear_failure(server_name);
    ctx.env.update_status_bar();
    Ok(execute_list(ctx, server_name))
}

/// The `\0auth_required\0<message>` marker [`execute_connect`]'s inner future uses to distinguish an
/// auth refusal from a connect failure. A NUL is chosen because it cannot occur in a server message.
fn strip_auth_required_marker(error: &McpError) -> Option<String> {
    let McpError::Other(text) = error else { return None };
    text.strip_prefix("\u{0}auth_required\u{0}").map(str::to_string)
}

// ==================================================================================================
// 12 · `executeCall` — the resolution state machine (MCP-163, **critical**) and the invocation
//      half (MCP-164, MCP-165)
// ==================================================================================================

/// The function-scoped `autoAuthAttempted` boolean of `proxy-modes.ts:817`.
///
/// **At most one auto-auth per [`execute_call`] invocation, across all five call sites** — phase 3,
/// phase 4, phase 6's pre-connect and post-connect checks, and `recoverAuthConnection`, which fires
/// from inside `withSessionRecovery` *after* the request started. The latch is the defence against a
/// browser-flow loop on a misconfigured OAuth server: get it wrong and a single tool call opens one
/// browser tab per resolution attempt.
///
/// Shared (rather than a plain local `bool`) for exactly one reason: [`AuthRecovery`] must read and
/// set the *same* latch from inside the env's session-recovery loop.
#[derive(Debug, Default, Clone)]
pub struct AutoAuthLatch(Arc<std::sync::atomic::AtomicBool>);

impl AutoAuthLatch {
    /// A fresh, unlatched instance — one per `executeCall`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `if (!autoAuthAttempted) { autoAuthAttempted = true; … }` — `true` when *this* caller won the
    /// latch and may run the ladder.
    #[must_use]
    pub fn claim(&self) -> bool {
        !self.0.swap(true, std::sync::atomic::Ordering::SeqCst)
    }

    /// The value reported as `details.autoAuthAttempted` on the `auth_required` catch arm.
    #[must_use]
    pub fn attempted(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// `proxy-modes.ts:1152` `recoverAuthConnection` — the `onNeedsAuth` callback handed to
/// `withSessionRecovery`.
///
/// Lives here, not in the env, because it reuses [`AutoAuthLatch`] and because its ladder is the
/// port. The env calls [`AuthRecovery::recover`] from inside its recovery loop and translates the
/// answer into "retry the request" or "give up".
pub struct AuthRecovery<'a> {
    ctx: &'a ProxyCtx,
    server: String,
    latch: AutoAuthLatch,
    cancel: CancelToken,
}

impl AuthRecovery<'_> {
    /// The recovery ladder: an already-connected connection short-circuits; otherwise a single
    /// auto-auth attempt, then close-if-still-needs-auth, `clearFailure`, reconnect.
    ///
    /// A **failed** auto-auth raises [`ProxyCallError::SessionRecoveryAuthRequired`] carrying the
    /// message, which is upstream's `throw new SessionRecoveryAuthRequiredError(server, message)`.
    pub async fn recover(&self) -> Result<Option<ConnectionStatus>, ProxyCallError> {
        let current = self.ctx.env.get_connection(&self.server);
        if current == Some(ConnectionStatus::Connected) {
            return Ok(current);
        }
        if self.latch.claim() {
            let attempt = attempt_auto_auth(self.ctx, &self.server, &self.cancel)
                .await
                .map_err(ProxyCallError::Other)?;
            match attempt {
                AutoAuthResult::Failed(message) => {
                    return Err(ProxyCallError::SessionRecoveryAuthRequired {
                        server: self.server.clone(),
                        auth_message: Some(message),
                    });
                }
                AutoAuthResult::Success => {
                    if !self.ctx.config().mcp_servers.contains_key(&self.server) {
                        return Ok(None);
                    }
                    let after = self.ctx.env.get_connection(&self.server);
                    if after == Some(ConnectionStatus::Connected) {
                        return Ok(after);
                    }
                    if after == Some(ConnectionStatus::NeedsAuth) {
                        self.ctx.env.close(&self.server).await;
                    }
                    self.ctx.env.clear_failure(&self.server);
                    let outcome = self
                        .ctx
                        .env
                        .connect(&self.server, &self.cancel)
                        .await
                        .map_err(ProxyCallError::Other)?;
                    return Ok(outcome.status);
                }
                AutoAuthResult::Skipped => {}
            }
        }
        Ok(self.ctx.env.get_connection(&self.server))
    }
}

/// `{server, resourceUri}` for a resource tool, `{server, tool: originalName}` otherwise.
///
/// **Fixed once in phase 6 and reused by every subsequent result** — that is why a reconnect which
/// re-resolves the tool still reports the identity computed before it.
fn call_identity(server: &str, tool: &ToolMetadata) -> Vec<(String, Value)> {
    match tool.resource_uri.as_ref() {
        Some(uri) => vec![
            ("server".to_string(), Value::String(server.to_string())),
            ("resourceUri".to_string(), Value::String(uri.clone())),
        ],
        None => vec![
            ("server".to_string(), Value::String(server.to_string())),
            ("tool".to_string(), Value::String(tool.original_name.clone())),
        ],
    }
}

/// Splice a `callIdentity` into a `details` map at the point the JS spread would land.
fn spread(map: &mut JsonMap<String, Value>, identity: &[(String, Value)]) {
    for (key, value) in identity {
        map.insert(key.clone(), value.clone());
    }
}

/// `proxy-modes.ts:833` `disabledCallResult(disabledServer, metadata?)`.
///
/// The **disabled check happens after resolution** so the error can name the resolved tool: a
/// resource tool reports `{server, resourceUri}`, a normal tool `{server, tool: originalName}`, and
/// an unresolved name falls back to `{server, requestedTool}`.
fn disabled_call_result(disabled_server: &str, tool_name: &str, metadata: Option<&ToolMetadata>) -> ToolResult {
    let message = format!(
        "Server \"{disabled_server}\" is disabled. Run /mcp enable {disabled_server} and /reload to enable it."
    );
    let mut map = details_err("call", McpErrorCode::ServerDisabled);
    match metadata {
        None => {
            map.insert("server".to_string(), Value::String(disabled_server.to_string()));
            map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
        }
        Some(tool) => spread(&mut map, &call_identity(disabled_server, tool)),
    }
    map.insert("message".to_string(), Value::String(message.clone()));
    text_result(message, map)
}

/// The `Did you mean: …` suffix, empty when there is nothing to suggest.
fn suggestion_text(suggestions: &[String]) -> String {
    if suggestions.is_empty() {
        String::new()
    } else {
        format!(" Did you mean: {}", suggestions.join(", "))
    }
}

/// `proxy-modes.ts:806` `executeCall(state, toolName, args?, serverOverride?, getPiTools?, signal?,
/// origin?)`.
///
/// Eight phases; see the module docs for why this is the section's only `critical`. The short
/// version: [`get_single_tool_match`] and [`get_enabled_tool_matches`] exist to **refuse rather than
/// guess**, and a port that resolves ambiguity by first-match sends `create_issue` to whichever
/// server happens to be first in the map — a silently wrong tool call against a live external
/// system, on a normal path.
#[allow(clippy::too_many_lines)]
pub async fn execute_call(
    ctx: &ProxyCtx,
    tool_name: &str,
    args: Option<&Value>,
    server_override: Option<&str>,
    cancel: &CancelToken,
    origin: Option<ApprovalOrigin>,
) -> McpResult<ToolResult> {
    let owned = ctx.owned_signal(cancel);
    throw_if_aborted(&owned, ctx.owner().stop_reason().as_deref().map(String::as_str))?;

    let mut server_name: Option<String> = server_override.map(str::to_string);
    let mut tool_meta: Option<ToolMetadata> = None;
    let latch = AutoAuthLatch::new();
    let prefix_mode = ctx.config().tool_prefix();

    // ---- Phase 1 — a server hint was given ------------------------------------------------------
    if let Some(hint) = server_name.clone() {
        if !ctx.config().mcp_servers.contains_key(&hint) {
            let mut map = details_err("call", McpErrorCode::ServerNotFound);
            map.insert("server".to_string(), Value::String(hint.clone()));
            map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
            return Ok(text_result(
                format!("Server \"{hint}\" not found. Use mcp({{}}) to see available servers."),
                map,
            ));
        }
        let matched = ctx.with_metadata(|metadata| get_single_tool_match(metadata.get(&hint), tool_name));
        match matched {
            SingleMatch::Ambiguous => return Ok(ambiguous_tool_result("call", tool_name)),
            SingleMatch::One(found) => tool_meta = Some(found),
            SingleMatch::None => {}
        }
        // The disabled check runs AFTER resolution so the error can name the resolved tool.
        if ctx.is_disabled(&hint) {
            return Ok(disabled_call_result(&hint, tool_name, tool_meta.as_ref()));
        }
    } else {
        // ---- Phase 2 — no hint: the ambiguity gate, then two ordered scans -----------------------
        let gate = ctx.with_metadata(|metadata| {
            let exact = get_enabled_tool_matches(ctx.config(), metadata, tool_name, true);
            if exact.len() > 1 {
                return Err(());
            }
            if exact.is_empty()
                && get_enabled_tool_matches(ctx.config(), metadata, tool_name, false).len() > 1
            {
                return Err(());
            }
            Ok(())
        });
        if gate.is_err() {
            return Ok(ambiguous_tool_result("call", tool_name));
        }

        let scan = ctx.with_metadata(|metadata| {
            let mut disabled_match: Option<(String, ToolMetadata)> = None;
            let mut resolved: Option<(String, ToolMetadata)> = None;
            // Scan A — **exact name only**.
            for (server, tools) in metadata {
                let Some(found) = tools.iter().find(|tool| tool.name == tool_name) else { continue };
                if ctx.config().mcp_servers.get(server).is_some_and(ServerEntry::is_disabled) {
                    if disabled_match.is_none() {
                        disabled_match = Some((server.clone(), found.clone()));
                    }
                    continue;
                }
                resolved = Some((server.clone(), found.clone()));
                break;
            }
            // Scan B — the fuzzy pass, **guarded by `!toolMeta && !disabledMatch`**. An exact match
            // on a *disabled* server suppresses the fuzzy pass entirely, so a fuzzy-matching enabled
            // server is never reached. Faithful, and deliberately not "fixed".
            if resolved.is_none() && disabled_match.is_none() {
                for (server, tools) in metadata {
                    let Some(found) = find_tool_by_name(tools, tool_name) else { continue };
                    if ctx.config().mcp_servers.get(server).is_some_and(ServerEntry::is_disabled) {
                        if disabled_match.is_none() {
                            disabled_match = Some((server.clone(), found.clone()));
                        }
                        continue;
                    }
                    resolved = Some((server.clone(), found.clone()));
                    break;
                }
            }
            (resolved, disabled_match)
        });
        match scan {
            (Some((server, found)), _) => {
                server_name = Some(server);
                tool_meta = Some(found);
            }
            (None, Some((disabled, found))) => {
                return Ok(disabled_call_result(&disabled, tool_name, Some(&found)));
            }
            (None, None) => {}
        }
    }

    // ---- Phase 3 — hinted server, tool still unknown: lazy connect + the auto-auth ladder --------
    if let (Some(hint), None) = (server_name.clone(), tool_meta.clone()) {
        let connected = ctx.env.lazy_connect(&hint, &owned).await;
        if connected {
            match ctx.with_metadata(|metadata| get_single_tool_match(metadata.get(&hint), tool_name)) {
                SingleMatch::Ambiguous => return Ok(ambiguous_tool_result("call", tool_name)),
                SingleMatch::One(found) => tool_meta = Some(found),
                SingleMatch::None => {}
            }
        } else {
            if ctx.env.get_connection(&hint) == Some(ConnectionStatus::NeedsAuth) {
                if latch.claim() {
                    match attempt_auto_auth(ctx, &hint, &owned).await? {
                        AutoAuthResult::Failed(message) => {
                            let mut map = details_err("call", McpErrorCode::AuthRequired);
                            map.insert("server".to_string(), Value::String(hint.clone()));
                            map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
                            map.insert("message".to_string(), Value::String(message.clone()));
                            return Ok(text_result(message, map));
                        }
                        AutoAuthResult::Success => {
                            ctx.env.close(&hint).await;
                            ctx.env.clear_failure(&hint);
                            if ctx.env.lazy_connect(&hint, &owned).await {
                                match ctx.with_metadata(|metadata| {
                                    get_single_tool_match(metadata.get(&hint), tool_name)
                                }) {
                                    SingleMatch::Ambiguous => {
                                        return Ok(ambiguous_tool_result("call", tool_name));
                                    }
                                    SingleMatch::One(found) => tool_meta = Some(found),
                                    SingleMatch::None => {
                                        let suggestions = ctx.suggestions(tool_name, 5);
                                        let mut map = details_err(
                                            "call",
                                            McpErrorCode::ToolNotFoundAfterReconnect,
                                        );
                                        map.insert("server".to_string(), Value::String(hint.clone()));
                                        map.insert(
                                            "requestedTool".to_string(),
                                            Value::String(tool_name.to_string()),
                                        );
                                        map.insert(
                                            "suggestions".to_string(),
                                            Value::Array(
                                                suggestions
                                                    .iter()
                                                    .map(|name| Value::String(name.clone()))
                                                    .collect(),
                                            ),
                                        );
                                        return Ok(text_result(
                                            format!(
                                                "Tool \"{tool_name}\" not found on \"{hint}\" after reconnect.{}",
                                                suggestion_text(&suggestions)
                                            ),
                                            map,
                                        ));
                                    }
                                }
                            }
                        }
                        AutoAuthResult::Skipped => {}
                    }
                }

                if tool_meta.is_none()
                    && ctx.env.get_connection(&hint) == Some(ConnectionStatus::NeedsAuth)
                {
                    let message = get_auth_required_message(ctx.settings(), &hint);
                    let mut map = details_err("call", McpErrorCode::AuthRequired);
                    map.insert("server".to_string(), Value::String(hint.clone()));
                    map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
                    map.insert("message".to_string(), Value::String(message.clone()));
                    return Ok(text_result(message, map));
                }
            }

            if tool_meta.is_none()
                && let Some(failed_ago) = ctx.env.failure_age_seconds(&hint)
            {
                let mut map = details_err("call", McpErrorCode::ServerBackoff);
                map.insert("server".to_string(), Value::String(hint.clone()));
                map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
                return Ok(text_result(
                    format!("Server \"{hint}\" not available (last failed {failed_ago}s ago)"),
                    map,
                ));
            }
        }
    }

    // ---- Phase 4 — lazy prefix discovery ---------------------------------------------------------
    // Only when there is **no** server and **no** tool and the prefix mode is not `none`.
    let mut prefix_matched_server: Option<String> = None;
    if server_name.is_none() && tool_meta.is_none() && prefix_mode != ToolPrefix::None {
        let mut candidates: Vec<(String, String)> = ctx
            .config()
            .mcp_servers
            .keys()
            .filter(|name| !ctx.is_disabled(name))
            .map(|name| (name.clone(), get_server_prefix(name, prefix_mode)))
            .filter(|(_, prefix)| !prefix.is_empty() && tool_name.starts_with(&format!("{prefix}_")))
            .collect();
        // Descending prefix length: with servers `foo` and `foo-bar`, `foo-bar_x` must resolve
        // against `foo-bar`, not `foo`.
        candidates.sort_by_key(|(_, prefix)| std::cmp::Reverse(prefix.len()));

        let mut lazy_exact: Vec<(String, ToolMetadata)> = Vec::new();
        let mut lazy_fallback: Vec<(String, ToolMetadata)> = Vec::new();
        for (candidate, _) in candidates {
            let existing = ctx.env.get_connection(&candidate);
            // Skip a server in failure backoff **unless** it is `needs-auth` — an auth wall is not
            // a transport failure and must stay reachable.
            if ctx.env.failure_age_seconds(&candidate).is_some()
                && existing != Some(ConnectionStatus::NeedsAuth)
            {
                continue;
            }

            let mut connected = ctx.env.lazy_connect(&candidate, &owned).await;
            if !connected
                && ctx.env.get_connection(&candidate) == Some(ConnectionStatus::NeedsAuth)
                && latch.claim()
            {
                match attempt_auto_auth(ctx, &candidate, &owned).await? {
                    AutoAuthResult::Failed(message) => {
                        let mut map = details_err("call", McpErrorCode::AuthRequired);
                        map.insert("server".to_string(), Value::String(candidate.clone()));
                        map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
                        map.insert("message".to_string(), Value::String(message.clone()));
                        return Ok(text_result(message, map));
                    }
                    AutoAuthResult::Success => {
                        ctx.env.close(&candidate).await;
                        ctx.env.clear_failure(&candidate);
                        connected = ctx.env.lazy_connect(&candidate, &owned).await;
                    }
                    AutoAuthResult::Skipped => {}
                }
            }
            if !connected {
                continue;
            }
            if prefix_matched_server.is_none() {
                prefix_matched_server = Some(candidate.clone());
            }

            let per_server = ctx.with_metadata(|metadata| {
                let tools = metadata.get(&candidate).cloned().unwrap_or_default();
                let exact: Vec<ToolMetadata> =
                    get_tool_matches(&tools, tool_name, true).into_iter().cloned().collect();
                if exact.len() > 1 {
                    return Err(());
                }
                if let Some(single) = exact.into_iter().next() {
                    return Ok(Some((true, single)));
                }
                let fallback: Vec<ToolMetadata> =
                    get_tool_matches(&tools, tool_name, false).into_iter().cloned().collect();
                if fallback.len() > 1 {
                    return Err(());
                }
                Ok(fallback.into_iter().next().map(|single| (false, single)))
            });
            match per_server {
                Err(()) => return Ok(ambiguous_tool_result("call", tool_name)),
                Ok(Some((true, found))) => lazy_exact.push((candidate.clone(), found)),
                Ok(Some((false, found))) => lazy_fallback.push((candidate.clone(), found)),
                Ok(None) => {}
            }
        }

        // Exacts win if any; `>1` in the winning set is ambiguous; exactly one is adopted.
        let lazy_matches = if lazy_exact.is_empty() { lazy_fallback } else { lazy_exact };
        if lazy_matches.len() > 1 {
            return Ok(ambiguous_tool_result("call", tool_name));
        }
        if let Some((server, found)) = lazy_matches.into_iter().next() {
            server_name = Some(server);
            tool_meta = Some(found);
        }
    }

    // ---- Phase 5 — unresolved --------------------------------------------------------------------
    let (Some(server_name), Some(mut tool_meta)) = (server_name.clone(), tool_meta.clone()) else {
        // `getPiTools` is an OPTIONAL callback upstream, invoked as `getPiTools?.()`.
        // `all_tool_names() == None` — "no live session backend attached" — is exactly that branch:
        // skip the native-tool check and fall through to `tool_not_found` (MCP-199).
        let native = if server_override.is_none() {
            ctx.env.all_tool_names().and_then(|names| {
                names.into_iter().find(|name| name == tool_name && name != MCP_TOOL_NAME)
            })
        } else {
            None
        };
        if let Some(native) = native {
            let mut map = details_err("call", McpErrorCode::NativeTool);
            map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
            // MCP-163 naming decision: `Pi` → `cyrup`. The only text this port changes for a reason
            // other than the scope cuts.
            return Ok(text_result(
                format!("\"{native}\" is a native cyrup tool. Call {native} directly instead of using mcp({{ tool: \"{native}\" }})."),
                map,
            ));
        }

        let hint_server = server_name.clone().or_else(|| prefix_matched_server.clone());
        let available = hint_server.as_ref().map(|server| ctx.tool_names(server)).unwrap_or_default();
        let mut message = format!("Tool \"{tool_name}\" not found.");
        if available.is_empty() {
            message.push_str(" Use mcp({ search: \"...\" }) to search.");
        } else {
            message.push_str(&format!(
                " Server \"{}\" has: {}",
                hint_server.clone().unwrap_or_default(),
                available.join(", ")
            ));
        }
        let suggestions = ctx.suggestions(tool_name, 5);
        message.push_str(&suggestion_text(&suggestions));

        let mut map = details_err("call", McpErrorCode::ToolNotFound);
        map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
        map.insert(
            "hintServer".to_string(),
            hint_server.map_or(Value::Null, Value::String),
        );
        map.insert(
            "suggestions".to_string(),
            Value::Array(suggestions.iter().map(|name| Value::String(name.clone())).collect()),
        );
        return Ok(text_result(message, map));
    };

    // ---- Phase 6 — connection readiness -----------------------------------------------------------
    // `callIdentity` is fixed HERE and reused by every subsequent result — including after a
    // reconnect re-resolves `toolMeta` to a different record.
    let identity = call_identity(&server_name, &tool_meta);

    let mut connection = ctx.env.get_connection(&server_name);
    if connection == Some(ConnectionStatus::NeedsAuth) {
        if latch.claim() {
            match attempt_auto_auth(ctx, &server_name, &owned).await? {
                AutoAuthResult::Failed(message) => {
                    let mut map = details_err("call", McpErrorCode::AuthRequired);
                    spread(&mut map, &identity);
                    map.insert("message".to_string(), Value::String(message.clone()));
                    return Ok(text_result(message, map));
                }
                AutoAuthResult::Success => {
                    ctx.env.close(&server_name).await;
                    ctx.env.clear_failure(&server_name);
                    connection = ctx.env.get_connection(&server_name);
                }
                AutoAuthResult::Skipped => {}
            }
        }
        if connection == Some(ConnectionStatus::NeedsAuth) {
            let message = get_auth_required_message(ctx.settings(), &server_name);
            let mut map = details_err("call", McpErrorCode::AuthRequired);
            spread(&mut map, &identity);
            map.insert("message".to_string(), Value::String(message.clone()));
            return Ok(text_result(message, map));
        }
    }

    if connection != Some(ConnectionStatus::Connected) {
        if let Some(failed_ago) = ctx.env.failure_age_seconds(&server_name) {
            let mut map = details_err("call", McpErrorCode::ServerBackoff);
            spread(&mut map, &identity);
            return Ok(text_result(
                format!("Server \"{server_name}\" not available (last failed {failed_ago}s ago)"),
                map,
            ));
        }
        if !ctx.config().mcp_servers.contains_key(&server_name) {
            let mut map = details_err("call", McpErrorCode::ServerNotConnected);
            spread(&mut map, &identity);
            return Ok(text_result(format!("Server \"{server_name}\" not connected"), map));
        }

        let reconnected: McpResult<Option<ToolResult>> = async {
            ctx.set_status(&format!("connecting to {server_name}..."));
            let mut outcome = ctx.env.connect(&server_name, &owned).await?;
            if outcome.needs_auth() {
                if latch.claim() {
                    match attempt_auto_auth(ctx, &server_name, &owned).await? {
                        AutoAuthResult::Failed(message) => {
                            let mut map = details_err("call", McpErrorCode::AuthRequired);
                            spread(&mut map, &identity);
                            map.insert("message".to_string(), Value::String(message.clone()));
                            return Ok(Some(text_result(message, map)));
                        }
                        AutoAuthResult::Success => {
                            ctx.env.close(&server_name).await;
                            outcome = ctx.env.connect(&server_name, &owned).await?;
                        }
                        AutoAuthResult::Skipped => {}
                    }
                }
                if outcome.needs_auth() {
                    let message = get_auth_required_message(ctx.settings(), &server_name);
                    let mut map = details_err("call", McpErrorCode::AuthRequired);
                    spread(&mut map, &identity);
                    map.insert("message".to_string(), Value::String(message.clone()));
                    return Ok(Some(text_result(message, map)));
                }
            }
            ctx.env.clear_failure(&server_name);
            ctx.env.update_server_metadata(&server_name);
            ctx.env.update_metadata_cache(&server_name);
            ctx.state.notify_tool_metadata_updated(&server_name, "proxy-call-reconnect");
            ctx.env.mark_keep_alive_after_connect(&server_name);
            ctx.env.update_status_bar();
            Ok(None)
        }
        .await;

        match reconnected {
            Ok(Some(early)) => return Ok(early),
            Ok(None) => {}
            Err(error) => {
                // As in [`execute_connect`]: an abort is reported, not rethrown, and never recorded
                // as a connect failure.
                let message = error.to_string();
                let aborted = is_abort_error(&error, Some(&owned));
                if !aborted {
                    ctx.env.record_failure(&server_name, &message);
                }
                ctx.env.update_status_bar();
                let code = if aborted { McpErrorCode::Aborted } else { McpErrorCode::ConnectFailed };
                let mut map = details_err("call", code);
                spread(&mut map, &identity);
                map.insert("message".to_string(), Value::String(message.clone()));
                return Ok(text_result(format!("Failed to connect to \"{server_name}\": {message}"), map));
            }
        }

        // Re-resolve after the handshake.
        match ctx.with_metadata(|metadata| get_single_tool_match(metadata.get(&server_name), tool_name)) {
            SingleMatch::Ambiguous => return Ok(ambiguous_tool_result("call", tool_name)),
            SingleMatch::One(found) => tool_meta = found,
            SingleMatch::None => {
                let available = ctx.tool_names(&server_name);
                let hint = if available.is_empty() {
                    format!("Server \"{server_name}\" has no tools.")
                } else {
                    format!("Available tools on \"{server_name}\": {}", available.join(", "))
                };
                let suggestions = ctx.suggestions(tool_name, 5);
                let mut map = details_err("call", McpErrorCode::ToolNotFoundAfterReconnect);
                map.insert("server".to_string(), Value::String(server_name.clone()));
                map.insert("requestedTool".to_string(), Value::String(tool_name.to_string()));
                map.insert(
                    "suggestions".to_string(),
                    Value::Array(suggestions.iter().map(|name| Value::String(name.clone())).collect()),
                );
                return Ok(text_result(
                    format!(
                        "Tool \"{tool_name}\" not found on \"{server_name}\" after reconnect. {hint}{}",
                        suggestion_text(&suggestions)
                    ),
                    map,
                ));
            }
        }
    }

    // ---- Phase 7 — post-connect disabled recheck --------------------------------------------------
    // The definition may have been swapped under a live connection.
    if ctx.is_disabled(&server_name) {
        return Ok(disabled_call_result(&server_name, tool_name, Some(&tool_meta)));
    }

    // ---- Phase 8 — approval -----------------------------------------------------------------------
    // `toolMeta.resourceUri ? (args ?? {}) : normalizeToolArguments(args)`. Both arms are the same
    // value here: `normalizeToolArguments`' JSON round-trip exists to strip non-serializable
    // JavaScript values, and `parse_args` has already produced a plain JSON object.
    let normalized_args = args.cloned().unwrap_or_else(|| Value::Object(JsonMap::new()));
    let resolved_origin =
        origin.unwrap_or_else(|| ApprovalOrigin::for_proxy_call(tool_meta.resource_uri.as_ref()));
    match ctx
        .env
        .ensure_tool_call_approved(&server_name, &tool_meta, &normalized_args, resolved_origin, &owned)
        .await
    {
        ApprovalOutcome::Approved => {}
        outcome => {
            let denied = outcome == ApprovalOutcome::Denied;
            let message = if denied {
                format!(
                    "The user declined approval to run MCP tool \"{}\" on server \"{server_name}\".",
                    tool_meta.original_name
                )
            } else {
                format!(
                    "MCP tool \"{}\" on server \"{server_name}\" is approval-gated and requires an interactive session.",
                    tool_meta.original_name
                )
            };
            let code =
                if denied { McpErrorCode::ApprovalDenied } else { McpErrorCode::ApprovalRequired };
            let mut map = details_err("call", code);
            // NOT `callIdentity`: a resource tool reports `tool` here rather than `resourceUri`.
            map.insert("server".to_string(), Value::String(server_name.clone()));
            map.insert("tool".to_string(), Value::String(tool_meta.original_name.clone()));
            return Ok(text_result(message, map));
        }
    }

    // ---- Invocation ------------------------------------------------------------------------------
    let schema_suffix = tool_meta
        .input_schema
        .as_ref()
        .map(|schema| format!("\n\nExpected parameters:\n{}", ctx.env.format_schema(schema, "  ")))
        .unwrap_or_default();
    let recovery =
        AuthRecovery { ctx, server: server_name.clone(), latch: latch.clone(), cancel: owned.clone() };

    // try { touch; incrementInFlight; … } finally { decrementInFlight; touch }
    ctx.env.touch(&server_name);
    ctx.env.increment_in_flight(&server_name);
    let outcome = invoke(
        ctx,
        &server_name,
        &tool_meta,
        &identity,
        &normalized_args,
        &schema_suffix,
        &recovery,
        &latch,
        &owned,
    )
    .await;
    ctx.env.decrement_in_flight(&server_name);
    ctx.env.touch(&server_name);
    outcome
}

/// The body of [`execute_call`]'s `try` — the three result paths and the three catch arms.
///
/// **Three result paths** after the MCP Apps cut (upstream had four; the UI-enabled-tool path is
/// gone): resource read, tool error, tool success. The spread order differs between them and is
/// reproduced: paths 1 and 2 put `callIdentity` before the guard keys, path 3 after.
#[allow(clippy::too_many_arguments)]
async fn invoke(
    ctx: &ProxyCtx,
    server_name: &str,
    tool_meta: &ToolMetadata,
    identity: &[(String, Value)],
    normalized_args: &Value,
    schema_suffix: &str,
    recovery: &AuthRecovery<'_>,
    latch: &AutoAuthLatch,
    owned: &CancelToken,
) -> McpResult<ToolResult> {
    // Path 1 — a resource tool. Note the read is NOT wrapped in `abortable`: upstream's asymmetry,
    // reproduced rather than "fixed" (13d §10). Cancellation reaches it only through the request
    // options carried by the env.
    if let Some(uri) = tool_meta.resource_uri.as_ref() {
        return match ctx.env.read_resource(server_name, uri, recovery, owned).await {
            Ok(content) => {
                let content = if content.is_empty() {
                    vec![Content::Text { text: "(empty resource)".to_string(), text_signature: None }]
                } else {
                    content
                };
                let guarded = ctx.env.guard_mcp_output(content, OutputGuardOptions::default()).await;
                let mut map = details("call");
                spread(&mut map, identity);
                guarded.write_details(&mut map);
                Ok(ToolResult {
                    content: guarded.content,
                    details: Some(Value::Object(map)),
                    ..Default::default()
                })
            }
            Err(error) => {
                catch_arm(ctx, server_name, identity, schema_suffix, latch, owned, error).await
            }
        };
    }

    let arguments = normalized_args.as_object().cloned().unwrap_or_default();
    match ctx.env.call_tool(server_name, &tool_meta.original_name, arguments, recovery, owned).await {
        Err(error) => catch_arm(ctx, server_name, identity, schema_suffix, latch, owned, error).await,
        // Path 2 — the server returned `isError: true`.
        Ok(result) if result.is_error => {
            let content = if result.content.is_empty() {
                vec![Content::Text { text: "(empty result)".to_string(), text_signature: None }]
            } else {
                result.content
            };
            let guarded = ctx
                .env
                .guard_mcp_output(
                    content,
                    OutputGuardOptions {
                        prefix: "Error: ".to_string(),
                        suffix: schema_suffix.to_string(),
                        empty_text_fallback: Some("Tool execution failed".to_string()),
                        raw_mcp_result: result.raw,
                    },
                )
                .await;
            let mut map = details_err("call", McpErrorCode::ToolError);
            spread(&mut map, identity);
            guarded.write_details(&mut map);
            Ok(ToolResult {
                content: guarded.content,
                details: Some(Value::Object(map)),
                ..Default::default()
            })
        }
        // Path 3 — success. `callIdentity` is spread AFTER the guard keys here.
        Ok(result) => {
            let content = if result.content.is_empty() {
                vec![Content::Text { text: "(empty result)".to_string(), text_signature: None }]
            } else {
                result.content
            };
            let guarded = ctx
                .env
                .guard_mcp_output(
                    content,
                    OutputGuardOptions { raw_mcp_result: result.raw, ..Default::default() },
                )
                .await;
            let mut map = details("call");
            guarded.write_details(&mut map);
            spread(&mut map, identity);
            Ok(ToolResult {
                content: guarded.content,
                details: Some(Value::Object(map)),
                ..Default::default()
            })
        }
    }
}

/// `executeCall`'s catch block, in order (MCP-165).
///
/// 1. `SessionRecoveryAuthRequiredError` ⇒ `auth_required`, with `details.autoAuthAttempted`;
/// 2. `UrlElicitationRequiredError` ⇒ `url_elicitation_required` with the three action-specific
///    messages;
/// 3. anything else ⇒ the guard is applied to the *message* with `prefix: "Failed to call tool: "`,
///    and `details.message` becomes the literal `output truncated; see outputGuard.fullOutputPath`
///    when the guard spilled.
///
/// Upstream additionally fired `uiSession?.sendToolCancelled(message)` on all three arms; that goes
/// with Cut 2. The `finally` still does `decrementInFlight` + `touch` — see [`execute_call`].
async fn catch_arm(
    ctx: &ProxyCtx,
    server_name: &str,
    identity: &[(String, Value)],
    schema_suffix: &str,
    latch: &AutoAuthLatch,
    owned: &CancelToken,
    error: ProxyCallError,
) -> McpResult<ToolResult> {
    match error {
        ProxyCallError::SessionRecoveryAuthRequired { auth_message, .. } => {
            let message = auth_message
                .unwrap_or_else(|| get_auth_required_message(ctx.settings(), server_name));
            let mut map = details_err("call", McpErrorCode::AuthRequired);
            spread(&mut map, identity);
            map.insert("message".to_string(), Value::String(message.clone()));
            map.insert("autoAuthAttempted".to_string(), Value::Bool(latch.attempted()));
            Ok(text_result(message, map))
        }
        ProxyCallError::UrlElicitationRequired { detail } => {
            let action = ctx.env.handle_url_elicitation_required(server_name, &detail).await;
            let message = match action {
                UrlElicitationAction::Accept =>
                    "The original MCP tool did not run. Complete the opened browser interaction, then retry the tool.".to_string(),
                UrlElicitationAction::Decline => "The URL interaction was declined.".to_string(),
                UrlElicitationAction::Cancel => "The URL interaction was cancelled.".to_string(),
            };
            let mut map = details_err("call", McpErrorCode::UrlElicitationRequired);
            spread(&mut map, identity);
            map.insert("action".to_string(), Value::String(action.as_str().to_string()));
            Ok(text_result(message, map))
        }
        ProxyCallError::Other(error) => {
            let message = error.to_string();
            let guarded = ctx
                .env
                .guard_mcp_output(
                    vec![Content::Text { text: message.clone(), text_signature: None }],
                    OutputGuardOptions {
                        prefix: "Failed to call tool: ".to_string(),
                        suffix: schema_suffix.to_string(),
                        ..Default::default()
                    },
                )
                .await;
            let code = if is_abort_error(&error, Some(owned)) {
                McpErrorCode::Aborted
            } else {
                McpErrorCode::CallFailed
            };
            let mut map = details_err("call", code);
            spread(&mut map, identity);
            map.insert(
                "message".to_string(),
                Value::String(if guarded.output_guard.is_some() {
                    "output truncated; see outputGuard.fullOutputPath".to_string()
                } else {
                    message
                }),
            );
            guarded.write_details(&mut map);
            Ok(ToolResult {
                content: guarded.content,
                details: Some(Value::Object(map)),
                ..Default::default()
            })
        }
    }
}

// ==================================================================================================
// 13 · `buildProxyDescription` — the regenerated description (MCP-152, MCP-198)
// ==================================================================================================

/// One `directSpecs` entry, reduced to the two fields the description reads.
///
/// The full `DirectToolSpec` is 13e's; this is the projection `buildProxyDescription` actually
/// consumes, so the two never have to agree on more than a name and a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectToolSummary {
    /// The `mcpServers` key that contributed the tool.
    pub server_name: String,
    /// The registered, model-visible name.
    pub prefixed_name: String,
}

/// The subset of `mcp-cache.json` [`build_proxy_description`] reads for one server.
///
/// **Only cache-valid entries reach here** — the caller applies `isServerCacheValid(entry,
/// definition)` and passes `None` for a stale one. A stale entry is *not* skipped by the caller's
/// loop, it just yields zero counts, and a zero total is what drops the server out of the summary.
#[derive(Debug, Clone, Default)]
pub struct CachedServerEntry {
    /// The cached tools, with their `uiVisibility` intact.
    pub tools: Vec<ToolMetadata>,
    /// `(name, uri)` per cached resource.
    pub resources: Vec<(String, String)>,
    /// The server's own `instructions` from the initialize handshake.
    pub instructions: Option<String>,
}

/// `direct-tools.ts:259` `hasToolFilters` (upstream `faf55f7`) — does this server declare a tool
/// selector at all?
///
/// Upstream tests `Array.isArray(x) && x.length > 0` on both fields, so a JSON `null`, a non-array
/// and `[]` all read as "no filter" — which is exactly `Option<Vec<String>>` plus the emptiness
/// check. The predicate has to be *cheap* and *total*: it is the guard that decides whether the
/// collision scan runs at all.
///
/// [`crate::registration`] carries a private twin for the cache-side copy of `buildProxyDescription`;
/// the two collapse into one when MCP-207 merges this file's simple candidate-set form into 13e's
/// memoised [`crate::registration::CandidateIndex`].
fn server_has_tool_filters(definition: &ServerEntry) -> bool {
    definition.include_tools.as_ref().is_some_and(|list| !list.is_empty())
        || definition.exclude_tools.as_ref().is_some_and(|list| !list.is_empty())
}

/// The MCP-198 cross-server collision set: every *current-form* name candidate of every enabled
/// server that has a cache entry — including the server being filtered, whose own candidates are
/// subtracted by match *count* inside [`index_has_other_current_match`] rather than pre-deleted.
///
/// **Empty unless some server declares a selector.** `direct-tools.ts:257-262`, upstream `faf55f7`
/// ("avoid O(tools²) startup collision scan when no tool filters are configured"): [`is_tool_allowed`]
/// short-circuits on absent/empty `includeTools` *and* `excludeTools` before it ever reads the set,
/// so building one nothing consults is pure startup cost — the report behind that commit had 14
/// servers / ~800 tools, where the equivalent scan cost ~2.6s of synchronous startup and dominated
/// `pi`'s 3.66s launch. This description is regenerated on every metadata update, so the waste was
/// per-reconnect, not once.
///
/// Returning an empty set rather than `Option` is safe *because* this gate and the per-server gate
/// in [`build_proxy_description`] test the identical predicate: no server can consult a set that
/// was never built. One build serves the whole call, where upstream rebuilds an identical index per
/// filtered server (it is not parameterised by the server being filtered).
fn collision_candidates(
    config: &McpConfig,
    cache: &IndexMap<String, CachedServerEntry>,
    prefix: ToolPrefix,
) -> IndexSet<String> {
    let mut all_candidates: IndexSet<String> = IndexSet::new();
    if !config.mcp_servers.values().any(server_has_tool_filters) {
        return all_candidates;
    }
    for (other_server, other_definition) in &config.mcp_servers {
        let Some(other_entry) = cache.get(other_server) else { continue };
        if other_definition.is_disabled() {
            continue;
        }
        let other_prefix = resolve_tool_prefix(Some(other_definition), prefix);
        for tool in &other_entry.tools {
            // `isUiToolVisibleToModel` **survives the MCP Apps cut**: dropping it would expose to
            // the model tools the server explicitly marked app-only.
            if !is_ui_tool_visible_to_model(tool.ui_visibility.as_deref()) {
                continue;
            }
            all_candidates
                .extend(get_tool_name_candidates(&tool.name, other_server, other_prefix, false));
        }
        if other_definition.expose_resources() {
            for (name, _) in &other_entry.resources {
                let base = format!("read_{}", resource_name_to_tool_name(name));
                all_candidates.extend(get_tool_name_candidates(&base, other_server, other_prefix, false));
            }
        }
    }
    all_candidates
}

/// `direct-tools.ts:234` `buildProxyDescription(config, cache, directSpecs)`.
///
/// Six blocks in this exact order, each appended only when non-empty:
/// 1. the header, always, ending in a newline;
/// 2. direct-tool counts per server, in `directSpecs` iteration order;
/// 3. per-server proxy counts (`totalItems − directCount`, emitted only when `> 0`);
/// 4. disabled servers;
/// 5. 150-character instruction snippets;
/// 6. the usage block, always, byte-exact including the two-space indent, the arrow glyph `→` and
///    the **absence** of a trailing newline on the final `Mode:` line.
///
/// **MCP-198 — the counts are an O(servers × tools) cross-server computation, not a per-server
/// filter.** [`collision_candidates`] builds the set of name candidates produced by *every other*
/// cache-valid, enabled server (including `read_<resource>` names when `exposeResources !== false`)
/// and hands it to `isToolAllowed` as its collision set, so adding an unrelated server can change a
/// third server's advertised count. Built **once per call**, and **not at all** unless some server
/// declares a selector (`direct-tools.ts:257`, upstream `faf55f7`; upstream builds one index per
/// *filtered* server and, before that commit, rebuilt it per *tool* — the O(tools²) scan). Simplifying
/// it to a per-server `includeTools`/`excludeTools` filter would silently differ from pi's for any
/// workspace with overlapping tool names.
///
/// **Post-cut edits, both deliberate:** the header's `use mcpScript.` sentence is removed (Cut 4)
/// and `Pi` becomes `cyrup` (MCP-163's naming decision); the
/// `mcp({ action: "ui-messages" })` usage line is removed (Cut 2). Every other line, including the
/// `Mode:` precedence line, is unchanged.
#[must_use]
pub fn build_proxy_description(
    config: &McpConfig,
    cache: &IndexMap<String, CachedServerEntry>,
    direct_specs: &[DirectToolSummary],
) -> String {
    let prefix = config.tool_prefix();
    let mut desc = String::from(
        "MCP gateway — server status, tool search/describe, auth, and single MCP tool calls. Non-MCP cyrup tools should be called directly, not through mcp.\n",
    );

    // 2 · Direct tools, counted in `directSpecs` iteration order.
    let mut direct_by_server: IndexMap<String, usize> = IndexMap::new();
    for spec in direct_specs {
        *direct_by_server.entry(spec.server_name.clone()).or_insert(0) += 1;
    }
    if !direct_by_server.is_empty() {
        let parts: Vec<String> =
            direct_by_server.iter().map(|(server, count)| format!("{server} ({count})")).collect();
        desc.push_str(&format!(
            "\nDirect tools available (call as normal tools): {}\n",
            parts.join(", ")
        ));
    }

    // MCP-198 · the cross-server candidate-collision set — built once, and only when a selector
    // exists to read it. See [`collision_candidates`].
    let all_candidates = collision_candidates(config, cache, prefix);

    // 3 · Per-server proxy counts.
    let mut server_summaries: Vec<String> = Vec::new();
    for (server_name, definition) in &config.mcp_servers {
        if definition.is_disabled() {
            continue;
        }
        let entry = cache.get(server_name);
        let effective_prefix = resolve_tool_prefix(Some(definition), prefix);
        // `direct-tools.ts:284` — the set is consulted only when *this* server declares a selector.
        // [`collision_candidates`] tests the same predicate across every server before it builds
        // anything, so `Some(&all_candidates)` here can never name a set that was skipped.
        let collision = server_has_tool_filters(definition).then_some(&all_candidates);

        let tool_count = entry.map_or(0, |entry| {
            entry
                .tools
                .iter()
                .filter(|tool| {
                    is_ui_tool_visible_to_model(tool.ui_visibility.as_deref())
                        && is_tool_allowed(
                            &tool.name,
                            server_name,
                            effective_prefix,
                            definition.include_tools.as_deref(),
                            definition.exclude_tools.as_deref(),
                            collision,
                        )
                })
                .count()
        });
        let resource_count = if definition.expose_resources() {
            entry.map_or(0, |entry| {
                entry
                    .resources
                    .iter()
                    .filter(|(name, _)| {
                        let base = format!("read_{}", resource_name_to_tool_name(name));
                        is_tool_allowed(
                            &base,
                            server_name,
                            effective_prefix,
                            definition.include_tools.as_deref(),
                            definition.exclude_tools.as_deref(),
                            collision,
                        )
                    })
                    .count()
            })
        } else {
            0
        };

        let total_items = tool_count + resource_count;
        if total_items == 0 {
            // This is how a stale or missing cache entry drops out of the summary.
            continue;
        }
        let direct_count = direct_by_server.get(server_name).copied().unwrap_or(0);
        let proxy_count = total_items.saturating_sub(direct_count);
        if proxy_count > 0 {
            server_summaries.push(format!("{server_name} ({proxy_count} tools)"));
        }
    }
    if !server_summaries.is_empty() {
        desc.push_str(&format!("\nServers: {}\n", server_summaries.join(", ")));
    }

    // 4 · Disabled servers.
    let disabled: Vec<&String> = config
        .mcp_servers
        .iter()
        .filter(|(_, definition)| definition.is_disabled())
        .map(|(name, _)| name)
        .collect();
    if !disabled.is_empty() {
        let names: Vec<&str> = disabled.iter().map(|name| name.as_str()).collect();
        desc.push_str(&format!(
            "\nDisabled servers (enable with /mcp enable <server> and /reload): {}\n",
            names.join(", ")
        ));
    }

    // 5 · Instruction snippets.
    let mut instruction_summaries: Vec<String> = Vec::new();
    for (server_name, definition) in &config.mcp_servers {
        if definition.is_disabled() {
            continue;
        }
        let Some(instructions) =
            cache.get(server_name).and_then(|entry| entry.instructions.as_ref()).filter(|text| !text.is_empty())
        else {
            continue;
        };
        // `instructions.replace(/\s+/g, " ").trim()` before truncating.
        let flattened = instructions.split_whitespace().collect::<Vec<_>>().join(" ");
        let snippet = truncate_at_word(&flattened, INSTRUCTIONS_SNIPPET_LENGTH);
        // The two-space indent is part of each summary line, not of the joiner.
        instruction_summaries.push(format!("  {server_name}: {snippet}"));
    }
    if !instruction_summaries.is_empty() {
        desc.push_str(&format!(
            "\nServer instructions (truncated - full text via mcp({{ instructions: \"name\" }})):\n{}\n",
            instruction_summaries.join("\n")
        ));
    }

    // 6 · The usage block. Byte-exact; the final `Mode:` line carries NO trailing newline.
    desc.push_str("\nUsage:\n");
    desc.push_str("  mcp({ })                              → Show server status\n");
    desc.push_str("  mcp({ server: \"name\" })               → List tools from server\n");
    desc.push_str("  mcp({ search: \"query\" })              → Search MCP tools by name/description\n");
    desc.push_str("  mcp({ describe: \"tool_name\" })        → Show tool details and parameters\n");
    desc.push_str("  mcp({ instructions: \"name\" })         → Show full server usage instructions\n");
    desc.push_str("  mcp({ connect: \"server-name\" })       → Connect to a server and refresh metadata\n");
    desc.push_str("  mcp({ tool: \"name\", args: { key: \"value\" } })         → Call a tool (object args; JSON string also accepted)\n");
    desc.push_str("  mcp({ action: \"auth-start\", server: \"name\" })      → Start manual OAuth and get a browser URL\n");
    desc.push_str("  mcp({ action: \"auth-complete\", server: \"name\", args: { redirectUrl: \"...\" } }) → Complete manual OAuth\n");
    desc.push_str("\nMode: action > tool (call) > connect > describe > instructions > search > server (list) > nothing (status)");
    desc
}

// ==================================================================================================
// 14 · The registered tool (MCP-151, MCP-153, MCP-192, MCP-194, MCP-197, MCP-199)
// ==================================================================================================

/// `index.ts:829` — the JSON Schema handed to the provider.
///
/// Twelve properties, **all optional** (so no `required` is emitted), `args` a `string | object`
/// union. Upstream's `optionalNumber` helper exists only to dodge a TypeBox 1.x artefact — an
/// enumerable `~optional` key that Gemini rejects with `400 INVALID_ARGUMENT` — and both of its
/// branches serialise identically, so in Rust (where [`Tool::parameters`] returns a raw JSON Schema)
/// the shim evaporates.
///
/// **One cut-driven edit**: `action`'s description upstream reads
/// `"Action: 'ui-messages', 'auth-start', or 'auth-complete'"`. With MCP Apps out of scope there are
/// exactly two legal values and the description must say so — a model told about `ui-messages` will
/// call it and get a `mcp_status` fall-through with no explanation.
///
/// **All twelve keep their upstream names.** `cyrup_permission_system::manager`'s
/// `create_mcp_permission_targets` reads `{tool, server, connect, describe, search}` in that
/// precedence; renaming any of the five silently changes which permission rules apply (13d §13.2).
///
/// **MCP-194**: this serialises with keys in *alphabetical* order — `action, args, connect,
/// describe, includeSchemas, instructions, limit, offset, regex, search, server, tool` — because the
/// workspace builds `serde_json` without `preserve_order`, so `serde_json::Map` is a `BTreeMap`.
/// Accepted, per the recommendation. Holding the schema as a pre-rendered `&'static str` is the
/// trap: parsing still normalises into a `Map`.
#[must_use]
pub fn mcp_tool_schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        json!({
            "type": "object",
            "properties": {
                "tool": {"type": "string", "description": "Tool name to call (e.g., 'xcodebuild_list_sims')"},
                "args": {
                    "anyOf": [
                        {"type": "string", "description": "Arguments as a JSON string (e.g., '{\"key\": \"value\"}')"},
                        {"type": "object", "additionalProperties": true, "description": "Arguments as a JSON object (e.g., { \"key\": \"value\" })"}
                    ],
                    "description": "Tool arguments as a JSON object, or as a JSON string encoding one"
                },
                "connect": {"type": "string", "description": "Server name to connect (lazy connect + metadata refresh)"},
                "describe": {"type": "string", "description": "Tool name to describe (shows parameters)"},
                "instructions": {"type": "string", "description": "Server name to show that server's usage instructions"},
                "search": {"type": "string", "description": "Search tools by name/description"},
                "regex": {"type": "boolean", "description": "Treat search as regex (default: substring match)"},
                "includeSchemas": {"type": "boolean", "description": "Include parameter schemas in search results (default: true)"},
                "limit": {"type": "number", "minimum": 1, "description": "Maximum search results to return (default: 12)"},
                "offset": {"type": "number", "minimum": 0, "description": "Search result offset (default: 0)"},
                "server": {"type": "string", "description": "Filter to specific server (also disambiguates tool calls)"},
                "action": {"type": "string", "description": "Action: 'auth-start' or 'auth-complete'"}
            }
        })
    })
}

/// `Some` for **any** present value including `null`; `None` only when the key is absent.
///
/// This is JavaScript's `"args" in params` / `params.args !== undefined` distinction, which serde's
/// `Option<Value>` erases by mapping a present `null` onto `None`. Only [`McpToolParams::args`]
/// needs it — see the field's own note.
fn present_value<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

/// The twelve gateway parameters, deserialised.
///
/// `limit`/`offset` are `f64` because the schema says `number` and [`paginate`] reproduces JS's
/// `Number.isFinite` / `Math.trunc` handling of a fractional or absurd value.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct McpToolParams {
    /// Tool name to call.
    pub tool: Option<String>,
    /// Arguments as a JSON object, or a JSON string encoding one.
    ///
    /// [`present_value`], not serde's own `Option`, because this field is the **only** one whose
    /// presence is load-bearing twice over: `parseArgs` rejects an explicit `null` (`index.ts:880`)
    /// and `1bf3671`'s rescue tests `params.args !== undefined` (`index.ts:903`). Serde folds a
    /// present `null` into `None`, which would answer `mcp({ args: null })` with a status envelope
    /// where upstream throws. The sibling `Option` fields keep the plain mapping: their modes are
    /// selected by `!== undefined` too, but no upstream arm distinguishes an explicit `null` from
    /// an absent key for them.
    #[serde(default, deserialize_with = "present_value")]
    pub args: Option<Value>,
    /// Server name to connect.
    pub connect: Option<String>,
    /// Tool name to describe.
    pub describe: Option<String>,
    /// Server name whose instructions to show.
    pub instructions: Option<String>,
    /// Search query. Dispatch tests `!== undefined`, so `""` reaches the mode.
    pub search: Option<String>,
    /// Treat `search` as a regex.
    pub regex: Option<bool>,
    /// Include parameter schemas in search results.
    pub include_schemas: Option<bool>,
    /// Maximum search results.
    pub limit: Option<f64>,
    /// Search result offset.
    pub offset: Option<f64>,
    /// Server filter / call disambiguator.
    pub server: Option<String>,
    /// `auth-start` or `auth-complete`.
    pub action: Option<String>,
}

impl McpToolParams {
    /// `index.ts:886` `hasGatewayMode(value)` — whether any of the seven dispatch-bearing keys is
    /// present. Drives the "gateway params were nested inside `args`" rescue.
    fn has_gateway_mode(&self) -> bool {
        self.tool.is_some()
            || self.connect.is_some()
            || self.describe.is_some()
            || self.instructions.is_some()
            || self.search.is_some()
            || self.server.is_some()
            || self.action.is_some()
    }
}

/// `index.ts:863` `parseArgs(value)`.
///
/// `undefined` and `""` yield `None`. A string is `JSON.parse`d and a `SyntaxError` rethrown as
/// `Invalid args JSON: <e.message>`; anything that is not a non-null, non-array object throws
/// `Invalid args: expected a JSON object, got <gotType>`.
///
/// **These two are thrown, not returned** — they surface as tool-execution errors
/// (`Err(ToolError)`), never as `details.error` codes.
fn parse_args(value: Option<&Value>) -> Result<Option<Value>, ToolError> {
    let Some(value) = value else { return Ok(None) };
    let parsed: Value = match value {
        // NOT an early `Ok(None)`: upstream's only early return is `value === undefined || value
        // === ""` (`index.ts:865`). A present `null` falls through to the object test, where
        // `typeof null === "object"` is defeated by the explicit `args === null` clause and it
        // throws `got null` (`index.ts:880-882`).
        Value::String(text) if text.is_empty() => return Ok(None),
        Value::String(text) => serde_json::from_str(text)
            .map_err(|error| ToolError::new(format!("Invalid args JSON: {error}")))?,
        other => other.clone(),
    };
    let got_type = match &parsed {
        Value::Object(_) => return Ok(Some(parsed)),
        Value::Array(_) => "array",
        Value::Null => "null",
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
    };
    Err(ToolError::new(format!("Invalid args: expected a JSON object, got {got_type}")))
}

/// The init gate's four states — upstream's `state` slot crossed with its `initPromise` slot.
#[derive(Clone)]
pub enum InitPhase {
    /// `state === null && initPromise === undefined` — nothing is coming.
    NotInitialized,
    /// `state === null && initPromise !== undefined` — a build is live.
    Pending,
    /// The build resolved and committed.
    Ready(Arc<ProxyCtx>),
    /// The build rejected. Carries the message the `init_failed` envelope reports.
    Failed(String),
}

/// `index.ts:906`'s init gate: `awaitWithTimeout(initPromise, INIT_WAIT_TIMEOUT_MS)`.
///
/// A [`tokio::sync::watch`] rather than a promise, because a cyrup generation publishes its state
/// once and every later `execute` reads the same slot. `current_owner` is upstream's module-scoped
/// `currentOwner`, captured at the top of `execute` and used for the generation fence.
pub struct ProxyInitGate {
    phase: tokio::sync::watch::Receiver<InitPhase>,
    owner: arc_swap::ArcSwapOption<McpRuntimeOwner>,
}

/// What [`ProxyInitGate::wait`] resolved to.
enum InitWait {
    Ready(Arc<ProxyCtx>),
    TimedOut,
    Failed(String),
    NotInitialized,
}

impl ProxyInitGate {
    /// Build a gate over the generation's phase channel.
    #[must_use]
    pub fn new(phase: tokio::sync::watch::Receiver<InitPhase>) -> Self {
        Self { phase, owner: arc_swap::ArcSwapOption::empty() }
    }

    /// Publish the generation's owner — upstream's `currentOwner = owner` assignment.
    pub fn set_owner(&self, owner: Option<Arc<McpRuntimeOwner>>) {
        self.owner.store(owner);
    }

    /// `const executeOwner = currentOwner;` — read once at the top of `execute`.
    fn current_owner(&self) -> Option<Arc<McpRuntimeOwner>> {
        self.owner.load_full()
    }

    /// Race the live init against [`INIT_WAIT_TIMEOUT_MS`], with the already-settled phases
    /// short-circuiting.
    async fn wait(&self) -> InitWait {
        let mut rx = self.phase.clone();
        loop {
            match rx.borrow_and_update().clone() {
                InitPhase::Ready(ctx) => return InitWait::Ready(ctx),
                InitPhase::Failed(message) => return InitWait::Failed(message),
                InitPhase::NotInitialized => return InitWait::NotInitialized,
                InitPhase::Pending => {}
            }
            let changed = tokio::time::timeout(
                std::time::Duration::from_millis(INIT_WAIT_TIMEOUT_MS),
                rx.changed(),
            )
            .await;
            match changed {
                // Timer won the race. Upstream's timer is `unref`'d; a `tokio::time::timeout`
                // future is dropped here, which is the same "does not hold the process open".
                Err(_) => return InitWait::TimedOut,
                // The sender was dropped: the generation went away without ever committing.
                Ok(Err(_)) => return InitWait::NotInitialized,
                Ok(Ok(())) => {}
            }
        }
    }
}

/// The one tool the model sees.
///
/// `renderShell` is **not a constant**: `index.ts:137` computes
/// `toolRenderShell = toolRenderOptions.resultRendering === "compact" ? "self" : "default"`, and
/// `tool-result-renderer.ts`'s `resolveMcpToolRenderOptions` sets
/// `resultRendering = settings?.toolResultRendering === "boxed" ? "boxed" : "compact"`. So the shell
/// is [`ToolRenderKind::SelfRendered`] **by default** and [`ToolRenderKind::Default`] exactly when
/// the user sets `settings.toolResultRendering: "boxed"` — read from the *early* config at load
/// time, so it never changes within a session (MCP-197).
pub struct McpTool {
    description: String,
    render_kind: ToolRenderKind,
    guidelines: Vec<String>,
    gate: Arc<ProxyInitGate>,
}

impl McpTool {
    /// Construct the tool with a description produced by [`build_proxy_description`].
    ///
    /// **MCP-193 / `HA-1`**: the description is frozen here because
    /// `Tool::description(&self) -> &str` returns a *borrowed* `&str`, so an `RwLock` cannot satisfy
    /// the signature without leaking. Re-registration is the mechanism upstream uses
    /// (`syncProxyTool` → `pi.registerTool`), and cyrup's equivalent —
    /// `ExtensionHost::register_late_tool` → `refresh_tools` →
    /// `AgentSession::{refresh_extension_tools, push_active_tools}` — is complete and reaches a live
    /// agent at every turn boundary; what a *native* extension lacks is the handle. Until it has
    /// one, a cold `mcp-cache.json` means the first session's description names no servers and
    /// `mcp({connect:"x"})` cannot refresh it within that session. Every mode still functions and
    /// the next session is correct — that is scheduling, not severity. It also means
    /// `settings.disableProxyTool` must be treated as unsupported until then, because hiding a tool
    /// you cannot re-register is one-way.
    #[must_use]
    pub fn new(description: String, settings: &McpSettings, gate: Arc<ProxyInitGate>) -> Self {
        let render_kind = match settings.tool_result_rendering() {
            ToolResultRendering::Boxed => ToolRenderKind::Default,
            ToolResultRendering::Compact => ToolRenderKind::SelfRendered,
        };
        Self {
            description,
            render_kind,
            guidelines: vec![MCP_TOOL_GUIDELINE.to_string()],
            gate,
        }
    }

    /// The description this instance was registered with — `syncProxyTool` compares it against a
    /// freshly generated one and re-registers only when they differ.
    #[must_use]
    pub fn description_text(&self) -> &str {
        &self.description
    }
}

#[async_trait::async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        MCP_TOOL_NAME
    }

    fn parameters(&self) -> &Value {
        mcp_tool_schema()
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn label(&self) -> Option<&str> {
        Some(MCP_TOOL_LABEL)
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(MCP_TOOL_PROMPT_SNIPPET)
    }

    fn prompt_guidelines(&self) -> Vec<&str> {
        self.guidelines.iter().map(String::as_str).collect()
    }

    fn render_kind(&self) -> ToolRenderKind {
        self.render_kind
    }

    /// `index.ts:849` `execute` — the dispatch preamble and the nine-arm router (MCP-153).
    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let execute_owner = self.gate.current_owner();
        let mut params: McpToolParams = serde_json::from_value(params).unwrap_or_default();

        // 1 · Args coercion, and the "nested gateway params" rescue.
        let mut parsed_args = parse_args(params.args.as_ref())?;
        if !params.has_gateway_mode() {
            match parsed_args.clone() {
                Some(nested_value) => {
                    let nested: McpToolParams =
                        serde_json::from_value(nested_value).unwrap_or_default();
                    if nested.has_gateway_mode() {
                        parsed_args = parse_args(nested.args.as_ref())?;
                        params = nested;
                    } else {
                        return Err(ToolError::new(
                            "Gateway params were nested inside `args`; pass them top-level (for example, mcp({ search: \"...\" }) or mcp({ tool: \"...\", args: {} })).",
                        ));
                    }
                }
                None if params.args.is_some() => {
                    return Err(ToolError::new(
                        "Gateway params were nested inside `args`; pass them top-level (for example, mcp({ search: \"...\" }) or mcp({ tool: \"...\", args: {} })).",
                    ));
                }
                None => {}
            }
        }

        // 2 · The init-wait gate. These three envelopes carry **no `mode` key**.
        let ctx = match self.gate.wait().await {
            InitWait::Ready(ctx) => ctx,
            InitWait::TimedOut => {
                let mut map = JsonMap::new();
                map.insert(
                    "error".to_string(),
                    Value::String(McpErrorCode::InitTimeout.as_str().to_string()),
                );
                map.insert("timeoutMs".to_string(), json!(INIT_WAIT_TIMEOUT_MS));
                return Ok(text_result(
                    "MCP initialization is still in progress. Try again shortly.",
                    map,
                ));
            }
            InitWait::Failed(message) => {
                // An owner abort rethrows rather than reporting; anything else is `init_failed`.
                if let Some(owner) = execute_owner.as_ref()
                    && owner.token().is_cancelled()
                {
                    return Err(ToolError::new(message));
                }
                let mut map = JsonMap::new();
                map.insert(
                    "error".to_string(),
                    Value::String(McpErrorCode::InitFailed.as_str().to_string()),
                );
                map.insert("message".to_string(), Value::String(message.clone()));
                return Ok(text_result(format!("MCP initialization failed: {message}"), map));
            }
            InitWait::NotInitialized => {
                let mut map = JsonMap::new();
                map.insert(
                    "error".to_string(),
                    Value::String(McpErrorCode::NotInitialized.as_str().to_string()),
                );
                return Ok(text_result("MCP not initialized", map));
            }
        };

        // 3 · The generation fence — a stale lifecycle generation aborts rather than writing into a
        // restarted session.
        if let Some(owner) = execute_owner.as_ref() {
            owner.throw_if_inactive().map_err(|error| ToolError::new(error.to_string()))?;
        }

        // 4 · Dispatch, first match wins. Nine arms after the cut, in unchanged relative order. An
        // unrecognised `action` (`"frobnicate"`, and now also `"ui-messages"`) falls through arms
        // 1-2 and lands on whichever of 3-9 matches — it is **not** an error.
        let to_tool_error = |error: McpError| ToolError::new(error.to_string());
        match params.action.as_deref() {
            Some("auth-start") => {
                let Some(server) = params.server.as_deref().filter(|value| !value.is_empty()) else {
                    let map = details_err("auth-start", McpErrorCode::MissingServer);
                    return Ok(text_result(
                        "auth-start requires `server`. Example: mcp({ action: \"auth-start\", server: \"linear-server\" })",
                        map,
                    ));
                };
                return execute_auth_start(&ctx, server, &cancel).await.map_err(to_tool_error);
            }
            Some("auth-complete") => {
                let Some(server) = params.server.as_deref().filter(|value| !value.is_empty()) else {
                    let map = details_err("auth-complete", McpErrorCode::MissingServer);
                    return Ok(text_result("auth-complete requires `server`.", map));
                };
                let input = parsed_args
                    .as_ref()
                    .and_then(|args| {
                        args.get("redirectUrl").or_else(|| args.get("code")).or_else(|| args.get("input"))
                    })
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty());
                let Some(input) = input else {
                    let map = details_err("auth-complete", McpErrorCode::MissingInput);
                    return Ok(text_result(
                        "auth-complete requires args with `redirectUrl`, `code`, or `input`.",
                        map,
                    ));
                };
                return execute_auth_complete(&ctx, server, input, &cancel).await.map_err(to_tool_error);
            }
            _ => {}
        }

        if let Some(tool) = params.tool.as_deref().filter(|value| !value.is_empty()) {
            // `origin` is left unset here; `executeCall` derives `resource` or `proxy`.
            return execute_call(
                &ctx,
                tool,
                parsed_args.as_ref(),
                params.server.as_deref().filter(|value| !value.is_empty()),
                &cancel,
                None,
            )
            .await
            .map_err(to_tool_error);
        }
        if let Some(server) = params.connect.as_deref().filter(|value| !value.is_empty()) {
            let result = execute_connect(&ctx, server, &cancel).await.map_err(to_tool_error)?;
            // `syncToolSurface(ctx)` runs AFTER the mode returns and BEFORE the result is handed
            // back, so the next turn sees the refreshed surface.
            ctx.env.sync_tool_surface();
            return Ok(result);
        }
        if let Some(name) = params.describe.as_deref().filter(|value| !value.is_empty()) {
            return Ok(execute_describe(&ctx, name));
        }
        if let Some(server) = params.instructions.as_deref().filter(|value| !value.is_empty()) {
            return Ok(execute_instructions(&ctx, server));
        }
        // `!== undefined`, so `search: ""` reaches the mode rather than falling through to status.
        if let Some(query) = params.search.as_deref() {
            return Ok(execute_search(
                &ctx,
                query,
                params.regex,
                params.server.as_deref().filter(|value| !value.is_empty()),
                params.include_schemas,
                params.limit,
                params.offset,
            ));
        }
        if let Some(server) = params.server.as_deref().filter(|value| !value.is_empty()) {
            return Ok(execute_list(&ctx, server));
        }
        Ok(execute_status(&ctx))
    }
}

// ==================================================================================================
// 15 · `tool-approval.ts` — the approval predicate and the approval dialog (MCP-231, MCP-232)
// ==================================================================================================
//
// Upstream these are `tool-approval.ts`'s two exported functions over the mutable
// `McpExtensionState` record. They land here rather than in a module of their own for the reason
// section 4 already gives: `ToolMetadata`, `ApprovalOrigin`, `ApprovalOutcome`,
// `get_tool_name_candidates` and `matches_tool_pattern` are all in this file, the sole caller
// ([`execute_call`], phase 8) is in this file, and the third piece — the session cache key — is in
// [`crate::state`] beside the set it keys, exactly where `tool-approval.ts:151-152 @v2.26.1` puts it
// relative to `state.approvedToolCalls`.
//
// **Free functions, not `ProxyEnv` methods.** [`ProxyEnv::ensure_tool_call_approved`] and
// [`ProxyEnv::is_tool_call_approval_required`] stay on the trait — that is the seam a mode test
// scripts a denial through — but the trait has no production implementor yet, and upstream's are
// free functions over the state. So the *port* is here, and the eventual production `ProxyEnv`
// forwards to it in two lines. Anything else would put the gate somewhere a direct tool
// (`direct-tools.ts:432`, which has no `ProxyEnv` at all) cannot reach it.

/// `tool-approval.ts:35-93 @v2.26.1` `isToolCallApprovalRequired(config, serverName, toolMeta,
/// toolMetadata?)` — does this tool prompt before it runs? (MCP-231)
///
/// # The ladder
///
/// A per-server `approveTools` wins on **presence**, not on truthiness: `approveTools: false` on a
/// server switches approval off for it even when the global setting is `true`. `true` always
/// requires; anything that is not a non-empty list never does.
///
/// # The legacy arm, and the collision test that makes it safe
///
/// A pattern is first matched against the tool's **current** names
/// (`get_tool_name_candidates(..., include_legacy = false)`). Only when that misses does the
/// pre-2.x residue get a look — the legacy-inclusive set minus everything already current, plus one
/// explicit injection: the first non-bare current candidate with `-` mapped to `_`, which is the
/// spelling a config written against an older adapter would carry. That residue only gates the tool
/// for a pattern that does **not** also reach some other *current* tool name, which is what stops a
/// stale `approveTools` entry from silently gating a different server's tool after a rename.
///
/// # The two scopes differ in exactly one expression
///
/// `otherCurrentCandidates` — this server's tools under this server's prefix for the server scope,
/// every server's tools each under its own prefix for the global one. Upstream writes the whole
/// twenty-line block twice; here it is one parameter, per 13e's own instruction.
///
/// # The `tool_metadata == None` asymmetry is real
///
/// With no metadata to test collisions against, the **server** scope falls back to matching the
/// full legacy-inclusive set while the **global** scope returns `false`. That is not a bug to
/// normalise: a server-scoped `approveTools` names tools the user has already scoped to one server,
/// so a legacy alias cannot reach anything else, whereas a global pattern with no way to check
/// collisions must not gate on a guess. `ensure_tool_call_approved` never takes this path — it
/// always supplies a map, as upstream always passes `state.toolMetadata` — so it is reachable only
/// from `describe`/`search`'s marker and from a caller that omits the argument.
#[must_use]
pub fn is_tool_call_approval_required(
    config: &McpConfig,
    server_name: &str,
    tool: &ToolMetadata,
    tool_metadata: Option<&IndexMap<String, Vec<ToolMetadata>>>,
) -> bool {
    let definition = config.mcp_servers.get(server_name);
    let server_approval = definition.and_then(|entry| entry.approve_tools.as_ref());
    // `serverApproval !== undefined ? serverApproval : config.settings?.approveTools` — presence,
    // not truthiness, so a per-server `false` beats a global `true`.
    let approval = match server_approval {
        Some(value) => Some(value),
        None => config.settings_or_default().approve_tools(),
    };
    let patterns: &[String] = match approval {
        // `if (approval === true) return true;`
        Some(BoolOrList::All(true)) => return true,
        Some(BoolOrList::Named(list)) if !list.is_empty() => list.as_slice(),
        // `if (!Array.isArray(approval) || approval.length === 0) return false;` — which is
        // `false`, an empty list, and an absent value alike.
        _ => return false,
    };

    let prefix = resolve_tool_prefix(definition, config.tool_prefix());
    let current = get_tool_name_candidates(&tool.original_name, server_name, prefix, false);
    // Both scopes run this test first and identically, so it is hoisted out of the branch.
    if matches_tool_pattern(&current, Some(patterns)) {
        return true;
    }

    let Some(metadata) = tool_metadata else {
        return if server_approval.is_some() {
            // `matchesToolPattern(getToolNameCandidates(originalName, serverName, prefix), approval)`
            // — the DEFAULT fourth argument, i.e. legacy-inclusive and *not* minus the current set.
            matches_tool_pattern(
                &get_tool_name_candidates(&tool.original_name, server_name, prefix, true),
                Some(patterns),
            )
        } else {
            false
        };
    };

    let other_current = if server_approval.is_some() {
        // Server scope: `toolMetadata.get(serverName) ?? []`, under THIS server's prefix.
        let mut set = IndexSet::new();
        for other in metadata.get(server_name).map(Vec::as_slice).unwrap_or_default() {
            set.extend(get_tool_name_candidates(&other.original_name, server_name, prefix, false));
        }
        set
    } else {
        // Global scope: every server, each under `resolveToolPrefix(config.mcpServers[name], …)`.
        let mut set = IndexSet::new();
        for (other_server, tools) in metadata {
            let other_prefix =
                resolve_tool_prefix(config.mcp_servers.get(other_server), config.tool_prefix());
            for other in tools {
                set.extend(get_tool_name_candidates(
                    &other.original_name,
                    other_server,
                    other_prefix,
                    false,
                ));
            }
        }
        set
    };

    approval_legacy_arm(&tool.original_name, server_name, prefix, patterns, &current, other_current)
}

/// The tail both scopes of [`is_tool_call_approval_required`] share (`tool-approval.ts:53-67 @v2.26.1`,
/// repeated verbatim at `:74-92` of the same tag).
///
/// The **order** of the two mutations is load-bearing and is upstream's: the `-`→`_` alias is added
/// to the legacy set *before* the current candidates are deleted from it. If the emitted name
/// carries no `-` it IS a current candidate, and adding it after the deletion would smuggle a
/// current name into the legacy set — turning a pattern that already failed the current test into a
/// match.
fn approval_legacy_arm(
    original_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    patterns: &[String],
    current: &IndexSet<String>,
    mut other_current: IndexSet<String>,
) -> bool {
    let mut legacy = get_tool_name_candidates(original_name, server_name, prefix, true);
    // `[...currentCandidates].find(c => c !== toolMeta.originalName)?.replace(/-/g, "_")` — the
    // first prefixed spelling, normalised. `IndexSet` iterates in insertion order, which is what
    // makes "first" mean the same thing here as in a JS `Set`.
    if let Some(emitted) = current.iter().find(|candidate| *candidate != original_name) {
        legacy.insert(emitted.replace('-', "_"));
    }
    for candidate in current {
        legacy.shift_remove(candidate);
    }
    for candidate in current {
        other_current.shift_remove(candidate);
    }
    patterns.iter().any(|pattern| {
        matches_tool_pattern(&legacy, Some(std::slice::from_ref(pattern)))
            && !matches_tool_pattern(&other_current, Some(std::slice::from_ref(pattern)))
    })
}

/// `tool-approval.ts:174-176 @v2.26.1` — `JSON.stringify(args ?? {}, null, 2)` → `sanitizeTerminalText` →
/// the 500-character preview with a literal `...` tail.
///
/// The order is the security property, and the two halves cover different bytes. `JSON.stringify`
/// escapes `U+0000..U+001F` (so an `ESC` in an argument *value* reaches the dialog as the literal
/// six characters `\u001b`, inert), but it emits `U+007F` and the whole C1 block —
/// **including `U+009D`, the one-byte OSC introducer** — raw. Sanitising the rendered JSON is what
/// neutralises those. Sanitising the arguments *before* rendering would instead let the renderer
/// re-introduce nothing and would corrupt the values shown; sanitising after is both safe and
/// faithful.
///
/// `sanitized.length > 500` and `.slice(0, 500)` count UTF-16 code units in JS, so this counts
/// [`str::encode_utf16`] — the same measure [`truncate_at_word`] uses and for the same reason. The
/// one divergence, also shared with it: a cut that would land inside an astral character stops
/// before it rather than emitting the lone surrogate JS would.
///
/// **Recorded display divergence.** `JSON.stringify` emits object keys in insertion order;
/// `serde_json` without `preserve_order` emits them sorted, and the arguments arrived through
/// `serde_json` in the first place, so the model's original order is not recoverable here at all.
/// This affects only what the dialog *shows*. It cannot affect what is approved: the cache key runs
/// over [`crate::dirs::stable_stringify`], which sorts keys by construction
/// ([`crate::state::approval_cache_key`]).
fn approval_argument_preview(args: &Value) -> String {
    let empty = Value::Object(JsonMap::new());
    let rendered = serde_json::to_string_pretty(if args.is_null() { &empty } else { args })
        .unwrap_or_else(|_| "{}".to_string());
    let sanitized = crate::ui::sanitize_terminal_text(&rendered);
    if sanitized.encode_utf16().count() <= APPROVAL_PREVIEW_LENGTH {
        return sanitized;
    }
    let mut cut = sanitized.len();
    let mut used = 0usize;
    for (index, ch) in sanitized.char_indices() {
        let width = ch.len_utf16();
        if used + width > APPROVAL_PREVIEW_LENGTH {
            cut = index;
            break;
        }
        used += width;
    }
    format!("{}...", sanitized.get(..cut).unwrap_or(&sanitized))
}

/// `tool-approval.ts:142-195 @v2.26.1` `ensureToolCallApproved(state, serverName, toolMeta, args, signal,
/// origin, approvalMetadata?)` — the user's last line of defence before an MCP tool runs with
/// model-chosen arguments (MCP-232).
///
/// # The order of the checks is the unit
///
/// 1. **Session cache** — [`crate::state::approval_cache_key`]'s `(server, tool, sha256(args))`
///    triple. A hit approves without asking.
/// 2. **Is approval required at all** — [`is_tool_call_approval_required`]. Not required ⇒
///    approved, and no dialog.
/// 3. **Is there a UI** — `if (!state.ui) return {ok:false, reason:"approval_required_headless"}`.
///    **This runs BEFORE the dialog, and that ordering is the point, not an implementation
///    detail.** `HostServices::select` answers `None` for a dismissed dialog *and* for no
///    interactive surface, so a port that called `select` first and inferred the reason from `None`
///    would report "the user declined" to a batch job with no user in the room — and, worse, the
///    two states would be one, so the caller could not tell an operator "run this interactively"
///    from "someone said no".
/// 4. **The dialog** — three options, and every other answer denies.
///
/// # Fail-closed, on every arm
///
/// `Deny`, a dismissal (`None`), an unknown label, a poisoned cache lock and a cancellation all
/// resolve to [`ApprovalOutcome::Denied`] or [`ApprovalOutcome::NoInteractiveSession`]. There is no
/// path on which not-answering approves.
///
/// # The two deltas from upstream, both deliberate
///
/// * **No approval broker.** `requestBrokerApproval`'s synchronous `EventEmitter.emit` with a
///   `claim(handler)` closure is MCP-233's cut: cyrup's bus is deferred and has no return channel,
///   and `ExtHooks::before_tool_call` — which `cyrup-permission-system` already subscribes,
///   already derives MCP targets on, and which is the one `EventKind` whose `fails_closed()` is
///   `true` — *is* the broker, structurally. What that costs is recorded there: no `abstain` (a
///   permission extension that declines to decide simply does not block, which lands in the same
///   place) and no host-level `allow_for_session` (this function's own cache covers it for MCP).
/// * **Cancellation cannot interrupt an open dialog.** Upstream wraps the `select` in
///   `abortable(..., combineAbortSignals(state.owner?.signal, signal))`, which rejects mid-dialog.
///   `HostServices::select` is a blocking sync bridge with no cancellation parameter this crate can
///   supply (`DialogOptions::signal_id` is the host's own route and nothing wires it), so the token
///   is checked on **both** sides of the dialog instead: a cancelled call denies without asking,
///   and an answer that arrives after a cancellation is discarded rather than cached. The dialog
///   itself stays on screen until the human dismisses it. Stated rather than silently changed.
///
/// **Parameter order note.** Upstream's is `(state, serverName, toolMeta, args, signal, origin,
/// approvalMetadata)`; here `origin` precedes `cancel` so the signature matches
/// [`ProxyEnv::ensure_tool_call_approved`], which every other cancellable verb in this file already
/// spells with the token late. Same parameters, same meanings.
pub async fn ensure_tool_call_approved(
    state: &McpState,
    server_name: &str,
    tool: &ToolMetadata,
    args: &Value,
    origin: ApprovalOrigin,
    cancel: &CancelToken,
    approval_metadata: &IndexMap<String, Vec<ToolMetadata>>,
) -> ApprovalOutcome {
    // `origin` reaches only `requestBrokerApproval` upstream, and that is MCP-233's cut. It stays
    // in the signature because it is the caller's *statement of which surface is asking* — the
    // three-way derivation at each call site (`proxy`, `direct`, `resource`) is part of the port
    // and is asserted by the conformance tests — and because the broker's replacement, the
    // `before_tool_call` gate, is the natural place for it to become a fact again.
    let _ = origin;

    let cache_key = crate::state::approval_cache_key(server_name, &tool.original_name, args);
    // `approvedToolCalls.has(cacheKey)`. A poisoned lock reads as a MISS, so the worst a lock
    // panicked mid-insert can do is prompt the user a second time.
    if state.approved_tool_calls.lock().is_ok_and(|approved| approved.contains(&cache_key)) {
        return ApprovalOutcome::Approved;
    }

    if !is_tool_call_approval_required(&state.config, server_name, tool, Some(approval_metadata)) {
        return ApprovalOutcome::Approved;
    }

    // `if (!state.ui) return {ok: false, reason: "approval_required_headless"}` — BEFORE the
    // dialog. See the doc comment: this is what keeps "no UI" and "the user said no" apart.
    let Some(dialog) = state.dialog() else {
        return ApprovalOutcome::NoInteractiveSession;
    };

    // `ownedSignal = combineAbortSignals(state.owner?.signal, signal)` — the generation's token OR
    // the caller's. Read as a predicate rather than built with [`crate::abort::combine`]: that
    // helper spawns a joiner task so the result can be *awaited*, and this call site only ever
    // polls, so the task would be pure cost on every gated call.
    let cancelled = || !state.owner.is_active() || cancel.is_cancelled();
    if cancelled() {
        return ApprovalOutcome::Denied;
    }

    let title = format!(
        "MCP: {} wants to run {}",
        crate::ui::sanitize_terminal_text(server_name),
        crate::ui::sanitize_terminal_text(&tool.original_name)
    );
    let prompt = format!("{title}\n\nArguments:\n{}", approval_argument_preview(args));
    let decision = dialog.select(&prompt, &APPROVAL_OPTIONS).await;
    if cancelled() {
        // The answer arrived after the run was cancelled: discard it rather than caching a
        // session-wide approval nothing will use.
        return ApprovalOutcome::Denied;
    }

    match decision.as_deref() {
        Some(APPROVE_ONCE_OPTION) => ApprovalOutcome::Approved,
        Some(APPROVE_FOR_SESSION_OPTION) => {
            if let Ok(mut approved) = state.approved_tool_calls.lock() {
                approved.insert(cache_key);
            }
            // The insert is best-effort for the same reason the lookup is: a poisoned lock costs a
            // repeat prompt, never an ungated call. The approval itself still stands for THIS call.
            ApprovalOutcome::Approved
        }
        // `return {ok: false, reason: "denied"}` — the literal `Deny`, an unknown label, a
        // dismissal, a timeout, and a fenced (stopped-generation) handle all land here.
        _ => ApprovalOutcome::Denied,
    }
}

// ==================================================================================================
// 16 · Conformance (MCP-195, and the unit assertions 13d names per port unit)
// ==================================================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    // ---- fixtures --------------------------------------------------------------------------------

    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::lifecycle::McpLifecycleManager;
    use crate::state::{McpServerManager, McpStateParts};

    /// A scripted [`ProxyEnv`].
    ///
    /// MCP-196 names this as a requirement, not a convenience: the auto-auth suite "needs a
    /// controllable `needs-auth` connection state and an injectable `authenticate`". Everything the
    /// fake owns is one of those two knobs plus the counters the single-shot latch is asserted
    /// against.
    #[derive(Default)]
    struct FakeEnv {
        connections: Mutex<BTreeMap<String, ConnectionStatus>>,
        connecting: Mutex<BTreeSet<String>>,
        failures: Mutex<BTreeMap<String, u64>>,
        /// Servers `lazy_connect` succeeds for; everything else fails.
        lazy_ok: Mutex<BTreeSet<String>>,
        /// How many times `authenticate` was invoked — the latch assertion.
        authenticate_calls: AtomicUsize,
        /// `authenticate` fails when set, which drives the `failed` arm of the ladder.
        authenticate_fails: Mutex<Option<String>>,
        /// `supportsOAuth(definition)`.
        oauth_servers: Mutex<BTreeSet<String>>,
        approval: Mutex<Option<ApprovalOutcome>>,
        all_tools: Mutex<Option<Vec<String>>>,
        approval_required: Mutex<BTreeSet<String>>,
    }

    impl FakeEnv {
        fn with_connection(self, server: &str, status: ConnectionStatus) -> Self {
            self.connections.lock().unwrap().insert(server.to_string(), status);
            self
        }
        fn with_connecting(self, server: &str) -> Self {
            self.connecting.lock().unwrap().insert(server.to_string());
            self
        }
        fn with_failure(self, server: &str, age: u64) -> Self {
            self.failures.lock().unwrap().insert(server.to_string(), age);
            self
        }
        fn with_oauth(self, server: &str) -> Self {
            self.oauth_servers.lock().unwrap().insert(server.to_string());
            self
        }
        fn with_authenticate_failure(self, message: &str) -> Self {
            *self.authenticate_fails.lock().unwrap() = Some(message.to_string());
            self
        }
        fn with_all_tools(self, names: &[&str]) -> Self {
            *self.all_tools.lock().unwrap() =
                Some(names.iter().map(|name| (*name).to_string()).collect());
            self
        }
        fn with_approval_required(self, tool: &str) -> Self {
            self.approval_required.lock().unwrap().insert(tool.to_string());
            self
        }
    }

    #[async_trait::async_trait]
    impl ProxyEnv for FakeEnv {
        fn get_connection(&self, server: &str) -> Option<ConnectionStatus> {
            self.connections.lock().unwrap().get(server).copied()
        }
        fn is_connecting(&self, server: &str) -> bool {
            self.connecting.lock().unwrap().contains(server)
        }
        async fn connect(&self, server: &str, _cancel: &CancelToken) -> McpResult<ConnectOutcome> {
            Ok(ConnectOutcome { status: self.get_connection(server), ..ConnectOutcome::default() })
        }
        async fn reconnect(&self, server: &str, cancel: &CancelToken) -> McpResult<ConnectOutcome> {
            self.connect(server, cancel).await
        }
        async fn lazy_connect(&self, server: &str, _cancel: &CancelToken) -> bool {
            self.lazy_ok.lock().unwrap().contains(server)
        }
        async fn close(&self, server: &str) {
            self.connections.lock().unwrap().remove(server);
        }
        fn touch(&self, _server: &str) {}
        fn increment_in_flight(&self, _server: &str) {}
        fn decrement_in_flight(&self, _server: &str) {}
        async fn call_tool(
            &self,
            _server: &str,
            _tool: &str,
            _arguments: JsonMap<String, Value>,
            _recovery: &AuthRecovery<'_>,
            _cancel: &CancelToken,
        ) -> Result<CallToolOutcome, ProxyCallError> {
            Ok(CallToolOutcome::default())
        }
        async fn read_resource(
            &self,
            _server: &str,
            _uri: &str,
            _recovery: &AuthRecovery<'_>,
            _cancel: &CancelToken,
        ) -> Result<Vec<Content>, ProxyCallError> {
            Ok(Vec::new())
        }
        async fn handle_url_elicitation_required(
            &self,
            _server: &str,
            _detail: &str,
        ) -> UrlElicitationAction {
            UrlElicitationAction::Accept
        }
        fn failure_age_seconds(&self, server: &str) -> Option<u64> {
            self.failures.lock().unwrap().get(server).copied()
        }
        fn record_failure(&self, server: &str, _message: &str) {
            self.failures.lock().unwrap().insert(server.to_string(), 0);
        }
        fn clear_failure(&self, server: &str) {
            self.failures.lock().unwrap().remove(server);
        }
        fn update_status_bar(&self) {}
        fn update_server_metadata(&self, _server: &str) {}
        fn update_metadata_cache(&self, _server: &str) {}
        fn mark_keep_alive_after_connect(&self, _server: &str) {}
        fn commit_prompt_metadata(&self, _server: &str) {}
        fn sync_tool_surface(&self) {}
        fn supports_oauth(&self, definition: &ServerEntry) -> bool {
            definition
                .url
                .as_ref()
                .is_some_and(|url| self.oauth_servers.lock().unwrap().iter().any(|s| url.contains(s)))
        }
        fn resolve_server_url(&self, definition: &ServerEntry) -> McpResult<Option<String>> {
            Ok(definition.url.clone())
        }
        async fn authenticate(
            &self,
            _server: &str,
            _server_url: &str,
            _definition: &ServerEntry,
            _cancel: &CancelToken,
        ) -> McpResult<()> {
            self.authenticate_calls.fetch_add(1, Ordering::SeqCst);
            match self.authenticate_fails.lock().unwrap().clone() {
                Some(message) => Err(McpError::other(message)),
                None => Ok(()),
            }
        }
        async fn start_auth(
            &self,
            _server: &str,
            _server_url: &str,
            _definition: &ServerEntry,
            _cancel: &CancelToken,
        ) -> McpResult<Option<String>> {
            Ok(Some("https://auth.example.com/authorize".to_string()))
        }
        async fn complete_auth_from_input(
            &self,
            _server: &str,
            _input: &str,
            _cancel: &CancelToken,
        ) -> McpResult<String> {
            Ok("authenticated".to_string())
        }
        fn format_schema(&self, _schema: &Value, indent: &str) -> String {
            format!("{indent}(schema)")
        }
        fn render_ts_shape(&self, _schema: &Value) -> Option<String> {
            Some("{ a: string }".to_string())
        }
        fn is_tool_call_approval_required(&self, _server: &str, tool: &ToolMetadata) -> bool {
            self.approval_required.lock().unwrap().contains(&tool.name)
        }
        async fn ensure_tool_call_approved(
            &self,
            _server: &str,
            _tool: &ToolMetadata,
            _arguments: &Value,
            _origin: ApprovalOrigin,
            _cancel: &CancelToken,
        ) -> ApprovalOutcome {
            self.approval.lock().unwrap().unwrap_or(ApprovalOutcome::Approved)
        }
        async fn guard_mcp_output(
            &self,
            content: Vec<Content>,
            _options: OutputGuardOptions,
        ) -> GuardedOutput {
            GuardedOutput { content, ..GuardedOutput::default() }
        }
        fn all_tool_names(&self) -> Option<Vec<String>> {
            self.all_tools.lock().unwrap().clone()
        }
    }

    /// A context over a real [`McpState`] and a scripted [`FakeEnv`].
    fn ctx_with(
        config: McpConfig,
        metadata: &[(&str, Vec<ToolMetadata>)],
        instructions: &[(&str, &str)],
        env: FakeEnv,
    ) -> (Arc<ProxyCtx>, Arc<FakeEnv>) {
        let manager = Arc::new(McpServerManager::default());
        let lifecycle =
            Arc::new(McpLifecycleManager::new(Arc::clone(&manager), Arc::new(|_: &str| false)));
        let state = Arc::new(McpState::new(McpStateParts {
            owner: Arc::new(McpRuntimeOwner::new()),
            manager,
            lifecycle,
            config,
            programmatic_config: None,
            oauth_runtime: crate::oauth::create_oauth_runtime(None),
            auth_storage_options: crate::state::AuthStorageOptions::default(),
            ui: None,
            open_browser: Arc::new(|_| Box::pin(async { Ok(()) })),
            send_message: Arc::new(|_| {}),
        }));
        {
            let mut slot = state.server_instructions.lock().unwrap();
            for (server, text) in instructions {
                slot.insert((*server).to_string(), (*text).to_string());
            }
        }
        let env = Arc::new(env);
        let ctx = Arc::new(ProxyCtx::new(state, Arc::clone(&env) as Arc<dyn ProxyEnv>));
        {
            let mut slot = ctx.tool_metadata.lock().unwrap();
            for (server, tools) in metadata {
                slot.insert((*server).to_string(), tools.clone());
            }
        }
        (ctx, env)
    }

    fn text_of(result: &ToolResult) -> String {
        match result.content.first() {
            Some(Content::Text { text, .. }) => text.clone(),
            other => panic!("expected text content, got {other:?}"),
        }
    }

    fn stdio(command: &str) -> ServerEntry {
        ServerEntry { command: Some(command.to_string()), ..ServerEntry::default() }
    }

    fn http(url: &str) -> ServerEntry {
        ServerEntry { url: Some(url.to_string()), ..ServerEntry::default() }
    }


    /// `__tests__/search-ranking.test.ts`'s `tool(name, description)` helper.
    fn tool(name: &str, description: &str) -> ToolMetadata {
        ToolMetadata::new(name, name, description)
    }

    /// `definition(searchKeywords)` — a `command` server carrying only a keyword map.
    fn definition_with_keywords(pairs: &[(&str, &[&str])]) -> ServerEntry {
        // `IndexMap`, matching the field: insertion order is what upstream's `Object.entries`
        // walk preserves, and this helper feeds the glob-union ordering assertions below.
        let mut map: indexmap::IndexMap<String, Vec<String>> = indexmap::IndexMap::new();
        for (key, values) in pairs {
            map.insert((*key).to_string(), values.iter().map(|v| (*v).to_string()).collect());
        }
        ServerEntry {
            command: Some("npx".to_string()),
            search_keywords: Some(map),
            ..ServerEntry::default()
        }
    }

    fn config_with(servers: &[(&str, ServerEntry)]) -> McpConfig {
        let mut mcp_servers = IndexMap::new();
        for (name, entry) in servers {
            mcp_servers.insert((*name).to_string(), entry.clone());
        }
        McpConfig { mcp_servers, settings: None, imports: Vec::new() }
    }

    fn metadata_with(servers: &[(&str, Vec<ToolMetadata>)]) -> IndexMap<String, Vec<ToolMetadata>> {
        let mut map = IndexMap::new();
        for (name, tools) in servers {
            map.insert((*name).to_string(), tools.clone());
        }
        map
    }

    // ---- MCP-172 · `normalizeSearchText` / `tokenize` ---------------------------------------------

    #[test]
    fn normalize_splits_camel_case_before_lowercasing() {
        // `ID` does not split: the pattern needs a lowercase or digit BEFORE the uppercase.
        assert_eq!(normalize_search_text("getUserID_v2/foo"), "get user id v2 foo");
        // The separator class is exactly `_ . / : -`, runs collapsed to one space.
        assert_eq!(normalize_search_text("a__b..c//d::e--f"), "a b c d e f");
        // The global replace is non-overlapping: it consumes both characters of a match.
        assert_eq!(normalize_search_text("aBcD"), "a bc d");
    }

    #[test]
    fn tokenize_drops_empties_and_non_ascii() {
        assert!(tokenize("").is_empty());
        assert_eq!(tokenize("get_user_id"), vec!["get", "user", "id"]);
        // Non-ASCII identifiers tokenize to nothing — upstream's ASCII-only split.
        assert!(tokenize("日本語").is_empty());
    }

    // ---- MCP-195 · the eleven upstream ranking cases ----------------------------------------------

    /// `search ranking` › "ranks an exact name above a description match".
    #[test]
    fn ranks_an_exact_name_above_a_description_match() {
        let exact = score_tool_match(&tool("search_records", "Find records"), "demo", "search", None)
            .expect("exact name matches");
        let description =
            score_tool_match(&tool("find_records", "Search records"), "demo", "search", None)
                .expect("description matches");
        assert!(exact > description, "exact {exact} should beat description {description}");
    }

    /// `search ranking` › "drops partial two-token matches".
    #[test]
    fn drops_partial_two_token_matches() {
        assert_eq!(
            score_tool_match(&tool("search_records", "Find records"), "demo", "search missing", None),
            None
        );
    }

    /// `search ranking` › "ignores single-letter possessive tokens instead of stem-matching them".
    #[test]
    fn ignores_single_letter_possessive_tokens() {
        // "project's" tokenizes to ["project", "s"]; a bare "s" must not match "simulator".
        assert_eq!(
            score_tool_match(
                &tool("sync_icon", "Add an icon to your project's icons file."),
                "better-icons",
                "simulator",
                None
            ),
            None
        );
        // Real stems still match: "sync" (4+ chars) may prefix-match "synchronize".
        assert!(score_tool_match(&tool("sync_icon", "Sync an icon."), "better-icons", "synchronize", None)
            .is_some());
    }

    /// `search ranking` › "matches through configured keywords where the query would otherwise miss".
    #[test]
    fn matches_through_configured_keywords() {
        let advanced = tool("search_records_advanced", "Advanced record search with filters");
        let both = ["fuzzy lookup".to_string(), "legacy".to_string()];
        let one = ["fuzzy lookup".to_string()];

        assert_eq!(score_tool_match(&advanced, "demo", "fuzzy lookup", None), None);
        assert!(score_tool_match(&advanced, "demo", "fuzzy lookup", Some(&both)).is_some());
        // Single-token queries pass the coverage gate through keyword tokens too.
        assert_eq!(score_tool_match(&advanced, "demo", "fuzzy", None), None);
        assert!(score_tool_match(&advanced, "demo", "fuzzy", Some(&one)).is_some());
    }

    /// `search ranking` › "ranks an exact keyword alias above a description phrase match".
    #[test]
    fn ranks_an_exact_keyword_alias_above_a_description_phrase_match() {
        let keywords = ["fuzzy lookup".to_string()];
        let aliased = score_tool_match(
            &tool("search_records_advanced", "Advanced record search with filters"),
            "demo",
            "fuzzy lookup",
            Some(&keywords),
        )
        .expect("alias matches");
        let description =
            score_tool_match(&tool("record_search", "Fuzzy lookup across records"), "demo", "fuzzy lookup", None)
                .expect("description matches");
        assert!(aliased > description, "alias {aliased} should beat description {description}");
    }

    /// `search ranking` › "scores an exact alias above incidental cross-phrase token matches".
    ///
    /// The phrase bonus is a **max over phrases** added **once**, so "lookup legacy" — which spans
    /// two unrelated keywords — may token-match but must not collect a phrase bonus.
    #[test]
    fn scores_an_exact_alias_above_incidental_cross_phrase_token_matches() {
        let advanced = tool("search_records_advanced", "Advanced record search with filters");
        let keywords = ["fuzzy lookup".to_string(), "legacy".to_string()];
        let exact = score_tool_match(&advanced, "demo", "fuzzy lookup", Some(&keywords))
            .expect("exact alias matches");
        let cross = score_tool_match(&advanced, "demo", "lookup legacy", Some(&keywords))
            .expect("cross-phrase matches");
        assert!(exact > cross, "exact {exact} should beat cross-phrase {cross}");
    }

    /// `search ranking` › "does not change scoring when the keyword list is empty".
    ///
    /// `Some(&[])` is a no-op by [`score_tool_match`]'s non-empty guard — which is exactly why the
    /// `Some([])` / `None` distinction in [`rank_tool_matches`] is reproduced rather than collapsed.
    #[test]
    fn empty_keyword_list_does_not_change_scoring() {
        let advanced = tool("search_records_advanced", "Advanced record search");
        assert_eq!(
            score_tool_match(&advanced, "demo", "advanced", Some(&[])),
            score_tool_match(&advanced, "demo", "advanced", None)
        );
    }

    /// `search ranking` › "paginates including offsets beyond the result set".
    #[test]
    fn paginates_including_offsets_beyond_the_result_set() {
        let items = vec!["a", "b", "c"];
        assert_eq!(
            paginate(&items, 1.0, 1.0),
            Page { items: vec!["b"], total: 3, has_more: true, next_offset: Some(2) }
        );
        assert_eq!(
            paginate(&items, 5.0, 1.0),
            Page { items: Vec::new(), total: 3, has_more: false, next_offset: None }
        );
    }

    /// `resolveSearchKeywords` › "matches keys by original name, prefixed name, and glob".
    #[test]
    fn resolve_search_keywords_matches_by_original_prefixed_and_glob() {
        let cases: [(&str, &[&str]); 4] = [
            ("search_records_advanced", &["fuzzy lookup"]),
            ("demo_search_records_advanced", &["fuzzy lookup"]),
            ("search_*", &["records"]),
            ("*", &["records"]),
        ];
        let expected = ["fuzzy lookup", "fuzzy lookup", "records", "records"];
        let names = ["search_records_advanced", "search_records_advanced", "search_records_advanced", "anything"];
        for (index, (key, values)) in cases.iter().enumerate() {
            let entry = definition_with_keywords(&[(key, values)]);
            assert_eq!(
                resolve_search_keywords(Some(&entry), names[index], "demo", ToolPrefix::Server),
                vec![expected[index].to_string()],
                "case {index} ({key})"
            );
        }
    }

    /// `resolveSearchKeywords` › "unions and dedupes values from all matching keys".
    #[test]
    fn resolve_search_keywords_unions_and_dedupes() {
        let entry = definition_with_keywords(&[
            ("search_*", &["records", "fuzzy lookup"]),
            ("search_records_advanced", &["fuzzy lookup", "legacy"]),
        ]);
        assert_eq!(
            resolve_search_keywords(Some(&entry), "search_records_advanced", "demo", ToolPrefix::Server),
            vec!["records".to_string(), "fuzzy lookup".to_string(), "legacy".to_string()]
        );
    }

    /// `resolveSearchKeywords` › "returns nothing for non-matching keys or malformed config".
    ///
    /// The malformed-value arms upstream asserts (`"not-an-array"`, `["ok", 42, "  "]`) cannot be
    /// expressed while `ServerEntry::search_keywords` is `Option<BTreeMap<String, Vec<String>>>`:
    /// `lenient` has already dropped the whole field. The blank-value drop is still asserted, and
    /// the type-level divergence is recorded on [`resolve_search_keywords`].
    #[test]
    fn resolve_search_keywords_returns_nothing_for_non_matching_or_malformed() {
        let other = definition_with_keywords(&[("other_tool", &["nope"])]);
        assert!(resolve_search_keywords(Some(&other), "search_records_advanced", "demo", ToolPrefix::Server)
            .is_empty());
        let blanks = definition_with_keywords(&[("search_records_advanced", &["ok", "  "])]);
        assert_eq!(
            resolve_search_keywords(Some(&blanks), "search_records_advanced", "demo", ToolPrefix::Server),
            vec!["ok".to_string()]
        );
        assert!(resolve_search_keywords(None, "search_records_advanced", "demo", ToolPrefix::Server).is_empty());
    }

    // ---- MCP-175 · the coverage gate --------------------------------------------------------------

    #[test]
    fn coverage_gate_admits_two_of_three_and_refuses_one_of_three() {
        // No phrase match anywhere, so only the coverage ratio decides.
        let target = tool("alpha_bravo", "charlie delta");
        // 3 tokens, 2 matched = 0.667 ≥ 0.6 — survives.
        assert!(score_tool_match(&target, "srv", "alpha bravo zulu", None).is_some());
        // 3 tokens, 1 matched = 0.333 — dropped.
        assert_eq!(score_tool_match(&target, "srv", "alpha yankee zulu", None), None);
        // 2 tokens, 1 matched — a short query must match ALL its tokens.
        assert_eq!(score_tool_match(&target, "srv", "alpha zulu", None), None);
    }

    // ---- MCP-178 · `rankSuggestions` over a hyphenated server -------------------------------------

    /// The whole point of the four-mode, hyphen-**preserving** `sanitizeServerPrefix`: under
    /// `cyrup-ext-subagents`' hyphen-replacing rule the prefix would be `linear_server`, the
    /// `starts_with(prefix + "_")` test would fail, and the remainder would never be stripped.
    ///
    /// **Correction to 13d's verify line for MCP-178**, which names `linear-server_isues` →
    /// `linear-server_issues`: this ranker has no edit distance. `"isues"` tokenizes to `["isues"]`,
    /// `"issues".starts_with("isues")` is false, and a one-token query that matches none of its
    /// tokens is dropped by the coverage gate — upstream returns `[]` for that input too. The
    /// assertion below uses a near-miss the algorithm can actually resolve (a singular/plural slip),
    /// which is the behaviour the unit exists to pin.
    #[test]
    fn rank_suggestions_strips_a_hyphenated_server_prefix() {
        let config = config_with(&[("linear-server", ServerEntry::default())]);
        let metadata = metadata_with(&[(
            "linear-server",
            vec![ToolMetadata::new("linear-server_issues", "issues", "List issues")],
        )]);
        assert_eq!(
            rank_suggestions(&config, &metadata, "linear-server_issue", 5),
            vec!["linear-server_issues".to_string()]
        );
        // No edit distance: a transposed/dropped letter falls off the coverage gate, upstream and here.
        assert!(rank_suggestions(&config, &metadata, "linear-server_isues", 5).is_empty());
        assert_eq!(get_server_prefix("linear-server", ToolPrefix::Server), "linear-server");
        assert_eq!(get_server_prefix("gh-mcp", ToolPrefix::Short), "gh");
        assert_eq!(get_server_prefix("gh-mcp", ToolPrefix::Mcp), "mcp__gh-mcp");
        assert_eq!(get_server_prefix("gh-mcp", ToolPrefix::None), "");
    }

    #[test]
    fn longest_prefix_wins_for_lazy_discovery() {
        // Two servers whose prefixes nest: `foo-bar_x` must resolve against `foo-bar`, not `foo`.
        let mut candidates = vec![
            ("foo".to_string(), get_server_prefix("foo", ToolPrefix::Server)),
            ("foo-bar".to_string(), get_server_prefix("foo-bar", ToolPrefix::Server)),
        ];
        candidates.retain(|(_, prefix)| "foo-bar_x".starts_with(&format!("{prefix}_")));
        candidates.sort_by_key(|(_, prefix)| std::cmp::Reverse(prefix.len()));
        assert_eq!(candidates.first().map(|(name, _)| name.as_str()), Some("foo-bar"));
    }

    // ---- MCP-170 · insertion order decides which server is named ----------------------------------

    #[test]
    fn insertion_order_decides_the_disabled_server_named_first() {
        let disabled = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_with(&[("zeta", disabled.clone()), ("alpha", disabled)]);
        let metadata = metadata_with(&[
            ("zeta", vec![ToolMetadata::new("t", "t", "")]),
            ("alpha", vec![ToolMetadata::new("t", "t", "")]),
        ]);
        // Both are disabled, so `getEnabledToolMatches` is empty and the fallback scan names the
        // FIRST disabled hit in insertion order.
        assert!(get_enabled_tool_matches(&config, &metadata, "t", true).is_empty());
        let first_disabled = metadata
            .keys()
            .find(|server| config.mcp_servers.get(*server).is_some_and(ServerEntry::is_disabled))
            .cloned();
        assert_eq!(first_disabled, Some("zeta".to_string()));
    }

    // ---- MCP-163 · the ambiguity gate fails closed -------------------------------------------------

    #[test]
    fn get_single_tool_match_fails_closed_for_duplicates() {
        let duplicates = vec![
            ToolMetadata::new("create_issue", "create_issue", "a"),
            ToolMetadata::new("create_issue", "create_issue", "b"),
        ];
        assert_eq!(get_single_tool_match(Some(&duplicates), "create_issue"), SingleMatch::Ambiguous);

        // A single exact match beats an earlier normalized fallback.
        let mixed = vec![
            ToolMetadata::new("create-issue", "create-issue", "fuzzy"),
            ToolMetadata::new("create_issue", "create_issue", "exact"),
        ];
        match get_single_tool_match(Some(&mixed), "create_issue") {
            SingleMatch::One(found) => assert_eq!(found.description, "exact"),
            other => panic!("expected the exact match, got {other:?}"),
        }

        // Two tools that collide ONLY after `-`→`_` normalization also fail closed — upstream's
        // "fails closed for same-server normalized fallback collisions". The query must have no
        // exact match for the fuzzy set to be consulted at all; when it does have one, the exact
        // match wins outright and there is nothing ambiguous about it.
        let normalized = vec![
            ToolMetadata::new("cre-ate_issue", "cre-ate_issue", "a"),
            ToolMetadata::new("cre_ate_issue", "cre_ate_issue", "b"),
        ];
        assert_eq!(get_single_tool_match(Some(&normalized), "cre-ate-issue"), SingleMatch::Ambiguous);
        // …and an exact hit against one of the two is NOT ambiguous.
        match get_single_tool_match(Some(&normalized), "cre_ate_issue") {
            SingleMatch::One(found) => assert_eq!(found.description, "b"),
            other => panic!("an exact match wins outright, got {other:?}"),
        }
        assert_eq!(get_single_tool_match(None, "anything"), SingleMatch::None);
    }

    #[test]
    fn ambiguous_and_disabled_results_carry_their_codes() {
        let ambiguous = ambiguous_tool_result("call", "create_issue");
        let details = ambiguous.details.expect("details");
        assert_eq!(details["error"], json!("ambiguous_tool"));
        assert_eq!(details["mode"], json!("call"));
        assert_eq!(
            details["message"],
            json!("Tool \"create_issue\" matches multiple servers. Specify a server.")
        );

        let disabled = disabled_result("list", "gh");
        let details = disabled.details.expect("details");
        assert_eq!(details["error"], json!("server_disabled"));
        assert_eq!(
            details["message"],
            json!("Server \"gh\" is disabled. Run /mcp enable gh and /reload to enable it.")
        );
    }

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

    // ---- MCP-151 / MCP-194 · the tool schema --------------------------------------------------------

    #[test]
    fn tool_schema_declares_twelve_optional_properties_and_two_actions() {
        let schema = mcp_tool_schema();
        assert_eq!(schema["type"], json!("object"));
        assert!(schema.get("required").is_none(), "every property is optional");
        let properties = schema["properties"].as_object().expect("properties");
        assert_eq!(properties.len(), 12);
        for name in [
            "tool", "args", "connect", "describe", "instructions", "search", "regex",
            "includeSchemas", "limit", "offset", "server", "action",
        ] {
            assert!(properties.contains_key(name), "missing property {name}");
        }
        // `args` is a union, not a bare string.
        assert!(properties["args"]["anyOf"].is_array());
        // The cut-driven edit: exactly two legal actions are named.
        let action = properties["action"]["description"].as_str().expect("description");
        assert_eq!(action, "Action: 'auth-start' or 'auth-complete'");
        assert!(!action.contains("ui-messages"));

        // MCP-194: the decision is visible in the test. `serde_json::Map` is a `BTreeMap` under this
        // workspace's features, so the properties serialise alphabetically.
        let order: Vec<&str> = properties.keys().map(String::as_str).collect();
        assert_eq!(
            order,
            vec![
                "action", "args", "connect", "describe", "includeSchemas", "instructions", "limit",
                "offset", "regex", "search", "server", "tool"
            ]
        );
    }

    // ---- MCP-153 · args coercion --------------------------------------------------------------------

    #[test]
    fn parse_args_accepts_objects_and_json_strings_and_throws_otherwise() {
        assert_eq!(parse_args(None).unwrap(), None);
        assert_eq!(parse_args(Some(&json!(""))).unwrap(), None);
        assert_eq!(parse_args(Some(&json!({"a": 1}))).unwrap(), Some(json!({"a": 1})));
        assert_eq!(parse_args(Some(&json!("{\"a\":1}"))).unwrap(), Some(json!({"a": 1})));

        let array = parse_args(Some(&json!([]))).unwrap_err();
        assert_eq!(array.message, "Invalid args: expected a JSON object, got array");
        let null_literal = parse_args(Some(&json!("null"))).unwrap_err();
        assert_eq!(null_literal.message, "Invalid args: expected a JSON object, got null");
        let number = parse_args(Some(&json!(7))).unwrap_err();
        assert_eq!(number.message, "Invalid args: expected a JSON object, got number");
        let broken = parse_args(Some(&json!("{"))).unwrap_err();
        assert!(broken.message.starts_with("Invalid args JSON: "), "{}", broken.message);
    }

    #[test]
    fn has_gateway_mode_reads_exactly_the_seven_dispatch_keys() {
        let mut params = McpToolParams::default();
        assert!(!params.has_gateway_mode());
        params.args = Some(json!({"a": 1}));
        assert!(!params.has_gateway_mode(), "`args` alone is not a mode");
        params.regex = Some(true);
        params.include_schemas = Some(false);
        params.limit = Some(5.0);
        params.offset = Some(1.0);
        assert!(!params.has_gateway_mode(), "the four tuning keys are not modes");
        params.search = Some(String::new());
        assert!(params.has_gateway_mode(), "`search: \"\"` IS a mode");
    }

    // ---- MCP-167 · manual OAuth text -----------------------------------------------------------------

    #[test]
    fn manual_auth_instructions_are_byte_exact_with_and_without_a_port() {
        let with_port = format_manual_auth_instructions(
            "linear",
            "https://auth.example.com/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A8976%2Fcallback",
        );
        assert_eq!(get_redirect_port(
            "https://auth.example.com/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A8976%2Fcallback"
        ), Some(8976));
        assert!(with_port.starts_with("MCP OAuth required for \"linear\".\n"));
        // `portNote` begins with `\n`, so the rendered text carries a BLANK LINE before it.
        assert!(
            with_port.contains("supported.\n\nThe redirect URL will use local port 8976."),
            "expected a blank line before the port note:\n{with_port}"
        );
        assert!(with_port.contains(
            "mcp({ action: \"auth-complete\", server: \"linear\", args: { redirectUrl: \"PASTE_REDIRECT_URL_HERE\" } })"
        ));

        // No parseable port ⇒ the last two lines are absent entirely.
        let without_port = format_manual_auth_instructions("linear", "https://auth.example.com/authorize");
        assert_eq!(get_redirect_port("https://auth.example.com/authorize"), None);
        assert!(!without_port.contains("local port"));
        assert!(without_port.ends_with("JSON-string args remain supported."));
        // A default-port redirect is normalised away by both `new URL().port` and `Url::port()`.
        assert_eq!(
            get_redirect_port("https://a.example/x?redirect_uri=http%3A%2F%2Flocalhost%2Fcb"),
            None
        );
    }

    // ---- `truncateAtWord` ----------------------------------------------------------------------------

    #[test]
    fn truncate_at_word_cuts_at_the_last_space_past_sixty_percent() {
        assert_eq!(truncate_at_word("short", 50), "short");
        assert_eq!(truncate_at_word("", 50), "");
        // Last space at index 8 of a 10-char budget: 8 > 6, so cut there.
        assert_eq!(truncate_at_word("abcdefgh ijklmnop", 10), "abcdefgh...");
        // Last space at index 2 of a 10-char budget: 2 <= 6, so cut at the budget.
        assert_eq!(truncate_at_word("ab cdefghijklmnop", 10), "ab cdefghi...");
        // No space at all: cut at the budget.
        assert_eq!(truncate_at_word("abcdefghijklmnop", 10), "abcdefghij...");
    }

    // ---- MCP-192 · the permission system's guideline key ---------------------------------------------

    #[test]
    fn guideline_normalises_to_the_sanitizer_key() {
        // The sanitizer's normalisation is bullet-strip + whitespace-collapse + lowercase, so the
        // source string need not itself be lowercase.
        let normalised = MCP_TOOL_GUIDELINE
            .trim_start_matches(['-', '*', '•', ' '])
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        assert_eq!(
            normalised,
            "use mcp for mcp discovery first: search by capability, describe one exact tool name, then call it."
        );
        assert_eq!(MCP_TOOL_NAME, "mcp");
    }

    // ---- MCP-152 / MCP-198 · the regenerated description ----------------------------------------------

    #[test]
    fn proxy_description_renders_every_block_in_order() {
        let github = ServerEntry { command: Some("npx".to_string()), ..ServerEntry::default() };
        let docs = ServerEntry { command: Some("npx".to_string()), ..ServerEntry::default() };
        let off = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_with(&[("github", github), ("docs", docs), ("legacy", off)]);

        let mut cache: IndexMap<String, CachedServerEntry> = IndexMap::new();
        cache.insert(
            "github".to_string(),
            CachedServerEntry {
                tools: vec![
                    ToolMetadata::new("github_create_issue", "create_issue", "Open an issue"),
                    ToolMetadata::new("github_list_prs", "list_prs", "List PRs"),
                ],
                resources: Vec::new(),
                instructions: None,
            },
        );
        cache.insert(
            "docs".to_string(),
            CachedServerEntry {
                tools: vec![ToolMetadata::new("docs_search", "search", "Search docs")],
                resources: Vec::new(),
                instructions: Some("  Always   cite the   page number.  ".to_string()),
            },
        );

        let direct = [DirectToolSummary {
            server_name: "github".to_string(),
            prefixed_name: "github_create_issue".to_string(),
        }];
        let description = build_proxy_description(&config, &cache, &direct);

        // 1 · the post-cut header, with `mcpScript` removed and the host renamed.
        assert!(description.starts_with(
            "MCP gateway — server status, tool search/describe, auth, and single MCP tool calls. Non-MCP cyrup tools should be called directly, not through mcp.\n"
        ));
        assert!(!description.contains("mcpScript"));
        assert!(!description.contains("Pi tools"));
        // 2 · direct-tool counts.
        assert!(description.contains("\nDirect tools available (call as normal tools): github (1)\n"));
        // 3 · proxy counts: github has 2 cached tools minus 1 direct = 1.
        assert!(description.contains("\nServers: github (1 tools), docs (1 tools)\n"));
        // 4 · disabled servers.
        assert!(description.contains(
            "\nDisabled servers (enable with /mcp enable <server> and /reload): legacy\n"
        ));
        // 5 · instruction snippets — whitespace collapsed, two-space indent part of the line.
        assert!(description.contains(
            "\nServer instructions (truncated - full text via mcp({ instructions: \"name\" })):\n  docs: Always cite the page number.\n"
        ));
        // 6 · the usage block, with the ui-messages line gone and no trailing newline.
        assert!(!description.contains("ui-messages"));
        assert!(description.ends_with(
            "\nMode: action > tool (call) > connect > describe > instructions > search > server (list) > nothing (status)"
        ));
        assert_eq!(description.matches('→').count(), 9, "nine usage arrows survive the cut");
    }

    /// MCP-198 · a tool hidden by `uiVisibility` is not counted, and does not reserve its name in
    /// the cross-server collision set.
    #[test]
    fn hidden_tools_are_excluded_from_the_advertised_counts() {
        let server = ServerEntry { command: Some("npx".to_string()), ..ServerEntry::default() };
        let config = config_with(&[("app", server)]);
        let mut cache: IndexMap<String, CachedServerEntry> = IndexMap::new();
        let mut hidden = ToolMetadata::new("app_widget", "widget", "App-only");
        hidden.ui_visibility = Some(vec!["app".to_string()]);
        cache.insert(
            "app".to_string(),
            CachedServerEntry {
                tools: vec![hidden, ToolMetadata::new("app_open", "open", "Open")],
                resources: Vec::new(),
                instructions: None,
            },
        );
        let description = build_proxy_description(&config, &cache, &[]);
        assert!(description.contains("\nServers: app (1 tools)\n"), "{description}");
        assert!(is_ui_tool_visible_to_model(None));
        assert!(is_ui_tool_visible_to_model(Some(&["model".to_string()])));
        assert!(!is_ui_tool_visible_to_model(Some(&[])));
        assert!(!is_ui_tool_visible_to_model(Some(&["app".to_string()])));
    }

    /// MCP-198 · the two-tier selector and its collision guard.
    ///
    /// A pattern that only reaches a tool's **legacy** spelling is disarmed when that same spelling
    /// is some other configured tool's *current* name — which is the whole reason
    /// `buildProxyDescription` computes a cross-server candidate set at all.
    #[test]
    fn tool_selectors_are_two_tier_and_collision_guarded() {
        let none: Option<&[String]> = None;
        // No filters at all ⇒ allowed.
        assert!(is_tool_allowed("do-it", "srv", ToolPrefix::Server, none, none, None));

        // A current-candidate include selects; a miss does not.
        let include_current = ["srv_do-it".to_string()];
        assert!(is_tool_allowed("do-it", "srv", ToolPrefix::Server, Some(&include_current), none, None));
        let include_other = ["something_else".to_string()];
        assert!(!is_tool_allowed("do-it", "srv", ToolPrefix::Server, Some(&include_other), none, None));

        // A current-candidate exclude excludes.
        let exclude_current = ["srv_do-it".to_string()];
        assert!(!is_tool_allowed("do-it", "srv", ToolPrefix::Server, none, Some(&exclude_current), None));

        // `do_it` is a LEGACY-only candidate of `do-it`.
        let current = get_tool_name_candidates("do-it", "srv", ToolPrefix::Server, false);
        let legacy = get_tool_name_candidates("do-it", "srv", ToolPrefix::Server, true);
        assert!(!current.contains("do_it"));
        assert!(legacy.contains("do_it"));

        let exclude_legacy = ["do_it".to_string()];
        // …with no collision context it still excludes…
        assert!(!is_tool_allowed("do-it", "srv", ToolPrefix::Server, none, Some(&exclude_legacy), None));
        // …and with a collision set that does not contain it, likewise.
        let quiet: IndexSet<String> = current.clone();
        assert!(!is_tool_allowed(
            "do-it",
            "srv",
            ToolPrefix::Server,
            none,
            Some(&exclude_legacy),
            Some(&quiet)
        ));
        // But when `do_it` is another server's CURRENT name, the selector is disarmed.
        let mut collides: IndexSet<String> = current.clone();
        collides.insert("do_it".to_string());
        assert!(is_tool_allowed(
            "do-it",
            "srv",
            ToolPrefix::Server,
            none,
            Some(&exclude_legacy),
            Some(&collides)
        ));
    }

    /// A configured `excludeTools` really does lower the count the model reads.
    #[test]
    fn excluded_tools_drop_out_of_the_advertised_count() {
        let filtered = ServerEntry {
            command: Some("npx".to_string()),
            exclude_tools: Some(vec!["srv_secret".to_string()]),
            ..ServerEntry::default()
        };
        let config = config_with(&[("srv", filtered)]);
        let mut cache: IndexMap<String, CachedServerEntry> = IndexMap::new();
        cache.insert(
            "srv".to_string(),
            CachedServerEntry {
                tools: vec![
                    ToolMetadata::new("srv_secret", "secret", ""),
                    ToolMetadata::new("srv_public", "public", ""),
                ],
                resources: vec![("notes.md".to_string(), "file:///notes.md".to_string())],
                instructions: None,
            },
        );
        let description = build_proxy_description(&config, &cache, &[]);
        // 2 tools − 1 excluded + 1 resource (`read_notes_md`) = 2.
        assert!(description.contains("\nServers: srv (2 tools)\n"), "{description}");
        assert_eq!(resource_name_to_tool_name("notes.md"), "notes_md");
        assert_eq!(resource_name_to_tool_name("9lives"), "resource_9lives");
        assert_eq!(resource_name_to_tool_name("__A B__"), "a_b");
    }

    /// `faf55f7` — the cross-server collision scan does not run *at all* when no server declares a
    /// selector.
    ///
    /// Upstream proves this by mocking `getToolNameCandidates` and asserting zero calls
    /// (`__tests__/collision-scan-lazy.test.ts`). The Rust equivalent is to assert the scan's
    /// product: [`collision_candidates`] is the only thing on this path that expands candidates, so
    /// an empty set is proof the scan was *skipped*, not merely fast — two servers whose tool names
    /// collide would otherwise both be indexed.
    #[test]
    fn collision_scan_is_skipped_when_no_server_declares_a_selector() {
        let mut cache: IndexMap<String, CachedServerEntry> = IndexMap::new();
        for server in ["a", "b"] {
            cache.insert(
                server.to_string(),
                CachedServerEntry {
                    tools: vec![ToolMetadata::new(format!("{server}_search"), "search", "Search")],
                    resources: Vec::new(),
                    instructions: None,
                },
            );
        }

        let unfiltered = config_with(&[("a", stdio("npx")), ("b", stdio("npx"))]);
        assert!(
            collision_candidates(&unfiltered, &cache, unfiltered.tool_prefix()).is_empty(),
            "no includeTools/excludeTools anywhere — the O(tools²) scan must not run",
        );

        // Positive control: one selector on one server re-arms the scan for the whole call, and the
        // set spans the filtered server too — a tool's own candidates are subtracted by match count
        // inside `index_has_other_current_match`, never by omitting them here.
        let filtered =
            ServerEntry { exclude_tools: Some(vec!["a_search".to_string()]), ..stdio("npx") };
        let armed = config_with(&[("a", filtered), ("b", stdio("npx"))]);
        let candidates = collision_candidates(&armed, &cache, armed.tool_prefix());
        assert!(candidates.contains("b_search"), "{candidates:?}");
        assert!(candidates.contains("a_search"), "{candidates:?}");

        // …and skipping it changes nothing the model reads: the counts are identical either way,
        // which is the whole claim — a pure startup-cost fix, not a behaviour change.
        let described = build_proxy_description(&unfiltered, &cache, &[]);
        assert!(described.contains("\nServers: a (1 tools), b (1 tools)\n"), "{described}");
        let described = build_proxy_description(&armed, &cache, &[]);
        assert!(described.contains("\nServers: b (1 tools)\n"), "{described}");
    }

    // ---- MCP-159 · the regex path's rejection codes ---------------------------------------------------

    #[test]
    fn regex_gate_rejects_over_long_queries_and_uncompilable_patterns() {
        let long_query = "a".repeat(MAX_REGEX_SEARCH_QUERY_LENGTH + 1);
        assert!(long_query.chars().count() > MAX_REGEX_SEARCH_QUERY_LENGTH);
        // A backreference is JS-legal and `regex`-illegal: the named residual of dropping `recheck`.
        // Assembled at runtime because clippy's `invalid_regex` lint rejects the literal — which is
        // precisely the property under test.
        let backreference = format!("(a){}1", '\\');
        assert!(
            regex::RegexBuilder::new(&backreference)
                .case_insensitive(true)
                .size_limit(REGEX_SIZE_LIMIT)
                .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
                .build()
                .is_err()
        );
        // RE-SPECIFIED, not ported (MCP-159): upstream's "rejects catastrophic-backtracking regex
        // queries" asserted `unsafe_pattern`. Rust's `regex` is a finite automaton with a
        // linear-time matching guarantee, so the pattern compiles, runs, and finishes.
        let nested = regex::RegexBuilder::new("(a+)+$")
            .case_insensitive(true)
            .size_limit(REGEX_SIZE_LIMIT)
            .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
            .build()
            .expect("a nested quantifier compiles under a linear-time engine");
        let start = std::time::Instant::now();
        assert!(!nested.is_match(&format!("{}b", "a".repeat(64))));
        assert!(start.elapsed() < std::time::Duration::from_millis(250));
    }

    // ---- MCP-197 · the render-shell fork ----------------------------------------------------------------

    #[test]
    fn render_shell_defaults_to_self_and_flips_on_boxed() {
        let compact = McpSettings::default();
        assert_eq!(compact.tool_result_rendering(), ToolResultRendering::Compact);
        let boxed = McpSettings {
            tool_result_rendering: Some(ToolResultRendering::Boxed),
            ..McpSettings::default()
        };
        assert_eq!(boxed.tool_result_rendering(), ToolResultRendering::Boxed);

        let (_, rx) = tokio::sync::watch::channel(InitPhase::NotInitialized);
        let gate = Arc::new(ProxyInitGate::new(rx));
        let default_tool = McpTool::new(String::new(), &compact, Arc::clone(&gate));
        assert_eq!(default_tool.render_kind(), ToolRenderKind::SelfRendered);
        let boxed_tool = McpTool::new(String::new(), &boxed, gate);
        assert_eq!(boxed_tool.render_kind(), ToolRenderKind::Default);
        assert_eq!(boxed_tool.name(), "mcp");
        assert_eq!(boxed_tool.label(), Some("MCP"));
        assert_eq!(boxed_tool.prompt_snippet(), Some(MCP_TOOL_PROMPT_SNIPPET));
        assert_eq!(boxed_tool.prompt_guidelines(), vec![MCP_TOOL_GUIDELINE]);
    }

    // ---- the dispatch preamble's three mode-less envelopes ------------------------------------------------

    #[tokio::test]
    async fn not_initialized_and_timeout_envelopes_carry_no_mode_key() {
        let (_keep, rx) = tokio::sync::watch::channel(InitPhase::NotInitialized);
        let gate = Arc::new(ProxyInitGate::new(rx));
        let tool = McpTool::new(String::new(), &McpSettings::default(), gate);
        let result = tool
            .execute(
                ToolCallId::from("call-1"),
                json!({}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("the not-initialized envelope is returned, not thrown");
        let details = result.details.expect("details");
        assert_eq!(details["error"], json!("not_initialized"));
        assert!(details.get("mode").is_none(), "the init envelopes carry NO mode key");
        match result.content.first() {
            Some(Content::Text { text, .. }) => assert_eq!(text, "MCP not initialized"),
            other => panic!("expected text content, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_live_init_is_raced_against_the_thirty_second_timeout() {
        let (keep, rx) = tokio::sync::watch::channel(InitPhase::Pending);
        let gate = Arc::new(ProxyInitGate::new(rx));
        let tool = McpTool::new(String::new(), &McpSettings::default(), gate);
        let result = tool
            .execute(ToolCallId::from("call-2"), json!({}), CancelToken::new(), Box::new(|_| {}))
            .await
            .expect("the timeout envelope is returned, not thrown");
        drop(keep);
        let details = result.details.expect("details");
        assert_eq!(details["error"], json!("init_timeout"));
        assert_eq!(details["timeoutMs"], json!(30_000));
        assert!(details.get("mode").is_none());
    }

    #[tokio::test]
    async fn invalid_args_are_thrown_before_the_init_gate_is_consulted() {
        let (_keep, rx) = tokio::sync::watch::channel(InitPhase::NotInitialized);
        let gate = Arc::new(ProxyInitGate::new(rx));
        let tool = McpTool::new(String::new(), &McpSettings::default(), gate);
        let error = tool
            .execute(
                ToolCallId::from("call-3"),
                json!({"tool": "x", "args": "[]"}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect_err("a bad `args` is an Err(ToolError), never a details.error code");
        assert_eq!(error.message, "Invalid args: expected a JSON object, got array");
    }

    /// `index.ts:886-906` (upstream `1bf3671` "fix: recover nested mcp proxy args", #364) — a model
    /// that wraps the WHOLE gateway request in `args` used to match no arm and silently get status
    /// back. The rescue re-reads `args` as the params object and re-parses ITS `args`, so a nested
    /// request dispatches exactly as if it had been passed top-level.
    #[tokio::test]
    async fn gateway_params_nested_inside_args_are_rescued_and_dispatched() {
        let config = config_with(&[("srv", stdio("a"))]);
        let tools: Vec<ToolMetadata> = (0..5)
            .map(|index| {
                ToolMetadata::new(
                    format!("srv_report_{index}"),
                    format!("report_{index}"),
                    "Reporting",
                )
            })
            .collect();
        let (ctx, _env) = ctx_with(config, &[("srv", tools)], &[], FakeEnv::default());
        let (_keep, rx) = tokio::sync::watch::channel(InitPhase::Ready(ctx));
        let gate = Arc::new(ProxyInitGate::new(rx));
        let tool = McpTool::new(String::new(), &McpSettings::default(), gate);

        // A JSON-STRING nesting. `search` AND its `limit` come out of the rescued object, so the
        // page is 3 wide rather than the 12-wide default — proof the whole object, not just the
        // dispatch key, is what dispatch now reads.
        let rescued = tool
            .execute(
                ToolCallId::from("call-1"),
                json!({"args": "{\"search\":\"report\",\"limit\":3}"}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("a rescued request dispatches like a top-level one");
        let details = rescued.details.expect("details");
        assert_eq!(details["mode"], json!("search"), "`status` would mean the rescue never ran");
        assert_eq!(details["count"], json!(5));
        assert_eq!(details["nextOffset"], json!(3), "`limit: 3` came from the nested object");

        // An OBJECT nesting reaches the later arms too.
        let described = tool
            .execute(
                ToolCallId::from("call-2"),
                json!({"args": {"describe": "srv_report_0"}}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("a rescued request dispatches like a top-level one");
        assert_eq!(described.details.expect("details")["mode"], json!("describe"));

        // `parsedArgs = parseArgs(nestedParams.args)` — the INNER `args` is parsed a second time, so
        // a broken inner string throws instead of searching with the outer object still in hand.
        let error = tool
            .execute(
                ToolCallId::from("call-3"),
                json!({"args": {"search": "report", "args": "{"}}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect_err("the nested `args` is re-parsed after the rescue");
        assert!(error.message.starts_with("Invalid args JSON: "), "{}", error.message);
    }

    /// `index.ts:902` and `index.ts:905` — an `args` that is NOT a gateway request is a hard error, never a silent
    /// status. Both throw sites carry the same sentence: a parsed-but-modeless object, and an `args`
    /// that parses to nothing (`""`) yet was still supplied.
    #[tokio::test]
    async fn non_gateway_params_nested_inside_args_are_rejected_before_the_gate() {
        // A gate that never initialises: status would still RETURN an envelope here, so an `Err`
        // also proves the rescue runs ahead of the init gate.
        let (_keep, rx) = tokio::sync::watch::channel(InitPhase::NotInitialized);
        let gate = Arc::new(ProxyInitGate::new(rx));
        let tool = McpTool::new(String::new(), &McpSettings::default(), gate);
        const NESTED: &str = "Gateway params were nested inside `args`; pass them top-level (for example, mcp({ search: \"...\" }) or mcp({ tool: \"...\", args: {} })).";

        for params in [
            json!({"args": "{\"query\":\"screenshot\"}"}),
            json!({"args": {}}),
            // `parseArgs("")` yields nothing, but `params.args !== undefined` still holds.
            json!({"args": ""}),
        ] {
            let error = tool
                .execute(
                    ToolCallId::from("call-1"),
                    params.clone(),
                    CancelToken::new(),
                    Box::new(|_| {}),
                )
                .await
                .expect_err("a modeless `args` is thrown, never answered with status");
            assert_eq!(error.message, NESTED, "{params}");
        }

        // No `args` at all is NOT the nested case — it is plain status, which the dead gate reports.
        let status = tool
            .execute(ToolCallId::from("call-2"), json!({}), CancelToken::new(), Box::new(|_| {}))
            .await
            .expect("a bare call is status, not the nested error");
        assert_eq!(status.details.expect("details")["error"], json!("not_initialized"));
    }

    /// `index.ts:880-882` — `parseArgs(null)` is NOT `parseArgs(undefined)`. `typeof null ===
    /// "object"` is why upstream spells the null test separately, and `1bf3671`'s
    /// `params.args !== undefined` arm (`index.ts:903`) makes the distinction load-bearing a second
    /// time. Serde maps a present `null` onto `None`, so without
    /// [`super::present_value`] both arms would miss and the call would answer with a status
    /// envelope instead of throwing.
    #[tokio::test]
    async fn an_explicit_null_args_is_thrown_where_an_absent_args_is_status() {
        let (_keep, rx) = tokio::sync::watch::channel(InitPhase::NotInitialized);
        let gate = Arc::new(ProxyInitGate::new(rx));
        let tool = McpTool::new(String::new(), &McpSettings::default(), gate);

        // Modeless AND with a gateway mode: `parseArgs` runs first either way, so both throw the
        // args sentence rather than the nested one or a status envelope.
        for params in [json!({"args": null}), json!({"tool": "demo_run", "args": null})] {
            let error = tool
                .execute(
                    ToolCallId::from("call-null"),
                    params.clone(),
                    CancelToken::new(),
                    Box::new(|_| {}),
                )
                .await
                .expect_err("an explicit null `args` throws");
            assert_eq!(
                error.message, "Invalid args: expected a JSON object, got null",
                "{params}"
            );
        }

        // The absent key still reaches status — the two are distinguished, not merged the other way.
        let status = tool
            .execute(ToolCallId::from("call-absent"), json!({}), CancelToken::new(), Box::new(|_| {}))
            .await
            .expect("an absent `args` is status");
        assert_eq!(status.details.expect("details")["error"], json!("not_initialized"));

        // And the mapping is exactly "present or not" at the serde layer.
        let present: McpToolParams =
            serde_json::from_value(json!({"args": null})).expect("null is a valid args value");
        assert_eq!(present.args, Some(Value::Null));
        let absent: McpToolParams = serde_json::from_value(json!({})).expect("args is optional");
        assert_eq!(absent.args, None);
    }

    // ---- MCP-154 · `executeStatus` ------------------------------------------------------------------

    #[test]
    fn status_renders_all_six_rungs_with_their_glyphs() {
        let disabled = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_with(&[
            ("live", stdio("a")),
            ("waiting", http("https://a.example")),
            ("broken", stdio("b")),
            ("warm", stdio("c")),
            ("cold", stdio("d")),
            ("off", disabled),
        ]);
        let env = FakeEnv::default()
            .with_connection("live", ConnectionStatus::Connected)
            .with_connection("waiting", ConnectionStatus::NeedsAuth)
            .with_failure("broken", 12);
        let (ctx, _) = ctx_with(
            config,
            &[
                ("live", vec![ToolMetadata::new("live_a", "a", ""), ToolMetadata::new("live_b", "b", "")]),
                ("warm", vec![ToolMetadata::new("warm_a", "a", "")]),
            ],
            &[],
            env,
        );

        let result = execute_status(&ctx);
        let text = text_of(&result);
        // The header counts ENABLED servers only, and totals only their tools.
        assert!(text.starts_with("MCP: 1/5 servers, 3 tools (1 disabled)\n\n"), "{text}");
        assert!(text.contains("✓ live (2 tools)\n"), "{text}");
        assert!(text.contains("⚠ waiting (needs auth)\n"), "{text}");
        assert!(text.contains("✗ broken (failed 12s ago)\n"), "{text}");
        assert!(text.contains("○ warm (1 tools, cached)\n"), "{text}");
        assert!(text.contains("○ cold (not connected)\n"), "{text}");
        assert!(text.contains("⊘ off (disabled)\n"), "{text}");
        assert!(text.ends_with(
            "mcp({ server: \"name\" }) to list tools, mcp({ search: \"...\" }) to search"
        ));

        let details = result.details.expect("details");
        assert_eq!(details["mode"], json!("status"));
        assert_eq!(details["totalTools"], json!(3));
        assert_eq!(details["connectedCount"], json!(1));
        assert_eq!(details["disabledCount"], json!(1));
        let rows = details["servers"].as_array().expect("servers");
        // Insertion order, not alphabetical.
        let names: Vec<&str> = rows.iter().map(|row| row["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["live", "waiting", "broken", "warm", "cold", "off"]);
        // `disabled` is present ONLY when true.
        assert!(rows[0].get("disabled").is_none());
        assert_eq!(rows[5]["disabled"], json!(true));
        assert_eq!(rows[2]["failedAgo"], json!(12));
        assert_eq!(rows[0]["failedAgo"], Value::Null);
    }

    // ---- MCP-155 · `executeList` ---------------------------------------------------------------------

    #[test]
    fn list_covers_its_five_outcomes() {
        let disabled = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_with(&[
            ("empty_live", stdio("a")),
            ("empty_cached", stdio("b")),
            ("never", stdio("c")),
            ("full", stdio("d")),
            ("off", disabled),
        ]);
        let long = "word ".repeat(120);
        let env = FakeEnv::default()
            .with_connection("empty_live", ConnectionStatus::Connected)
            .with_connection("full", ConnectionStatus::Connected);
        let (ctx, _) = ctx_with(
            config,
            &[
                ("empty_cached", Vec::new()),
                (
                    "full",
                    vec![
                        ToolMetadata::new("full_a", "a", "Does a thing"),
                        ToolMetadata::new("full_b", "b", ""),
                    ],
                ),
            ],
            &[("full", long.as_str()), ("empty_live", "Short note.")],
            env,
        );

        // 1 · unknown server.
        let unknown = execute_list(&ctx, "nope");
        assert_eq!(unknown.details.clone().unwrap()["error"], json!("not_found"));
        // 2 · disabled.
        assert_eq!(execute_list(&ctx, "off").details.clone().unwrap()["error"], json!("server_disabled"));
        // 4a · connected with zero tools, plus a short instructions preview and NO pointer.
        let live = execute_list(&ctx, "empty_live");
        assert_eq!(text_of(&live), "Server \"empty_live\" has no tools.\n\nServer instructions:\nShort note.");
        let details = live.details.clone().unwrap();
        assert_eq!(details["count"], json!(0));
        assert_eq!(details["hasInstructions"], json!(true));
        assert!(details.get("error").is_none());
        // 4b · metadata present but not connected.
        let cached = execute_list(&ctx, "empty_cached");
        assert_eq!(text_of(&cached), "Server \"empty_cached\" has no cached tools (not connected).");
        assert_eq!(cached.details.clone().unwrap()["cached"], json!(true));
        // 4c · no metadata at all.
        let never = execute_list(&ctx, "never");
        assert_eq!(never.details.clone().unwrap()["error"], json!("not_connected"));
        // 5 · the listing, with the pointer BECAUSE the 300-char preview truncated.
        let full = execute_list(&ctx, "full");
        let text = text_of(&full);
        assert!(text.starts_with("full (2 tools):\n\n- full_a - Does a thing\n- full_b\n"), "{text}");
        assert!(text.contains("\nUse mcp({ instructions: \"full\" }) for the full text."), "{text}");
        let details = full.details.clone().unwrap();
        assert_eq!(details["tools"], json!(["full_a", "full_b"]));
        assert_eq!(details["count"], json!(2));
    }

    #[test]
    fn list_marks_a_cached_listing_when_not_connected() {
        let config = config_with(&[("srv", stdio("a"))]);
        let (ctx, _) = ctx_with(
            config,
            &[("srv", vec![ToolMetadata::new("srv_a", "a", "")])],
            &[],
            FakeEnv::default(),
        );
        assert!(text_of(&execute_list(&ctx, "srv")).starts_with("srv (1 tools (not connected, cached)):"));
    }

    // ---- MCP-156 · `executeInstructions` --------------------------------------------------------------

    #[test]
    fn cached_instructions_win_even_for_a_disconnected_server() {
        let disabled = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_with(&[
            ("cached", stdio("a")),
            ("live", stdio("b")),
            ("cold", stdio("c")),
            ("off", disabled),
        ]);
        let env = FakeEnv::default().with_connection("live", ConnectionStatus::Connected);
        let (ctx, _) = ctx_with(config, &[], &[("cached", "Use the API key.")], env);

        assert_eq!(
            execute_instructions(&ctx, "missing").details.clone().unwrap()["error"],
            json!("not_found")
        );
        assert_eq!(
            execute_instructions(&ctx, "off").details.clone().unwrap()["error"],
            json!("server_disabled")
        );
        // Cached, and NOT connected — the cache is consulted before the connection.
        let cached = execute_instructions(&ctx, "cached");
        assert_eq!(text_of(&cached), "cached instructions:\n\nUse the API key.");
        assert_eq!(cached.details.clone().unwrap()["length"], json!(16));
        assert_eq!(
            execute_instructions(&ctx, "live").details.clone().unwrap()["error"],
            json!("no_instructions")
        );
        assert_eq!(
            execute_instructions(&ctx, "cold").details.clone().unwrap()["error"],
            json!("not_connected")
        );
    }

    // ---- MCP-157 · `executeDescribe` -------------------------------------------------------------------

    #[test]
    fn describe_renders_a_resource_tool_and_the_approval_marker() {
        let config = config_with(&[("files", stdio("a"))]);
        let mut resource = ToolMetadata::new("files_read_notes", "read_notes", "Read the notes");
        resource.resource_uri = Some("file:///notes.md".to_string());
        let env = FakeEnv::default().with_approval_required("files_read_notes");
        let (ctx, _) = ctx_with(config, &[("files", vec![resource])], &[], env);

        let result = execute_describe(&ctx, "files_read_notes");
        assert_eq!(
            text_of(&result),
            "files_read_notes (requires approval)\nServer: files\nType: Resource (reads from file:///notes.md)\n\nRead the notes\n\nNo parameters required (resource tool)."
        );
        let details = result.details.clone().unwrap();
        assert_eq!(details["mode"], json!("describe"));
        assert_eq!(details["server"], json!("files"));
        assert_eq!(details["tool"]["originalName"], json!("read_notes"));
    }

    #[test]
    fn describe_reports_a_disabled_only_match_as_server_disabled() {
        let disabled = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_with(&[("off", disabled)]);
        let (ctx, _) = ctx_with(
            config,
            &[("off", vec![ToolMetadata::new("off_thing", "thing", "")])],
            &[],
            FakeEnv::default(),
        );
        let result = execute_describe(&ctx, "off_thing");
        assert_eq!(result.details.clone().unwrap()["error"], json!("server_disabled"));
    }

    #[test]
    fn describe_fails_closed_and_suggests_on_a_miss() {
        let config = config_with(&[("a", stdio("a")), ("b", stdio("b"))]);
        let (ctx, _) = ctx_with(
            config,
            &[
                ("a", vec![ToolMetadata::new("shared", "shared", "")]),
                ("b", vec![ToolMetadata::new("shared", "shared", "")]),
            ],
            &[],
            FakeEnv::default(),
        );
        assert_eq!(
            execute_describe(&ctx, "shared").details.clone().unwrap()["error"],
            json!("ambiguous_tool")
        );
        let miss = execute_describe(&ctx, "totally_absent");
        let details = miss.details.clone().unwrap();
        assert_eq!(details["error"], json!("tool_not_found"));
        assert_eq!(details["requestedTool"], json!("totally_absent"));
        assert!(text_of(&miss).starts_with("Tool \"totally_absent\" not found. Use mcp({ search: \"...\" }) to search."));
    }

    #[test]
    fn describe_forks_between_shape_and_parameters() {
        let config = config_with(&[("srv", stdio("a"))]);
        let mut with_schema = ToolMetadata::new("srv_run", "run", "Run it");
        with_schema.input_schema = Some(json!({"type": "object"}));
        let plain = ToolMetadata::new("srv_ping", "ping", "");
        let (ctx, _) = ctx_with(config, &[("srv", vec![with_schema, plain])], &[], FakeEnv::default());

        // `renderTsShape` returned a shape, so the `Shape:` arm is taken.
        assert!(text_of(&execute_describe(&ctx, "srv_run")).ends_with("\nShape:\n{ a: string }"));
        // No schema, no resource ⇒ the third arm, and an empty description renders the placeholder.
        assert_eq!(
            text_of(&execute_describe(&ctx, "srv_ping")),
            "srv_ping\nServer: srv\n\n(no description)\n\nNo parameters defined."
        );
    }

    // ---- MCP-158 / MCP-160 · `executeSearch` -----------------------------------------------------------

    #[test]
    fn regex_search_preserves_insertion_order_and_never_sorts() {
        // `zeta` is configured first, so its tools come first even though `alpha` sorts before it.
        let config = config_with(&[("zeta", stdio("a")), ("alpha", stdio("b"))]);
        let (ctx, _) = ctx_with(
            config,
            &[
                ("zeta", vec![ToolMetadata::new("z_two", "two", ""), ToolMetadata::new("z_one", "one", "")]),
                ("alpha", vec![ToolMetadata::new("a_one", "one", "")]),
            ],
            &[],
            FakeEnv::default(),
        );
        let result = execute_search(&ctx, "_", Some(true), None, Some(false), None, None);
        let details = result.details.clone().unwrap();
        let names: Vec<&str> =
            details["matches"].as_array().unwrap().iter().map(|m| m["tool"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["z_two", "z_one", "a_one"]);
        // Every regex match scores 0.
        assert!(details["matches"].as_array().unwrap().iter().all(|m| m["score"] == json!(0)));
    }

    #[test]
    fn regex_search_rejects_over_long_and_malformed_queries() {
        let config = config_with(&[("srv", stdio("a"))]);
        let (ctx, _) = ctx_with(config, &[("srv", Vec::new())], &[], FakeEnv::default());

        let long = "a".repeat(MAX_REGEX_SEARCH_QUERY_LENGTH + 1);
        let rejected = execute_search(&ctx, &long, Some(true), None, None, None, None);
        let details = rejected.details.clone().unwrap();
        assert_eq!(details["error"], json!("query_too_long"));
        assert_eq!(details["maxLength"], json!(256));
        assert_eq!(
            text_of(&rejected),
            "Regex query is too long; maximum length is 256 characters."
        );

        let malformed = execute_search(&ctx, "(a", Some(true), None, None, None, None);
        assert_eq!(malformed.details.clone().unwrap()["error"], json!("invalid_pattern"));
        // A non-regex search is unaffected by the cap.
        let plain = execute_search(&ctx, &long, None, None, None, None, None);
        assert_ne!(plain.details.clone().unwrap()["error"], json!("query_too_long"));
    }

    #[test]
    fn blank_search_needs_a_server_and_then_sorts_by_collation() {
        let config = config_with(&[("srv", stdio("a"))]);
        let (ctx, _) = ctx_with(
            config,
            &[(
                "srv",
                vec![ToolMetadata::new("Zeta", "Zeta", ""), ToolMetadata::new("alpha", "alpha", "")],
            )],
            &[],
            FakeEnv::default(),
        );
        // `search: ""` REACHES the mode (dispatch tests `!== undefined`).
        let empty = execute_search(&ctx, "", None, None, None, None, None);
        assert_eq!(empty.details.clone().unwrap()["error"], json!("empty_query"));
        assert_eq!(text_of(&empty), "Search query cannot be empty");

        let scoped = execute_search(&ctx, "  ", None, Some("srv"), Some(false), None, None);
        let details = scoped.details.clone().unwrap();
        let names: Vec<&str> =
            details["matches"].as_array().unwrap().iter().map(|m| m["tool"].as_str().unwrap()).collect();
        // ICU root collation, not byte order: `alpha` before `Zeta`.
        assert_eq!(names, vec!["alpha", "Zeta"]);
    }

    #[test]
    fn zero_results_report_connecting_servers_singular_and_plural() {
        let config = config_with(&[("one", stdio("a")), ("two", stdio("b"))]);
        let (ctx, env) =
            ctx_with(config.clone(), &[], &[], FakeEnv::default().with_connecting("one"));
        let single = execute_search(&ctx, "nothing", None, None, None, None, None);
        assert_eq!(
            text_of(&single),
            "No tools matching \"nothing\" Server \"one\" is still connecting; retry in a moment."
        );
        assert_eq!(single.details.clone().unwrap()["connectingServers"], json!(["one"]));
        drop(env);

        let (ctx, _) = ctx_with(
            config,
            &[],
            &[],
            FakeEnv::default().with_connecting("one").with_connecting("two"),
        );
        let many = execute_search(&ctx, "nothing", None, None, None, None, None);
        assert_eq!(
            text_of(&many),
            "No tools matching \"nothing\" Servers \"one\", \"two\" are still connecting; retry in a moment."
        );
        // A filtered search names only the filtered server, and the key is absent when empty.
        let filtered = execute_search(&ctx, "nothing", None, Some("one"), None, None, None);
        assert!(text_of(&filtered).starts_with("No tools matching \"nothing\" in \"one\""));
        let (ctx, _) = ctx_with(config_with(&[("one", stdio("a"))]), &[], &[], FakeEnv::default());
        let quiet = execute_search(&ctx, "nothing", None, None, None, None, None);
        assert!(quiet.details.clone().unwrap().get("connectingServers").is_none());
    }

    #[test]
    fn search_paginates_with_an_em_dash_footer() {
        let config = config_with(&[("srv", stdio("a"))]);
        let tools: Vec<ToolMetadata> = (0..5)
            .map(|index| ToolMetadata::new(format!("srv_report_{index}"), format!("report_{index}"), "Reporting"))
            .collect();
        let (ctx, _) = ctx_with(config, &[("srv", tools)], &[], FakeEnv::default());
        let page = execute_search(&ctx, "report", None, None, Some(false), Some(2.0), Some(0.0));
        let text = text_of(&page);
        assert!(text.starts_with("Found 5 tools matching \"report\":\n\n"), "{text}");
        assert!(text.ends_with("2 of 5 — offset: 2 for more"), "{text}");
        let details = page.details.clone().unwrap();
        assert_eq!(details["hasMore"], json!(true));
        assert_eq!(details["nextOffset"], json!(2));
        assert_eq!(details["count"], json!(5));

        // Singular header, and no footer, on the last page.
        let last = execute_search(&ctx, "report_4", None, None, Some(false), Some(12.0), Some(0.0));
        assert!(text_of(&last).starts_with("Found 1 tool matching"), "{}", text_of(&last));
        assert_eq!(last.details.clone().unwrap()["nextOffset"], Value::Null);
    }

    #[test]
    fn search_with_schemas_indents_the_shape_block_by_four() {
        let config = config_with(&[("srv", stdio("a"))]);
        let mut tool = ToolMetadata::new("srv_run", "run", "Run it");
        tool.input_schema = Some(json!({"type": "object"}));
        let (ctx, _) = ctx_with(config, &[("srv", vec![tool])], &[], FakeEnv::default());
        let text = text_of(&execute_search(&ctx, "run", None, None, None, None, None));
        assert!(text.contains("srv_run\n  Run it\n\n  Shape:\n    { a: string }"), "{text}");
    }

    #[test]
    fn a_disabled_server_filter_short_circuits_search() {
        let disabled = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_with(&[("off", disabled)]);
        let (ctx, _) = ctx_with(config, &[], &[], FakeEnv::default());
        assert_eq!(
            execute_search(&ctx, "anything", None, Some("off"), None, None, None)
                .details
                .unwrap()["error"],
            json!("server_disabled")
        );
    }

    // ---- MCP-163 · `executeCall`'s resolution state machine ---------------------------------------------

    #[tokio::test]
    async fn call_fails_closed_for_duplicate_unqualified_names() {
        let config = config_with(&[("zeta", stdio("a")), ("alpha", stdio("b"))]);
        let (ctx, _) = ctx_with(
            config,
            &[
                ("zeta", vec![ToolMetadata::new("create_issue", "create_issue", "")]),
                ("alpha", vec![ToolMetadata::new("create_issue", "create_issue", "")]),
            ],
            &[],
            FakeEnv::default(),
        );
        let result = execute_call(&ctx, "create_issue", None, None, &CancelToken::new(), None)
            .await
            .unwrap();
        let details = result.details.clone().unwrap();
        assert_eq!(details["error"], json!("ambiguous_tool"));
        assert_eq!(details["mode"], json!("call"));
    }

    #[tokio::test]
    async fn an_exact_disabled_match_suppresses_the_fuzzy_scan_entirely() {
        // `off` matches EXACTLY and is disabled; `live` would match fuzzily. Upstream guards the
        // fuzzy pass with `!toolMeta && !disabledMatch`, so `live` is never reached.
        let disabled = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_with(&[("off", disabled), ("live", stdio("b"))]);
        let (ctx, _) = ctx_with(
            config,
            &[
                ("off", vec![ToolMetadata::new("do-it", "do-it", "")]),
                ("live", vec![ToolMetadata::new("do_it", "do_it", "")]),
            ],
            &[],
            FakeEnv::default(),
        );
        let result =
            execute_call(&ctx, "do-it", None, None, &CancelToken::new(), None).await.unwrap();
        let details = result.details.clone().unwrap();
        assert_eq!(details["error"], json!("server_disabled"));
        assert_eq!(details["server"], json!("off"));
        // The identity names the resolved tool, not just the requested one.
        assert_eq!(details["tool"], json!("do-it"));
    }

    #[tokio::test]
    async fn an_unknown_server_hint_is_server_not_found() {
        let config = config_with(&[("srv", stdio("a"))]);
        let (ctx, _) = ctx_with(config, &[], &[], FakeEnv::default());
        let details = execute_call(&ctx, "t", None, Some("nope"), &CancelToken::new(), None)
            .await
            .unwrap()
            .details
            .unwrap();
        assert_eq!(details["error"], json!("server_not_found"));
        assert_eq!(details["requestedTool"], json!("t"));
    }

    // ---- MCP-199 · native-tool detection ------------------------------------------------------------------

    #[tokio::test]
    async fn a_same_named_host_tool_is_reported_as_native_and_none_falls_through() {
        let config = config_with(&[("srv", stdio("a"))]);
        let (ctx, _) =
            ctx_with(config.clone(), &[], &[], FakeEnv::default().with_all_tools(&["read", "mcp"]));
        let result = execute_call(&ctx, "read", None, None, &CancelToken::new(), None).await.unwrap();
        assert_eq!(result.details.clone().unwrap()["error"], json!("native_tool"));
        assert_eq!(
            text_of(&result),
            "\"read\" is a native cyrup tool. Call read directly instead of using mcp({ tool: \"read\" })."
        );

        // `all_tool_names() == None` is upstream's `getPiTools === undefined` branch.
        let (ctx, _) = ctx_with(config, &[], &[], FakeEnv::default());
        let details = execute_call(&ctx, "read", None, None, &CancelToken::new(), None)
            .await
            .unwrap()
            .details
            .unwrap();
        assert_eq!(details["error"], json!("tool_not_found"));
        assert_eq!(details["hintServer"], Value::Null);
    }

    // ---- MCP-162 · the single-shot auto-auth latch ----------------------------------------------------------

    /// A `client_credentials` server: the one grant type [`attempt_auto_auth`] step 4 lets through
    /// in a headless session.
    fn machine_oauth(url: &str) -> ServerEntry {
        ServerEntry {
            url: Some(url.to_string()),
            oauth: Some(OAuthSetting::Config(crate::config::OAuthConfig {
                grant_type: Some(OAuthGrantType::ClientCredentials),
                ..crate::config::OAuthConfig::default()
            })),
            ..ServerEntry::default()
        }
    }

    fn auto_auth_on(mut config: McpConfig) -> McpConfig {
        config.settings = Some(McpSettings { auto_auth: Some(true), ..McpSettings::default() });
        config
    }

    /// Upstream: "fails fast for non-ui browser auth when autoAuth is enabled".
    ///
    /// Step 4 of the ladder refuses **before** `authenticate` is ever called when there is no
    /// interactive surface and the grant type needs a browser — so the counter stays at zero and the
    /// model is told how to start the flow manually.
    #[tokio::test]
    async fn headless_browser_auth_fails_fast_without_calling_authenticate() {
        let config = auto_auth_on(config_with(&[("linear", http("https://linear.example/mcp"))]));
        let env = FakeEnv::default()
            .with_connection("linear", ConnectionStatus::NeedsAuth)
            .with_oauth("linear.example");
        let (ctx, env) = ctx_with(config, &[], &[], env);
        let result = execute_call(&ctx, "issues", None, Some("linear"), &CancelToken::new(), None)
            .await
            .unwrap();
        assert_eq!(result.details.clone().unwrap()["error"], json!("auth_required"));
        assert_eq!(env.authenticate_calls.load(Ordering::SeqCst), 0, "no browser, no attempt");
        assert_eq!(text_of(&result), default_auth_required_message("linear"));
    }

    /// Upstream: "uses custom authRequiredMessage for non-ui autoAuth failures" — the configured
    /// template still wins over the step-4 default, because that default routes through
    /// [`get_auth_required_message`] rather than being returned directly.
    #[tokio::test]
    async fn a_configured_auth_required_message_wins_over_the_headless_default() {
        let mut config = auto_auth_on(config_with(&[("linear", http("https://linear.example/mcp"))]));
        config.settings = Some(McpSettings {
            auto_auth: Some(true),
            auth_required_message: Some("Ask an admin to authorise ${server}.".to_string()),
            ..McpSettings::default()
        });
        let env = FakeEnv::default()
            .with_connection("linear", ConnectionStatus::NeedsAuth)
            .with_oauth("linear.example");
        let (ctx, _) = ctx_with(config, &[], &[], env);
        let result = execute_call(&ctx, "issues", None, Some("linear"), &CancelToken::new(), None)
            .await
            .unwrap();
        assert_eq!(text_of(&result), "Ask an admin to authorise linear.");
    }

    /// A failing `client_credentials` auto-auth reaches `authenticate` exactly once and reports
    /// through [`get_auth_failed_message`].
    #[tokio::test]
    async fn a_failed_auto_auth_reports_the_failure_message() {
        let config = auto_auth_on(config_with(&[("linear", machine_oauth("https://linear.example/mcp"))]));
        let env = FakeEnv::default()
            .with_connection("linear", ConnectionStatus::NeedsAuth)
            .with_oauth("linear.example")
            .with_authenticate_failure("token exchange refused");
        let (ctx, env) = ctx_with(config, &[], &[], env);
        let result = execute_call(&ctx, "issues", None, Some("linear"), &CancelToken::new(), None)
            .await
            .unwrap();
        let details = result.details.clone().unwrap();
        assert_eq!(details["error"], json!("auth_required"));
        assert_eq!(details["server"], json!("linear"));
        assert_eq!(details["requestedTool"], json!("issues"));
        assert_eq!(env.authenticate_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            text_of(&result),
            "OAuth authentication failed for \"linear\": token exchange refused. Run mcp({ action: \"auth-start\", server: \"linear\" }) to get a browser URL, or /mcp-auth linear in an interactive local session."
        );
    }

    /// **The latch, observed across two call sites.**
    ///
    /// `foo_bar_thing` matches the prefixes of both `foo_bar` and `foo`, so phase 4 visits two
    /// `needs-auth` candidates in descending-prefix-length order. The first claims the latch and
    /// authenticates; the second finds the latch already set and does **not** authenticate again.
    /// Without the latch this single tool call would open two browser flows.
    #[tokio::test]
    async fn the_latch_stops_a_second_auto_auth_in_the_same_call() {
        let config = auto_auth_on(config_with(&[
            ("foo", machine_oauth("https://foo.example/mcp")),
            ("foo_bar", machine_oauth("https://foo.example/bar")),
        ]));
        let env = FakeEnv::default()
            .with_connection("foo", ConnectionStatus::NeedsAuth)
            .with_connection("foo_bar", ConnectionStatus::NeedsAuth)
            .with_oauth("foo.example");
        let (ctx, env) = ctx_with(config, &[], &[], env);
        let result =
            execute_call(&ctx, "foo_bar_thing", None, None, &CancelToken::new(), None).await.unwrap();
        // Neither candidate ever connects, so the call ends unresolved…
        assert_eq!(result.details.clone().unwrap()["error"], json!("tool_not_found"));
        // …and `authenticate` ran ONCE across both of them.
        assert_eq!(env.authenticate_calls.load(Ordering::SeqCst), 1, "the latch is single-shot");
    }

    /// `autoAuth` is opt-in, not opt-out: unset means the ladder is never entered.
    #[tokio::test]
    async fn auto_auth_is_opt_in() {
        let config = config_with(&[("linear", machine_oauth("https://linear.example/mcp"))]);
        let env = FakeEnv::default()
            .with_connection("linear", ConnectionStatus::NeedsAuth)
            .with_oauth("linear.example");
        let (ctx, env) = ctx_with(config, &[], &[], env);
        let result = execute_call(&ctx, "issues", None, Some("linear"), &CancelToken::new(), None)
            .await
            .unwrap();
        assert_eq!(result.details.clone().unwrap()["error"], json!("auth_required"));
        assert_eq!(env.authenticate_calls.load(Ordering::SeqCst), 0);
        assert_eq!(text_of(&result), default_auth_required_message("linear"));
    }

    // ---- backoff and approval -------------------------------------------------------------------------------

    #[tokio::test]
    async fn a_recorded_failure_short_circuits_a_hinted_call() {
        let config = config_with(&[("srv", stdio("a"))]);
        let env = FakeEnv::default().with_failure("srv", 7);
        let (ctx, _) = ctx_with(config, &[], &[], env);
        let result =
            execute_call(&ctx, "thing", None, Some("srv"), &CancelToken::new(), None).await.unwrap();
        assert_eq!(result.details.clone().unwrap()["error"], json!("server_backoff"));
        assert_eq!(text_of(&result), "Server \"srv\" not available (last failed 7s ago)");
    }

    #[tokio::test]
    async fn approval_failures_report_tool_not_resource_uri() {
        let config = config_with(&[("files", stdio("a"))]);
        let mut resource = ToolMetadata::new("files_read_notes", "read_notes", "");
        resource.resource_uri = Some("file:///notes.md".to_string());
        let env = FakeEnv::default().with_connection("files", ConnectionStatus::Connected);
        *env.approval.lock().unwrap() = Some(ApprovalOutcome::Denied);
        let (ctx, _) = ctx_with(config, &[("files", vec![resource])], &[], env);

        let result =
            execute_call(&ctx, "files_read_notes", None, None, &CancelToken::new(), None).await.unwrap();
        let details = result.details.clone().unwrap();
        assert_eq!(details["error"], json!("approval_denied"));
        // NOT `callIdentity`: a resource tool reports `tool` here rather than `resourceUri`.
        assert_eq!(details["tool"], json!("read_notes"));
        assert!(details.get("resourceUri").is_none());
        assert_eq!(
            text_of(&result),
            "The user declined approval to run MCP tool \"read_notes\" on server \"files\"."
        );
    }

    #[tokio::test]
    async fn an_approval_gated_headless_session_reports_approval_required() {
        let config = config_with(&[("srv", stdio("a"))]);
        let env = FakeEnv::default().with_connection("srv", ConnectionStatus::Connected);
        *env.approval.lock().unwrap() = Some(ApprovalOutcome::NoInteractiveSession);
        let (ctx, _) =
            ctx_with(config, &[("srv", vec![ToolMetadata::new("srv_run", "run", "")])], &[], env);
        let result = execute_call(&ctx, "srv_run", None, None, &CancelToken::new(), None).await.unwrap();
        assert_eq!(result.details.clone().unwrap()["error"], json!("approval_required"));
        assert_eq!(
            text_of(&result),
            "MCP tool \"run\" on server \"srv\" is approval-gated and requires an interactive session."
        );
    }

    // ---- MCP-164 · result shaping --------------------------------------------------------------------------

    #[tokio::test]
    async fn an_empty_success_falls_back_to_the_placeholder_and_keeps_the_identity() {
        let config = config_with(&[("srv", stdio("a"))]);
        let env = FakeEnv::default().with_connection("srv", ConnectionStatus::Connected);
        let (ctx, _) =
            ctx_with(config, &[("srv", vec![ToolMetadata::new("srv_run", "run", "")])], &[], env);
        let result = execute_call(&ctx, "srv_run", None, None, &CancelToken::new(), None).await.unwrap();
        assert_eq!(text_of(&result), "(empty result)");
        let details = result.details.clone().unwrap();
        assert_eq!(details["mode"], json!("call"));
        assert_eq!(details["server"], json!("srv"));
        assert_eq!(details["tool"], json!("run"));
        assert!(details.get("error").is_none(), "a success carries no error code");
    }

    // ---- MCP-161 · `executeConnect` reports as a LIST --------------------------------------------------------

    #[tokio::test]
    async fn a_successful_connect_reports_mode_list() {
        let config = config_with(&[("srv", stdio("a"))]);
        let env = FakeEnv::default().with_connection("srv", ConnectionStatus::Connected);
        let (ctx, _) = ctx_with(config, &[], &[], env);
        let result = execute_connect(&ctx, "srv", &CancelToken::new()).await.unwrap();
        // `details.mode === "list"` — a successful connect renders as a listing.
        assert_eq!(result.details.clone().unwrap()["mode"], json!("list"));

        assert_eq!(
            execute_connect(&ctx, "missing", &CancelToken::new()).await.unwrap().details.clone().unwrap()["error"],
            json!("not_found")
        );
    }

    #[tokio::test]
    async fn connect_deletes_instructions_a_server_stopped_sending() {
        let config = config_with(&[("srv", stdio("a"))]);
        let env = FakeEnv::default().with_connection("srv", ConnectionStatus::Connected);
        let (ctx, _) = ctx_with(config, &[], &[("srv", "stale text")], env);
        assert_eq!(ctx.server_instructions("srv").as_deref(), Some("stale text"));
        // `ConnectOutcome::instructions` is `None`, which is `state.serverInstructions.delete(...)`.
        execute_connect(&ctx, "srv", &CancelToken::new()).await.unwrap();
        assert_eq!(ctx.server_instructions("srv"), None);
    }

    // ---- MCP-168 · `executeAuthComplete` ---------------------------------------------------------------------

    #[tokio::test]
    async fn auth_complete_closes_the_connection_and_clears_the_failure() {
        let config = config_with(&[("linear", http("https://linear.example/mcp"))]);
        let env = FakeEnv::default()
            .with_connection("linear", ConnectionStatus::NeedsAuth)
            .with_failure("linear", 5);
        let (ctx, env) = ctx_with(config, &[], &[], env);
        let result = execute_auth_complete(&ctx, "linear", "http://localhost:8976/cb?code=x", &CancelToken::new())
            .await
            .unwrap();
        assert_eq!(result.details.clone().unwrap()["authenticated"], json!(true));
        assert_eq!(
            text_of(&result),
            "OAuth authentication successful for \"linear\". Run mcp({ connect: \"linear\" }) to connect with the new token."
        );
        assert_eq!(env.get_connection("linear"), None, "the connection is closed");
        assert_eq!(env.failure_age_seconds("linear"), None, "the failure record is cleared");
    }

    // ---- MCP-167 · `executeAuthStart` ------------------------------------------------------------------------

    #[tokio::test]
    async fn auth_start_rejects_non_oauth_servers_and_renders_instructions_otherwise() {
        let config = config_with(&[("stdio_srv", stdio("a")), ("linear", http("https://linear.example/mcp"))]);
        let env = FakeEnv::default().with_oauth("linear.example");
        let (ctx, _) = ctx_with(config, &[], &[], env);

        let rejected = execute_auth_start(&ctx, "stdio_srv", &CancelToken::new()).await.unwrap();
        assert_eq!(rejected.details.clone().unwrap()["error"], json!("oauth_not_supported"));
        assert_eq!(
            text_of(&rejected),
            "Server \"stdio_srv\" is not configured for OAuth over HTTP."
        );

        let started = execute_auth_start(&ctx, "linear", &CancelToken::new()).await.unwrap();
        let details = started.details.clone().unwrap();
        assert_eq!(details["authorizationUrl"], json!("https://auth.example.com/authorize"));
        assert!(text_of(&started).starts_with("MCP OAuth required for \"linear\"."));
    }

    // ---- Integration regression: the two drifted copies this file used to carry -----------------

    /// Both functions now come from [`crate::registration`]; these pin the two behaviours the
    /// local copies had wrong, so a future re-fork is caught by a red test rather than by a
    /// silently unmatchable tool name.
    #[test]
    fn the_de_duplicated_naming_helpers_are_the_upstream_ones() {
        // `resource-tools.ts:13` — `"resource" + (result ? "_" + result : "")`. An all-punctuation
        // name yields `"resource"`, NOT `"resource_"`.
        assert_eq!(resource_name_to_tool_name("///"), "resource");
        assert_eq!(resource_name_to_tool_name(""), "resource");
        // A digit-leading name still gets the separator.
        assert_eq!(resource_name_to_tool_name("1-notes"), "resource_1_notes");
        assert_eq!(resource_name_to_tool_name("My Notes!!"), "my_notes");

        // `utils.ts:265-267` — `.length` / `.slice` are UTF-16 code units. An astral-plane
        // character is two units, so a four-unit budget takes exactly two emoji.
        assert_eq!(truncate_at_word("\u{1f600}\u{1f600}\u{1f600}", 4), "\u{1f600}\u{1f600}...");
        // The `lastSpace > target * 0.6` word cut, and the below-threshold hard cut.
        assert_eq!(truncate_at_word("hello world again", 12), "hello world...");
        assert_eq!(truncate_at_word("a bbbbbbbbbbbb", 10), "a bbbbbbbb...");
        // Short enough is returned untouched, with no ellipsis.
        assert_eq!(truncate_at_word("short", 10), "short");
    }

    // ==============================================================================================
    // MCP-231 / MCP-232 — `tool-approval.ts`, transcribed from `__tests__/tool-approval.test.ts`
    // ==============================================================================================
    //
    // The broker cases (`lets a broker allow/deny…`, `requires brokers to claim synchronously`,
    // `fails closed when a claimed broker …`, `propagates aborts while a claimed broker is
    // pending`) do not port: MCP-233 cuts the broker, and `ExtHooks::before_tool_call` is its
    // replacement. Their fail-closed *content* survives here as
    // `an_unrecognised_answer_denies` and `a_cancelled_call_denies_without_asking`.

    /// A scripted [`cyrup_ext::HostServices`] standing in for `{ ui: { select } }`.
    ///
    /// Records what it was asked, so a test can assert the dialog's exact text and — the assertion
    /// upstream makes most often — how many times it was asked *at all*.
    #[derive(Default)]
    struct ScriptedUi {
        answer: Mutex<Option<String>>,
        prompts: Mutex<Vec<String>>,
        options: Mutex<Vec<Vec<String>>>,
        /// The P-3 gate to observe from inside the dialog (MCP-471), when a test wires one.
        gate: Option<Arc<cyrup_ext::HumanWaitGate>>,
        waiting_during_dialog: Mutex<Vec<bool>>,
    }

    impl ScriptedUi {
        fn answering(answer: Option<&str>) -> Arc<Self> {
            Arc::new(Self {
                answer: Mutex::new(answer.map(str::to_string)),
                ..Self::default()
            })
        }

        fn watching(answer: Option<&str>, gate: Arc<cyrup_ext::HumanWaitGate>) -> Arc<Self> {
            Arc::new(Self {
                answer: Mutex::new(answer.map(str::to_string)),
                gate: Some(gate),
                ..Self::default()
            })
        }

        fn prompt_count(&self) -> usize {
            self.prompts.lock().unwrap().len()
        }

        fn last_prompt(&self) -> String {
            self.prompts.lock().unwrap().last().cloned().unwrap_or_default()
        }
    }

    impl cyrup_ext::HostServices for ScriptedUi {
        fn select(
            &self,
            prompt: &str,
            options: &Value,
            _opts: &cyrup_ext::DialogOptions,
        ) -> Option<String> {
            if let Some(gate) = self.gate.as_ref() {
                self.waiting_during_dialog.lock().unwrap().push(gate.is_waiting());
            }
            self.prompts.lock().unwrap().push(prompt.to_string());
            self.options.lock().unwrap().push(
                options
                    .as_array()
                    .map(|list| {
                        list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
                    })
                    .unwrap_or_default(),
            );
            self.answer.lock().unwrap().clone()
        }
    }

    /// An [`McpState`] over a real owner, with or without an interactive surface —
    /// `createState({interactive})`.
    fn approval_state(config: McpConfig, ui: Option<Arc<ScriptedUi>>) -> Arc<McpState> {
        let owner = Arc::new(McpRuntimeOwner::new());
        let manager = Arc::new(McpServerManager::default());
        let lifecycle =
            Arc::new(McpLifecycleManager::new(Arc::clone(&manager), Arc::new(|_: &str| false)));
        Arc::new(McpState::new(McpStateParts {
            owner: Arc::clone(&owner),
            manager,
            lifecycle,
            config,
            programmatic_config: None,
            oauth_runtime: crate::oauth::create_oauth_runtime(None),
            auth_storage_options: crate::state::AuthStorageOptions::default(),
            ui: ui.map(|services| {
                Arc::new(crate::owner::OwnedServices::new(
                    services as Arc<dyn cyrup_ext::HostServices>,
                    Arc::clone(&owner),
                ))
            }),
            open_browser: Arc::new(|_| Box::pin(async { Ok(()) })),
            send_message: Arc::new(|_| {}),
        }))
    }

    /// `const tool = { name: "demo_search-records", originalName: "search-records", … }`.
    fn demo_tool() -> ToolMetadata {
        ToolMetadata::new("demo_search-records", "search-records", "Search records")
    }

    /// `{ mcpServers: { demo: { command: "demo", approveTools } } }`.
    fn demo_config(approve: Option<BoolOrList>) -> McpConfig {
        let mut config = config_with(&[("demo", stdio("demo"))]);
        if let Some(entry) = config.mcp_servers.get_mut("demo") {
            entry.approve_tools = approve;
        }
        config
    }

    fn settings_approving(patterns: &[&str]) -> Option<McpSettings> {
        Some(McpSettings {
            approve_tools: Some(BoolOrList::Named(
                patterns.iter().map(|p| (*p).to_string()).collect(),
            )),
            ..McpSettings::default()
        })
    }

    // ---- MCP-231 · `isToolCallApprovalRequired` --------------------------------------------------

    /// `"matches original, prefixed, and read_* resource tool names"` — the three cases, each with
    /// **no** `toolMetadata`, which is also the only path that reaches the `None` asymmetry.
    #[test]
    fn approval_matches_original_prefixed_and_resource_tool_names() {
        let by_original = demo_config(Some(BoolOrList::Named(vec!["search-records".to_string()])));
        assert!(is_tool_call_approval_required(&by_original, "demo", &demo_tool(), None));

        let by_prefixed =
            demo_config(Some(BoolOrList::Named(vec!["demo_search-records".to_string()])));
        assert!(is_tool_call_approval_required(&by_prefixed, "demo", &demo_tool(), None));

        // A global glob against a `short`-mode alias: `docs-mcp` prefixes as `docs`, so the
        // resource tool's current candidate set carries `docs_read_handbook`.
        let mut resource_config = config_with(&[("docs-mcp", ServerEntry::default())]);
        resource_config.settings = settings_approving(&["docs_read_*"]);
        let mut resource_tool =
            ToolMetadata::new("docs_read_handbook", "read_handbook", "Read handbook");
        resource_tool.resource_uri = Some("docs://handbook".to_string());
        assert!(is_tool_call_approval_required(&resource_config, "docs-mcp", &resource_tool, None));
    }

    /// `"gates exact global selectors without applying them through a legacy collision"` — the
    /// selector names `my_2d_server`'s tool exactly, and must not reach `my-server`'s through the
    /// hyphen-escaped legacy alias the two servers share.
    #[test]
    fn a_global_selector_does_not_gate_the_wrong_server_through_a_legacy_alias() {
        let mut config =
            config_with(&[("my-server", stdio("hyphen")), ("my_2d_server", stdio("escaped"))]);
        config.settings = settings_approving(&["my_2d_server_do_thing"]);
        let hyphen = ToolMetadata::new("my-server_do-thing", "do-thing", "");
        let escaped = ToolMetadata::new("my_2d_server_do_thing", "do_thing", "");
        let metadata = metadata_with(&[
            ("my-server", vec![hyphen.clone()]),
            ("my_2d_server", vec![escaped.clone()]),
        ]);

        assert!(!is_tool_call_approval_required(&config, "my-server", &hyphen, Some(&metadata)));
        assert!(is_tool_call_approval_required(&config, "my_2d_server", &escaped, Some(&metadata)));
    }

    /// `"matches safe server-scoped normalized approval selectors"` and `"matches safe global
    /// normalized approval selectors"` — the legacy `-`→`_` alias DOES gate when nothing else
    /// currently answers to it, under either scope.
    #[test]
    fn a_normalized_legacy_selector_gates_when_nothing_else_answers_to_it() {
        let scoped = ToolMetadata::new("my-server_do_thing", "do_thing", "");
        let metadata = metadata_with(&[("my-server", vec![scoped.clone()])]);

        let mut server_scope = config_with(&[("my-server", stdio("demo"))]);
        if let Some(entry) = server_scope.mcp_servers.get_mut("my-server") {
            entry.approve_tools =
                Some(BoolOrList::Named(vec!["my_server_do_thing".to_string()]));
        }
        assert!(is_tool_call_approval_required(&server_scope, "my-server", &scoped, Some(&metadata)));

        let mut global_scope = config_with(&[("my-server", stdio("demo"))]);
        global_scope.settings = settings_approving(&["my_server_do_thing"]);
        assert!(is_tool_call_approval_required(&global_scope, "my-server", &scoped, Some(&metadata)));
    }

    /// `"does not gate a same-server legacy collision"` — `demo_search_records` is the *current*
    /// name of one tool and the *legacy* alias of another on the same server. The selector gates
    /// the tool that owns the name now, and only that one.
    #[test]
    fn a_same_server_legacy_collision_gates_only_the_current_owner() {
        let hyphen = ToolMetadata::new("demo_search-records", "search-records", "");
        let underscore = ToolMetadata::new("demo_search_records", "search_records", "");
        let mut config = config_with(&[("demo", stdio("demo"))]);
        config.settings = settings_approving(&["demo_search_records"]);
        let metadata = metadata_with(&[("demo", vec![hyphen.clone(), underscore.clone()])]);

        assert!(!is_tool_call_approval_required(&config, "demo", &hyphen, Some(&metadata)));
        assert!(is_tool_call_approval_required(&config, "demo", &underscore, Some(&metadata)));
    }

    /// The ladder itself: `true` always gates, a per-server value beats the global on **presence**
    /// (so a per-server `false` survives a global `true`), and neither `false` nor an empty list
    /// gates anything.
    #[test]
    fn the_approval_ladder_reads_presence_not_truthiness() {
        let tool = demo_tool();
        let always = demo_config(Some(BoolOrList::All(true)));
        assert!(is_tool_call_approval_required(&always, "demo", &tool, None));

        let mut server_off = demo_config(Some(BoolOrList::All(false)));
        server_off.settings = Some(McpSettings {
            approve_tools: Some(BoolOrList::All(true)),
            ..McpSettings::default()
        });
        assert!(
            !is_tool_call_approval_required(&server_off, "demo", &tool, None),
            "a per-server `false` overrides a global `true` — presence wins, not truthiness"
        );

        let empty = demo_config(Some(BoolOrList::Named(Vec::new())));
        assert!(!is_tool_call_approval_required(&empty, "demo", &tool, None));
        assert!(!is_tool_call_approval_required(&demo_config(None), "demo", &tool, None));
    }

    /// The `tool_metadata == None` asymmetry 13e names: with no collision context the **server**
    /// scope falls back to the full legacy set, the **global** scope refuses to guess.
    #[test]
    fn the_absent_metadata_asymmetry_between_the_two_scopes_is_preserved() {
        let scoped = ToolMetadata::new("my-server_do_thing", "do_thing", "");

        let mut server_scope = config_with(&[("my-server", stdio("demo"))]);
        if let Some(entry) = server_scope.mcp_servers.get_mut("my-server") {
            entry.approve_tools =
                Some(BoolOrList::Named(vec!["my_server_do_thing".to_string()]));
        }
        assert!(is_tool_call_approval_required(&server_scope, "my-server", &scoped, None));

        let mut global_scope = config_with(&[("my-server", stdio("demo"))]);
        global_scope.settings = settings_approving(&["my_server_do_thing"]);
        assert!(!is_tool_call_approval_required(&global_scope, "my-server", &scoped, None));
    }

    // ---- MCP-232 · `ensureToolCallApproved` -------------------------------------------------------

    /// **The unit's headline assertion** (13e MCP-232 "verify"): no UI and a cancelled dialog are
    /// two different answers, and the only thing that keeps them apart is checking for a UI
    /// *before* calling `select`. `HostServices::select` returns `None` for both.
    #[tokio::test]
    async fn no_ui_and_a_dismissed_dialog_are_not_the_same_answer() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let args = json!({ "query": "private" });

        // `createState({approveTools: true, interactive: false})`.
        let headless = approval_state(demo_config(Some(BoolOrList::All(true))), None);
        assert_eq!(
            ensure_tool_call_approved(
                &headless,
                "demo",
                &tool,
                &args,
                ApprovalOrigin::Proxy,
                &CancelToken::new(),
                &metadata,
            )
            .await,
            ApprovalOutcome::NoInteractiveSession
        );

        // The same call with a UI whose dialog is dismissed — upstream's `select` resolving
        // `undefined`.
        let ui = ScriptedUi::answering(None);
        let interactive =
            approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));
        assert_eq!(
            ensure_tool_call_approved(
                &interactive,
                "demo",
                &tool,
                &args,
                ApprovalOrigin::Proxy,
                &CancelToken::new(),
                &metadata,
            )
            .await,
            ApprovalOutcome::Denied
        );
        assert_eq!(ui.prompt_count(), 1, "the headless check must not suppress a real dialog");
    }

    /// `"caches only Allow for session decisions"` as rewritten by `5bcd6c5` — three calls, two
    /// prompts, two cache entries. The reordered payload is the same request; the changed `id` is
    /// a new one.
    #[tokio::test]
    async fn allow_for_session_caches_per_argument_payload() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let ui = ScriptedUi::answering(Some(APPROVE_FOR_SESSION_OPTION));
        let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));

        for args in [
            json!({ "record": { "id": "safe", "type": "demo" } }),
            json!({ "record": { "type": "demo", "id": "safe" } }),
            json!({ "record": { "id": "other", "type": "demo" } }),
        ] {
            assert_eq!(
                ensure_tool_call_approved(
                    &state,
                    "demo",
                    &tool,
                    &args,
                    ApprovalOrigin::Proxy,
                    &CancelToken::new(),
                    &metadata,
                )
                .await,
                ApprovalOutcome::Approved
            );
        }

        assert_eq!(ui.prompt_count(), 2, "the reordered payload reuses the first approval");
        assert_eq!(state.approved_tool_calls.lock().unwrap().len(), 2);
    }

    /// The other half of the same upstream case: `Allow once` approves and caches **nothing**, so
    /// an identical second call prompts again.
    #[tokio::test]
    async fn allow_once_approves_without_caching() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let ui = ScriptedUi::answering(Some(APPROVE_ONCE_OPTION));
        let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));

        for _ in 0..2 {
            assert_eq!(
                ensure_tool_call_approved(
                    &state,
                    "demo",
                    &tool,
                    &json!({}),
                    ApprovalOrigin::Proxy,
                    &CancelToken::new(),
                    &metadata,
                )
                .await,
                ApprovalOutcome::Approved
            );
        }
        assert_eq!(ui.prompt_count(), 2);
        assert!(state.approved_tool_calls.lock().unwrap().is_empty());
    }

    /// `"returns approval_denied without throwing"`, plus the fail-closed default: **any** answer
    /// that is not one of the two `Allow …` strings denies — the literal `Deny`, and a label the
    /// dialog never offered.
    #[tokio::test]
    async fn an_unrecognised_answer_denies() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        for answer in [DENY_OPTION, "Allow", "allow once", ""] {
            let ui = ScriptedUi::answering(Some(answer));
            let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(ui));
            assert_eq!(
                ensure_tool_call_approved(
                    &state,
                    "demo",
                    &tool,
                    &json!({}),
                    ApprovalOrigin::Proxy,
                    &CancelToken::new(),
                    &metadata,
                )
                .await,
                ApprovalOutcome::Denied,
                "answer {answer:?} must not approve"
            );
        }
    }

    /// A tool no rule gates is approved with **no dialog at all**, even headless — the cheap path
    /// every non-gated MCP call takes.
    #[tokio::test]
    async fn an_ungated_tool_is_approved_without_asking() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let ui = ScriptedUi::answering(Some(DENY_OPTION));
        let state = approval_state(demo_config(None), Some(Arc::clone(&ui)));
        assert_eq!(
            ensure_tool_call_approved(
                &state,
                "demo",
                &tool,
                &json!({}),
                ApprovalOrigin::Proxy,
                &CancelToken::new(),
                &metadata,
            )
            .await,
            ApprovalOutcome::Approved
        );
        assert_eq!(ui.prompt_count(), 0);

        let headless = approval_state(demo_config(None), None);
        assert_eq!(
            ensure_tool_call_approved(
                &headless,
                "demo",
                &tool,
                &json!({}),
                ApprovalOrigin::Proxy,
                &CancelToken::new(),
                &metadata,
            )
            .await,
            ApprovalOutcome::Approved
        );
    }

    /// The cancellation delta this port records: a token that is already cancelled denies
    /// **without opening a dialog**, which is `abortable`'s pre-await `throwIfAborted`.
    #[tokio::test]
    async fn a_cancelled_call_denies_without_asking() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let ui = ScriptedUi::answering(Some(APPROVE_ONCE_OPTION));
        let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));
        let cancel = CancelToken::new();
        cancel.cancel();

        assert_eq!(
            ensure_tool_call_approved(
                &state,
                "demo",
                &tool,
                &json!({}),
                ApprovalOrigin::Proxy,
                &cancel,
                &metadata,
            )
            .await,
            ApprovalOutcome::Denied
        );
        assert_eq!(ui.prompt_count(), 0);

        // The generation's own token is the other half of `combineAbortSignals(state.owner.signal,
        // signal)`: a stopped generation denies just as an aborted caller does.
        let stopped = approval_state(demo_config(Some(BoolOrList::All(true))), Some(ui));
        let _ = stopped.owner.stop(None).await;
        assert_eq!(
            ensure_tool_call_approved(
                &stopped,
                "demo",
                &tool,
                &json!({}),
                ApprovalOrigin::Proxy,
                &CancelToken::new(),
                &metadata,
            )
            .await,
            ApprovalOutcome::Denied
        );
    }

    /// `tool-approval.ts:177-184 @v2.26.1` — the dialog's exact text and its exact option list.
    #[tokio::test]
    async fn the_dialog_is_the_upstream_title_options_and_argument_block() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let ui = ScriptedUi::answering(Some(APPROVE_ONCE_OPTION));
        let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));

        let _ = ensure_tool_call_approved(
            &state,
            "demo",
            &tool,
            &json!({ "query": "private" }),
            ApprovalOrigin::Proxy,
            &CancelToken::new(),
            &metadata,
        )
        .await;

        // The argument block is `sanitizeTerminalText(JSON.stringify(args, null, 2))`, and the
        // sanitiser's `/\s+/g -> " "` tail collapses the pretty-printer's newlines and indent to
        // single spaces. Upstream shows exactly this one-line form; the `null, 2` argument survives
        // only as the spaces between tokens. Reproduced, not "fixed".
        assert_eq!(
            ui.last_prompt(),
            "MCP: demo wants to run search-records\n\nArguments:\n{ \"query\": \"private\" }"
        );
        assert_eq!(
            ui.options.lock().unwrap().last().cloned().unwrap_or_default(),
            vec!["Allow once", "Allow for session", "Deny"]
        );
    }

    /// MCP-235's two interpolations plus the argument block: nothing a hostile server controls can
    /// repaint the dialog it appears in.
    ///
    /// The **names** go through `sanitizeTerminalText` directly. The **arguments** are protected by
    /// two different mechanisms, and the test asserts both, because each covers bytes the other
    /// does not: `JSON.stringify` escapes C0 (an `ESC` becomes the inert literal `\u001b`), and the
    /// sanitiser removes `U+007F` and the C1 block, which `JSON.stringify` emits raw.
    #[tokio::test]
    async fn a_hostile_name_or_argument_cannot_repaint_the_dialog() {
        let tool = ToolMetadata::new("evil", "run\u{1b}[2Jclear", "");
        let metadata = metadata_with(&[("evil\u{7}server", vec![tool.clone()])]);
        let mut config = config_with(&[("evil\u{7}server", stdio("demo"))]);
        if let Some(entry) = config.mcp_servers.get_mut("evil\u{7}server") {
            entry.approve_tools = Some(BoolOrList::All(true));
        }
        let ui = ScriptedUi::answering(Some(DENY_OPTION));
        let state = approval_state(config, Some(Arc::clone(&ui)));

        let _ = ensure_tool_call_approved(
            &state,
            "evil\u{7}server",
            &tool,
            // `c1` carries DEL + a C1 control, which JSON escaping does NOT touch; `c0` carries a
            // real CSI, which JSON escaping renders inert before the sanitiser ever sees it.
            &json!({ "c1": "a\u{7f}\u{85}b", "c0": "x\u{1b}[31my" }),
            ApprovalOrigin::Proxy,
            &CancelToken::new(),
            &metadata,
        )
        .await;

        let prompt = ui.last_prompt();
        assert!(!prompt.contains('\u{1b}'), "no ESC survives: {prompt:?}");
        assert!(!prompt.contains('\u{7}'), "no BEL survives: {prompt:?}");
        assert!(!prompt.contains('\u{7f}'), "no DEL survives: {prompt:?}");
        assert!(!prompt.contains('\u{85}'), "no C1 control survives: {prompt:?}");
        // BEL is a C0 control and collapses to ONE space; `ESC [ 2 J` is a complete CSI and is
        // removed outright, leaving the two halves of the tool name joined.
        assert!(
            prompt.starts_with("MCP: evil server wants to run runclear\n\nArguments:\n"),
            "{prompt:?}"
        );
        // DEL + C1 are one control run and become one space.
        assert!(prompt.contains("\"c1\": \"a b\""), "{prompt:?}");
        // The CSI arrived pre-escaped by the JSON renderer and is shown, inertly, as text.
        assert!(prompt.contains(r#""c0": "x\u001b[31my""#), "{prompt:?}");
    }

    /// `tool-approval.ts:176 @v2.26.1` — over 500 UTF-16 units the preview is cut and gets a literal `...`
    /// tail (three ASCII periods, not `…`). The sanitiser collapses the pretty-printer’s newlines
    /// to single spaces, so the budget is spent on content rather than on indentation.
    #[tokio::test]
    async fn the_argument_preview_is_capped_at_five_hundred_units() {
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let ui = ScriptedUi::answering(Some(DENY_OPTION));
        let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));

        let _ = ensure_tool_call_approved(
            &state,
            "demo",
            &tool,
            &json!({ "blob": "x".repeat(4_000) }),
            ApprovalOrigin::Proxy,
            &CancelToken::new(),
            &metadata,
        )
        .await;

        let prompt = ui.last_prompt();
        let preview = prompt.split("Arguments:\n").nth(1).unwrap_or_default().to_string();
        assert!(preview.ends_with("..."), "the tail is three ASCII periods: {preview:?}");
        assert_eq!(
            preview.trim_end_matches("...").encode_utf16().count(),
            APPROVAL_PREVIEW_LENGTH
        );
    }

    /// MCP-471, end to end through the seam that actually carries it: the ctx a dispatch handed
    /// `McpExtension::on_event` is recorded on the state, and the approval dialog opened later —
    /// from `Tool::execute`, which has no ctx of its own — signals that very gate.
    ///
    /// This is the whole reason `McpState::human_wait_ctx` exists; with the slot unset the guard is
    /// silently never taken and nothing else in the system notices.
    #[tokio::test]
    async fn the_recorded_dispatch_ctx_reaches_the_approval_dialog() {
        let ctx = cyrup_ext::HostCtx::event(
            cyrup_ext::ExtMode::Tui,
            true,
            std::path::PathBuf::from("/workspace"),
        );
        let gate = ctx.human_wait_gate();
        let tool = demo_tool();
        let metadata = metadata_with(&[("demo", vec![tool.clone()])]);
        let ui = ScriptedUi::watching(Some(DENY_OPTION), Arc::clone(&gate));
        let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));

        // Nothing recorded yet — the dialog still opens, it just forgives no budget.
        let _ = ensure_tool_call_approved(
            &state,
            "demo",
            &tool,
            &json!({ "n": 1 }),
            ApprovalOrigin::Proxy,
            &CancelToken::new(),
            &metadata,
        )
        .await;

        // `on_event` records the dispatch ctx…
        state.set_human_wait_ctx(&ctx);
        let _ = ensure_tool_call_approved(
            &state,
            "demo",
            &tool,
            &json!({ "n": 2 }),
            ApprovalOrigin::Proxy,
            &CancelToken::new(),
            &metadata,
        )
        .await;

        assert_eq!(
            *ui.waiting_during_dialog.lock().unwrap(),
            vec![false, true],
            "the second dialog runs under the P-3 guard the recorded ctx supplies"
        );
        assert!(!gate.is_waiting(), "and releases it when the dialog returns");
    }

    /// The [`ProxyCtx`] seam joins the state and the metadata map for both gates — the two-line
    /// body a production [`ProxyEnv`] forwards to, exercised end to end so the bridge cannot rot
    /// while the trait has no production implementor.
    #[tokio::test]
    async fn the_proxy_ctx_bridge_reaches_both_gates() {
        let tool = demo_tool();
        let ui = ScriptedUi::answering(Some(DENY_OPTION));
        let state = approval_state(demo_config(Some(BoolOrList::All(true))), Some(Arc::clone(&ui)));
        let ctx = Arc::new(ProxyCtx::new(Arc::clone(&state), Arc::new(FakeEnv::default())));
        ctx.with_metadata_mut(|metadata| {
            metadata.insert("demo".to_string(), vec![tool.clone()]);
        });

        assert!(ctx.approval_required("demo", &tool));
        assert_eq!(
            ctx.ensure_tool_call_approved(
                "demo",
                &tool,
                &json!({}),
                ApprovalOrigin::Proxy,
                &CancelToken::new(),
            )
            .await,
            ApprovalOutcome::Denied
        );
        assert_eq!(ui.prompt_count(), 1);

        // The same context with no rule gating the tool answers without a dialog.
        let ungated = approval_state(demo_config(None), Some(Arc::clone(&ui)));
        let ungated_ctx = Arc::new(ProxyCtx::new(ungated, Arc::new(FakeEnv::default())));
        ungated_ctx.with_metadata_mut(|metadata| {
            metadata.insert("demo".to_string(), vec![tool.clone()]);
        });
        assert!(!ungated_ctx.approval_required("demo", &tool));
        assert_eq!(
            ungated_ctx
                .ensure_tool_call_approved(
                    "demo",
                    &tool,
                    &json!({}),
                    ApprovalOrigin::Proxy,
                    &CancelToken::new(),
                )
                .await,
            ApprovalOutcome::Approved
        );
        assert_eq!(ui.prompt_count(), 1, "an ungated tool opens no second dialog");
    }

    /// The two origin derivations differ only in their fallback, and that difference is the whole
    /// reason both exist (`proxy-modes.ts:1145` vs `direct-tools.ts:440 @v2.26.1`).
    #[test]
    fn the_two_origin_derivations_differ_only_in_their_fallback() {
        let uri = "docs://handbook".to_string();
        assert_eq!(ApprovalOrigin::for_proxy_call(None), ApprovalOrigin::Proxy);
        assert_eq!(ApprovalOrigin::for_direct_tool(None), ApprovalOrigin::Direct);
        assert_eq!(ApprovalOrigin::for_proxy_call(Some(&uri)), ApprovalOrigin::Resource);
        assert_eq!(ApprovalOrigin::for_direct_tool(Some(&uri)), ApprovalOrigin::Resource);
        assert_eq!(
            [ApprovalOrigin::Proxy.as_str(), ApprovalOrigin::Direct.as_str(), ApprovalOrigin::Resource.as_str()],
            ["proxy", "direct", "resource"]
        );
    }

}
