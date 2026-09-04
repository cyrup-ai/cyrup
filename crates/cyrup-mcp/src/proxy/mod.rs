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

pub mod approval;
pub mod auth;
pub mod call;
pub mod constants;
pub mod description;
pub mod discovery;
pub mod env;
pub mod error_vocab;
pub mod ranking;
pub mod results;
pub mod tool;
pub mod tool_metadata;

/// Fixtures shared by more than one submodule's tests (the crate's
/// `exec/testsupport.rs` convention).
#[cfg(test)]
pub(crate) mod testsupport;

// The glob re-exports hold every `crate::proxy::X` path the crate already uses — `lib.rs`'s
// five, `oauth.rs`'s `format_manual_auth_instructions`, `registration.rs`'s
// `build_proxy_description` and `extension.rs`'s `INIT_WAIT_TIMEOUT_MS` — so the split is
// invisible outside this directory.

pub use approval::*;
pub use auth::*;
pub use call::*;
pub use constants::*;
pub use description::*;
pub use discovery::*;
pub use env::*;
pub use error_vocab::*;
pub use ranking::*;
pub use results::*;
pub use tool::*;
pub use tool_metadata::*;

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

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
        assert_eq!(
            truncate_at_word("\u{1f600}\u{1f600}\u{1f600}", 4),
            "\u{1f600}\u{1f600}..."
        );
        // The `lastSpace > target * 0.6` word cut, and the below-threshold hard cut.
        assert_eq!(truncate_at_word("hello world again", 12), "hello world...");
        assert_eq!(truncate_at_word("a bbbbbbbbbbbb", 10), "a bbbbbbbb...");
        // Short enough is returned untouched, with no ellipsis.
        assert_eq!(truncate_at_word("short", 10), "short");
    }
}
