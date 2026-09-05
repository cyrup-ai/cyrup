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
use super::routing::SessionKey;
use super::state::BrokerState;

/// A process-unique extension-state directory for a test broker. Never the real
/// `<intercomDir>/extension-state`: a unit test must not read or write the developer's own state.
pub(super) fn test_extension_state_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "cyrup-intercom-extension-state-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ))
}

pub(super) fn make_state() -> BrokerState {
    BrokerState::new(30_000, Arc::new(Notify::new()), test_extension_state_dir())
}

pub(super) fn make_tx() -> UnboundedSender<Vec<u8>> {
    let (tx, _rx) = mpsc::unbounded_channel::<Vec<u8>>();
    tx
}

/// Register `id` on `conn_id`, in `scope` (ICOM-055) or unscoped when `None`.
///
/// `scopeId` is emitted only when `Some`, so an unscoped registration produces exactly the frame
/// this fixture produced before scopes existed — the same conditional spread upstream's client uses
/// (`...(scopeId ? { scopeId } : {})`, `v0.13.0 broker/client.ts:291`).
pub(super) fn register(
    state: &mut BrokerState,
    conn_id: u64,
    session_key: &mut Option<SessionKey>,
    id: &str,
    scope: Option<&str>,
) {
    let tx = make_tx();
    let mut value = json!({
        "type": "register",
        "sessionId": id,
        "session": {
            "cwd": "/tmp", "model": "test-model", "pid": 1, "startedAt": 0, "lastActivity": 0,
        }
    });
    if let Some(scope) = scope {
        value["scopeId"] = json!(scope);
    }
    let result = state.handle_register(conn_id, &tx, &value, session_key, 0);
    assert!(matches!(result.outcome, FrameOutcome::Continue));
}

/// Decode every queued frame on `rx` as JSON, dropping the 4-byte length prefix.
pub(super) fn payloads(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
) -> Vec<serde_json::Value> {
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
    session_key: &mut Option<SessionKey>,
    tx: &UnboundedSender<Vec<u8>>,
    id: &str,
    name: &str,
    cwd: &str,
    now: u64,
) {
    register_named_in_scope(state, conn_id, session_key, tx, id, name, cwd, None, now);
}

/// [`register_named`] with an explicit routing scope (ICOM-055).
#[allow(clippy::too_many_arguments)]
pub(super) fn register_named_in_scope(
    state: &mut BrokerState,
    conn_id: u64,
    session_key: &mut Option<SessionKey>,
    tx: &UnboundedSender<Vec<u8>>,
    id: &str,
    name: &str,
    cwd: &str,
    scope: Option<&str>,
    now: u64,
) {
    let mut value = json!({
        "type": "register",
        "sessionId": id,
        "session": {
            "name": name, "cwd": cwd, "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0,
        }
    });
    if let Some(scope) = scope {
        value["scopeId"] = json!(scope);
    }
    let result = state.handle_register(conn_id, tx, &value, session_key, now);
    assert!(matches!(result.outcome, FrameOutcome::Continue));
}

pub(super) fn send_frame(
    state: &mut BrokerState,
    conn_id: u64,
    tx: &UnboundedSender<Vec<u8>>,
    sid: &mut Option<SessionKey>,
    to: &str,
    message: serde_json::Value,
    now: u64,
) {
    state.handle_frame(
        conn_id,
        tx,
        &json!({ "type": "send", "to": to, "message": message }),
        sid,
        now,
    );
}
