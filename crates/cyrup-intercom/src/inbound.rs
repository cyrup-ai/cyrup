//! The inbound broker-message handler (`pi-intercom/index.ts:709-772` `handleIncomingMessage` +
//! `attachClientHandlers`) and the human surface it drives.
//!
//! WIRING: [`spawn_inbound_loop`] is started on `SessionStart` by [`crate::extension`] once the client
//! connects. For every inbound broker `Message` it (1) resolves an outstanding OUTBOUND ask first (the
//! process-global single slot, `index.ts:715-724`), else (2) records the inbound ask in the
//! [`ReplyTracker`](crate::reply_tracker::ReplyTracker), (3) SURFACES it to the human via
//! [`surface_incoming_message`] → `HostServices::append_entry` (the port doc §4.2/§7.2 human surface,
//! P-1 Route B), and (4) dispatches the inbound delivery policy ([`decide_inbound_policy`], pi
//! `index.ts:735-758`): an interactive session drives/steers an agent turn OVER the message
//! ([`trigger_turn_over_inbound`]), while a non-interactive session sends the sender the
//! "running in non-interactive mode" busy auto-reply ([`auto_reply_non_interactive`]). This is the
//! real production path the integration test drives with a scripted `HostServices` sink.

use std::sync::Arc;

use serde_json::json;

use crate::config::InboundTrigger;
use crate::reply_tracker::IntercomContext;
use crate::session_state::SharedIntercomState;
use crate::transport::client::{InboundEvent, IntercomClient, SendOptions};
use crate::transport::protocol::{Attachment, Message, SessionInfo, now_ms};
use crate::ui::{InlineMessage, PlainTheme};

/// The width the degraded inline card is pre-rendered at for the `append_entry` payload (cyrup has no
/// live terminal width outside a `HostCtx`; a conventional default, the port doc §4.3).
const SURFACE_CARD_WIDTH: usize = 80;
/// The reply-hint command shown when a sender expects a reply (pi `index.ts:730`).
const REPLY_HINT_COMMAND: &str = "intercom({ action: \"reply\", message: \"...\" })";
/// The custom-message type an inbound intercom message is injected under when it drives an agent turn
/// (matches the durable surface's `append_entry("intercom_message", …)` type, so the live host routes
/// both under the same kind — pi `sendMessage({customType:"intercom_message"})`, `index.ts:656`).
const INBOUND_MESSAGE_CUSTOM_TYPE: &str = "intercom_message";
/// The busy auto-reply sent back to a sender when this session is running non-interactively and
/// cannot surface the message to a human (pi's non-interactive busy reply, `index.ts:739-748`).
const NON_INTERACTIVE_BUSY_NOTICE: &str =
    "This session is running in non-interactive mode and cannot respond to messages right now.";

/// The inbound delivery policy decision (pi `handleIncomingMessage`, `index.ts:735-758`), computed
/// AFTER the durable surface (`append_entry`) from this session's static `has_ui` and the message
/// shape. Kept as a pure decision (no I/O) so the branch is unit-testable without a live host/broker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboundPolicy {
    /// Interactive session (`has_ui`): deliver the message through `HostServices::inject_message`,
    /// with `trigger` deciding whether that call also drives/steers an agent turn OVER it (pi's
    /// `shouldTriggerInboundMessage`, `index.ts:635-646`, applied at `sendIncomingMessage`'s
    /// `delivery === "trigger" && shouldTriggerInboundMessage(entry)`, `index.ts:663-665`):
    /// `config.inbound_trigger == Always` -> always `true`; `== Replies` -> `true` only when the
    /// message is itself a reply (`reply_to.is_some()`); `== Never` -> always `false` (still
    /// delivered, just without driving a turn — pi's `{ deliverAs: "followUp" }`).
    ///
    /// Still an OPEN exact-parity gap (unchanged by this fix, out of this file's scope): pi branches
    /// FIRST on `ctx.isIdle()`, not on `hasUI` — an IDLE non-interactive session is delivered here
    /// too, and a BUSY interactive session is queued (`pendingIdleMessages`, debounced) rather than
    /// steered immediately. That needs a live `HostServices::is_idle` seam (`cyrup-ext`) plus a
    /// pending-queue field on `SharedIntercomState` (`session_state.rs`), neither of which lives in
    /// this file.
    Deliver { trigger: bool },
    /// Non-interactive session (`!has_ui`) that received a fresh (non-reply) message while connected:
    /// send the sender the "running in non-interactive mode" busy auto-reply + `markReplied`
    /// (`index.ts:739-748`). Skipped for a message that is itself a reply (`reply_to.is_some()`).
    AutoReply,
    /// Nothing beyond the durable surface: a non-interactive session received a message that is itself
    /// a reply (nobody to auto-reply to), or the trigger/auto-reply preconditions are otherwise unmet.
    SurfaceOnly,
}

/// Decide the inbound delivery policy (pi `index.ts:735-758`, gated by `shouldTriggerInboundMessage`,
/// `index.ts:635-646`) from the session's static `has_ui`, the resolved `inbound_trigger` config, and
/// whether the message is itself a reply. Pure — no host/broker I/O — so the branch is directly
/// unit-testable. See [`InboundPolicy`].
#[must_use]
pub fn decide_inbound_policy(
    has_ui: bool,
    inbound_trigger: InboundTrigger,
    message: &Message,
) -> InboundPolicy {
    if has_ui {
        let trigger = match inbound_trigger {
            InboundTrigger::Always => true,
            InboundTrigger::Replies => message.reply_to.is_some(),
            InboundTrigger::Never => false,
        };
        InboundPolicy::Deliver { trigger }
    } else if message.reply_to.is_none() {
        InboundPolicy::AutoReply
    } else {
        InboundPolicy::SurfaceOnly
    }
}

/// Deliver an inbound message through the live `HostServices`, optionally driving/steering an agent
/// turn OVER it (the interactive `has_ui` branch of [`decide_inbound_policy`], pi's
/// `sendIncomingMessage(entry, "trigger")` gated by `shouldTriggerInboundMessage`): build the message
/// body and `inject_message(trigger_turn = trigger)` — the live host runs a fresh turn when idle and
/// steers onto the active run when busy, or (when `trigger` is `false`, `config.inbound_trigger`
/// having declined it) delivers the message as a non-triggering follow-up entry (pi's
/// `{ deliverAs: "followUp" }`). `display = false` because the durable card was ALREADY surfaced via
/// [`surface_incoming_message`]; this call only drives delivery/the turn, not a second visible copy.
/// A no-op (returns `false`) when no `HostServices` is bound (headless/degraded). Returns whether the
/// injection was attempted against a live host.
pub fn trigger_turn_over_inbound(
    state: &SharedIntercomState,
    from: &SessionInfo,
    message: &Message,
    trigger: bool,
) -> bool {
    let Some(services) = state.host_services() else {
        return false;
    };
    // `sendIncomingMessage` queues the turn context whenever `delivery !== "followUp"`
    // (`index.ts:651-653`) — i.e. on every call through THIS ("trigger" delivery mode) path,
    // regardless of whether `shouldTriggerInboundMessage` ultimately allows the turn-trigger itself.
    // `ReplyTracker::begin_turn` (fired on the next `turn_start`, `extension.rs`'s `HostEvent::TurnStart`
    // arm) shifts this queued context into `current_turn_context`, giving a bare
    // `intercom({action:"reply"})` (no `to`) absolute priority over the "single pending"/`to`-filter
    // fallbacks for the message that actually triggered/is steering this turn.
    state.tracker.lock().unwrap_or_else(|e| e.into_inner()).queue_turn_context(IntercomContext {
        from: from.clone(),
        message: message.clone(),
        received_at: now_ms(),
    });
    let body = build_inline_message(state, from, message).body().to_string();
    if let Err(e) = services.inject_message(&body, Some(INBOUND_MESSAGE_CUSTOM_TYPE), false, trigger) {
        tracing::warn!(error = %e, "intercom: failed to deliver an inbound message");
    }
    true
}

/// Send the "running in non-interactive mode" busy auto-reply back to the sender (the `AutoReply`
/// branch of [`decide_inbound_policy`], pi's non-interactive reply, `index.ts:739-748`): a
/// non-interactive (`!has_ui`) session that received a fresh (non-reply) message cannot involve a
/// human, so it replies [`NON_INTERACTIVE_BUSY_NOTICE`] to the sender over the live broker client —
/// correlated to the inbound message id (`reply_to`) so the sender's OWN outbound single-slot waiter
/// resolves with the notice rather than hanging — and then `markReplied`s the now-answered inbound
/// ask so it no longer shows as pending. A no-op (returns `false`) when no live client is bound
/// (disconnected/degraded). Returns whether the auto-reply was delivered.
pub async fn auto_reply_non_interactive(
    state: &SharedIntercomState,
    from: &SessionInfo,
    message: &Message,
) -> bool {
    let Some(client) = state.client() else {
        return false;
    };
    let send = client
        .send(&from.id, SendOptions {
            text: NON_INTERACTIVE_BUSY_NOTICE.to_string(),
            attachments: None,
            reply_to: Some(message.id.clone()),
            expects_reply: Some(false),
            message_id: None,
        })
        .await;
    match send {
        Ok(result) if result.delivered => {
            // markReplied (`index.ts:748`): the inbound ask is now answered — drop it from pending so
            // a later `intercom{list}`/`intercom{reply}` does not re-surface it.
            state
                .tracker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .mark_replied(&message.id);
            true
        }
        Ok(result) => {
            tracing::warn!(reason = ?result.reason, "intercom: non-interactive busy auto-reply not delivered");
            false
        }
        Err(e) => {
            tracing::warn!(error = %e, "intercom: failed to send non-interactive busy auto-reply");
            false
        }
    }
}

/// Spawn the inbound broker-event loop (`attachClientHandlers`, `index.ts:765-772`): resolve the
/// outbound single-slot waiter FIRST, else record + surface the inbound ask/message, then dispatch
/// the inbound delivery policy ([`decide_inbound_policy`]).
pub fn spawn_inbound_loop(state: Arc<SharedIntercomState>, client: Arc<IntercomClient>) {
    let mut rx = client.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(InboundEvent::Message { from, message }) => {
                    // (1) Resolve an outstanding OUTBOUND ask first (index.ts:715-724). When matched,
                    //     the message is the reply to our own ask — do NOT also surface it.
                    if state.waiter.try_deliver(&from, &message) {
                        continue;
                    }
                    // (2) Record the inbound ask (for a future `intercom{reply}` / the ClarifyChannel
                    //     correlation) and (3) surface it to the human.
                    state
                        .tracker
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .record_incoming_message(from.clone(), message.clone(), now_ms());
                    surface_incoming_message(&state, &from, &message);
                    // (4) Dispatch the inbound delivery policy (pi `handleIncomingMessage`,
                    //     `index.ts:735-758`), computed AFTER the durable surface from this session's
                    //     static `has_ui` and the message shape, then routed to the real host/broker
                    //     seam: an interactive session drives/steers a turn OVER the message; a
                    //     non-interactive one sends the sender the busy auto-reply.
                    match decide_inbound_policy(state.has_ui(), state.config.inbound_trigger, &message) {
                        InboundPolicy::Deliver { trigger } => {
                            trigger_turn_over_inbound(&state, &from, &message, trigger);
                        }
                        InboundPolicy::AutoReply => {
                            auto_reply_non_interactive(&state, &from, &message).await;
                        }
                        InboundPolicy::SurfaceOnly => {}
                    }
                }
                Ok(InboundEvent::Disconnected(_)) => break,
                Ok(_) => {} // joined/left/presence/error — presence UI is a later phase.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Format inbound attachments into the message body (pi `formatAttachments`, `index.ts:73-82`).
#[must_use]
pub fn format_attachments(attachments: &[Attachment]) -> String {
    let mut text = String::new();
    for att in attachments {
        match &att.language {
            Some(lang) => {
                text.push_str(&format!("\n\n---\n📎 {}\n~~~{lang}\n{}\n~~~", att.name, att.content));
            }
            None => {
                text.push_str(&format!("\n\n---\n📎 {}\n{}", att.name, att.content));
            }
        }
    }
    text
}

/// Build the inline card for an inbound message (pi's `entry` in `handleIncomingMessage`,
/// `index.ts:725-733`): body = text + attachment text; reply hint when the sender expects a reply and
/// `reply_hint` is on. Collapsed by default (pi opens the renderer with `!options.expanded`).
#[must_use]
pub fn build_inline_message(
    state: &SharedIntercomState,
    from: &SessionInfo,
    message: &Message,
) -> InlineMessage {
    let attachment_text = message
        .content
        .attachments
        .as_deref()
        .filter(|a| !a.is_empty())
        .map(format_attachments)
        .unwrap_or_default();
    let body_text = format!("{}{attachment_text}", message.content.text);
    let reply_command = (state.config.reply_hint && message.expects_reply == Some(true))
        .then(|| REPLY_HINT_COMMAND.to_string());
    InlineMessage {
        from: from.clone(),
        message: message.clone(),
        reply_command,
        body_text: Some(body_text),
        collapsed: true,
    }
}

/// Surface an inbound message to the human via `HostServices::append_entry("intercom_message", …)`
/// (the port doc §4.2/§7.2; pi's `sendMessage({customType:"intercom_message", …})`, `index.ts:656`).
/// The payload carries the markdown `content` (pi's message body), the pre-rendered `card` (the §4.3
/// degrade of the inline renderer), and the structured details. Returns the new entry id, or `None`
/// when no `HostServices` is bound (headless/degraded) or the append failed — never panics/blocks.
pub fn surface_incoming_message(
    state: &SharedIntercomState,
    from: &SessionInfo,
    message: &Message,
) -> Option<String> {
    let services = state.host_services()?;
    let card = build_inline_message(state, from, message);
    let payload = json!({
        "content": card.content_markdown(),
        "card": card.render(&PlainTheme, SURFACE_CARD_WIDTH),
        "from": from,
        "message": message,
        "replyCommand": card.reply_command,
        "bodyText": card.body(),
        "collapsed": card.collapsed,
    });
    match services.append_entry("intercom_message", &payload) {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!(error = %e, "intercom: failed to surface inbound message via append_entry");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use crate::config::IntercomConfig;
    use crate::transport::protocol::{AttachmentKind, MessageContent};
    use std::path::PathBuf;

    fn state(reply_hint: bool) -> SharedIntercomState {
        let config = IntercomConfig { reply_hint, ..IntercomConfig::default() };
        SharedIntercomState::new(config, 600_000, PathBuf::from("/w"))
    }

    fn from() -> SessionInfo {
        SessionInfo {
            id: "child-1234".to_string(),
            name: Some("subagent-chat-1".to_string()),
            cwd: "/w".to_string(),
            model: "m".to_string(),
            pid: 1,
            started_at: 0,
            last_activity: 0,
            status: None,
            peer_uid: None,
            trusted_local: None,
        }
    }

    fn ask(text: &str) -> Message {
        Message {
            id: "q1".to_string(),
            timestamp: 0,
            reply_to: None,
            expects_reply: Some(true),
            content: MessageContent { text: text.to_string(), attachments: None },
        }
    }

    #[test]
    fn reply_hint_present_only_when_expecting_reply_and_enabled() {
        let s = state(true);
        let card = build_inline_message(&s, &from(), &ask("Which DB?"));
        assert!(card.reply_command.is_some());
        // Disabled hint → no reply command even for an ask.
        let s2 = state(false);
        assert!(build_inline_message(&s2, &from(), &ask("Which DB?")).reply_command.is_none());
    }

    #[test]
    fn attachment_text_is_appended_to_body() {
        let s = state(true);
        let mut msg = ask("see this");
        msg.content.attachments = Some(vec![Attachment {
            kind: AttachmentKind::Context,
            name: "ctx.md".to_string(),
            content: "details".to_string(),
            language: None,
        }]);
        let card = build_inline_message(&s, &from(), &msg);
        assert!(card.body().contains("see this"));
        assert!(card.body().contains("📎 ctx.md"));
        assert!(card.body().contains("details"));
    }

    #[test]
    fn surface_without_host_services_is_a_noop() {
        let s = state(true);
        // No HostServices bound → None (degrade, never panics).
        assert!(surface_incoming_message(&s, &from(), &ask("hi")).is_none());
    }

    #[test]
    fn inbound_policy_routes_by_has_ui_and_reply_shape() {
        // Interactive (`has_ui`) + the default `Always` trigger policy → deliver AND trigger,
        // regardless of the message's reply shape.
        assert_eq!(
            decide_inbound_policy(true, InboundTrigger::Always, &ask("hi")),
            InboundPolicy::Deliver { trigger: true }
        );
        // Non-interactive + a fresh (non-reply) message → the busy auto-reply (trigger policy is
        // irrelevant to the non-interactive branch).
        assert_eq!(
            decide_inbound_policy(false, InboundTrigger::Always, &ask("hi")),
            InboundPolicy::AutoReply
        );
        // Non-interactive + a message that is itself a reply → nobody to auto-reply to → surface only.
        let mut reply = ask("hi");
        reply.reply_to = Some("q1".to_string());
        assert_eq!(
            decide_inbound_policy(false, InboundTrigger::Always, &reply),
            InboundPolicy::SurfaceOnly
        );
    }

    #[test]
    fn inbound_policy_honors_inbound_trigger_config_for_interactive_sessions() {
        // Regression proof (pi `shouldTriggerInboundMessage`, `index.ts:635-646`): pre-fix,
        // `decide_inbound_policy` ignored `config.inbound_trigger` entirely and ALWAYS drove a turn
        // for an interactive (`has_ui`) session — this test fails against that behavior for both
        // `Never` and `Replies`.
        let fresh = ask("hi");
        let mut reply = ask("hi");
        reply.reply_to = Some("q1".to_string());

        // `Never` → still deliver the message, but never drive/steer a turn — not even for a reply.
        assert_eq!(
            decide_inbound_policy(true, InboundTrigger::Never, &fresh),
            InboundPolicy::Deliver { trigger: false }
        );
        assert_eq!(
            decide_inbound_policy(true, InboundTrigger::Never, &reply),
            InboundPolicy::Deliver { trigger: false }
        );

        // `Replies` → trigger only when the message is itself a reply to an outstanding ask.
        assert_eq!(
            decide_inbound_policy(true, InboundTrigger::Replies, &fresh),
            InboundPolicy::Deliver { trigger: false }
        );
        assert_eq!(
            decide_inbound_policy(true, InboundTrigger::Replies, &reply),
            InboundPolicy::Deliver { trigger: true }
        );

        // `Always` → always trigger regardless of shape.
        assert_eq!(
            decide_inbound_policy(true, InboundTrigger::Always, &fresh),
            InboundPolicy::Deliver { trigger: true }
        );
    }

    #[tokio::test]
    async fn auto_reply_without_a_client_is_a_noop() {
        // No live broker client bound → the non-interactive auto-reply degrades to a no-op (returns
        // false), never panics/blocks — the same headless/degraded contract the surface path holds.
        let s = state(false);
        assert!(!auto_reply_non_interactive(&s, &from(), &ask("hi")).await);
    }
}
