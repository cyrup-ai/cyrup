//! The stdio transport bootstrap, the `Agent` builder chain, and handler registration.
//!
//! `ACP-003` and `ACP-023`. Port of pi-acp v0.0.33 `src/index.ts`'s `ndJsonStream(...)` +
//! `new AgentSideConnection(agent, stream)` pair and of the handler table at the head of
//! `src/acp/agent.ts`'s `class PiAcpAgent`.
//!
//! # Why every handler is registered, including the ones that are not written yet
//!
//! `agent_client_protocol::Agent` is a **role marker with no trait to implement**, so there is no
//! compiler check that the handler set is complete. An unregistered method falls through to
//! `default_handle_dispatch_from`, which returns `Handled::No { retry: message.has_session_id() }`
//! — a session-scoped method is **retained and retried**, so a forgotten handler is a HANG, not a
//! `method_not_found`. That is `ACP-014`'s point and it is why every method this port serves is
//! registered here from the first commit. Every one is now implemented; the rule stands for
//! whatever the protocol grows next.
//!
//! # Why request handlers never receive `ConnectionTo<Client>`
//!
//! ADR-0028 F2: `ConnectionTo::send_notification` is **synchronous** — it enqueues on an `mpsc` and
//! returns `Result<(), Error>` with no `.await` — so the natural port of pi-acp's
//! `setTimeout(() => cx.sessionUpdate(...), 0)` deferral ("just call `send_notification` at the end
//! of the handler, then respond") writes the notification FIRST, and Zed drops updates for a
//! `sessionId` it has not yet been told about. There is no timer left to accidentally save you.
//!
//! The fix is a **visibility rule**: the pure handler functions in [`crate::sessions`] and
//! [`crate::commands`] return [`crate::HandlerOutcome`] and are never handed `cx`; this module,
//! which does hold `cx`, responds first and drains `follow_up` second. Do not "just pass `cx` in so
//! we can send the update inline" — that undoes the invariant silently at runtime.

use std::sync::{Arc, OnceLock};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, DeleteSessionRequest,
    Implementation, InitializeRequest, InitializeResponse, ListSessionsRequest, LoadSessionRequest,
    NewSessionRequest, PromptRequest, SetSessionConfigOptionRequest, SetSessionModeRequest,
};
use agent_client_protocol::{Agent, Client, ConnectTo, ConnectionTo};

use crate::error::AcpError;
use crate::sessions::{AcpHost, SessionManager};

/// The agent identity advertised in `initialize`'s `agentInfo` (`ACP-051`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `agentInfo` literal, whose `name`/`version` came from
/// `readNearestPackageJson`'s six-level directory walk with two `??` fallbacks. Rust bakes identity
/// in at compile time, so the walk and **both** fallbacks are cut: a `.unwrap_or("cyrup")` here
/// would be dead code asserting an impossible failure. This also removes the last filesystem read
/// from `initialize`, making it a pure function of the request.
///
/// # [CYRUP-DELTA] — the product's name, not the crate's
///
/// **What differs.** This is the literal `"cyrup"`, not `env!("CARGO_PKG_NAME")` (which is
/// `cyrup-acp`). `ACP-051`'s verify reads `agentInfo.name == "cyrup"` and this string is what a Zed
/// user sees in the agent picker: the adapter crate is an implementation detail of the product, and
/// naming the crate there tells the user about cyrup's source layout rather than about the agent
/// they are talking to. The **version** stays `CARGO_PKG_VERSION`, which is the workspace version
/// and is therefore the product's.
///
/// **What it costs.** A reader who greps for the crate name will not find it in the wire frames;
/// [`AGENT_VERSION`]'s provenance is unchanged.
pub const AGENT_NAME: &str = "cyrup";
/// See [`AGENT_NAME`].
pub const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Clamp a requested protocol version onto the one this agent serves (`ACP-050`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s
/// `protocolVersion: requested === supportedVersion ? requested : supportedVersion`. Every non-1
/// request is answered with 1; there is no error path, and a client that cannot live with 1 is
/// expected to disconnect itself.
///
/// It is a **named total function with its own test** rather than an inline ternary on purpose: the
/// ternary reads as dead code and simplifies to `protocolVersion: requested`, and a maintainer
/// making that simplification introduces a silent protocol lie.
///
/// # [CYRUP-DELTA] — the SDK narrows the clamp's domain
///
/// **What differs.** `InitializeRequest.protocol_version` carries no `DefaultOnError`, so a value
/// outside `u16` or of the wrong JSON type fails deserialization and the whole request is rejected,
/// where pi-acp clamped it to 1.
///
/// **What it costs.** A client sending `"protocolVersion": "1.0.0"` gets a parse error instead of a
/// graceful downgrade. The divergence is imposed by the schema crate and cannot be closed from a
/// handler; it is recorded so a later reader does not file it as a port defect.
#[must_use]
pub fn clamp_protocol_version(_requested: ProtocolVersion) -> ProtocolVersion {
    ProtocolVersion::V1
}

/// What the `initialize` handler learned about the peer, for every later handler to read.
///
/// ADR-0028 §5 rejects connection typestate for this and prescribes exactly this shape. The reason
/// is mechanical: the SDK's `Builder` registers every handler **before** `connect_to` is awaited
/// and `ChainedHandler` dispatches to whichever link claims a message for the connection's entire
/// life, so there is no ownership path along which the compiler could withhold `session/new` until
/// `initialize` has been answered. Worse, a "state-gated" handler that declined would return
/// `Handled::No { retry: .. }` and the request would be retained and retried — a hang.
///
/// So it is a runtime check: a `OnceLock` set once by `initialize` and read by the others.
#[derive(Clone, Debug)]
pub struct ClientView {
    /// The version this connection settled on — always [`ProtocolVersion::V1`] today, kept as a
    /// field so `ACP-050`'s clamp has an observable result.
    pub protocol_version: ProtocolVersion,
    /// `ClientCapabilities.auth.terminal` — the **typed** negotiation 2.1.0 added, which is what
    /// pi-acp's `_meta["terminal-auth"]` probe stood in for (`ACP-012`).
    pub auth_terminal: bool,
    /// `clientCapabilities._meta["terminal-auth"] === true` — the legacy Zed probe. A **strict**
    /// boolean-true test, as upstream's `=== true` is, so a truthy non-boolean does not qualify.
    pub terminal_auth_meta: bool,
    /// `ClientCapabilities.terminal` — whether the client serves the `terminal/*` family.
    pub terminal: bool,
    /// Whether the client advertised **form-mode** `elicitation`, which `ACP-147` needs for
    /// `UiKind::Input` and `UiKind::Editor`.
    ///
    /// # [CYRUP-DELTA] — the half of the capability that is read is the half that is used
    ///
    /// **What differs.** `ElicitationCapabilities` has two independent halves, `form` and `url`,
    /// and everything [`crate::permission`] sends is a form. Reading `elicitation.is_some()` — as
    /// this field did before integration — sends a form request to a client that advertised only
    /// `{"elicitation": {"url": {}}}`.
    ///
    /// **What it costs.** Nothing, now: a client with URL-only elicitation takes
    /// [`crate::permission`]'s unsupported-dialog fallback chunk and the guest gets
    /// `Text(None)`, which is what it would have got from the declined form anyway — minus one
    /// wasted round trip and plus the chunk that tells the user why.
    pub elicitation: bool,
}

impl Default for ClientView {
    /// A view for a peer that has told us nothing: protocol v1 and every capability off.
    ///
    /// `ProtocolVersion` has no `Default` in the schema crate — deliberately, since "version zero"
    /// is not a sensible default — so this is written out rather than derived, and the value is the
    /// one [`clamp_protocol_version`] would produce for any request.
    fn default() -> Self {
        Self {
            protocol_version: ProtocolVersion::V1,
            auth_terminal: false,
            terminal_auth_meta: false,
            terminal: false,
            elicitation: false,
        }
    }
}

impl ClientView {
    /// Project the client's `initialize` payload. Pure, so `ACP-012`/`ACP-054`'s gating is
    /// table-testable with no connection.
    #[must_use]
    pub fn from_request(req: &InitializeRequest) -> Self {
        let caps = &req.client_capabilities;
        Self {
            protocol_version: clamp_protocol_version(req.protocol_version),
            auth_terminal: caps.auth.terminal,
            // Upstream's `=== true`: `Value::as_bool` yields `None` for a truthy non-boolean, so a
            // `"terminal-auth": 1` does NOT qualify. Reproducing the strictness matters — it is the
            // difference between emitting a legacy `_meta` shim to a client that asked for it and
            // emitting it to one that merely mentioned the key.
            terminal_auth_meta: caps
                .meta
                .as_ref()
                .and_then(|m| m.get("terminal-auth"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            terminal: caps.terminal,
            // `.form`, not `.is_some()` — see the field's doc.
            elicitation: caps.elicitation.as_ref().is_some_and(|e| e.form.is_some()),
        }
    }
}

/// Everything one ACP connection owns. Cloned into each registered handler.
///
/// One live session per connection, structurally: [`SessionManager`] holds one slot, because
/// `AgentSessionRuntime` is a one-slot replacer and the native-extension host-services slots are
/// first-write-wins `OnceLock`s shared across a factory (gap-analysis 15 §1). See
/// [`crate::sessions`] for what that costs.
#[derive(Clone)]
pub struct AcpConnection {
    /// Set once by `initialize`; `None` until then. See [`ClientView`].
    client: Arc<OnceLock<ClientView>>,
    sessions: Arc<SessionManager>,
}

impl AcpConnection {
    /// Build a connection over `host`, which supplies the session-building capability that lives in
    /// the `cyrup` binary. See [`AcpHost`].
    #[must_use]
    pub fn new(host: Arc<dyn AcpHost>) -> Self {
        let sessions = Arc::new(SessionManager::new(host));
        Self {
            // One cell, two readers — see `SessionManager::client_cell`.
            client: sessions.client_cell(),
            sessions,
        }
    }

    /// What `initialize` recorded, or `None` if the client has not sent it yet.
    #[must_use]
    pub fn client(&self) -> Option<&ClientView> {
        self.client.get()
    }

    /// The one-live-session manager.
    #[must_use]
    pub fn sessions(&self) -> &Arc<SessionManager> {
        &self.sessions
    }

    /// Record what `initialize` learned. Idempotent by construction: a second `initialize` on the
    /// same connection does not change the view.
    ///
    /// **`ACP-Q`-adjacent decision, recorded here:** a second `initialize` is answered normally
    /// rather than erroring, matching pi-acp (which keeps no such state at all and simply answers
    /// again). The cost is that a client that re-negotiates capabilities mid-connection is answered
    /// with the FIRST view's gating — which is strictly safer than letting a later `initialize`
    /// widen what the agent will emit to a client that has already been told otherwise.
    fn record_client(&self, view: ClientView) -> &ClientView {
        // `get_or_init` cannot re-enter, so the closure runs at most once.
        self.client.get_or_init(|| view)
    }
}

/// Serve ACP on the process's stdin/stdout until the client closes the connection (`ACP-003`).
///
/// `agent_client_protocol::Stdio::new()` is the transport upstream had to hand-roll with
/// `ndJsonStream` because Node gives it no stdio transport; here it is one value.
///
/// # [CYRUP-DELTA] — the stdin reader thread is not cancellable
///
/// **What differs.** `Stdio::connect_to` wraps `std::io::stdin()` in `blocking::Unblock`, which
/// parks a thread of the `blocking` pool inside `read(2)`. A clean EOF returns from that read and
/// the connection ends normally (verified against the Architecture phase's probe), but a teardown
/// initiated while stdin is still open cannot cancel the parked thread.
///
/// **What it costs.** Any teardown path other than EOF must simply **return from `main`** rather
/// than await the reader — a `join` on it would hang until the client happened to write. `ACP-005`
/// (EOF) and `ACP-023` (signals) are both written to that rule, and this is why `ACP-023`'s
/// watcher exits the process itself rather than unwinding through here.
///
/// # Errors
///
/// [`AcpError::Transport`] for a real transport fault. A client that closes the pipe is **not** a
/// fault — see `crate::run_acp`, which is the function `crates/cyrup/src/run.rs` calls, and which
/// applies `ACP-004`'s broken-pipe rule before this error escapes.
pub async fn serve_stdio(host: Arc<dyn AcpHost>) -> Result<(), AcpError> {
    serve(host, agent_client_protocol::Stdio::new()).await
}

/// [`serve_stdio`] over an arbitrary transport, so `cyrup-it` can drive a pipe pair without
/// spawning a process and the handler table can be exercised in one place.
pub async fn serve(
    host: Arc<dyn AcpHost>,
    transport: impl ConnectTo<Agent> + 'static,
) -> Result<(), AcpError> {
    let conn = AcpConnection::new(host);

    // The builder chain. Every handler below is registered for the life of the connection; see the
    // module docs for why a missing one is a hang rather than a `method_not_found`.
    let init = conn.clone();
    let authenticate = conn.clone();
    let new_session = conn.clone();
    let load_session = conn.clone();
    let list_sessions = conn.clone();
    let delete_session = conn.clone();
    let prompt = conn.clone();
    let set_mode = conn.clone();
    let set_config = conn.clone();
    let cancel = conn.clone();

    Agent
        .builder()
        .name("cyrup-acp")
        // ---- initialize ---------------------------------------------------------------------
        .on_receive_request(
            async move |req: InitializeRequest, responder, _cx: ConnectionTo<Client>| {
                let view = init.record_client(ClientView::from_request(&req)).clone();
                // `ACP-053` — the env read is the one impure part of the advertisement, so it is
                // performed HERE and the predicate that judges it
                // ([`crate::config_options::embedded_context_enabled`]) stays pure and
                // table-testable.
                let embedded_context = crate::config_options::embedded_context_enabled(
                    std::env::var(crate::config_options::EMBEDDED_CONTEXT_ENV)
                        .ok()
                        .as_deref(),
                );
                responder.respond(
                    InitializeResponse::new(view.protocol_version)
                        // `ACP-052` / `ACP-053` — the four capability blocks.
                        .agent_capabilities(crate::config_options::agent_capabilities(
                            embedded_context,
                        ))
                        // `ACP-010` / `ACP-011` / `ACP-012` / `ACP-054` — one terminal method, plus
                        // the legacy `_meta["terminal-auth"]` shim gated on the strict probe and
                        // suppressed by the typed `auth.terminal` negotiation.
                        .auth_methods(crate::config_options::auth_methods(&view))
                        .agent_info(Implementation::new(AGENT_NAME, AGENT_VERSION)),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        // ---- authenticate -------------------------------------------------------------------
        .on_receive_request(
            async move |_req: AuthenticateRequest, responder, _cx: ConnectionTo<Client>| {
                // `ACP-014` — a successful no-op, answered inline because it is short and
                // non-blocking (unlike `session/new` and `session/prompt`). Port of pi-acp v0.0.33
                // `agent.ts`'s `authenticate`, which ignores its params INCLUDING `methodId` and
                // returns success: terminal auth happens out of band, so by the time a client calls
                // this there is nothing to do. **It must not error** — Zed calls it after the
                // terminal flow and an error reads as a failed login.
                let _ = &authenticate;
                responder.respond(AuthenticateResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        // ---- session/new --------------------------------------------------------------------
        .on_receive_request(
            async move |req: NewSessionRequest, responder, cx: ConnectionTo<Client>| {
                let sessions = Arc::clone(new_session.sessions());
                sessions.attach(&cx);
                let view = new_session.client().cloned();
                // `ACP-057` — build OFF the dispatch loop. Awaiting the build inline blocks every
                // other method, including `session/cancel`, for the whole of it.
                let out = cx.clone();
                cx.spawn(async move {
                    let outcome = sessions.new_session(&req, view.as_ref()).await;
                    crate::respond_then_notify(outcome, responder, &out)
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        // ---- session/load -------------------------------------------------------------------
        .on_receive_request(
            async move |req: LoadSessionRequest, responder, cx: ConnectionTo<Client>| {
                // `ACP-217` — the replay must be on the wire BEFORE the response and the command
                // advertisement AFTER it, which `crate::respond_then_notify` cannot express (it
                // writes the response first, by construction, which is right for `session/new`).
                // `handle_load` is `session/load`'s own driver; see its doc.
                let sessions = Arc::clone(load_session.sessions());
                sessions.attach(&cx);
                sessions.handle_load(req, responder, cx)
            },
            agent_client_protocol::on_receive_request!(),
        )
        // ---- session/list -------------------------------------------------------------------
        .on_receive_request(
            async move |req: ListSessionsRequest, responder, cx: ConnectionTo<Client>| {
                let sessions = Arc::clone(list_sessions.sessions());
                sessions.attach(&cx);
                let out = cx.clone();
                cx.spawn(async move {
                    let outcome = sessions.list_sessions(&req).await;
                    crate::respond_then_notify(outcome, responder, &out)
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        // ---- session/delete -----------------------------------------------------------------
        .on_receive_request(
            async move |req: DeleteSessionRequest, responder, cx: ConnectionTo<Client>| {
                let sessions = Arc::clone(delete_session.sessions());
                sessions.attach(&cx);
                let out = cx.clone();
                cx.spawn(async move {
                    let outcome = sessions.delete_session(&req).await;
                    crate::respond_then_notify(outcome, responder, &out)
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        // ---- session/prompt -----------------------------------------------------------------
        .on_receive_request(
            async move |req: PromptRequest, responder, cx: ConnectionTo<Client>| {
                // `ACP-121`/`ACP-153` — the responder is MOVED into the turn, which owns it until
                // `AgentSettled`. Nothing here awaits the turn: `ACP-123`'s interleaving test
                // asserts that a `session/cancel` issued straight after this is dispatched BEFORE
                // the prompt response, which is only true if this handler returns immediately.
                let sessions = Arc::clone(prompt.sessions());
                sessions.attach(&cx);
                sessions.dispatch_prompt(req, responder, cx)
            },
            agent_client_protocol::on_receive_request!(),
        )
        // ---- session/set_mode ---------------------------------------------------------------
        .on_receive_request(
            async move |req: SetSessionModeRequest, responder, cx: ConnectionTo<Client>| {
                let sessions = Arc::clone(set_mode.sessions());
                sessions.attach(&cx);
                // `ACP-079` — the setters do real blocking work and must leave the dispatch loop.
                let out = cx.clone();
                cx.spawn(async move {
                    let outcome = sessions.set_mode(&req).await;
                    crate::respond_then_notify(outcome, responder, &out)
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        // ---- session/set_config_option ------------------------------------------------------
        .on_receive_request(
            async move |req: SetSessionConfigOptionRequest, responder, cx: ConnectionTo<Client>| {
                let sessions = Arc::clone(set_config.sessions());
                sessions.attach(&cx);
                let out = cx.clone();
                cx.spawn(async move {
                    let outcome = sessions.set_config_option(&req).await;
                    crate::respond_then_notify(outcome, responder, &out)
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        // ---- session/cancel (a notification, not a request) ----------------------------------
        .on_receive_notification(
            async move |notif: CancelNotification, _cx: ConnectionTo<Client>| {
                // `ACP-123` — cancel is idempotent and never answers anything itself; the
                // `stopReason: "cancelled"` is produced by the turn's own settle (`ACP-121`).
                cancel.sessions().request_cancel(&notif.session_id);
                Ok::<_, agent_client_protocol::Error>(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(transport)
        .await
        .map_err(AcpError::Transport)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{AuthCapabilities, ClientCapabilities, Meta};

    /// ACP-050 — every requested version maps to V1, and the function is total over `u16`.
    #[test]
    fn every_requested_protocol_version_clamps_to_v1() {
        for raw in [0u16, 1, 2, 65535] {
            assert_eq!(
                clamp_protocol_version(ProtocolVersion::from(raw)),
                ProtocolVersion::V1,
                "requested {raw}"
            );
        }
        assert_eq!(ProtocolVersion::V1.as_u16(), 1);
    }

    /// ACP-051 — `agentInfo` is compile-time identity, with no filesystem read and no fallback.
    #[test]
    fn agent_info_is_compile_time_identity() {
        assert_eq!(
            AGENT_NAME, "cyrup",
            "ACP-051's verify: the product, not the crate"
        );
        assert!(!AGENT_VERSION.is_empty());
    }

    /// ACP-012 — the `_meta["terminal-auth"]` probe is a STRICT `=== true`, and the typed
    /// `auth.terminal` capability is read independently of it.
    #[test]
    fn the_terminal_auth_probe_is_strict() {
        let strict = |value: serde_json::Value| {
            let mut meta = Meta::new();
            meta.insert("terminal-auth".into(), value);
            let req = InitializeRequest::new(ProtocolVersion::V1)
                .client_capabilities(ClientCapabilities::new().meta(meta));
            ClientView::from_request(&req).terminal_auth_meta
        };
        assert!(strict(serde_json::json!(true)));
        assert!(!strict(serde_json::json!(1)));
        assert!(!strict(serde_json::json!("true")));
        assert!(!strict(serde_json::json!(false)));
        assert!(!strict(serde_json::json!(null)));

        // Absent entirely.
        let bare = InitializeRequest::new(ProtocolVersion::V1);
        let view = ClientView::from_request(&bare);
        assert!(!view.terminal_auth_meta);
        assert!(!view.auth_terminal);

        // The typed 2.1.0 negotiation, which is what the `_meta` hack stood in for.
        let typed = InitializeRequest::new(ProtocolVersion::V1).client_capabilities(
            ClientCapabilities::new().auth(AuthCapabilities::new().terminal(true)),
        );
        let view = ClientView::from_request(&typed);
        assert!(view.auth_terminal);
        assert!(!view.terminal_auth_meta, "the two are independent");
    }

    /// The `OnceLock` ADR-0028 §5 prescribes instead of connection typestate: unset before
    /// `initialize`, set once after, and a second `initialize` does not widen it.
    #[test]
    fn the_client_view_is_set_once_and_never_widened() {
        let conn = AcpConnection::new(crate::sessions::null_host());
        assert!(conn.client().is_none(), "unset before initialize");

        let first = InitializeRequest::new(ProtocolVersion::V1);
        conn.record_client(ClientView::from_request(&first));
        assert_eq!(conn.client().map(|v| v.auth_terminal), Some(false));

        let second = InitializeRequest::new(ProtocolVersion::V1).client_capabilities(
            ClientCapabilities::new().auth(AuthCapabilities::new().terminal(true)),
        );
        conn.record_client(ClientView::from_request(&second));
        assert_eq!(
            conn.client().map(|v| v.auth_terminal),
            Some(false),
            "a later initialize must not widen what the agent will emit"
        );
    }
}
