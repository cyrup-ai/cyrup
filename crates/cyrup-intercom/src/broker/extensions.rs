//! The extension-bus frames and the shared `extensions`-field validators
//! (`v0.9.2 broker/broker.ts:446-456,551-585,961-969,1159-1182`).
//!
//! cyrup does not implement the bus, so it never advertises `EXTENSION_BUS_FEATURE` on `registered`
//! and a conforming pi client never sends these frames. A non-conforming peer on a socket every
//! process on the box can reach still can, so each handler ports upstream's validation prefix and
//! upstream's miss branch; the bus EFFECTS stay unported.
//!
//! [`extensions_field_is_valid`] lives here rather than in a validation grab-bag because it IS the
//! `extensions` field's guard — upstream shares it verbatim between `case "register"` and
//! `case "extension_capabilities_update"`, which is why `super::session` imports it.

use tokio::sync::mpsc::UnboundedSender;

use crate::transport::protocol::{BrokerMessage, ExtensionCapability};

use super::frame::{FrameResult, send_msg};
use super::js::js_string_or_empty;
use super::limits::MAX_EXTENSIONS_PER_SESSION;
use super::state::BrokerState;

/// pi's `extensions` field guard, shared verbatim by `case "register"`
/// (`v0.9.2 broker/broker.ts:446-456`) and `case "extension_capabilities_update"`
/// (`v0.9.2 broker/broker.ts:559-567`): the value must be an ARRAY of at most
/// [`MAX_EXTENSIONS_PER_SESSION`] entries, each passing `validateExtensionCapability`
/// (`v0.9.2 broker/broker.ts:1159-1168`). Anything else `throw`s, i.e. destroys the socket.
///
/// The per-entry decode into [`ExtensionCapability`] reproduces upstream's
/// `typeof c.namespace !== "string" || typeof c.ownerEligible !== "boolean"` check *and* its
/// rejection of an array-shaped entry (`[]["namespace"]` is `undefined`), the latter because that
/// struct is `[MAP-ONLY]` — see `crate::transport::protocol`.
pub(super) fn extensions_field_is_valid(extensions: &serde_json::Value) -> bool {
    let Some(items) = extensions.as_array() else {
        return false;
    };
    if items.len() > MAX_EXTENSIONS_PER_SESSION {
        return false;
    }
    items.iter().all(|item| {
        serde_json::from_value::<ExtensionCapability>(item.clone())
            .is_ok_and(|cap| namespace_is_valid(&cap.namespace))
    })
}

/// `validateNamespace` (`v0.9.2 broker/broker.ts:1170-1182`): `^[a-z0-9][a-z0-9._/-]{0,63}$`, with
/// the length bound checked first.
///
/// [CYRUP-DELTA] pi's `ns.length` counts UTF-16 code units and this counts `char`s; the two can
/// disagree only for non-ASCII input, which the character test rejects on both sides anyway, so the
/// accepted set is identical.
fn namespace_is_valid(ns: &str) -> bool {
    if ns.is_empty() || ns.chars().count() > 64 {
        return false;
    }
    let mut chars = ns.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '/' | '-'))
}

impl BrokerState {
    /// `case "extension_capabilities_update"` (`v0.9.2 broker/broker.ts:551-585`).
    ///
    /// cyrup does not implement the extension bus, so pi's *effects* — `session.extensions = …`,
    /// `recomputeNamespaceOwners()`, and the `extension_owner`/`extension_state` replies
    /// (`v0.9.2 broker/broker.ts:568-585`) — stay unported and the frame is ignored, exactly like
    /// `extension_publish`/`extension_state_commit` above.
    ///
    /// pi's **validation prefix** (`v0.9.2 broker/broker.ts:559-567`) is ported, though, because it
    /// runs before any of that and every one of its failures is a `throw` → `socket.destroy`
    /// (`framing.ts:44-51`). Without it cyrup accepts an `extensions` payload pi kills the
    /// connection over — including the array-shaped `[["ns", true]]`, which serde would otherwise
    /// fill into [`ExtensionCapability`] positionally — on a socket every process on the box can
    /// reach. Ignoring a *well-formed* frame is a survivability choice; ignoring a malformed one is
    /// an input-validation hole.
    ///
    /// The "before register" throw at `:552-554` is covered by the shared pre-registration guard in
    /// [`Self::handle_frame`]. The "session not found" throw at `:555-558` is ported below: pi's
    /// `session.socket !== socket` is [`Self::session_owns_connection`] here, and it IS reachable —
    /// an identity takeover (`handle_register`'s `close.notify_one()`) reassigns the id to the new
    /// connection before the superseded reader task observes the notify, so a frame already in the
    /// old socket's buffer arrives with a stale `conn_id`. Upstream destroys that connection; so
    /// does this.
    pub(super) fn handle_extension_capabilities_update(
        &self,
        conn_id: u64,
        value: &serde_json::Value,
        session_id: &Option<String>,
    ) -> FrameResult {
        let Some(current_id) = session_id.as_deref() else {
            return FrameResult::protocol_error();
        };
        // `throw new Error("Extension capability session not found")`
        // (`v0.9.2 broker/broker.ts:556-558`) — fatal, unlike the two handlers below, which answer.
        if !self.session_owns_connection(current_id, conn_id) {
            return FrameResult::protocol_error();
        }
        // `!Array.isArray(undefined)` is true, so a missing field throws upstream too
        // (`v0.9.2 broker/broker.ts:559-562`).
        let extensions = value.get("extensions").unwrap_or(&serde_json::Value::Null);
        if !extensions_field_is_valid(extensions) {
            return FrameResult::protocol_error();
        }
        tracing::debug!(
            "intercom broker: extension_capabilities_update ignored (bus not implemented)"
        );
        FrameResult::cont()
    }

    /// pi's `session.socket !== socket` guard (`v0.9.2 broker/broker.ts:556,1272,1368`), expressed
    /// against cyrup's connection ids: the session must exist AND still be owned by the connection
    /// the frame arrived on.
    fn session_owns_connection(&self, session_id: &str, conn_id: u64) -> bool {
        self.sessions.get(session_id).map(|s| s.conn_id) == Some(conn_id)
    }

    /// `case "extension_publish"` (`v0.9.2 broker/broker.ts:961-964`) → `handleExtensionPublish`
    /// (`v0.9.2 broker/broker.ts:1262-1356`).
    ///
    /// cyrup does not implement the extension bus, which means it never records
    /// `session.extensions` — pi's `case "extension_capabilities_update"` assignment at
    /// `v0.9.2 broker/broker.ts:568` is exactly the effect this crate leaves unported. So
    /// `!session.extensions?.length` (`:1277`) is **unconditionally true** here and pi's own
    /// not-advertised miss branch is the whole handler: `error`
    /// `"Session has not advertised extension capability"` (`:1278`). Everything past `:1281` —
    /// the namespace / audience / payload-size checks and the fan-out — is unreachable while the
    /// bus is unported, so porting it would be dead code guessing at state that cannot exist.
    ///
    /// Answering matters for the same reason it did for `cancel_message`: pi's client resolves an
    /// extension publish only on a broker frame, so a silent drop strands the caller. Ignoring the
    /// frame outright was also LOOSER than upstream, on a socket every process on the box can open.
    pub(super) fn handle_extension_publish(
        &self,
        conn_id: u64,
        self_tx: &UnboundedSender<Vec<u8>>,
        session_id: &Option<String>,
    ) -> FrameResult {
        let Some(current_id) = session_id.as_deref() else {
            return FrameResult::protocol_error();
        };
        // `!session || session.socket !== socket` → `error: "Session not found"`, and NOT fatal
        // (`v0.9.2 broker/broker.ts:1271-1275`).
        let error = if self.session_owns_connection(current_id, conn_id) {
            "Session has not advertised extension capability"
        } else {
            "Session not found"
        };
        send_msg(self_tx, &BrokerMessage::Error { error: error.to_string() });
        FrameResult::cont()
    }

    /// `case "extension_state_commit"` (`v0.9.2 broker/broker.ts:966-969`) →
    /// `handleExtensionStateCommit` (`v0.9.2 broker/broker.ts:1358-1495`).
    ///
    /// Same shape as [`Self::handle_extension_publish`], with pi's different refusal frame: every
    /// exit from this handler writes an `extension_state_result`, so the two miss branches at
    /// `v0.9.2 broker/broker.ts:1367-1388` are `committed: false`, `revision: 0` and a reason —
    /// `"Session not found"` (`:1374`) or `"Session has not advertised extension capability"`
    /// (`:1385`). With the bus unported the second is unconditional, exactly as above.
    ///
    /// Both branches echo `String(msg.namespace || "")` (`:1371`, `:1382`) — the raw, not-yet-
    /// type-checked value — so [`js_string_or_empty`] reproduces that coercion rather than
    /// requiring a string.
    ///
    /// A commit is a promise upstream; dropping it silently hangs the committer forever, which is
    /// the same defect the `cancel_message` port already fixed.
    pub(super) fn handle_extension_state_commit(
        &self,
        conn_id: u64,
        self_tx: &UnboundedSender<Vec<u8>>,
        value: &serde_json::Value,
        session_id: &Option<String>,
    ) -> FrameResult {
        let Some(current_id) = session_id.as_deref() else {
            return FrameResult::protocol_error();
        };
        let reason = if self.session_owns_connection(current_id, conn_id) {
            "Session has not advertised extension capability"
        } else {
            "Session not found"
        };
        send_msg(self_tx, &BrokerMessage::ExtensionStateResult {
            namespace: js_string_or_empty(value.get("namespace")),
            committed: false,
            revision: 0,
            reason: Some(reason.to_string()),
        });
        FrameResult::cont()
    }

}
