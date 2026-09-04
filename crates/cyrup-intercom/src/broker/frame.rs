//! What a frame handler returns, and how it queues a reply.
//!
//! [`FrameOutcome`] is pi's three post-`onMessage` states (keep reading / destroy after replying /
//! destroy on a `throw`, `broker.ts:217-230`); [`FrameResult`] carries the two extra bits the reader
//! task needs to act on OUTSIDE the state lock. [`send_msg`] is pi's fire-and-forget `socket.write`.
//!
//! Split out of `broker/mod.rs` so every handler module can depend on this plumbing without
//! depending on the state layout.

use tokio::sync::mpsc::UnboundedSender;

use crate::transport::framing::encode_json;
use crate::transport::protocol::BrokerMessage;

/// What the reader task should do after one frame.
pub(super) enum FrameOutcome {
    /// Keep reading.
    Continue,
    /// Reply already queued; destroy this connection (cap/rate-limit, `broker.ts:220,355`).
    CloseSelf,
    /// A malformed/illegal frame — destroy this connection (pi `throw` → `socket.destroy`).
    ProtocolError,
}

/// The result of handling one frame: what to do next + whether a session left (so the reader can
/// schedule the auto-shutdown check outside the state lock).
pub(super) struct FrameResult {
    pub(super) outcome: FrameOutcome,
    pub(super) schedule_shutdown: bool,
    /// Whether this frame transitioned the connection back to unregistered (re-arm reg timeout).
    pub(super) rearmed_registration: bool,
}

impl FrameResult {
    pub(super) fn cont() -> Self {
        Self {
            outcome: FrameOutcome::Continue,
            schedule_shutdown: false,
            rearmed_registration: false,
        }
    }
    pub(super) fn close_self() -> Self {
        Self {
            outcome: FrameOutcome::CloseSelf,
            schedule_shutdown: false,
            rearmed_registration: false,
        }
    }
    pub(super) fn protocol_error() -> Self {
        Self {
            outcome: FrameOutcome::ProtocolError,
            schedule_shutdown: false,
            rearmed_registration: false,
        }
    }
}

/// Encode `msg` and queue it on `tx` (best-effort; a full/closed channel drops, as pi's
/// `socket.write` is fire-and-forget).
pub(super) fn send_msg(tx: &UnboundedSender<Vec<u8>>, msg: &BrokerMessage) {
    match encode_json(msg) {
        Ok(frame) => {
            let _ = tx.send(frame);
        }
        Err(e) => tracing::warn!(error = %e, "failed to encode broker message"),
    }
}
