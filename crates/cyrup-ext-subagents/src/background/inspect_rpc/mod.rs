//! SCOPE_12 — on-demand inspection of the CURRENT session's async children: the RPC surface that
//! reads one run's (or one child's) messages and final output back out of the canonical artifacts.
//!
//! Ports pi `runs/background/inspect-rpc.ts` (443 LOC, `@7fe9dee1`). Its own header sentence says
//! what this is and is not: *"Re-reads canonical artifacts after the same reconciliation as status;
//! nothing is persisted or broadcast."* It is a READ. It writes no file, emits no event, and
//! consults no authority policy.
//!
//! # Why this is the last consumer of the session-partitioned index
//!
//! Every other reader of a run's terminal payload had already been routed through
//! [`crate::background::result_index`]. Inspect is the one that still had no port, and it is the
//! most demanding consumer: it uses the session THREE times in one function
//! ([`read_output::read_result_output`]) — to ADDRESS the payload
//! ([`crate::background::result_index::result_payload_path_for_session_run`], pi `:234`), to FILTER
//! the durable replay record
//! ([`crate::background::completion_replay::read_completion_replay`], pi `:247`), and to
//! RE-VERIFY the raw record's own `sessionId` (pi `:255`). The last is belt-and-braces over the
//! second and they guard different failure modes: a filter that was not supplied, versus a record
//! that disagrees with the filter it was given.
//!
//! # File layout — one file, one concern
//!
//! ```text
//! mod.rs         facade: the format constants (they ARE the wire shape) and this narrative
//! request.rs     InspectRequest / InspectErrorCode / parse_inspect_request (the slash arg form)
//! read_output.rs the three session uses: payload → replay → archive ladder
//! respond.rs     InspectReply and its bounding, build_inspect_reply, the encoders
//! ```
//!
//! # `[CYRUP-DELTA]`s, each recorded at its own seam as well
//!
//! * **`action: "inspect"` is cyrup's own verb.** Upstream reaches this file ONLY through the
//!   `/subagents-inspect-rpc` slash command ([`request::parse_inspect_request`],
//!   [`respond::handle_inspect_rpc_args`]); `inspect` does not appear in upstream's
//!   `SUBAGENT_ACTIONS`. Routing it as a tool action is cyrup's decision — see the dispatch arm in
//!   `extension/tool/routing.rs`. Both surfaces are live and both produce the same
//!   [`respond::InspectReply`], which is the format.
//! * **The nested arm of `findChildNode` (pi `:153-169`) is unportable.** cyrup's per-step nested
//!   tracking is [`crate::background::StepStatus::nested_run_ids`] — a `Vec<RunId>`, not pi's
//!   `NestedRunSummary { asyncDir, agent, agents, children }` — and `reconcileNestedAsyncDescendants`
//!   has no port, exactly as [`crate::background::child_identity`]'s own module doc already states.
//!   So `resolved.kind === "nested"` (pi `:344-347`) and the nested descent collapse away, and
//!   child resolution goes through [`crate::background::child_identity::resolve_async_status_child`],
//!   whose `NotFound` sentence is already upstream's verbatim.
//! * **`status.sessionRoot` is unrepresentable.** [`crate::background::RunStatus`] declares
//!   `session_file` but no `session_root`, so the second half of upstream's trusted-root union
//!   (pi `:375`) has nothing to read. The roots are supplied by the caller instead — see
//!   [`respond::InspectDeps::trusted_roots`].
//! * **`trustedSessionFileRoot` / the `trustedFiles` allowance is unrepresentable.** See
//!   [`crate::background::fleet_view::read_session_messages_tail`].
//! * **`step.label` collapses to `step.agent`.** [`crate::background::StepStatus`] carries no
//!   user-facing label, the same delta [`crate::background::fleet_view`] and
//!   [`crate::background::run_status`] already record.
//! * **`context === "fork"` has no field to read.** See [`respond::build_inspect_reply`]'s
//!   task-attribution note.
//! * **The `FAILED_OUTPUT_ARTIFACT_PREFIX` branch is dead for a cyrup-written artifact** and is
//!   ported anyway. See [`read_output::read_output_artifact`].

pub mod read_output;
pub mod request;
pub mod respond;

pub use read_output::{ResultOutput, read_result_output};
pub use request::{InspectErrorCode, InspectRequest, parse_inspect_request};
pub use respond::{
    InspectDeps, InspectReply, InspectReplyError, InspectReplyMessage, InspectTruncation,
    build_inspect_reply, encode_inspect_reply, handle_inspect_rpc_args,
};

/// pi `INSPECT_REPLY_KIND` (`inspect-rpc.ts:14`).
pub const INSPECT_REPLY_KIND: &str = "pi-subagents.inspect-reply";
/// pi `INSPECT_REPLY_VERSION` (`:15`).
pub const INSPECT_REPLY_VERSION: u32 = 1;
/// pi `INSPECT_WIDGET_KEY` (`:16`) — the host-side widget slot this reply renders into.
pub const INSPECT_WIDGET_KEY: &str = "subagent-inspect";
/// pi `INSPECT_WIDGET_PREFIX` (`:17`) — the single-line envelope
/// [`respond::encode_inspect_reply`] emits.
pub const INSPECT_WIDGET_PREFIX: &str = "PI_SUBAGENT_INSPECT_JSON:";

/// pi `REQUEST_ID_PATTERN` (`:59`), as a predicate rather than a regex — the crate pulls in no
/// regex engine and `^[A-Za-z0-9_-]{1,64}$` is a character-class test with a length bound.
#[must_use]
pub fn is_valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// pi `MAX_ID_LENGTH` (`:60`).
pub const MAX_ID_LENGTH: usize = 256;
/// pi `MAX_LABEL_LENGTH` (`:61`).
pub const MAX_LABEL_LENGTH: usize = 160;
/// pi `MAX_TASK_LENGTH` (`:62`).
pub const MAX_TASK_LENGTH: usize = 2_000;
/// pi `MAX_FINAL_OUTPUT_LENGTH` (`:63`).
pub const MAX_FINAL_OUTPUT_LENGTH: usize = 8_000;
/// pi `MAX_MESSAGE_TEXT_LENGTH` (`:64`).
pub const MAX_MESSAGE_TEXT_LENGTH: usize = 1_000;
/// pi `DEFAULT_MESSAGE_LINES` (`:65`).
pub const DEFAULT_MESSAGE_LINES: usize = 100;
/// pi `MAX_MESSAGE_LINES` (`:66`).
pub const MAX_MESSAGE_LINES: usize = 200;
/// pi `MAX_SERIALIZED_BYTES` (`:68`) — 64 KiB, the ceiling
/// [`respond::enforce_byte_budget`] fits the reply under.
pub const MAX_SERIALIZED_BYTES: usize = 64 * 1024;

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    /// `^[A-Za-z0-9_-]{1,64}$`, spelled out — the anchors matter (a value containing a newline must
    /// not pass, which is exactly what an unanchored JS `test` would let through and what would
    /// break [`INSPECT_WIDGET_PREFIX`]'s one-line envelope).
    #[test]
    fn the_request_id_pattern_is_anchored_and_bounded() {
        assert!(is_valid_request_id("a"));
        assert!(is_valid_request_id("A-Za-z0-9_-"));
        assert!(is_valid_request_id(&"x".repeat(64)));
        assert!(!is_valid_request_id(""));
        assert!(!is_valid_request_id(&"x".repeat(65)));
        assert!(!is_valid_request_id("has space"));
        assert!(!is_valid_request_id("has.dot"));
        assert!(
            !is_valid_request_id("ok\nnot-ok"),
            "a newline would split the widget envelope"
        );
        assert!(!is_valid_request_id("é"), "ASCII alphanumerics only");
    }
}
