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
//! is delivered an agent turn OVER the message; a BUSY interactive session has it STEERED straight
//! onto the live run ([`send_incoming_message`] with [`InboundDelivery::Steer`], v0.9.3 `25ffb96`);
//! and only a BUSY non-interactive session sends the sender the
//! "running in non-interactive mode" busy auto-reply ([`auto_reply_non_interactive`]). This is the
//! real production path the integration test drives with a scripted `HostServices` sink.

use std::sync::Arc;

use serde_json::json;

use crate::config::InboundTrigger;
use crate::reply_tracker::IntercomContext;
use crate::session_state::SharedIntercomState;
use crate::transport::client::{InboundEvent, IntercomClient, SendOptions};
use crate::transport::protocol::{Attachment, Message, MessageReceiptStatus, SessionInfo, now_ms};
use crate::ui::{InlineMessage, PlainTheme};

/// The width the degraded inline card is pre-rendered at for the `append_entry` payload (cyrup has no
/// live terminal width outside a `HostCtx`; a conventional default, the port doc §4.3).
const SURFACE_CARD_WIDTH: usize = 80;
/// The reply-hint command shown when a sender expects a reply (pi `index.ts:730`).
const REPLY_HINT_COMMAND: &str = "intercom({ action: \"reply\", message: \"...\" })";
/// The custom-message type an inbound intercom message is injected under when it drives an agent turn
/// (matches the durable surface's `append_entry("intercom_message", …)` type, so the live host routes
/// both under the same kind — pi `sendMessage({customType:"intercom_message"})`, `index.ts:656`).
pub(crate) const INBOUND_MESSAGE_CUSTOM_TYPE: &str = "intercom_message";
/// The busy auto-reply sent back to a sender when this session is running non-interactively and
/// cannot surface the message to a human (pi's non-interactive busy reply,
/// `v0.10.1 index.ts:946-947`, byte for byte).
///
/// ICOM-013. The shortened cyrup wording ("This session is running in non-interactive mode and
/// cannot respond to messages right now.") dropped the two facts the sender acts on: that the peer
/// is *working* rather than merely unattended, and that it will finish and exit rather than come
/// back. A supervisor reading the short form has no way to tell "retry in a moment" from "this
/// worker is gone", so it retries a peer that will never answer.
const NON_INTERACTIVE_BUSY_NOTICE: &str =
    "This agent is running in non-interactive mode and cannot respond to intercom messages while it is working. It will continue its current task and exit when done.";

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
    /// The session is BUSY and interactive (`!is_idle && has_ui`): hand the message STRAIGHT to the
    /// live run's steering queue — `sendIncomingMessage(entry, "steer")`
    /// (`v0.10.1 index.ts:956`).
    ///
    /// v0.9.3 (`25ffb96`, "fix: steer busy inbound messages promptly") deleted the whole
    /// park-until-idle machine — `pendingIdleMessages`, `queueIdleMessage`, `scheduleInboundFlush`,
    /// `flushIdleMessages`, `clearInboundFlushTimer`, `expirePendingIdleMessages`,
    /// `INBOUND_FLUSH_DELAY_MS` and `INBOUND_IDLE_RETRY_MS` — and replaced it with this one line.
    /// CHANGELOG 0.9.3: "Hand busy interactive inbound messages directly to Pi's safe steering queue
    /// instead of waiting for aggregate idle, preventing stale coordination from appearing hours
    /// after it was received."
    Steer,
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
        return InboundPolicy::Steer;
    }
    InboundPolicy::Deliver { trigger: should_trigger_inbound_message(inbound_trigger, message) }
}

/// pi's two `sendIncomingMessage` delivery modes (`v0.10.1 index.ts:876`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboundDelivery {
    /// `"trigger"`: the session is idle, so (when `shouldTriggerInboundMessage` allows) drive a
    /// fresh agent turn OVER the message.
    Trigger,
    /// `"steer"`: the session is busy and interactive, so hand the message to the live run's
    /// steering queue. v0.9.3 replaced the old `"followUp"` mode with this one.
    Steer,
}

/// Answer-and-forget an INBOUND ask (pi `dismissIncomingAsk`, `v0.10.1 index.ts:529-531`):
///
/// ```text
/// function dismissIncomingAsk(messageId: string): void {
///   replyTracker.dismissPendingAsk(messageId);
/// }
/// ```
///
/// v0.9.2's second half — splicing the id out of `pendingIdleMessages` — went away with the queue
/// itself at v0.9.3 (`25ffb96`). A busy interactive message is now steered onto the live run
/// immediately, so there is nothing left holding a copy to re-inject.
///
/// Every pi call site is a point where the inbound ask has just been ANSWERED or has become
/// undeliverable: the busy non-interactive auto-reply (`v0.10.1 index.ts:975`), a `send` carrying
/// an effective `replyTo` (`:2045`), and both `reply` outcomes — delivered (`:2226`) and
/// `"Session not found"` (`:2219`).
pub fn dismiss_incoming_ask(state: &SharedIntercomState, message_id: &str) {
    state.tracker.lock().unwrap_or_else(|e| e.into_inner()).dismiss_pending_ask(message_id);
}

/// `sendIncomingMessage(entry, delivery, generation?, forceTrigger?)`
/// (`v0.10.1 index.ts:876-901`, 26 lines) — the ONE delivery function, for both an idle session's
/// fresh turn and a busy interactive session's steer.
///
/// ```text
/// const injectedMessage = { ...entry.message, injectedAt: Date.now() };
/// const replyCommand = delivery === "steer" && entry.replyCommand && entry.message.expectsReply
///   ? `intercom({ action: "reply", replyTo: ${JSON.stringify(entry.message.id)}, message: "..." })`
///   : entry.replyCommand;
/// replyTracker.queueTurnContext({ from: entry.from, message: injectedMessage, receivedAt: Date.now() });
/// pi.sendMessage({ customType: "intercom_message", content: …, display: true, details: deliveredEntry },
///   delivery === "trigger" && shouldTriggerInboundMessage(entry, forceTrigger)
///     ? { triggerTurn: true } : { deliverAs: "steer" });
/// ```
///
/// Three mechanisms live here, all load-bearing:
/// - **the `injectedAt` stamp** is on a per-delivery COPY, and it is that copy which is queued as
///   the turn context and rendered into the `_…_` metadata line, so the two agree;
/// - **the steer-mode reply-hint rewrite** carries the EXPLICIT message id, because a steered
///   message lands mid-run where a bare `intercom({action:"reply"})` cannot rely on being the
///   current turn context;
/// - **`queueTurnContext` is unconditional** (v0.9.3 dropped the old `delivery !== "followUp"`
///   guard), so a reply resolves against the entry that actually drove or steered this turn.
///
/// `display = true` unconditionally, matching pi's single `pi.sendMessage({… display: true …})`.
/// It used to be `false` on the theory that [`surface_incoming_message`] had already shown the card
/// — but the host honours the caller's flag only on the not-streaming, not-trigger branch, so the
/// message was written to the session tree HIDDEN for `inboundTrigger: "replies"`/`"never"`: it
/// appeared live and then vanished from `--resume` and every transcript replay.
///
/// Returns whether a live host was there to deliver through (`false` = headless/degraded).
pub fn send_incoming_message(
    state: &SharedIntercomState,
    from: &SessionInfo,
    message: &Message,
    delivery: InboundDelivery,
) -> bool {
    // `generation = runtimeGeneration` is a DEFAULT parameter upstream (`v0.10.1 index.ts:876`), so
    // a caller that does not stamp one is checked against the current generation — which still
    // catches `disposed`/`shuttingDown`/no-context, just not a generation the caller predates.
    send_incoming_message_at(state, from, message, delivery, state.connect.generation())
}

/// [`send_incoming_message`] with the caller's captured runtime generation — pi's explicit third
/// argument (`sendIncomingMessage(entry, "trigger", messageGeneration)`, `v0.10.1 index.ts:963`).
///
/// The guard is upstream's first statement (`:877`):
///
/// ```text
/// if (runtimeStarted && !getLiveContext(runtimeContext, generation)) return;
/// ```
///
/// `runtimeStarted &&` matters: before `SessionStart` there is no runtime to be stale relative to,
/// and the local subagent relay delivers through this path in exactly that window.
pub fn send_incoming_message_at(
    state: &SharedIntercomState,
    from: &SessionInfo,
    message: &Message,
    delivery: InboundDelivery,
    generation: u64,
) -> bool {
    if state.connect.runtime_ever_started() && !crate::connect::is_live_at(state, generation) {
        return false;
    }
    let Some(services) = state.host_services() else {
        return false;
    };
    let message = &stamp_injected_at(message);
    // `emitMessageReceipt(injectedMessage.id, "injected")` (`v0.10.1 index.ts:881`) — emitted from
    // the INJECTION site, after the liveness guard and before the content is built, so a delivery
    // that the guard above dropped emits nothing.
    state.emit_message_receipt(&message.id, MessageReceiptStatus::Injected, None);
    state.tracker.lock().unwrap_or_else(|e| e.into_inner()).queue_turn_context(IntercomContext {
        from: from.clone(),
        message: message.clone(),
        received_at: now_ms(),
    });
    // `details: deliveredEntry` (`index.ts:1216`) — the SAME entry the content was rendered from,
    // carrying the `injectedAt`-stamped message and the delivery-adjusted reply command.
    let card = build_inline_message_for(state, from, message, delivery);
    let content = card.content_markdown();
    let details = serde_json::to_value(&card).ok();
    // `{ triggerTurn: true }` vs `{ deliverAs: "steer" }`. cyrup's seam takes the boolean:
    // `AgentSession::inject_message` routes to `agent.steer(msg)` whenever `is_streaming()`
    // regardless of the flag (`cyrup-session-svc/src/session.rs:3926-3928`), so a busy session's
    // delivery steers exactly as upstream's `deliverAs: "steer"` does, and the flag only decides
    // whether an IDLE session spawns a run over the message.
    let trigger_turn = delivery == InboundDelivery::Trigger
        && should_trigger_inbound_message(state.config.inbound_trigger, message);
    if let Err(e) = services.inject_message(
        &content,
        Some(INBOUND_MESSAGE_CUSTOM_TYPE),
        true,
        details.as_ref(),
        trigger_turn,
    ) {
        tracing::warn!(error = %e, "intercom: failed to deliver an inbound message");
    }
    true
}

/// [`send_incoming_message`] with an explicit trigger decision already made — the entry point the
/// local subagent-relay seam uses, which is upstream's `forceTrigger = true`
/// (`v0.10.1 index.ts:1251`, bypassing `shouldTriggerInboundMessage`).
pub fn trigger_turn_over_inbound(
    state: &SharedIntercomState,
    from: &SessionInfo,
    message: &Message,
    trigger: bool,
) -> bool {
    let Some(services) = state.host_services() else {
        return false;
    };
    let message = &stamp_injected_at(message);
    // `emitMessageReceipt(injectedMessage.id, "injected")` (`v0.10.1 index.ts:881`) — emitted from
    // the INJECTION site, after the liveness guard and before the content is built, so a delivery
    // that the guard above dropped emits nothing.
    state.emit_message_receipt(&message.id, MessageReceiptStatus::Injected, None);
    state.tracker.lock().unwrap_or_else(|e| e.into_inner()).queue_turn_context(IntercomContext {
        from: from.clone(),
        message: message.clone(),
        received_at: now_ms(),
    });
    let card = build_inline_message(state, from, message);
    let content = card.content_markdown();
    let details = serde_json::to_value(&card).ok();
    if let Err(e) = services.inject_message(
        &content,
        Some(INBOUND_MESSAGE_CUSTOM_TYPE),
        true,
        details.as_ref(),
        trigger,
    ) {
        tracing::warn!(error = %e, "intercom: failed to deliver an inbound message");
    }
    true
}

/// `{ ...entry.message, injectedAt: Date.now() }` (`v0.10.1 index.ts:878`) — the per-delivery copy
/// upstream stamps before it renders the content, queues the turn context and passes `details`.
#[must_use]
fn stamp_injected_at(message: &Message) -> Message {
    let mut injected = message.clone();
    injected.injected_at = Some(now_ms().into());
    injected
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
    auto_reply_non_interactive_at(state, from, message, state.connect.generation()).await
}

/// [`auto_reply_non_interactive`] fenced on the caller's captured runtime generation
/// (`v0.10.1 index.ts:950`: `if (result.delivered && getLiveContext(liveContext, messageGeneration))`).
///
/// The `send` here is an `await` across which the session runtime can be replaced. Upstream
/// re-checks liveness AFTER it resolves and before `dismissIncomingAsk`, so a reply that lands
/// against a runtime that has since been swapped does not mutate the NEW runtime's pending-ask
/// state.
pub async fn auto_reply_non_interactive_at(
    state: &SharedIntercomState,
    from: &SessionInfo,
    message: &Message,
    generation: u64,
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
            supersedes: None,
            retry_of: None,
            provenance: None,
        })
        .await;
    match send {
        Ok(result) if result.delivered => {
            // dismissIncomingAsk (`index.ts:755`): the inbound ask is now answered — drop it from
            // pending so a later `intercom{list}`/`intercom{reply}` does not re-surface it, AND
            // from the pending-idle queue so the debounced flush does not re-inject it.
            //
            // `result.delivered && getLiveContext(liveContext, messageGeneration)`
            // (`v0.10.1 index.ts:950`): the reply was delivered either way, but the state mutation
            // belongs to the runtime that asked for it.
            if !crate::connect::is_live_at(state, generation) {
                return false;
            }
            dismiss_incoming_ask(state, &message.id);
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
                    let message = *message;
                    // (0) `const messageGeneration = runtimeGeneration;`
                    //     `const liveContext = getLiveContext(ctx, messageGeneration);`
                    //     `if (!liveContext) return;` (`v0.10.1 index.ts:903-906`) — the FIRST two
                    //     statements of `handleIncomingMessage`, before the dedupe, the waiter match
                    //     and `recordIncomingMessage`. A message that arrives against a runtime that
                    //     has already been replaced must not touch this session's state at all;
                    //     without the stamp, the checks further down would pass against whatever
                    //     runtime happens to be live by the time they run.
                    let message_generation = state.connect.generation();
                    if !crate::connect::is_live_at(&state, message_generation) {
                        continue;
                    }
                    // (0b) ICOM-017 — `const receiverReceivedAt = Date.now();` then
                    //     `if (hasSeenInboundMessage(...)) { emitMessageReceipt(id, "acknowledged",
                    //     "duplicate message id suppressed"); return; }` (`v0.10.1 index.ts:908-912`).
                    //     ONE timestamp is taken and reused as the dedupe clock AND the stamp, so a
                    //     second `now_ms()` below would be a divergence, not a tidy-up.
                    let receiver_received_at = now_ms();
                    if state.has_seen_inbound_message(&from.id, &message.id, receiver_received_at) {
                        state.emit_message_receipt(
                            &message.id,
                            MessageReceiptStatus::Acknowledged,
                            Some("duplicate message id suppressed"),
                        );
                        continue;
                    }
                    // `const receivedMessage = { ...message, receiverReceivedAt };` (`:913`) — every
                    // use below is of the STAMPED copy, which is what puts `receiver received …`
                    // into the delivery-metadata line (`v0.10.1 index.ts:480-481`).
                    let mut message = message;
                    message.receiver_received_at = Some(receiver_received_at.into());
                    let message = message;
                    state.emit_message_receipt(
                        &message.id,
                        MessageReceiptStatus::ReceiverReceived,
                        None,
                    );
                    // (1) Resolve an outstanding OUTBOUND ask first (index.ts:715-724). When matched,
                    //     the message is the reply to our own ask — do NOT also surface it.
                    if state.waiter.try_deliver(&from, &message) {
                        state.emit_message_receipt(
                            &message.id,
                            MessageReceiptStatus::Acknowledged,
                            Some("matched reply waiter"),
                        );
                        continue;
                    }
                    // (2) Record the inbound ask (for a future `intercom{reply}` / the ClarifyChannel
                    //     correlation) and (3) surface it to the human.
                    state.tracker.lock().unwrap_or_else(|e| e.into_inner()).record_incoming_message(
                        from.clone(),
                        message.clone(),
                        receiver_received_at,
                    );
                    state.emit_message_receipt(
                        &message.id,
                        MessageReceiptStatus::Acknowledged,
                        Some("accepted by receiver"),
                    );
                    // (3) The durable `append_entry` surface is NOT written here any more: the
                    //     delivering arms below inject a custom message that the registered message
                    //     renderer draws, and `append_custom_message(…, details)` persists it, so
                    //     writing both would paint the card TWICE for every delivery. Upstream has
                    //     exactly one surface. It is written by the two arms that inject nothing —
                    //     see `AutoReply`/`SurfaceOnly` below.
                    // (4) Dispatch the inbound delivery policy (pi `handleIncomingMessage`,
                    //     `index.ts:745-765`), computed AFTER the durable surface from whether a run
                    //     is in flight (`ctx.isIdle()`, read live off `HostServices`), this session's
                    //     static `has_ui`, and the message shape, then routed to the real
                    //     host/broker seam: an IDLE session (interactive or not) is delivered
                    //     through `inject_message`; a BUSY interactive one is STEERED onto the live
                    //     run (`v0.10.1 index.ts:956`); a BUSY non-interactive one sends the sender
                    //     the busy auto-reply.
                    // `const activeContext = getLiveContext(liveContext, messageGeneration);`
                    // `if (!activeContext) return;` (`v0.10.1 index.ts:937-940`) — the head of the
                    // async IIFE, i.e. the re-check after the synchronous recording work above and
                    // before ANY delivery decision is taken. `isIdle`/`hasUI` are read off the live
                    // context upstream, so reading them from a dead one is the exact defect.
                    if !crate::connect::is_live_at(&state, message_generation) {
                        continue;
                    }
                    match decide_inbound_policy(
                        state.is_idle(),
                        state.has_ui(),
                        state.config.inbound_trigger,
                        &message,
                    ) {
                        InboundPolicy::Deliver { .. } => {
                            // `if (getLiveContext(liveContext, messageGeneration)) {`
                            // `  sendIncomingMessage(entry, "trigger", messageGeneration); }`
                            // (`:962-963`) — the trigger arm is the only one upstream double-checks
                            // AND stamps, because it is the one that spawns a whole new run.
                            if crate::connect::is_live_at(&state, message_generation) {
                                send_incoming_message_at(
                                    &state,
                                    &from,
                                    &message,
                                    InboundDelivery::Trigger,
                                    message_generation,
                                );
                            }
                        }
                        InboundPolicy::Steer => {
                            // `sendIncomingMessage(entry, "steer");` (`:961`) — no explicit
                            // generation, so upstream's default (`= runtimeGeneration`) applies and
                            // the guard degenerates to the disposed/shuttingDown/no-context check.
                            send_incoming_message(&state, &from, &message, InboundDelivery::Steer);
                        }
                        InboundPolicy::AutoReply => {
                            // Busy + non-interactive: nothing is injected, so the durable entry IS
                            // the surface. Drawn by `IntercomExtension::render_entry` from the
                            // pre-rendered card.
                            surface_incoming_message(&state, &from, &message);
                            auto_reply_non_interactive_at(
                                &state,
                                &from,
                                &message,
                                message_generation,
                            )
                            .await;
                        }
                        InboundPolicy::SurfaceOnly => {
                            // Busy + non-interactive + the message is itself a reply: no auto-reply
                            // and no injection, so this arm is the durable surface and nothing else.
                            // Without it the message would be recorded and then drawn by nothing.
                            surface_incoming_message(&state, &from, &message);
                        }
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
                // ICOM-017 — `case "message_receipt"` (`v0.10.1 index.ts:1018-1024`): a receipt
                // about a message THIS session sent. Recorded, never surfaced: its only reader is
                // `latestDeliveryState`, which the ask timeout quotes.
                Ok(InboundEvent::MessageReceipt { receipt, .. }) => {
                    state.record_outbound_receipt(&receipt);
                }
                // ICOM-017 — `case "message_control"` (`:1025-1027`): the sender withdrew or
                // replaced a message it sent US. `handleMessageControl` retracts the pending ask.
                Ok(InboundEvent::MessageControl { control, .. }) => {
                    state.handle_message_control(&control);
                }
                Ok(_) => {} // joined/left/presence/error — presence UI is a later phase.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Format inbound attachments into the message body (pi `formatAttachments`,
/// `v0.10.1 index.ts:94-104`).
///
/// The delimiter is `\n\n---\nAttachment: {name}` at v0.10.0 and later (`633e782`, "refactor:
/// deslop intercom protocol cleanup") — it used to be `📎 {name}`. This string reaches the MODEL,
/// which is expected to parse it when a peer sends files, so the two forms are not interchangeable.
#[must_use]
pub fn format_attachments(attachments: &[Attachment]) -> String {
    let mut text = String::new();
    for att in attachments {
        match &att.language {
            Some(lang) => {
                text.push_str(&format!(
                    "\n\n---\nAttachment: {}\n~~~{lang}\n{}\n~~~",
                    att.name, att.content
                ));
            }
            None => {
                text.push_str(&format!("\n\n---\nAttachment: {}\n{}", att.name, att.content));
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
    build_inline_message_for(state, from, message, InboundDelivery::Trigger)
}

/// [`build_inline_message`] with the delivery mode, which decides the reply hint's shape
/// (`v0.10.1 index.ts:880-884`):
///
/// ```text
/// const replyCommand = delivery === "steer" && entry.replyCommand && entry.message.expectsReply
///   ? `intercom({ action: "reply", replyTo: ${JSON.stringify(entry.message.id)}, message: "..." })`
///   : entry.replyCommand;
/// ```
///
/// A steered message lands in the MIDDLE of a run, where `resolveReplyTarget`'s current-turn-context
/// shortcut does not point at it — so the hint has to name the id explicitly or the model is told to
/// use a command that would answer the wrong ask. `JSON.stringify` is what quotes the id, which for
/// a broker-minted UUID is exactly a pair of double quotes.
#[must_use]
pub fn build_inline_message_for(
    state: &SharedIntercomState,
    from: &SessionInfo,
    message: &Message,
    delivery: InboundDelivery,
) -> InlineMessage {
    let attachment_text = message
        .content
        .attachments
        .as_deref()
        .filter(|a| !a.is_empty())
        .map(format_attachments)
        .unwrap_or_default();
    let body_text = format!("{}{attachment_text}", message.content.text);
    let reply_command = (state.config.reply_hint && message.expects_reply == Some(true)).then(|| {
        if delivery == InboundDelivery::Steer {
            format!(
                "intercom({{ action: \"reply\", replyTo: {}, message: \"...\" }})",
                serde_json::Value::String(message.id.clone())
            )
        } else {
            REPLY_HINT_COMMAND.to_string()
        }
    });
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
    // The entry payload IS the serialized card (upstream's `deliveredEntry`) plus the two
    // cyrup-only pre-rendered surfaces the entry renderer reads: the model-facing markdown
    // `content` and the width-80 `card` degrade. Serializing the card rather than restating its
    // fields is what keeps this payload and the `inject_message` details the same bytes.
    let mut payload = serde_json::to_value(&card).unwrap_or_else(|_| json!({}));
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("content".into(), json!(card.content_markdown()));
        obj.insert("card".into(), json!(card.render(&PlainTheme, SURFACE_CARD_WIDTH)));
    }
    match services.append_entry(INBOUND_MESSAGE_CUSTOM_TYPE, &payload) {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!(error = %e, "intercom: failed to surface inbound message via append_entry");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::unreachable
    )]
    use super::*;
    use crate::config::IntercomConfig;
    use crate::transport::protocol::{AttachmentKind, MessageContent};
    use std::path::PathBuf;

    fn state(reply_hint: bool) -> SharedIntercomState {
        let config = IntercomConfig { reply_hint, ..IntercomConfig::default() };
        SharedIntercomState::new(config, 600_000, PathBuf::from("/w"))
    }

    /// ICOM-013 / `v0.10.1 index.ts:947`. This string is prompt-visible on the SENDER's side — it
    /// arrives as the reply to a blocking `ask` — so a paraphrase changes what a peer agent
    /// concludes. Byte-for-byte against upstream, not a "means the same thing" check.
    #[test]
    fn the_busy_auto_reply_is_upstreams_exact_text() {
        assert_eq!(
            NON_INTERACTIVE_BUSY_NOTICE,
            "This agent is running in non-interactive mode and cannot respond to intercom messages while it is working. It will continue its current task and exit when done."
        );
    }

    fn from() -> SessionInfo {
        SessionInfo {
            id: "child-1234".to_string(),
            name: Some("subagent-chat-1".to_string()),
            runtime_fallback_alias: None,
            cwd: "/w".to_string(),
            model: "m".to_string(),
            pid: 1u32.into(),
            started_at: 0u64.into(),
            last_activity: 0u64.into(),
            status: None,
            peer_uid: None,
            trusted_local: None,
            context_pct: None,
            context_tokens: None,
            context_window: None,
            extra: Default::default(),
        }
    }

    fn ask(text: &str) -> Message {
        Message {
            id: "q1".to_string(),
            timestamp: 0u64.into(),
            reply_to: None,
            expects_reply: Some(true),
            content: MessageContent { text: text.to_string(), attachments: None, ..Default::default() },
            ..Default::default()
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
            extra: Default::default(),
        }]);
        let card = build_inline_message(&s, &from(), &msg);
        // `formatAttachments` (`v0.10.1 index.ts:95-105`) — the `📎 {name}` delimiter became
        // `Attachment: {name}` in v0.10.0 (`633e782`, "refactor: deslop intercom protocol
        // cleanup"). This string reaches the MODEL, so the exact delimiter is the contract.
        assert_eq!(card.body(), "see this\n\n---\nAttachment: ctx.md\ndetails");
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
    ///  * a BUSY INTERACTIVE session took the headless auto-reply arm instead of a delivery.
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
        // BUSY + interactive → STEERED onto the live run (`v0.10.1 index.ts:956`), not parked.
        assert_eq!(
            decide_inbound_policy(false, true, InboundTrigger::Always, &fresh),
            InboundPolicy::Steer
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

    /// A `HostServices` double pinned at a FIXED `is_idle` — the live run-in-flight signal
    /// (`cyrup_ext::HostServices::is_idle`, pi `ctx.isIdle()`) — recording every `inject_message`
    /// so the delivery a policy actually performs is observable.
    ///
    /// The flag is chosen once at construction and never transitions, because nothing in this
    /// crate observes a busy-to-idle flip: `decide_inbound_policy` takes `is_idle` BY VALUE and
    /// re-reads it per call, and the park-until-idle machine whose debounced flush a settable
    /// flag would once have driven was deleted upstream at v0.9.3 (`25ffb96`) — see
    /// `InboundPolicy::Steer` above, which names all eight removed symbols. The busy-to-idle
    /// transition contract is owned solely by
    /// `crates/cyrup-it/tests/intercom/dismiss_incoming_ask.rs:206`, where its own near-copy of
    /// this double flips the flag over the real socket.
    struct IdleControlledHost {
        idle: bool,
        injected: std::sync::Mutex<Vec<InjectedCall>>,
    }
    impl IdleControlledHost {
        fn new(idle: bool) -> Self {
            Self {
                idle,
                injected: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn injected(&self) -> Vec<InjectedCall> {
            self.injected.lock().unwrap().clone()
        }
    }
    impl IdleControlledHost {
        fn clear_injected(&self) {
            self.injected.lock().unwrap().clear();
        }
    }
    impl cyrup_ext::HostServices for IdleControlledHost {
        fn is_idle(&self) -> bool {
            self.idle
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
            _details: Option<&serde_json::Value>,
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

    /// ICOM-049 / `v0.10.1 index.ts:877`, `:903`, `:963`. An inbound delivery is stamped with the
    /// runtime generation it was decided under, and re-checked before it injects. Without the
    /// stamp, a delivery task still in flight when the session runtime is replaced (an RPC
    /// re-attach, a runtime rebuild) lands in the NEW session, attributed to a peer that session
    /// never talked to.
    ///
    /// RED before the fix: `send_incoming_message` took no generation and consulted none, so the
    /// second `assert!(!…)` below was `true` and `host.injected()` held the stale message.
    #[tokio::test]
    async fn a_delivery_stamped_at_a_replaced_runtime_is_not_injected_into_the_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let params = || crate::connect::ConnectParams {
            agent_dir: dir.path().join("agent"),
            metadata: None,
            model: None,
        };
        let s = Arc::new(state(true));
        let host = Arc::new(IdleControlledHost::new(true));
        s.set_host_services(host.clone());
        s.set_has_ui(true);

        crate::connect::begin_runtime(&s, params());
        let stamped = s.connect.generation();
        assert!(
            send_incoming_message_at(&s, &from(), &ask("first"), InboundDelivery::Trigger, stamped),
            "a delivery at its own generation is live"
        );
        assert_eq!(host.injected().len(), 1);
        host.clear_injected();

        // The session runtime is replaced under the in-flight delivery.
        crate::connect::begin_runtime(&s, params());
        assert_ne!(s.connect.generation(), stamped, "begin_runtime bumps the generation");
        assert!(
            !send_incoming_message_at(&s, &from(), &ask("first"), InboundDelivery::Trigger, stamped),
            "the stale-generation delivery must be fenced out"
        );
        assert!(
            host.injected().is_empty(),
            "nothing reaches the new runtime: {:?}",
            host.injected()
        );

        // …and the new runtime's own generation still delivers, so the fence is not a blanket stop.
        assert!(send_incoming_message_at(
            &s,
            &from(),
            &ask("second"),
            InboundDelivery::Trigger,
            s.connect.generation()
        ));
        assert_eq!(host.injected().len(), 1);

        // A deliberate shutdown fences everything, including the current generation
        // (`disposed || shuttingDown`, `v0.10.1 index.ts:647`).
        //
        // This half is why the guard reads `runtime_ever_started()` and NOT the supervisor's
        // `started` flag. pi's `runtimeStarted` is a latch — set at `index.ts:1253` and never
        // cleared — whereas cyrup's `started` is cleared by `shutdown` because the reconnect ladder
        // needs "is a runtime active right now". Collapsing the two makes `runtimeStarted &&` false
        // after shutdown, which SKIPS the fence and lets a stale delivery through, inverting the
        // guard. Found while writing this test.
        host.clear_injected();
        let live = s.connect.generation();
        crate::connect::shutdown(&s);
        assert!(
            s.connect.runtime_ever_started(),
            "pi's runtimeStarted latch must survive shutdown (v0.10.1 index.ts:522,1253)"
        );
        assert!(!send_incoming_message_at(
            &s,
            &from(),
            &ask("third"),
            InboundDelivery::Trigger,
            live
        ));
        assert!(host.injected().is_empty());
    }

    /// ICOM-035 / v0.9.3 `25ffb96` ("fix: steer busy inbound messages promptly"): a message that
    /// arrives while an interactive session is BUSY reaches the agent **immediately**, as a
    /// non-turn-driving delivery, rather than being parked until the run ends.
    ///
    /// This test would be red against the pre-fix branch on both halves. There, the arrival-time
    /// assertion below was the *inverse* — `host.injected()` was empty and `pending_inbound_len()`
    /// was 1 — and the delivery only appeared after `INBOUND_FLUSH_DELAY_MS + INBOUND_IDLE_RETRY_MS`
    /// AND only once `set_idle(true)` had been called. CHANGELOG 0.9.3 names the harm the old
    /// behaviour caused: "preventing stale coordination from appearing hours after it was received".
    #[tokio::test]
    async fn busy_interactive_session_steers_inbound_onto_the_live_run_immediately() {
        let s = Arc::new(state(true));
        let host = Arc::new(IdleControlledHost::new(false)); // a run is in flight
        s.set_host_services(host.clone());
        s.set_has_ui(true);

        assert_eq!(
            decide_inbound_policy(s.is_idle(), s.has_ui(), s.config.inbound_trigger, &ask("first")),
            InboundPolicy::Steer
        );
        assert!(send_incoming_message(&s, &from(), &ask("first"), InboundDelivery::Steer));

        let injected = host.injected();
        assert_eq!(injected.len(), 1, "the message reaches the running agent at once: {injected:?}");
        assert!(injected[0].0.contains("first"));
        // `{ deliverAs: "steer" }`, never `{ triggerTurn: true }` (`v0.10.1 index.ts:897-899`): the
        // host routes a custom message to `agent.steer` whenever it is streaming, so the flag stays
        // false and a busy session is never handed a competing run.
        assert!(!injected[0].3, "a steered delivery never drives a second turn");
        assert!(injected[0].2, "display = true, so the message survives a transcript replay");
    }

    /// `v0.10.1 index.ts:880-884` — a STEERED ask rewrites the reply hint to name the message id
    /// explicitly, because it lands mid-run where a bare `intercom({action:"reply"})` has no current
    /// turn context to resolve against. A triggered delivery keeps the bare hint.
    #[tokio::test]
    async fn a_steered_ask_gets_an_explicit_reply_to_in_its_hint() {
        let s = Arc::new(state(true));
        let host = Arc::new(IdleControlledHost::new(false));
        s.set_host_services(host.clone());
        s.set_has_ui(true);

        assert!(send_incoming_message(&s, &from(), &ask("q"), InboundDelivery::Steer));
        let injected = host.injected();
        assert_eq!(injected.len(), 1);
        assert!(
            injected[0].0.contains(
                "To reply, use the intercom tool: intercom({ action: \"reply\", replyTo: \"q1\", message: \"...\" })"
            ),
            "steer hint must carry the explicit message id: {:?}",
            injected[0].0
        );

        // The trigger path is unchanged: the bare hint, because the message IS the turn context.
        host.clear_injected();
        assert!(send_incoming_message(&s, &from(), &ask("q"), InboundDelivery::Trigger));
        let injected = host.injected();
        assert_eq!(injected.len(), 1);
        assert!(
            injected[0]
                .0
                .contains("To reply, use the intercom tool: intercom({ action: \"reply\", message: \"...\" })"),
            "trigger hint stays bare: {:?}",
            injected[0].0
        );
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

    /// ICOM-022 regression (pi `sendIncomingMessage`, `index.ts:652-672`): the string the model
    /// receives IS the string the human sees — `**From {sender}** ({cwd}){replyInstruction}` then
    /// a blank line then the body. Pre-fix the injected content was
    /// `build_inline_message(..).body()`, i.e. the body ALONE: no sender attribution, no cwd, and no
    /// `intercom({action:"reply"})` guidance even though the hint was already computed and shown on
    /// the `append_entry` surface. The model could not tell an intercom message from a user turn.
    #[tokio::test]
    async fn injected_inbound_message_carries_the_pi_attribution_header_and_reply_instruction() {
        let s = Arc::new(state(true));
        let host = Arc::new(IdleControlledHost::new(true));
        s.set_host_services(host.clone());
        s.set_has_ui(true);

        let before = now_ms();
        assert!(trigger_turn_over_inbound(&s, &from(), &ask("Which DB?"), true));
        let after = now_ms();

        let injected = host.injected();
        assert_eq!(injected.len(), 1, "one delivery: {injected:?}");
        let content = &injected[0].0;
        assert!(
            content.starts_with("**From subagent-chat-1** (/w)"),
            "attribution header + sender cwd lead the injected content: {content:?}"
        );
        assert!(
            content.contains("To reply, use the intercom tool: intercom({ action: \"reply\", message: \"...\" })"),
            "the reply instruction reaches the MODEL, not just the append_entry surface: {content:?}"
        );
        assert!(content.ends_with("\n\nWhich DB?"), "body last, after a blank line: {content:?}");
        // The injected string is byte-identical to the card the human surface carries — pi builds
        // it exactly once, off the SAME per-delivery copy it stamps (`v0.10.1 index.ts:878,890-895`:
        // `const injectedMessage = { ...entry.message, injectedAt: Date.now() }`, then
        // `formatInboundDeliveryMetadata(injectedMessage)`). `Date.now()` is a live clock, so the
        // one unknown byte-range is the instant — recovered here by rendering the card for every
        // millisecond the call could have observed and requiring the delivery to be one of them.
        // Every other byte, including the whole `_id … · sent … · injected …_` segment, is pinned,
        // and the stamp is pinned to the delivery instant.
        let candidates: Vec<String> = (before..=after)
            .map(|ms| {
                let mut stamped = ask("Which DB?");
                stamped.injected_at = Some(ms.into());
                build_inline_message(&s, &from(), &stamped).content_markdown()
            })
            .collect();
        assert!(
            candidates.contains(content),
            "injected content must be the card rendered from the injected-at-stamped message; \
             got {content:?}, expected one of {candidates:?}"
        );
        assert_eq!(injected[0].1.as_deref(), Some(INBOUND_MESSAGE_CUSTOM_TYPE));
    }

    /// ICOM-022, applied to the steer path: two peers' messages arriving during one run must be N
    /// separately attributed messages, not N header-less bodies. Pre-fix a busy session's backlog
    /// reached the model as a run of bare bodies, so several peers' messages were indistinguishable
    /// from each other and from the user's own turn.
    #[tokio::test]
    async fn every_steered_message_carries_its_own_attribution_header() {
        let s = Arc::new(state(true));
        let host = Arc::new(IdleControlledHost::new(false)); // a run is in flight
        s.set_host_services(host.clone());
        s.set_has_ui(true);

        // Two DIFFERENT peers ask while this session is busy.
        let mut peer_b = from();
        peer_b.id = "child-9999".to_string();
        peer_b.name = Some("subagent-chat-2".to_string());
        peer_b.cwd = "/other".to_string();
        assert!(send_incoming_message(&s, &from(), &ask("first"), InboundDelivery::Steer));
        assert!(send_incoming_message(&s, &peer_b, &ask("second"), InboundDelivery::Steer));

        let injected = host.injected();
        assert_eq!(injected.len(), 2, "both steers land: {injected:?}");
        assert!(
            injected[0].0.starts_with("**From subagent-chat-1** (/w)"),
            "the first steer is attributed: {injected:?}"
        );
        assert!(
            injected[1].0.starts_with("**From subagent-chat-2** (/other)"),
            "the second steer is attributed to ITS OWN sender: {injected:?}"
        );
        for call in &injected {
            assert!(
                call.0.contains("To reply, use the intercom tool: intercom("),
                "each steered message keeps its reply instruction: {call:?}"
            );
        }
    }

    /// A peer with no `name` falls back to the first 8 chars of its session id (pi
    /// `entry.from.name || entry.from.id.slice(0, 8)`, `index.ts:659`) — asserted on the INJECTED
    /// string so the fallback is proven on the model-facing path.
    #[tokio::test]
    async fn injected_attribution_falls_back_to_the_session_id_slice() {
        let s = Arc::new(state(false)); // reply hint off → no reply instruction at all
        let host = Arc::new(IdleControlledHost::new(true));
        s.set_host_services(host.clone());
        let mut anon = from();
        anon.name = None;

        assert!(trigger_turn_over_inbound(&s, &anon, &ask("hi"), false));

        let injected = host.injected();
        assert_eq!(injected.len(), 1);
        // `v0.10.1 index.ts:891-893`: no `📨` (the v0.10.0 deslop), and the `_…_` delivery-metadata
        // line sits between the header and the body. `injected …` carries a live clock, so the
        // clock-independent parts are asserted exactly.
        let content = &injected[0].0;
        assert!(content.starts_with("**From child-12** (/w)\n\n_id q1 · "), "{content:?}");
        assert!(content.ends_with("_\n\nhi"), "{content:?}");
        assert!(!content.contains("To reply"), "reply hint off ⇒ no instruction: {content:?}");
    }
}
