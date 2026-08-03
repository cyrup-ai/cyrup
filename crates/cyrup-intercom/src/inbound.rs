//! The inbound broker-message handler (`pi-intercom/index.ts:709-772` `handleIncomingMessage` +
//! `attachClientHandlers`) and the human surface it drives.
//!
//! WIRING: [`spawn_inbound_loop`] is started on `SessionStart` by [`crate::extension`] once the client
//! connects. For every inbound broker `Message` it (1) resolves an outstanding OUTBOUND ask first (the
//! process-global single slot, `index.ts:715-724`), else (2) records the inbound ask in the
//! [`ReplyTracker`](crate::reply_tracker::ReplyTracker), (3) SURFACES it to the human via
//! [`surface_incoming_message`] → `HostServices::append_entry` (the port doc §4.2/§7.2 human surface,
//! P-1 Route B), and (4) dispatches the inbound delivery policy ([`decide_inbound_policy`], pi
//! `index.ts:745-765`), which branches FIRST on whether an agent run is in flight: an IDLE session
//! is delivered an agent turn OVER the message ([`trigger_turn_over_inbound`]); a BUSY interactive
//! session PARKS the message ([`queue_idle_message`]) for the debounced [`flush_idle_messages`] to
//! deliver once the run ends; and only a BUSY non-interactive session sends the sender the
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
/// How long a queued idle message waits before the flush is attempted, so a burst of inbound
/// messages coalesces into one delivery (pi `INBOUND_FLUSH_DELAY_MS`, `index.ts:18`).
pub const INBOUND_FLUSH_DELAY_MS: u64 = 200;
/// How long the flush backs off for when the session is STILL busy at flush time (pi
/// `INBOUND_IDLE_RETRY_MS`, `index.ts:19`).
pub const INBOUND_IDLE_RETRY_MS: u64 = 500;

/// The inbound delivery policy decision (pi `handleIncomingMessage`, `index.ts:745-765`), computed
/// AFTER the durable surface (`append_entry`) from whether an agent run is in flight (`is_idle`),
/// this session's static `has_ui`, and the message shape. Kept as a pure decision (no I/O) so the
/// branch is directly unit-testable without a live host/broker.
///
/// pi's tree, in order: `if (!isIdle) { if (!hasUI) { busy auto-reply; return } queueIdleMessage;
/// return } sendIncomingMessage(entry, "trigger")`. The IDLE case is therefore delivered
/// **regardless of `hasUI`** — a headless session that happens to be idle gets a real trigger
/// delivery, and the busy auto-reply is reachable only while a run is actually in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboundPolicy {
    /// The session is IDLE: deliver the message through `HostServices::inject_message` (pi
    /// `sendIncomingMessage(entry, "trigger")`), with `trigger` deciding whether that call also
    /// drives an agent turn OVER it (pi's `shouldTriggerInboundMessage`, `index.ts:641-651`, applied
    /// at `delivery === "trigger" && shouldTriggerInboundMessage(entry)`, `index.ts:669-671`):
    /// `config.inbound_trigger == Always` -> always `true`; `== Replies` -> `true` only when the
    /// message is itself a reply (`reply_to.is_some()`); `== Never` -> always `false` (still
    /// delivered, just without driving a turn — pi's `{ deliverAs: "followUp" }`).
    Deliver { trigger: bool },
    /// The session is BUSY and interactive (`!is_idle && has_ui`): park the message in
    /// [`SharedIntercomState`]'s pending-idle queue and let the debounced flush deliver it when the
    /// run finishes (pi `queueIdleMessage`, `index.ts:711-714`) — never steer onto a live run.
    Queue,
    /// The session is BUSY and non-interactive (`!is_idle && !has_ui`) and the message is a fresh
    /// (non-reply) one: there is no human to involve and no turn to attach to, so send the sender the
    /// "running in non-interactive mode" busy auto-reply + `markReplied` (`index.ts:747-760`).
    AutoReply,
    /// Nothing beyond the durable surface: a BUSY non-interactive session received a message that is
    /// itself a reply (nobody to auto-reply to).
    SurfaceOnly,
}

/// pi `shouldTriggerInboundMessage` (`index.ts:641-651`): whether a `"trigger"`-mode delivery may
/// actually drive an agent turn, per the resolved `inboundTrigger` config and the message's shape.
#[must_use]
pub fn should_trigger_inbound_message(
    inbound_trigger: InboundTrigger,
    message: &Message,
) -> bool {
    match inbound_trigger {
        InboundTrigger::Always => true,
        InboundTrigger::Replies => message.reply_to.is_some(),
        InboundTrigger::Never => false,
    }
}

/// Decide the inbound delivery policy (pi `handleIncomingMessage`, `index.ts:745-765`, gated by
/// `shouldTriggerInboundMessage`, `index.ts:641-651`) from whether a run is in flight, the session's
/// static `has_ui`, the resolved `inbound_trigger` config, and whether the message is itself a reply.
/// Pure — no host/broker I/O. See [`InboundPolicy`].
#[must_use]
pub fn decide_inbound_policy(
    is_idle: bool,
    has_ui: bool,
    inbound_trigger: InboundTrigger,
    message: &Message,
) -> InboundPolicy {
    if !is_idle {
        if !has_ui {
            return if message.reply_to.is_none() {
                InboundPolicy::AutoReply
            } else {
                InboundPolicy::SurfaceOnly
            };
        }
        return InboundPolicy::Queue;
    }
    InboundPolicy::Deliver { trigger: should_trigger_inbound_message(inbound_trigger, message) }
}

/// One message parked in [`SharedIntercomState`]'s pending-idle queue (pi's `InboundMessageEntry` in
/// `pendingIdleMessages`, `index.ts:711-714`) — the sender plus the message, everything
/// [`send_incoming_message`] needs to re-derive the card at flush time.
#[derive(Clone, Debug)]
pub struct PendingInbound {
    /// The sending session (pi `entry.from`).
    pub from: SessionInfo,
    /// The message that arrived while this session was busy (pi `entry.message`).
    pub message: Message,
}

/// pi's two `sendIncomingMessage` delivery modes (`index.ts:652`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboundDelivery {
    /// `"trigger"`: queue the turn context AND (when `shouldTriggerInboundMessage` allows) drive an
    /// agent turn over the message.
    Trigger,
    /// `"followUp"`: deliver the message as a non-triggering follow-up entry only — pi's
    /// `{ deliverAs: "followUp" }`, which also skips `queueTurnContext` (`index.ts:655-657`). Used
    /// for every queued message after the first when a burst is flushed.
    FollowUp,
}

/// Queue an inbound message until this session goes idle (pi `queueIdleMessage`, `index.ts:711-714`):
/// park it and (re)arm the [`INBOUND_FLUSH_DELAY_MS`] debounce.
pub fn queue_idle_message(state: &Arc<SharedIntercomState>, from: SessionInfo, message: Message) {
    state.push_pending_inbound(PendingInbound { from, message });
    schedule_inbound_flush(state, INBOUND_FLUSH_DELAY_MS);
}

/// (Re)arm the debounced pending-idle flush (pi `scheduleInboundFlush`, `index.ts:674-684`):
/// `clearInboundFlushTimer()` then a fresh timer. `delay_ms == 0` is pi's
/// `scheduleInboundFlush(0)` — the immediate drain the `agent_end`/`turn_end` handlers fire
/// (`index.ts:1086`,`:1117`); it still hops through the scheduler (pi's `setTimeout(…, 0)`), so the
/// event handler never blocks on delivery.
pub fn schedule_inbound_flush(state: &Arc<SharedIntercomState>, delay_ms: u64) {
    let flush_state = state.clone();
    let handle = tokio::spawn(async move {
        if delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }
        flush_idle_messages(&flush_state);
    });
    state.set_flush_timer(Some(handle));
}

/// Deliver everything parked in the pending-idle queue, if the session is now idle (pi
/// `flushIdleMessages`, `index.ts:685-710`): nothing queued → return; still busy → back off
/// [`INBOUND_IDLE_RETRY_MS`] and try again; otherwise drain the whole queue oldest-first, the FIRST
/// entry as a `"trigger"` delivery and every later one as a `"followUp"` (pi
/// `sendIncomingMessage(entry, index === 0 ? "trigger" : "followUp")`, `index.ts:707-709`).
pub fn flush_idle_messages(state: &Arc<SharedIntercomState>) {
    // The handle currently in the timer slot is THIS task's own — release it rather than aborting.
    state.release_flush_timer();
    if state.pending_inbound_len() == 0 {
        return;
    }
    if !state.is_idle() {
        schedule_inbound_flush(state, INBOUND_IDLE_RETRY_MS);
        return;
    }
    for (index, pending) in state.take_pending_inbound().into_iter().enumerate() {
        let delivery =
            if index == 0 { InboundDelivery::Trigger } else { InboundDelivery::FollowUp };
        send_incoming_message(state, &pending.from, &pending.message, delivery);
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
    // (`index.ts:655-657`) — i.e. on every call through THIS ("trigger" delivery mode) path,
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

/// pi `sendIncomingMessage(entry, delivery)` (`index.ts:652-672`) — the delivery-mode-aware entry
/// point the pending-idle flush uses. [`InboundDelivery::Trigger`] is exactly
/// [`trigger_turn_over_inbound`] with pi's `shouldTriggerInboundMessage` applied;
/// [`InboundDelivery::FollowUp`] delivers the message as a plain follow-up entry — no turn context
/// queued, no turn driven — which is what every queued message after the first gets when a busy
/// session's backlog is flushed. Returns whether a live host was there to deliver through.
pub fn send_incoming_message(
    state: &SharedIntercomState,
    from: &SessionInfo,
    message: &Message,
    delivery: InboundDelivery,
) -> bool {
    match delivery {
        InboundDelivery::Trigger => trigger_turn_over_inbound(
            state,
            from,
            message,
            should_trigger_inbound_message(state.config.inbound_trigger, message),
        ),
        InboundDelivery::FollowUp => {
            let Some(services) = state.host_services() else {
                return false;
            };
            let body = build_inline_message(state, from, message).body().to_string();
            if let Err(e) =
                services.inject_message(&body, Some(INBOUND_MESSAGE_CUSTOM_TYPE), false, false)
            {
                tracing::warn!(error = %e, "intercom: failed to deliver a follow-up inbound message");
            }
            true
        }
    }
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
                    //     `index.ts:745-765`), computed AFTER the durable surface from whether a run
                    //     is in flight (`ctx.isIdle()`, read live off `HostServices`), this session's
                    //     static `has_ui`, and the message shape, then routed to the real
                    //     host/broker seam: an IDLE session (interactive or not) is delivered
                    //     through `inject_message`; a BUSY interactive one queues the message for
                    //     the debounced idle flush; a BUSY non-interactive one sends the sender the
                    //     busy auto-reply.
                    match decide_inbound_policy(
                        state.is_idle(),
                        state.has_ui(),
                        state.config.inbound_trigger,
                        &message,
                    ) {
                        InboundPolicy::Deliver { trigger } => {
                            trigger_turn_over_inbound(&state, &from, &message, trigger);
                        }
                        InboundPolicy::Queue => {
                            queue_idle_message(&state, from.clone(), message.clone());
                        }
                        InboundPolicy::AutoReply => {
                            auto_reply_non_interactive(&state, &from, &message).await;
                        }
                        InboundPolicy::SurfaceOnly => {}
                    }
                }
                Ok(InboundEvent::Disconnected(reason)) => {
                    // pi's `client.on("disconnected", …)` handler (`index.ts:779-789`): fail the
                    // in-flight outbound ask, drop the dead client, and arm the reconnect ladder
                    // (unless this session is deliberately shutting down). Before this, the loop
                    // just `break`ed and `state.client()` kept handing out a dead client for the
                    // rest of the process — ONE drop disabled intercom permanently.
                    crate::connect::handle_disconnect(&state, &client, &reason);
                    break;
                }
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

    /// ICOM-002 regression (pi `handleIncomingMessage`, `index.ts:745-765`): the delivery policy's
    /// FIRST axis is `ctx.isIdle()`, not `hasUI`. Pre-fix, `decide_inbound_policy` took only
    /// `has_ui`, so:
    ///
    ///  * an IDLE HEADLESS session (`!has_ui`) received the busy auto-reply instead of a real
    ///    delivery — the sender was told "cannot respond right now" by a session that was doing
    ///    nothing at all; and
    ///  * a BUSY INTERACTIVE session was steered onto its live run instead of having the message
    ///    parked for the idle flush.
    ///
    /// Both of those are asserted here directly, so this test fails against the pre-fix branch.
    #[test]
    fn inbound_policy_branches_on_is_idle_before_has_ui() {
        let fresh = ask("hi");
        let mut reply = ask("hi");
        reply.reply_to = Some("q1".to_string());

        // IDLE + interactive → delivered, trigger per the config.
        assert_eq!(
            decide_inbound_policy(true, true, InboundTrigger::Always, &fresh),
            InboundPolicy::Deliver { trigger: true }
        );
        // IDLE + HEADLESS → still delivered (pi's idle arm is reached regardless of `hasUI`); the
        // busy auto-reply is NOT reachable here.
        assert_eq!(
            decide_inbound_policy(true, false, InboundTrigger::Always, &fresh),
            InboundPolicy::Deliver { trigger: true }
        );
        // BUSY + interactive → parked for the idle flush, never steered onto the live run.
        assert_eq!(
            decide_inbound_policy(false, true, InboundTrigger::Always, &fresh),
            InboundPolicy::Queue
        );
        // BUSY + headless + a fresh message → the busy auto-reply (the only arm that reaches it).
        assert_eq!(
            decide_inbound_policy(false, false, InboundTrigger::Always, &fresh),
            InboundPolicy::AutoReply
        );
        // BUSY + headless + a message that is itself a reply → nobody to auto-reply to.
        assert_eq!(
            decide_inbound_policy(false, false, InboundTrigger::Always, &reply),
            InboundPolicy::SurfaceOnly
        );
    }

    #[test]
    fn inbound_policy_honors_inbound_trigger_config_for_idle_sessions() {
        // Regression proof (pi `shouldTriggerInboundMessage`, `index.ts:641-651`): an earlier fix
        // made `decide_inbound_policy` honor `config.inbound_trigger` instead of always driving a
        // turn. That gating lives on the IDLE arm now (pi's only `"trigger"`-delivery site).
        let fresh = ask("hi");
        let mut reply = ask("hi");
        reply.reply_to = Some("q1".to_string());

        // `Never` → still deliver the message, but never drive/steer a turn — not even for a reply.
        assert_eq!(
            decide_inbound_policy(true, true, InboundTrigger::Never, &fresh),
            InboundPolicy::Deliver { trigger: false }
        );
        assert_eq!(
            decide_inbound_policy(true, true, InboundTrigger::Never, &reply),
            InboundPolicy::Deliver { trigger: false }
        );

        // `Replies` → trigger only when the message is itself a reply to an outstanding ask.
        assert_eq!(
            decide_inbound_policy(true, true, InboundTrigger::Replies, &fresh),
            InboundPolicy::Deliver { trigger: false }
        );
        assert_eq!(
            decide_inbound_policy(true, true, InboundTrigger::Replies, &reply),
            InboundPolicy::Deliver { trigger: true }
        );

        // `Always` → always trigger regardless of shape.
        assert_eq!(
            decide_inbound_policy(true, true, InboundTrigger::Always, &fresh),
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

    /// One captured `inject_message` call: `(content, custom_type, display, trigger_turn)`.
    type InjectedCall = (String, Option<String>, bool, bool);

    /// A `HostServices` double with a SETTABLE `is_idle` — the live run-in-flight signal
    /// (`cyrup_ext::HostServices::is_idle`, pi `ctx.isIdle()`) — recording every `inject_message`
    /// so the pending-idle flush's real delivery is observable.
    struct IdleControlledHost {
        idle: std::sync::atomic::AtomicBool,
        injected: std::sync::Mutex<Vec<InjectedCall>>,
    }
    impl IdleControlledHost {
        fn new(idle: bool) -> Self {
            Self {
                idle: std::sync::atomic::AtomicBool::new(idle),
                injected: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn set_idle(&self, idle: bool) {
            self.idle.store(idle, std::sync::atomic::Ordering::SeqCst);
        }
        fn injected(&self) -> Vec<InjectedCall> {
            self.injected.lock().unwrap().clone()
        }
    }
    impl cyrup_ext::HostServices for IdleControlledHost {
        fn is_idle(&self) -> bool {
            self.idle.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn append_entry(
            &self,
            _custom_type: &str,
            _data: &serde_json::Value,
        ) -> std::result::Result<String, String> {
            Ok("entry-1".to_string())
        }
        fn inject_message(
            &self,
            content: &str,
            custom_type: Option<&str>,
            display: bool,
            trigger_turn: bool,
        ) -> std::result::Result<(), String> {
            self.injected.lock().unwrap().push((
                content.to_string(),
                custom_type.map(str::to_string),
                display,
                trigger_turn,
            ));
            Ok(())
        }
    }

    /// ICOM-002's live half (pi `queueIdleMessage` → `flushIdleMessages`, `index.ts:685-714`): a
    /// BUSY interactive session must deliver NOTHING at arrival time, and once the run ends the
    /// whole backlog must be delivered in arrival order — the first as a turn-driving `"trigger"`
    /// delivery, every later one as a non-triggering `"followUp"`.
    ///
    /// Pre-fix this could not even be expressed: `decide_inbound_policy` had no idle axis, so both
    /// messages were injected IMMEDIATELY (steering a live run), and both with `trigger_turn: true`.
    #[tokio::test]
    async fn busy_interactive_session_parks_inbound_and_flushes_the_backlog_when_the_run_ends() {
        let s = Arc::new(state(true));
        let host = Arc::new(IdleControlledHost::new(false)); // a run is in flight
        s.set_host_services(host.clone());
        s.set_has_ui(true);

        // Two messages arrive while busy → both parked, nothing injected.
        assert_eq!(
            decide_inbound_policy(s.is_idle(), s.has_ui(), s.config.inbound_trigger, &ask("first")),
            InboundPolicy::Queue
        );
        queue_idle_message(&s, from(), ask("first"));
        queue_idle_message(&s, from(), ask("second"));
        assert_eq!(s.pending_inbound_len(), 2);
        assert!(host.injected().is_empty(), "a busy session must not be steered mid-run");

        // The debounce fires while still busy → it backs off and re-arms, delivering nothing.
        tokio::time::sleep(std::time::Duration::from_millis(
            INBOUND_FLUSH_DELAY_MS + 100,
        ))
        .await;
        assert!(
            host.injected().is_empty(),
            "the flush must back off while the run is still in flight"
        );
        assert_eq!(s.pending_inbound_len(), 2, "the backlog is retained across a busy flush");

        // The run ends → the retry (INBOUND_IDLE_RETRY_MS) drains the whole backlog.
        host.set_idle(true);
        tokio::time::sleep(std::time::Duration::from_millis(
            INBOUND_IDLE_RETRY_MS + 200,
        ))
        .await;

        let injected = host.injected();
        assert_eq!(injected.len(), 2, "both queued messages are delivered: {injected:?}");
        assert!(injected[0].0.contains("first"), "arrival order is preserved: {injected:?}");
        assert!(injected[1].0.contains("second"), "arrival order is preserved: {injected:?}");
        assert!(injected[0].3, "the FIRST queued message drives the turn (pi's \"trigger\")");
        assert!(!injected[1].3, "every later message is a non-triggering \"followUp\"");
        assert_eq!(s.pending_inbound_len(), 0, "the queue is drained");
    }

    /// The other half of the same branch swap: an IDLE HEADLESS session (`!has_ui`) delivers the
    /// message for real rather than answering the sender with the busy auto-reply. Pre-fix the
    /// `!has_ui` arm was taken unconditionally, so `trigger_turn_over_inbound` was never reached and
    /// NOTHING was injected.
    #[tokio::test]
    async fn idle_headless_session_delivers_instead_of_auto_replying() {
        let s = Arc::new(state(true));
        let host = Arc::new(IdleControlledHost::new(true));
        s.set_host_services(host.clone());
        s.set_has_ui(false);

        let policy =
            decide_inbound_policy(s.is_idle(), s.has_ui(), s.config.inbound_trigger, &ask("ping"));
        assert_eq!(policy, InboundPolicy::Deliver { trigger: true });
        // Drive the real delivery seam the `Deliver` arm routes to, exactly as `spawn_inbound_loop`
        // does, so the assertion lands on an injected message and not merely on the enum.
        let InboundPolicy::Deliver { trigger } = policy else {
            unreachable!("asserted equal to Deliver above")
        };
        assert!(trigger_turn_over_inbound(&s, &from(), &ask("ping"), trigger));

        let injected = host.injected();
        assert_eq!(injected.len(), 1, "the message reaches the agent: {injected:?}");
        assert!(injected[0].0.contains("ping"));
        assert!(injected[0].3, "an idle session gets a real turn-driving delivery");
    }
}
