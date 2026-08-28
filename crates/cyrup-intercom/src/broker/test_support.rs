//! Shared fixtures for the broker's unit tests.
//!
//! `make_state`/`make_tx`/`register` existed in duplicate while the tests lived in two modules at
//! the bottom of `broker/mod.rs`; the copies were identical apart from JSON literal wrapping, so
//! they are kept once here and imported by the six `mod tests` that need them — `state`, `session`,
//! `send`, `receipts`, `mailbox` and `conn`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use serde_json::json;
use tokio::sync::Notify;
use tokio::sync::mpsc::{self, UnboundedSender};

use super::frame::FrameOutcome;
use super::state::BrokerState;

pub(super) fn make_state() -> BrokerState {
    BrokerState::new(30_000, Arc::new(Notify::new()))
}

pub(super) fn make_tx() -> UnboundedSender<Vec<u8>> {
    let (tx, _rx) = mpsc::unbounded_channel::<Vec<u8>>();
    tx
}

pub(super) fn register(state: &mut BrokerState, conn_id: u64, session_id: &mut Option<String>, id: &str) {
    let tx = make_tx();
    let value = json!({
        "type": "register",
        "sessionId": id,
        "session": {
            "cwd": "/tmp", "model": "test-model", "pid": 1, "startedAt": 0, "lastActivity": 0,
        }
    });
    let result = state.handle_register(conn_id, &tx, &value, session_id, 0);
    assert!(matches!(result.outcome, FrameOutcome::Continue));
}

/// Decode every queued frame on `rx` as JSON, dropping the 4-byte length prefix.
pub(super) fn payloads(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        out.push(
            serde_json::from_slice(frame.get(4..).unwrap_or_default())
                .expect("a broker frame is JSON"),
        );
    }
    out
}

/// Register `id` on `conn_id` with an explicit name + cwd, so the mailbox identity rules
/// (`v0.10.1 broker/broker.ts:1039-1048`) have something to match on.
// Mirrors the broker's own registration surface (`v0.10.1 broker/broker.ts:1039-1048`); the
// arity is the identity tuple it matches on, so grouping it would obscure what each field is.
#[allow(clippy::too_many_arguments)]
pub(super) fn register_named(
    state: &mut BrokerState,
    conn_id: u64,
    session_id: &mut Option<String>,
    tx: &UnboundedSender<Vec<u8>>,
    id: &str,
    name: &str,
    cwd: &str,
    now: u64,
) {
    let value = json!({
        "type": "register",
        "sessionId": id,
        "session": {
            "name": name, "cwd": cwd, "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0,
        }
    });
    let result = state.handle_register(conn_id, tx, &value, session_id, now);
    assert!(matches!(result.outcome, FrameOutcome::Continue));
}

pub(super) fn send_frame(
    state: &mut BrokerState,
    conn_id: u64,
    tx: &UnboundedSender<Vec<u8>>,
    sid: &mut Option<String>,
    to: &str,
    message: serde_json::Value,
    now: u64,
) {
    state.handle_frame(conn_id, tx, &json!({ "type": "send", "to": to, "message": message }), sid, now);
}
