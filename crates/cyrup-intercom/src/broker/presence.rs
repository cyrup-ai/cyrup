//! The `presence` frame and its context-usage tri-state
//! (`v0.9.2 broker/broker.ts:900-960`).
//!
//! [`BrokerState::handle_presence`] broadcasts on any change, and on an unchanged `presence` frame
//! only once [`super::limits::PRESENCE_HEARTBEAT_MS`] has passed since that session's last
//! broadcast — the coalescing is frame-driven, not timed. [`apply_presence_context`] ports the
//! absent/`null`/number tri-state that decides whether a field is left alone, cleared, or set — and
//! whether that counts as a change worth broadcasting.

use crate::transport::protocol::BrokerMessage;

use super::frame::FrameResult;
use super::limits::PRESENCE_HEARTBEAT_MS;
use super::routing::SessionKey;
use super::state::BrokerState;

impl BrokerState {
    pub(super) fn handle_presence(
        &mut self,
        conn_id: u64,
        value: &serde_json::Value,
        session_key: &Option<SessionKey>,
        now: u64,
    ) -> FrameResult {
        let Some(current_key) = session_key.clone() else {
            return FrameResult::protocol_error();
        };
        // OWNERSHIP FIRST. Every `throw new Error("Invalid presence …")` upstream is nested INSIDE
        // `if (session?.socket === socket) { … }` (`v0.10.1 broker/broker.ts:763-805`, guard at
        // `:765`), so a NON-OWNING socket's malformed presence is ignored, not fatal. Running the
        // type checks first killed a superseded socket's late malformed frame as a protocol error;
        // the reconnect ladder deliberately re-offers the previous session id, so takeover races are
        // a live path, not a theoretical one.
        let Some(session) = self
            .sessions
            .get_mut(&current_key)
            .filter(|s| s.conn_id == conn_id)
        else {
            return FrameResult::cont();
        };
        // A wrong type IS fatal for the owner (`v0.9.2 broker/broker.ts:892-894`, `:901-903`,
        // `:910-912`). `value.get` yields `Some(Value::Null)` for an explicit `null`, which is not a
        // string, so `null` is fatal here exactly as `typeof null === "object"` makes it upstream.
        for key in ["name", "status", "model"] {
            if let Some(v) = value.get(key)
                && !v.is_string()
            {
                return FrameResult::protocol_error();
            }
        }
        // `runtimeFallbackAlias` (`v0.10.1 broker/broker.ts:779-787`) is a BOOLEAN, and its check
        // sits inside the same ownership block.
        if let Some(v) = value.get("runtimeFallbackAlias")
            && !v.is_boolean()
        {
            return FrameResult::protocol_error();
        }
        // The context-usage trio obeys a DIFFERENT rule from the string trio above, and from
        // `isSessionInfo`'s (`v0.9.2 broker/client.ts:182-186`, where `null` is fatal). Here an
        // explicit `null` is a legal CLEAR — the value is genuinely unknown right after a
        // compaction, and carrying the stale-high one forward would be a lie — so only a value that
        // is neither `null` nor a number is fatal (`v0.9.2 broker/broker.ts:921-950`, the
        // `else if (typeof … !== "number") throw` arm at `:924`, `:934`, `:944`).
        for key in ["contextPct", "contextTokens", "contextWindow"] {
            if let Some(v) = value.get(key)
                && !v.is_null()
                && !v.is_number()
            {
                return FrameResult::protocol_error();
            }
        }
        let mut changed = false;
        if let Some(name) = value.get("name").and_then(|v| v.as_str())
            && session.info.name.as_deref() != Some(name)
        {
            session.info.name = Some(name.to_string());
            changed = true;
        }
        if let Some(status) = value.get("status").and_then(|v| v.as_str())
            && session.info.status.as_deref() != Some(status)
        {
            session.info.status = Some(status.to_string());
            changed = true;
        }
        if let Some(model) = value.get("model").and_then(|v| v.as_str())
            && session.info.model != model
        {
            session.info.model = model.to_string();
            changed = true;
        }
        // `v0.10.1 broker/broker.ts:779-787` — additive at v0.10.0 (`126875e`). The flag tells a
        // peer a chosen name from a synthesized alias, and the mailbox identity guard reads it
        // (`:1039-1047`).
        if let Some(alias) = value
            .get("runtimeFallbackAlias")
            .and_then(serde_json::Value::as_bool)
            && session.info.runtime_fallback_alias != Some(alias)
        {
            session.info.runtime_fallback_alias = Some(alias);
            changed = true;
        }
        // `v0.9.2 broker/broker.ts:921-950`, one arm per field. Kept as three explicit calls (not a
        // loop) because Rust cannot index a struct by name; the helper carries the whole tri-state.
        changed |= apply_presence_context(&mut session.info.context_pct, value.get("contextPct"));
        changed |=
            apply_presence_context(&mut session.info.context_tokens, value.get("contextTokens"));
        changed |=
            apply_presence_context(&mut session.info.context_window, value.get("contextWindow"));
        session.info.last_activity = now.into();
        let should_broadcast = changed
            || now.saturating_sub(session.last_presence_broadcast_at) >= PRESENCE_HEARTBEAT_MS;
        if should_broadcast {
            session.last_presence_broadcast_at = now;
            let info = session.info.clone();
            // `this.broadcast({ type: "presence_update", session: session.info }, currentKey,
            // session.scopeId)` (`v0.13.0 broker/broker.ts:957`).
            self.broadcast(
                &BrokerMessage::PresenceUpdate { session: info },
                Some(&current_key),
                current_key.scope.as_ref(),
            );
        }
        FrameResult::cont()
    }
}

/// Apply one `presence` context-usage field to the session's stored [`crate::transport::protocol::SessionInfo`], returning
/// whether it changed (`v0.9.2 broker/broker.ts:921-930` for `contextPct`, `:931-940` for
/// `contextTokens`, `:941-950` for `contextWindow` — three copies of one 10-line shape).
///
/// The tri-state is upstream's, verbatim, and it is NOT the rule the same three fields obey inside
/// `isSessionInfo` (`v0.9.2 broker/client.ts:182-186`), where `null` is a rejection:
///
/// * key absent (`undefined`) — leave the field untouched, no change.
/// * key present and `null` — **CLEAR** the field (`delete session.info[key]`,
///   `v0.9.2 broker/broker.ts:923`), and count that as a change only if it was set. pi's guard is
///   `if (session.info.contextPct !== undefined)`, so clearing an already-absent field is a no-op
///   and must NOT trigger a `presence_update` broadcast.
/// * key present and a number — set it, counting a change only if it differs (`:926-929`).
///
/// Anything else is unreachable: `handle_presence` has already returned
/// [`FrameResult::protocol_error`] for it, matching pi's `throw` at `:924`/`:934`/`:944`. The arm
/// is written as a no-op rather than a `panic!`/`unreachable!` so a future refactor that drops the
/// pre-validation degrades to "ignored" instead of taking the whole broker down.
fn apply_presence_context(
    slot: &mut Option<serde_json::Number>,
    incoming: Option<&serde_json::Value>,
) -> bool {
    match incoming {
        None => false,
        Some(serde_json::Value::Null) => slot.take().is_some(),
        Some(serde_json::Value::Number(n)) => {
            if slot.as_ref() == Some(n) {
                false
            } else {
                *slot = Some(n.clone());
                true
            }
        }
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::super::frame::FrameOutcome;
    use super::super::state::BrokerState;
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Notify;

    /// R4 — the `presence` tri-state, arm by arm (`v0.9.2 broker/broker.ts:921-950`). The
    /// no-op-clear arm (`:923`'s `if (session.info.contextPct !== undefined)`) is asserted here
    /// rather than over the socket because `PRESENCE_HEARTBEAT_MS` is 1 s, so "no broadcast" is not
    /// a claim a live probe can make without racing the heartbeat.
    #[test]
    fn apply_presence_context_matches_pis_tristate() {
        // undefined — untouched, no change.
        let mut slot = Some(serde_json::Number::from(42));
        assert!(!apply_presence_context(&mut slot, None));
        assert_eq!(slot, Some(serde_json::Number::from(42)));

        // null on a SET field — cleared, and that IS a change (`v0.9.2 broker/broker.ts:923`).
        assert!(apply_presence_context(&mut slot, Some(&json!(null))));
        assert_eq!(slot, None);

        // null on an ALREADY-ABSENT field — still cleared, but NOT a change, so it must not
        // trigger a `presence_update` broadcast.
        assert!(!apply_presence_context(&mut slot, Some(&json!(null))));
        assert_eq!(slot, None);

        // a number on an absent field — set, and a change.
        assert!(apply_presence_context(&mut slot, Some(&json!(7))));
        assert_eq!(slot, Some(serde_json::Number::from(7)));

        // the SAME number again — set, but not a change (`v0.9.2 broker/broker.ts:926`).
        assert!(!apply_presence_context(&mut slot, Some(&json!(7))));

        // a different number — a change.
        assert!(apply_presence_context(&mut slot, Some(&json!(8))));
        assert_eq!(slot, Some(serde_json::Number::from(8)));

        // a fractional number is a `number` upstream too, so it is accepted, not coerced.
        assert!(apply_presence_context(&mut slot, Some(&json!(8.5))));
        assert_eq!(slot, Some(serde_json::Number::from_f64(8.5).unwrap()));
    }
    /// The whole handler, driven through `handle_frame`: a wrong-typed context field destroys the
    /// connection (`v0.9.2 broker/broker.ts:924,934,944`) while `null` and a number do not
    /// (`:922-923,926-929`) — the two halves that must not be collapsed into one rule.
    #[test]
    fn handle_presence_rejects_non_number_context_but_accepts_null_and_numbers() {
        fn drive(patch: serde_json::Value) -> FrameOutcome {
            let mut state = BrokerState::new(
                30_000,
                Arc::new(Notify::new()),
                super::super::test_support::test_extension_state_dir(),
            );
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let mut sid = None;
            state.handle_frame(
                1,
                &tx,
                &json!({
                    "type": "register", "sessionId": "s1",
                    "session": { "cwd": "/w", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0 },
                }),
                &mut sid,
                1_000,
            );
            let mut frame = json!({ "type": "presence" });
            if let (Some(dst), Some(src)) = (frame.as_object_mut(), patch.as_object()) {
                for (k, v) in src {
                    dst.insert(k.clone(), v.clone());
                }
            }
            state.handle_frame(1, &tx, &frame, &mut sid, 2_000).outcome
        }

        for key in ["contextPct", "contextTokens", "contextWindow"] {
            for bad in [json!("42"), json!({}), json!([]), json!(true)] {
                assert!(
                    matches!(drive(json!({ key: bad })), FrameOutcome::ProtocolError),
                    "`presence.{key} = {bad}` must destroy the connection"
                );
            }
            // POSITIVE CONTROL: null (clear) and a number (set) are both legal here.
            for good in [json!(null), json!(0), json!(42), json!(99.5)] {
                assert!(
                    matches!(drive(json!({ key: good })), FrameOutcome::Continue),
                    "`presence.{key} = {good}` must be served, not disconnected"
                );
            }
        }
    }
    /// ICOM-014 — every `throw new Error("Invalid presence …")` upstream is nested INSIDE
    /// `if (session?.socket === socket)` (`v0.10.1 broker/broker.ts:763-805`, guard at `:765`), so a
    /// NON-OWNING socket's malformed presence is IGNORED, not fatal.
    ///
    /// Red against the pre-fix ordering: the type-check loops ran before the ownership filter, so a
    /// superseded socket sending a late `{"name": 5}` had its connection destroyed as a protocol
    /// error. The reconnect ladder deliberately re-offers the previous session id, so a takeover
    /// race is a live path.
    #[test]
    fn a_non_owning_socket_s_malformed_presence_is_ignored_not_fatal() {
        let mut state = BrokerState::new(
            30_000,
            Arc::new(Notify::new()),
            super::super::test_support::test_extension_state_dir(),
        );
        let (tx_owner, _rx_owner) = tokio::sync::mpsc::unbounded_channel();
        let (tx_loser, _rx_loser) = tokio::sync::mpsc::unbounded_channel();

        // conn 1 registers as `s1`, then conn 2 takes the id over — conn 1 is now the LOSER.
        let mut sid_loser = None;
        let reg = json!({
            "type": "register", "sessionId": "s1",
            "session": { "cwd": "/w", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0 },
        });
        state.handle_frame(1, &tx_loser, &reg, &mut sid_loser, 1_000);
        let mut sid_owner = None;
        state.handle_frame(2, &tx_owner, &reg, &mut sid_owner, 1_001);

        // The loser still believes it is `s1`, and sends a malformed presence.
        let bad = json!({ "type": "presence", "name": 5 });
        assert!(
            matches!(
                state
                    .handle_frame(1, &tx_loser, &bad, &mut sid_loser, 2_000)
                    .outcome,
                FrameOutcome::Continue
            ),
            "a non-owning socket's malformed presence must be ignored, not a protocol error"
        );

        // POSITIVE CONTROL: the OWNER sending the same frame is still fatal
        // (`v0.10.1 broker/broker.ts:766-768`).
        assert!(
            matches!(
                state
                    .handle_frame(2, &tx_owner, &bad, &mut sid_owner, 2_001)
                    .outcome,
                FrameOutcome::ProtocolError
            ),
            "the owner's malformed presence is still fatal"
        );
    }
    /// ICOM-041 — `runtimeFallbackAlias` is a BOOLEAN checked inside the ownership block
    /// (`v0.10.1 broker/broker.ts:779-787`) and applied to the stored `SessionInfo`.
    #[test]
    fn presence_carries_runtime_fallback_alias() {
        let mut state = BrokerState::new(
            30_000,
            Arc::new(Notify::new()),
            super::super::test_support::test_extension_state_dir(),
        );
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut sid = None;
        state.handle_frame(
            1,
            &tx,
            &json!({
                "type": "register", "sessionId": "s1",
                "session": { "cwd": "/w", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0,
                             "name": "subagent-chat-0192", "runtimeFallbackAlias": true },
            }),
            &mut sid,
            1_000,
        );
        // `v0.10.1 broker/broker.ts:358` copies it off the registration.
        assert_eq!(
            state
                .sessions
                .get(&SessionKey::unscoped("s1".to_string()))
                .and_then(|s| s.info.runtime_fallback_alias),
            Some(true),
            "register must carry the flag onto the stored SessionInfo"
        );

        // A presence frame flips it (`:779-787`).
        assert!(matches!(
            state
                .handle_frame(
                    1,
                    &tx,
                    &json!({ "type": "presence", "runtimeFallbackAlias": false }),
                    &mut sid,
                    2_000
                )
                .outcome,
            FrameOutcome::Continue
        ));
        assert_eq!(
            state
                .sessions
                .get(&SessionKey::unscoped("s1".to_string()))
                .and_then(|s| s.info.runtime_fallback_alias),
            Some(false)
        );

        // A non-boolean is fatal, like every other presence type check.
        assert!(matches!(
            state
                .handle_frame(
                    1,
                    &tx,
                    &json!({ "type": "presence", "runtimeFallbackAlias": "yes" }),
                    &mut sid,
                    3_000
                )
                .outcome,
            FrameOutcome::ProtocolError
        ));
    }
}
