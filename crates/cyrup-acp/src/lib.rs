//! # `cyrup-acp` — the Agent Client Protocol adapter
//!
//! The surface that lets an editor — Zed is the reference client — drive cyrup over ACP JSON-RPC
//! 2.0 on stdio. The editor sends `initialize`, `session/new`, `session/prompt`, `session/cancel`,
//! `session/load`, `session/list`, `session/delete`, `session/set_mode` and
//! `session/set_config_option`; cyrup answers, and streams the turn back as `session/update`
//! notifications plus `session/request_permission` requests.
//!
//! It is **not** a new agent, a new tool tier or a new provider — it is a **fourth front-end**
//! beside the TUI, `--mode rpc` and print/json, and everything below the front-end is the
//! `cyrup_session_svc::AgentSession` those three already drive.
//!
//! ## Provenance
//!
//! Upstream is **`svkozak/pi-acp` v0.0.33**, MIT © Sergii Kozak (17 TypeScript files, 4 238 lines
//! under `src/`). Every ported item in this crate carries a doc comment naming the upstream file
//! and symbol at that version, so a reviewer can open one file and check the claim.
//!
//! The Rust binding is `agent-client-protocol = "2.1"`, **default features, no feature flags** —
//! its schema crate is pinned transitively at `agent-client-protocol-schema =1.7.0`, whose default
//! surface *is* protocol v1. The design decisions this crate is built on are
//! `docs/gap-analysis/15-cyrup-acp.md` (the seam map, 168 `ACP-NNN` units) and
//! `docs/adr/ADR-0028-cyrup-acp-type-design.md` (the type design).
//!
//! ## The architecture, in one paragraph
//!
//! `cyrup-acp` binds to `AgentSession` **in-process**, as a new `cyrup_config::AppMode::Acp` beside
//! `Interactive`/`Print`/`Json`/`Rpc`, with a **one-live-session** manager. This is the inversion of
//! its upstream, which is an out-of-process adapter by necessity: pi-acp is a separate npm package
//! that cannot link into pi, so it spawns `pi --mode rpc` and bridges two wires, which is why
//! roughly 40% of its code is defensive key-probing over `Record<string, unknown>`.
//!
//! Two arguments decided it. **The out-of-process design cannot see what ACP needs, by contract**:
//! `cyrup_modes::is_upstream_wire_event` deliberately keeps `SessionReplaced`, `ModelChanged`,
//! `SessionStart` and `SessionShutdown` *off* the RPC wire, so an out-of-process `cyrup-acp` would
//! have no source for ACP's `current_mode_update` or `available_commands_update` and no way to
//! learn that the session was replaced under it. And **the permission seam only closes
//! in-process**: over RPC a host sees an `extension_ui_request` with a title and a method and
//! cannot tell a permission ask from any other dialog — which is exactly why pi-acp synthesizes
//! `allow_once` options for *every* select. See [`permission`].
//!
//! What it costs is stated at each site: one live session per connection ([`sessions`]), and a
//! terminal delta that can desync where upstream's could not ([`ledger::TerminalAppender`]).
//!
//! ## What has no counterpart and must not be written
//!
//! `PiRpcProcess` and the spawn diagnostics (there is no child); `pi-rpc/command.ts` entire (every
//! line is an npm-installation assumption); the UUID correlation map (correlation is the call
//! stack); the ANSI prelude buffer and `stripAnsi` (`cyrup_session_svc::bash::strip_ansi` is
//! better); `slash-commands.ts` entire (cyrup expands templates server-side in
//! `AgentSession::prepare_and_assemble` — see [`commands`]); `pi-settings.ts` entire
//! (`cyrup_config::EffectiveSettings` covers it, and re-reading the files directly would
//! reintroduce a **trust bypass**); and the `~/.pi/pi-acp/session-map.json` sidecar (the mapping is
//! derivable — see [`sessions`]).
//!
//! ## Module map
//!
//! | module | what it owns |
//! |---|---|
//! | [`error`] | [`AcpFailure::classify`] — the typed replacement for `maybeAuthRequiredError` |
//! | [`ids`] | [`AbsCwd`], [`SessionFile`], [`AcpSessionId`] — the strings that become filesystem authorities |
//! | [`connection`] | the transport bootstrap, the builder chain, and the `initialize` [`ClientView`] |
//! | [`turn`] | [`Turn`] — the sole owner of a `session/prompt`'s responder |
//! | [`ledger`] | [`ToolCallLedger`] and [`TerminalAppender`] — the translator's state |
//! | [`mod@translate`] | the pure `(event, ledger) -> Vec<SessionUpdate>` core |
//! | [`permission`] | the `UiSink` bridge and [`DialogChoice`] |
//! | [`sessions`] | the one-live-session manager and the `session/*` entry points |
//! | [`commands`] | the seven headless built-ins, and the `PromptRequest` -> `UserInput` translation |
//! | [`startup`] | the markdown startup prelude `session/new` sends after its response |
//! | [`config_options`] | [`SessionConfigKnob`] — one enum that advertises and accepts |
//!
//! ## How the modules fit together
//!
//! One connection owns one [`sessions::SessionManager`], and one live session owns **three tasks**.
//! That split is `ACP-155`'s and it is the crate's load-bearing structural decision:
//!
//! * the **turn actor** ([`turn::TurnActor`]) owns the run-scoped event stream, the
//!   [`ToolCallLedger`], every `session/update` it derives, and the `session/prompt` responder. It
//!   never awaits anything with client or human latency — [`turn::TurnSink::notify`] is a plain
//!   `fn`, so there is no `await` for a maintainer to put a round trip behind.
//! * the **dialog bridge** ([`permission::PermissionBridge`]) owns its own channel and detaches one
//!   task per dialog, so a human sitting on a permission prompt blocks neither the agent nor
//!   another guest's dialog.
//! * the **config pump** owns the *session-wide* stream and is the single emitter of
//!   `config_option_update` / `current_mode_update` / `session_info_update` (`ACP-077`, `ACP-Q20`),
//!   which describe the session rather than any run and are emitted while no run exists.
//!
//! Between them sits [`translate()`], which is pure: it takes an event and a `&mut ToolCallLedger`
//! and returns updates plus one [`TurnSignal`]. It performs no I/O and it settles nothing.
//!
//! ## Reading the markers
//!
//! **`CYRUP-DELTA`** — a mechanism that differs from upstream, in two parts: what differs, and
//! what it costs. A divergence that is not written down is a defect.
//!
//! There is no skeleton marker, no `AcpError::Unimplemented` and no panic macro anywhere: every
//! body in this crate is written, and the error type no longer carries a variant that would let an
//! unfinished one answer a frame instead of being finished.

#![forbid(unsafe_code)]

pub mod commands;
pub mod config_options;
pub mod connection;
pub mod error;
pub mod ids;
pub mod ledger;
pub mod permission;
pub mod sessions;
pub mod startup;
pub mod translate;
pub mod turn;

use agent_client_protocol::schema::v1::{SessionId, SessionNotification, SessionUpdate};
use agent_client_protocol::{Client, ConnectionTo, JsonRpcResponse, Responder};

/// Re-exported so a downstream implementor of [`AcpHost`] — `crate`-external by construction,
/// since the binary implements it — can name every type in that trait's signature without taking a
/// direct edge on `agent-client-protocol` or `cyrup-session`. Adding those edges to the `cyrup`
/// binary would put the ACP schema in its namespace for every other module too, which is exactly
/// the coupling this crate exists to contain.
pub use agent_client_protocol::BoxFuture;
/// See [`BoxFuture`]. This is the JSON-RPC wire error type, distinct from [`AcpError`].
pub use agent_client_protocol::Error as WireError;
/// See [`BoxFuture`]. The sessions root [`AcpHost::sessions_root`] answers with.
pub use cyrup_session::layout::SessionsRoot;
/// See [`BoxFuture`]. The runtime [`AcpHost::build_runtime`] produces.
pub use cyrup_session_svc::AgentSessionRuntime;

pub use commands::{
    BUILTIN_STOP_REASON, BUILTINS, Builtin, available_commands, available_commands_update,
    dispatch, intercept, merge_commands, prompt_to_user_input,
};
pub use config_options::{
    AppliedKnob, SessionConfigKnob, SessionConfigView, agent_capabilities, apply_config_option,
    apply_mode, auth_methods, session_surface,
};
pub use connection::{AcpConnection, ClientView, serve, serve_stdio};
pub use error::{AcpError, AcpFailure};
pub use ids::{AbsCwd, AcpSessionId, SessionFile};
pub use ledger::{FileSnapshot, Push, TerminalAppender, ToolCallLedger, ToolCallStream, ToolClass};
pub use permission::{
    DialogCaps, DialogChoice, DialogClient, DialogOptionTable, DialogRequest, PermissionBridge,
    deny_default,
};
pub use sessions::{
    AcpHost, LoadOutcome, RestoreGate, RuntimeRequest, SessionManager, StoredSession, find_stored,
    replay_updates,
};
pub use translate::{
    SnapshotPhase, SnapshotRequest, Translated, TurnSignal, snapshot_needed, translate,
};
pub use turn::{
    Admission, PromptReply, RunStarted, RunningTurn, RuntimeAgent, SettleAction, Turn, TurnActor,
    TurnAgent, TurnHandle, TurnMessage, TurnOutcome, TurnSink,
};

/// The literal token cyrup publishes to ACP clients in `AuthMethod::Terminal.args` (`ACP-011`).
///
/// Deliberately a **second, independent declaration** of the same string
/// `crate::acp_terminal_login_cmd::SUBCOMMAND` in the `cyrup` binary recognises, following the
/// precedent documented on `subagent_runner_cmd::SUBCOMMAND`: "what the client is told to send" and
/// "what `main` recognises" must each be free-standing enough to unit-test without the other. The
/// binary's `the_recognised_token_is_the_one_advertised_to_acp_clients` test is the cross-check
/// that keeps them in step.
pub const TERMINAL_LOGIN_ARG: &str = "--terminal-login";

/// A request handler's answer, plus what must be sent **after** it.
///
/// ADR-0028 F2, made structural. Port of the *intent* of pi-acp v0.0.33 `agent.ts`'s two
/// `setTimeout(() => …, 0)` blocks in `newSession` and `loadSession`, whose comment states the
/// hazard in the upstream author's own words: *"some clients (e.g. Zed) will ignore notifications
/// for an unknown sessionId"*.
///
/// # [CYRUP-DELTA] — a visibility rule replaces a timer
///
/// **What differs.** `ConnectionTo::send_notification` is **synchronous** — it enqueues on an
/// `mpsc` and returns `Result<(), Error>` with no `.await` — so the natural Rust port of that
/// deferral ("call `cx.send_notification(..)` at the end of the handler, then
/// `responder.respond(..)`") writes the notification **first**, which is the exact bug the
/// `setTimeout` avoids, reintroduced with **no timer left to hide it**.
///
/// **What it costs.** Handlers must not receive `ConnectionTo<Client>`; they return this instead,
/// and only [`respond_then_notify`] holds the connection. The cost is one wrapper type and the
/// discipline of not "just passing `cx` in so we can send the update inline" — which would undo the
/// ordering silently at runtime. ADR-0028 §7 recommends a `trybuild` compile-fail case pinning
/// exactly that, and it is the one compile-fail test in the port that earns its keep.
pub struct HandlerOutcome<R> {
    /// The response, written first.
    pub response: R,
    /// Sent after the response, in order. `ACP-068`/`ACP-069`/`ACP-293`.
    pub follow_up: Vec<SessionUpdate>,
}

impl<R> HandlerOutcome<R> {
    /// A response with nothing to follow.
    pub fn plain(response: R) -> Self {
        Self {
            response,
            follow_up: Vec::new(),
        }
    }

    /// A response plus the updates that must reach the client after it.
    pub fn with_follow_up(response: R, follow_up: Vec<SessionUpdate>) -> Self {
        Self {
            response,
            follow_up,
        }
    }
}

/// Answer a request, then drain its follow-up updates — **in that order, always**.
///
/// The one place the response/notification ordering is enforced. See [`HandlerOutcome`].
///
/// `session_id` is taken from the outcome by the caller rather than threaded here, because a
/// handler that produced no session (a failed `session/new`) has none and must not fabricate one.
///
/// # Errors
///
/// Never. A per-request failure is answered through `responder` and this returns `Ok(())`:
/// `ConnectionTo::spawn`'s own doc is explicit that *"if the spawned task returns an error, the
/// entire server will shut down"*, so propagating a handler's failure out of a spawned task turns
/// one bad request into a dead connection. `ACP-057`'s second assertion is exactly that the
/// connection answers a later request after a build failure.
pub fn respond_then_notify<R>(
    outcome: Result<HandlerOutcome<R>, AcpFailure>,
    responder: Responder<R>,
    cx: &ConnectionTo<Client>,
) -> Result<(), agent_client_protocol::Error>
where
    R: JsonRpcResponse + SessionScoped,
{
    match outcome {
        Ok(HandlerOutcome {
            response,
            follow_up,
        }) => {
            let session_id = response.session_id();
            // Response FIRST. Its `Err` is a dead transport, which the connection loop is already
            // tearing down.
            let _ = responder.respond(response);
            if let Some(session_id) = session_id {
                for update in follow_up {
                    // `ACP-122` — a `send_notification` that fails must not stop the turn
                    // completing, mirroring upstream's unconditional silent `.catch(() => {})`.
                    let _ =
                        cx.send_notification(SessionNotification::new(session_id.clone(), update));
                }
            }
            Ok(())
        }
        Err(failure) => {
            let _ = responder.respond_with_error(failure.into());
            Ok(())
        }
    }
}

/// Which session a response's follow-up updates belong to.
///
/// A trait rather than a parameter so that a response type carrying no session — `session/list`,
/// `session/delete` — **cannot** be given follow-up updates addressed to a session it never named.
/// That is the mistake ADR-0028 F2 lists as still possible for the shell ("sent against the wrong
/// `SessionId`"); this closes the half of it that is expressible in a signature.
pub trait SessionScoped {
    /// The session these updates are for, or `None` when the response names no session.
    fn session_id(&self) -> Option<SessionId>;
}

macro_rules! session_scoped {
    ($ty:ty, |$this:ident| $body:expr) => {
        impl SessionScoped for $ty {
            fn session_id(&self) -> Option<SessionId> {
                let $this = self;
                $body
            }
        }
    };
}

session_scoped!(
    agent_client_protocol::schema::v1::NewSessionResponse,
    |this| Some(this.session_id.clone())
);
// `LoadSessionResponse` names no session — the id was the client's, in the request. The follow-up
// updates for a load are addressed by the handler that knows it, not from the response.
session_scoped!(
    agent_client_protocol::schema::v1::LoadSessionResponse,
    |_this| None
);
session_scoped!(
    agent_client_protocol::schema::v1::ListSessionsResponse,
    |_this| None
);
session_scoped!(
    agent_client_protocol::schema::v1::DeleteSessionResponse,
    |_this| None
);
session_scoped!(
    agent_client_protocol::schema::v1::SetSessionModeResponse,
    |_this| None
);
session_scoped!(
    agent_client_protocol::schema::v1::SetSessionConfigOptionResponse,
    |_this| None
);

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// ACP-011's other half lives in the `cyrup` binary; this pins the string itself so a rename
    /// here is a deliberate act with a test to change.
    #[test]
    fn the_advertised_terminal_login_arg_is_one_token() {
        assert_eq!(TERMINAL_LOGIN_ARG, "--terminal-login");
        assert!(
            !TERMINAL_LOGIN_ARG.contains(' '),
            "ACP-013: `args` is exactly one element, not two"
        );
    }

    /// A response that names no session cannot carry follow-up updates addressed to one.
    #[test]
    fn only_a_session_bearing_response_can_address_follow_up_updates() {
        use agent_client_protocol::schema::v1::{ListSessionsResponse, NewSessionResponse};
        let new = NewSessionResponse::new(SessionId::new("s1"));
        assert_eq!(new.session_id().map(|s| s.to_string()), Some("s1".into()));
        assert!(ListSessionsResponse::new(vec![]).session_id().is_none());
    }
}
