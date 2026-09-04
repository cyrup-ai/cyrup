//! Reply tracking — two distinct single-slot/pending systems (do not conflate; the port doc §6.7):
//!
//! - [`ReplyTracker`] (inbound asks → local reply) — a port of `pi-intercom/reply-tracker.ts:18-123`.
//! - [`OutboundReplyWaiter`] (one outbound ask → its reply) — the process-global single slot from
//!   `index.ts:455-503,715-724`: a second outbound ask returns "Already waiting for a reply".

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::oneshot;

use crate::transport::protocol::{Message, SessionInfo};

/// One inbound intercom context (`IntercomContext`, `reply-tracker.ts:4-8`).
#[derive(Clone, Debug)]
pub struct IntercomContext {
    /// The sender's session info.
    pub from: SessionInfo,
    /// The received message.
    pub message: Message,
    /// Epoch-ms receipt time (for the sort/prune).
    pub received_at: u64,
}

/// `matchesPendingSender` (`v0.10.1 reply-tracker.ts:10-16`). The `starts_with` arm is v0.9.3
/// (`c3543d6`) — `intercom{list}` prints an ID *prefix* as the addressable column, so the reply path
/// has to accept the same form the roster handed the model.
fn matches_pending_sender(context: &IntercomContext, to: &str) -> bool {
    if context.from.id == to || context.from.id.starts_with(to) {
        return true;
    }
    context.from.name.as_deref().map(str::to_lowercase) == Some(to.to_lowercase())
}

/// `resolvePendingSender` (`v0.10.1 reply-tracker.ts:18-45`, v0.9.3 `c3543d6`): a four-tier ladder —
/// exact id → exact name → id prefix → miss — with a *distinct* message per ambiguity so the caller
/// is told which KIND of collision it hit and how to break it.
///
/// ```text
/// exactIdMatches   == 1 → hit;  > 1 → `Multiple pending asks from session ID "${to}" — specify \`replyTo\``
/// exactNameMatches == 1 → hit;  > 1 → `Multiple pending asks match sender name "${to}" — specify a full session ID or \`replyTo\``
/// idPrefixMatches  == 1 → hit;  > 1 → `Multiple pending asks match ID prefix "${to}" — use a longer session ID prefix or specify \`replyTo\``
/// otherwise            → `No pending ask from "${to}"`
/// ```
///
/// Note the exact-id tier can only exceed one match when two pending asks share a sender, and that
/// the prefix tier is evaluated LAST, so an id that is also somebody's exact name resolves by name.
fn resolve_pending_sender(
    pending: &[IntercomContext],
    to: &str,
) -> Result<IntercomContext, String> {
    let exact_id: Vec<&IntercomContext> = pending.iter().filter(|c| c.from.id == to).collect();
    if exact_id.len() == 1
        && let Some(hit) = exact_id.first()
    {
        return Ok((*hit).clone());
    }
    if exact_id.len() > 1 {
        return Err(format!(
            "Multiple pending asks from session ID \"{to}\" — specify `replyTo`"
        ));
    }

    let lower_to = to.to_lowercase();
    let exact_name: Vec<&IntercomContext> = pending
        .iter()
        .filter(|c| c.from.name.as_deref().map(str::to_lowercase) == Some(lower_to.clone()))
        .collect();
    if exact_name.len() == 1
        && let Some(hit) = exact_name.first()
    {
        return Ok((*hit).clone());
    }
    if exact_name.len() > 1 {
        return Err(format!(
            "Multiple pending asks match sender name \"{to}\" — specify a full session ID or `replyTo`"
        ));
    }

    let id_prefix: Vec<&IntercomContext> = pending
        .iter()
        .filter(|c| c.from.id.starts_with(to))
        .collect();
    if id_prefix.len() == 1
        && let Some(hit) = id_prefix.first()
    {
        return Ok((*hit).clone());
    }
    if id_prefix.len() > 1 {
        return Err(format!(
            "Multiple pending asks match ID prefix \"{to}\" — use a longer session ID prefix or specify `replyTo`"
        ));
    }

    Err(format!("No pending ask from \"{to}\""))
}

/// Inbound ask → local reply tracking (`ReplyTracker`, `reply-tracker.ts:18-123`).
#[derive(Debug)]
pub struct ReplyTracker {
    pending_asks: HashMap<String, IntercomContext>,
    pending_turn_contexts: Vec<IntercomContext>,
    current_turn_context: Option<IntercomContext>,
    ask_timeout_ms: u64,
}

impl ReplyTracker {
    /// A tracker pruning asks older than `ask_timeout_ms`.
    #[must_use]
    pub fn new(ask_timeout_ms: u64) -> Self {
        Self {
            pending_asks: HashMap::new(),
            pending_turn_contexts: Vec::new(),
            current_turn_context: None,
            ask_timeout_ms,
        }
    }

    /// Record an inbound message; asks (`expects_reply`) are added to the pending map
    /// (`recordIncomingMessage`, `reply-tracker.ts:25-31`).
    pub fn record_incoming_message(
        &mut self,
        from: SessionInfo,
        message: Message,
        received_at: u64,
    ) -> IntercomContext {
        let context = IntercomContext {
            from,
            message,
            received_at,
        };
        if context.message.expects_reply == Some(true) {
            self.pending_asks
                .insert(context.message.id.clone(), context.clone());
        }
        context
    }

    /// Queue a context for a future turn (`queueTurnContext`, `reply-tracker.ts:33-35`).
    pub fn queue_turn_context(&mut self, context: IntercomContext) {
        self.pending_turn_contexts.push(context);
    }

    /// Begin a turn: prune expired, then adopt the next queued context (`beginTurn`, `:37-40`).
    pub fn begin_turn(&mut self, now: u64) {
        self.prune_expired(now);
        self.current_turn_context = if self.pending_turn_contexts.is_empty() {
            None
        } else {
            Some(self.pending_turn_contexts.remove(0))
        };
    }

    /// End a turn (`endTurn`, `:42-44`).
    pub fn end_turn(&mut self) {
        self.current_turn_context = None;
    }

    /// Reset all tracking (`reset`, `:46-50`).
    pub fn reset(&mut self) {
        self.pending_asks.clear();
        self.pending_turn_contexts.clear();
        self.current_turn_context = None;
    }

    /// Resolve which inbound ask a `reply` targets (`resolveReplyTarget`, `:52-90`) with the exact
    /// precedence: explicit `reply_to` → explicit `to` → current turn context → single pending →
    /// error. Both explicit hints outrank the inferred target, and the `to`-filter is TERMINAL
    /// (`:67-76`): zero matches errors with `No pending ask from "…"` rather than falling through to
    /// the turn context or the lone pending ask, so an addressed reply can never be misrouted.
    ///
    /// # Errors
    /// Returns a human-readable message when the target cannot be uniquely resolved.
    pub fn resolve_reply_target(
        &mut self,
        to: Option<&str>,
        reply_to: Option<&str>,
        now: u64,
    ) -> Result<IntercomContext, String> {
        self.prune_expired(now);

        if let Some(reply_to) = reply_to {
            let target = self
                .pending_asks
                .get(reply_to)
                .cloned()
                .ok_or_else(|| format!("No pending ask with message ID \"{reply_to}\""))?;
            if let Some(to) = to
                && !matches_pending_sender(&target, to)
            {
                return Err(format!("Pending ask \"{reply_to}\" is not from \"{to}\""));
            }
            return Ok(target);
        }

        let pending: Vec<IntercomContext> = self.pending_asks.values().cloned().collect();

        // `v0.10.1 reply-tracker.ts:96-97`: the whole `to` branch is `resolvePendingSender`, which
        // is still TERMINAL — a miss errors rather than falling through to the turn context or the
        // lone pending ask.
        if let Some(to) = to {
            return resolve_pending_sender(&pending, to);
        }

        if let Some(current) = &self.current_turn_context {
            return Ok(current.clone());
        }

        if pending.len() == 1
            && let Some(only) = pending.first().cloned()
        {
            return Ok(only);
        }

        if pending.is_empty() {
            return Err("No active intercom context to reply to".to_string());
        }
        Err("Multiple pending asks — specify `to`".to_string())
    }

    /// `findUniquePendingAskFrom` (`v0.10.1 reply-tracker.ts:114-123`, v0.9.3 `5d76146` "Resolve
    /// asks answered with send"): the single unexpired pending ask from `to`, or `None`.
    ///
    /// ```text
    /// const candidates = Array.from(this.pendingAsks.values()).filter((context) => {
    ///   if (now - context.receivedAt > this.askTimeoutMs) return false;
    ///   return context.from.id === to || context.from.name?.toLowerCase() === to.toLowerCase();
    /// });
    /// return candidates.length === 1 ? candidates[0]! : null;
    /// ```
    ///
    /// Deliberately NOT `matches_pending_sender`: upstream filters on exact id or exact name here,
    /// with **no** prefix arm, because inferring a reply target is a silent side effect and a prefix
    /// hit is too weak a signal for one. It also does not prune — it filters by age in place —
    /// hence `&self`, so a plain `send` never mutates ask state before its delivery is confirmed.
    #[must_use]
    pub fn find_unique_pending_ask_from(&self, to: &str, now: u64) -> Option<IntercomContext> {
        let lower_to = to.to_lowercase();
        let mut candidates = self.pending_asks.values().filter(|c| {
            if now.saturating_sub(c.received_at) > self.ask_timeout_ms {
                return false;
            }
            c.from.id == to
                || c.from.name.as_deref().map(str::to_lowercase) == Some(lower_to.clone())
        });
        let first = candidates.next()?;
        if candidates.next().is_some() {
            return None;
        }
        Some(first.clone())
    }

    /// Mark an ask replied (`markReplied` → `dismissPendingAsk`, `:95-109`).
    pub fn mark_replied(&mut self, reply_to: &str) {
        self.dismiss_pending_ask(reply_to);
    }

    /// Drop a pending ask + any queued/current turn context for it (`dismissPendingAsk`, `:99-109`).
    pub fn dismiss_pending_ask(&mut self, reply_to: &str) {
        self.pending_asks.remove(reply_to);
        self.pending_turn_contexts
            .retain(|c| c.message.id != reply_to);
        if self
            .current_turn_context
            .as_ref()
            .map(|c| c.message.id.as_str())
            == Some(reply_to)
        {
            self.current_turn_context = None;
        }
    }

    /// Find a pending inbound ask whose message text contains `needle` (used by the ClarifyChannel
    /// seam to correlate a `ClarifyRequest{run_id}` to the child ask this orchestrator RECEIVED —
    /// the body carries `Run: <run_id>`, `formatChildOrchestratorMessage`, `index.ts:104-119`).
    #[must_use]
    pub fn find_pending_containing(&self, needle: &str) -> Option<IntercomContext> {
        self.pending_asks
            .values()
            .find(|c| c.message.content.text.contains(needle))
            .cloned()
    }

    /// List pending asks, sorted by receipt time, pruning expired (`listPending`, `:111-114`).
    pub fn list_pending(&mut self, now: u64) -> Vec<IntercomContext> {
        self.prune_expired(now);
        let mut pending: Vec<IntercomContext> = self.pending_asks.values().cloned().collect();
        pending.sort_by_key(|c| c.received_at);
        pending
    }

    fn prune_expired(&mut self, now: u64) {
        let timeout = self.ask_timeout_ms;
        let expired: Vec<String> = self
            .pending_asks
            .iter()
            .filter(|(_, c)| now.saturating_sub(c.received_at) > timeout)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.dismiss_pending_ask(&id);
        }
    }
}

/// The process-global single-slot outbound reply waiter (`replyWaiter`, `index.ts:455-503`). One
/// outstanding outbound ask at a time; a second [`OutboundReplyWaiter::register`] returns
/// "Already waiting for a reply" (`index.ts:462-464`).
#[derive(Debug, Default)]
pub struct OutboundReplyWaiter {
    slot: Mutex<Option<WaiterSlot>>,
}

#[derive(Debug)]
struct WaiterSlot {
    from: String,
    reply_to: String,
    /// `Ok(reply)` on delivery; `Err(reason)` when the slot is failed out from under the waiter —
    /// pi `rejectReplyWaiter(new Error(...))` (`index.ts:507-509`), which the client `disconnected`
    /// handler fires so an ask cannot hang across a reconnect (`index.ts:783-784`).
    tx: oneshot::Sender<std::result::Result<Message, String>>,
}

impl OutboundReplyWaiter {
    /// A fresh, empty waiter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an outbound ask for a reply from `from` matching `reply_to` (the outbound
    /// `questionId`). Returns the receiver to await, or `Err("Already waiting for a reply")` when a
    /// waiter is already outstanding (`waitForReply`, `index.ts:460-464`).
    ///
    /// # Errors
    /// Returns the busy message when a slot is already occupied.
    pub fn register(
        &self,
        from: String,
        reply_to: String,
    ) -> Result<oneshot::Receiver<std::result::Result<Message, String>>, String> {
        let mut guard = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            return Err("Already waiting for a reply".to_string());
        }
        let (tx, rx) = oneshot::channel();
        *guard = Some(WaiterSlot { from, reply_to, tx });
        Ok(rx)
    }

    /// Try to satisfy the outstanding waiter with an inbound message (`handleIncomingMessage`,
    /// `index.ts:713-724`): the sender must match (`from.name || from.id`, case-insensitive, or the
    /// raw id) and `message.reply_to` must equal the waiter's `reply_to`. Returns `true` if the
    /// waiter was resolved (the caller then stops surfacing this message).
    pub fn try_deliver(&self, from: &SessionInfo, message: &Message) -> bool {
        let mut guard = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        let matched = match guard.as_ref() {
            None => false,
            Some(slot) => {
                let sender_target = from.name.clone().unwrap_or_else(|| from.id.clone());
                let from_matches = sender_target.to_lowercase() == slot.from.to_lowercase()
                    || from.id == slot.from;
                let reply_matches = message.reply_to.as_deref() == Some(slot.reply_to.as_str());
                from_matches && reply_matches
            }
        };
        if matched && let Some(slot) = guard.take() {
            let _ = slot.tx.send(Ok(message.clone()));
            return true;
        }
        matched
    }

    /// Fail whatever ask is outstanding with `reason` — pi `rejectReplyWaiter`
    /// (`index.ts:507-509`), fired from the client `disconnected` handler
    /// (`Disconnected while waiting for reply: …`, `index.ts:783-784`) and from the session
    /// replace/shutdown paths (`index.ts:940,1066`).
    ///
    /// Without this, an ask that was in flight when the socket dropped would sit in its slot until
    /// the full ask timeout (10 min by default) waiting for a reply that can never arrive: the
    /// broker has no mailbox, so the answer is not redelivered after a reconnect. Returns whether a
    /// waiter was actually failed.
    pub fn fail_pending(&self, reason: &str) -> bool {
        let slot = self.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        match slot {
            Some(slot) => {
                let _ = slot.tx.send(Err(reason.to_string()));
                true
            }
            None => false,
        }
    }

    /// Clear the slot if it is the one for `reply_to` (tool cleanup on timeout/cancel/error). Dropping
    /// the sender causes any pending receiver to observe a cancellation.
    pub fn clear_matching(&self, reply_to: &str) {
        let mut guard = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().map(|s| s.reply_to.as_str()) == Some(reply_to) {
            *guard = None;
        }
    }

    /// Whether a waiter is currently outstanding.
    #[must_use]
    pub fn is_waiting(&self) -> bool {
        self.slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use crate::transport::protocol::{MessageContent, now_ms};

    fn session(id: &str, name: Option<&str>) -> SessionInfo {
        SessionInfo {
            id: id.to_string(),
            name: name.map(str::to_string),
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
            tmux_pane: None,
            extra: Default::default(),
        }
    }

    fn ask(id: &str) -> Message {
        Message {
            id: id.to_string(),
            timestamp: 0u64.into(),
            reply_to: None,
            expects_reply: Some(true),
            content: MessageContent {
                text: "q".to_string(),
                attachments: None,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    // reply-tracker.test.ts:28-60 — explicit replyTo resolves the exact pending ask.
    #[test]
    fn resolve_by_explicit_reply_to() {
        let mut rt = ReplyTracker::new(600_000);
        rt.record_incoming_message(session("s1", Some("alice")), ask("q1"), now_ms());
        rt.record_incoming_message(session("s2", Some("bob")), ask("q2"), now_ms());
        let target = rt
            .resolve_reply_target(None, Some("q2"), now_ms())
            .expect("resolves");
        assert_eq!(target.message.id, "q2");
        assert_eq!(target.from.id, "s2");
    }

    #[test]
    fn resolve_single_pending_without_hint() {
        let mut rt = ReplyTracker::new(600_000);
        rt.record_incoming_message(session("s1", Some("alice")), ask("q1"), now_ms());
        let target = rt
            .resolve_reply_target(None, None, now_ms())
            .expect("single pending resolves");
        assert_eq!(target.message.id, "q1");
    }

    #[test]
    fn resolve_multiple_pending_needs_hint() {
        let mut rt = ReplyTracker::new(600_000);
        rt.record_incoming_message(session("s1", Some("alice")), ask("q1"), now_ms());
        rt.record_incoming_message(session("s2", Some("bob")), ask("q2"), now_ms());
        let err = rt
            .resolve_reply_target(None, None, now_ms())
            .expect_err("ambiguous");
        assert!(err.contains("specify"));
        // With a `to` hint that uniquely matches, it resolves.
        let target = rt
            .resolve_reply_target(Some("alice"), None, now_ms())
            .expect("resolves by name");
        assert_eq!(target.from.id, "s1");
    }

    // reply-tracker.test.ts:57-66 — "explicit to overrides the current turn context". An explicit
    // `to` is evaluated BEFORE the current turn context, so a reply addressed to the reviewer must
    // route to the reviewer's ask even while the planner's ask is the active turn context.
    #[test]
    fn explicit_to_overrides_current_turn_context() {
        let mut rt = ReplyTracker::new(600_000);
        let current =
            rt.record_incoming_message(session("planner-id", Some("planner")), ask("ask-1"), 1000);
        rt.record_incoming_message(session("reviewer-id", Some("reviewer")), ask("ask-2"), 1001);
        rt.queue_turn_context(current);
        rt.begin_turn(1002);

        // Sanity: with no `to`, the current turn context still wins (the other ordering).
        let bare = rt
            .resolve_reply_target(None, None, 1003)
            .expect("current turn context resolves");
        assert_eq!(
            bare.message.id, "ask-1",
            "no `to` must fall back to the current turn context"
        );
        assert_eq!(bare.from.id, "planner-id");

        // The inverted-precedence bug: `to` must beat the current turn context.
        let target = rt
            .resolve_reply_target(Some("reviewer"), None, 1003)
            .expect("`to` resolves");
        assert_eq!(
            target.message.id, "ask-2",
            "explicit `to` must override the current turn context"
        );
        assert_eq!(target.from.id, "reviewer-id");

        // And a `to` that matches nothing must error rather than silently falling back.
        let err = rt
            .resolve_reply_target(Some("missing"), None, 1003)
            .expect_err("unmatched `to` errors");
        assert_eq!(err, "No pending ask from \"missing\"");
    }

    // An unmatched `to` must error even when exactly ONE ask is pending — upstream's
    // `No pending ask from "..."` throw at reply-tracker.ts:75 is unconditional.
    #[test]
    fn explicit_to_beats_the_single_pending_shortcut() {
        let mut rt = ReplyTracker::new(600_000);
        rt.record_incoming_message(session("planner-id", Some("planner")), ask("ask-1"), 1000);

        // Sanity: with no `to`, the single pending ask resolves (the other ordering).
        let bare = rt
            .resolve_reply_target(None, None, 1001)
            .expect("single pending resolves");
        assert_eq!(bare.message.id, "ask-1");

        let err = rt
            .resolve_reply_target(Some("reviewer"), None, 1001)
            .expect_err("a `to` naming nobody must not fall through to the lone pending ask");
        assert_eq!(err, "No pending ask from \"reviewer\"");
    }

    // reply-tracker.test.ts:47-55 — `to` matches by session id or by case-insensitive name.
    #[test]
    fn explicit_to_matches_by_id_or_name() {
        let mut rt = ReplyTracker::new(600_000);
        rt.record_incoming_message(session("planner-id", Some("planner")), ask("ask-1"), 1000);
        rt.record_incoming_message(session("reviewer-id", Some("reviewer")), ask("ask-2"), 1001);

        assert_eq!(
            rt.resolve_reply_target(Some("reviewer"), None, 1002)
                .expect("by name")
                .message
                .id,
            "ask-2"
        );
        assert_eq!(
            rt.resolve_reply_target(Some("planner-id"), None, 1002)
                .expect("by id")
                .message
                .id,
            "ask-1"
        );
        assert_eq!(
            rt.resolve_reply_target(Some("REVIEWER"), None, 1002)
                .expect("case-insensitive")
                .message
                .id,
            "ask-2"
        );
    }

    // reply-tracker.ts:72-74 — two pending asks from the same sender is ambiguous, even when a
    // turn context is active (which under the inverted order would have masked the error).
    #[test]
    fn explicit_to_with_multiple_matches_errors_over_turn_context() {
        let mut rt = ReplyTracker::new(600_000);
        let first =
            rt.record_incoming_message(session("planner-id", Some("planner")), ask("ask-1"), 1000);
        rt.record_incoming_message(session("planner-id", Some("planner")), ask("ask-2"), 1001);
        rt.queue_turn_context(first);
        rt.begin_turn(1002);

        // `v0.10.1 reply-tracker.ts:30-32`: two asks from ONE sender addressed by that sender's
        // NAME is the sender-name tier, so it is the "specify a full session ID or `replyTo`" text.
        let err = rt
            .resolve_reply_target(Some("planner"), None, 1003)
            .expect_err("ambiguous `to`");
        assert_eq!(
            err,
            "Multiple pending asks match sender name \"planner\" — specify a full session ID or `replyTo`"
        );
    }

    /// `v0.10.1 reply-tracker.ts:18-45` — all four tiers of `resolvePendingSender`, one assertion
    /// per distinct upstream message. Before v0.9.3 cyrup collapsed the three ambiguity cases into a
    /// single generic string that named no candidates and no remedy.
    #[test]
    fn resolve_pending_sender_ladder_has_four_distinct_upstream_messages() {
        // Tier 1: exact id, two asks from the same sender id.
        let mut rt = ReplyTracker::new(600_000);
        rt.record_incoming_message(session("0192aaaa-1111", None), ask("a1"), 1000);
        rt.record_incoming_message(session("0192aaaa-1111", None), ask("a2"), 1001);
        let err = rt
            .resolve_reply_target(Some("0192aaaa-1111"), None, 1002)
            .expect_err("exact-id collision");
        assert_eq!(
            err,
            "Multiple pending asks from session ID \"0192aaaa-1111\" — specify `replyTo`"
        );

        // Tier 2: exact name shared by two DIFFERENT sender ids (what ICOM-040's 8-char alias made
        // routine). Note the ids share no prefix with the name, so the name tier is the one hit.
        let mut rt = ReplyTracker::new(600_000);
        rt.record_incoming_message(session("id-a", Some("worker")), ask("b1"), 1000);
        rt.record_incoming_message(session("id-b", Some("worker")), ask("b2"), 1001);
        let err = rt
            .resolve_reply_target(Some("worker"), None, 1002)
            .expect_err("name collision");
        assert_eq!(
            err,
            "Multiple pending asks match sender name \"worker\" — specify a full session ID or `replyTo`"
        );

        // Tier 3: two distinct UUIDv7 senders sharing an id prefix.
        let mut rt = ReplyTracker::new(600_000);
        rt.record_incoming_message(session("0192f3c1-aaaa", Some("alpha")), ask("c1"), 1000);
        rt.record_incoming_message(session("0192f3c1-bbbb", Some("beta")), ask("c2"), 1001);
        let err = rt
            .resolve_reply_target(Some("0192f3c1"), None, 1002)
            .expect_err("prefix collision");
        assert_eq!(
            err,
            "Multiple pending asks match ID prefix \"0192f3c1\" — use a longer session ID prefix or specify `replyTo`"
        );
        // …and a UNIQUE prefix resolves, which is the feature: `list` prints prefixes, so `reply`
        // must accept them.
        let hit = rt
            .resolve_reply_target(Some("0192f3c1-a"), None, 1002)
            .expect("unique prefix resolves");
        assert_eq!(hit.message.id, "c1");

        // Tier 4: miss.
        let err = rt
            .resolve_reply_target(Some("nobody"), None, 1002)
            .expect_err("miss");
        assert_eq!(err, "No pending ask from \"nobody\"");
    }

    /// `v0.10.1 reply-tracker.ts:114-123`. A plain `send` to the sole pending asker is that ask's
    /// reply (v0.9.3 `5d76146`); two asks from the same peer make it ambiguous, and an expired ask
    /// is not a candidate at all.
    #[test]
    fn find_unique_pending_ask_from_matches_by_id_or_name_and_honours_the_timeout() {
        let mut rt = ReplyTracker::new(600_000);
        rt.record_incoming_message(session("s1", Some("Alice")), ask("q1"), 1000);
        rt.record_incoming_message(session("s2", Some("bob")), ask("q2"), 1000);

        assert_eq!(
            rt.find_unique_pending_ask_from("s1", 1001)
                .map(|c| c.message.id),
            Some("q1".to_string())
        );
        assert_eq!(
            rt.find_unique_pending_ask_from("alice", 1001)
                .map(|c| c.message.id),
            Some("q1".to_string())
        );
        assert!(rt.find_unique_pending_ask_from("nobody", 1001).is_none());

        // Upstream has NO prefix arm here — inference is silent, so it demands an exact hit.
        assert!(rt.find_unique_pending_ask_from("s", 1001).is_none());

        // Two asks from one sender → ambiguous → None (no inference, the send stays a plain send).
        rt.record_incoming_message(session("s1", Some("Alice")), ask("q3"), 1000);
        assert!(rt.find_unique_pending_ask_from("s1", 1001).is_none());

        // Past the ask timeout the candidate is filtered out rather than inferred.
        let mut rt = ReplyTracker::new(100);
        rt.record_incoming_message(session("s1", Some("Alice")), ask("q1"), 1000);
        assert!(rt.find_unique_pending_ask_from("s1", 1_000_000).is_none());
    }

    // reply-tracker.ts:55-64 — `reply_to` still outranks `to`, and `to` is only a cross-check.
    #[test]
    fn explicit_reply_to_still_outranks_to() {
        let mut rt = ReplyTracker::new(600_000);
        rt.record_incoming_message(session("planner-id", Some("planner")), ask("ask-1"), 1000);
        rt.record_incoming_message(session("reviewer-id", Some("reviewer")), ask("ask-2"), 1001);

        let target = rt
            .resolve_reply_target(None, Some("ask-2"), 1002)
            .expect("reply_to resolves");
        assert_eq!(target.from.id, "reviewer-id");

        let err = rt
            .resolve_reply_target(Some("planner"), Some("ask-2"), 1002)
            .expect_err("mismatched pair");
        assert_eq!(err, "Pending ask \"ask-2\" is not from \"planner\"");
    }

    #[test]
    fn list_pending_sorted_and_pruned() {
        let mut rt = ReplyTracker::new(100);
        rt.record_incoming_message(session("s1", None), ask("q1"), 0);
        rt.record_incoming_message(session("s2", None), ask("q2"), 50);
        // At now=1000 both are older than the 100ms timeout → pruned.
        assert!(rt.list_pending(1000).is_empty());
    }

    #[test]
    fn mark_replied_dismisses_the_ask() {
        let mut rt = ReplyTracker::new(600_000);
        rt.record_incoming_message(session("s1", None), ask("q1"), now_ms());
        rt.mark_replied("q1");
        assert!(rt.resolve_reply_target(None, Some("q1"), now_ms()).is_err());
    }

    #[test]
    fn outbound_waiter_is_single_slot() {
        let waiter = OutboundReplyWaiter::new();
        let _rx = waiter
            .register("supervisor".to_string(), "q1".to_string())
            .expect("first registers");
        let second = waiter.register("supervisor".to_string(), "q2".to_string());
        assert_eq!(second.err().as_deref(), Some("Already waiting for a reply"));
    }

    #[tokio::test]
    async fn outbound_waiter_delivers_matching_reply() {
        let waiter = OutboundReplyWaiter::new();
        let rx = waiter
            .register("supervisor".to_string(), "q1".to_string())
            .expect("registers");
        let reply = Message {
            id: "r1".to_string(),
            timestamp: 0u64.into(),
            reply_to: Some("q1".to_string()),
            expects_reply: None,
            content: MessageContent {
                text: "answer".to_string(),
                attachments: None,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(waiter.try_deliver(&session("supervisor", Some("supervisor")), &reply));
        let got = rx.await.expect("received").expect("delivered, not failed");
        assert_eq!(got.content.text, "answer");
        assert!(!waiter.is_waiting(), "slot freed after delivery");
    }

    /// The disconnect edge must FAIL an in-flight ask with pi's reason
    /// (`Disconnected while waiting for reply: …`, `index.ts:783-784`) rather than leaving it to
    /// hang until the ask timeout — the answer can never arrive, because the broker has no mailbox.
    #[tokio::test]
    async fn outbound_waiter_fails_pending_on_disconnect() {
        let waiter = OutboundReplyWaiter::new();
        let rx = waiter
            .register("supervisor".to_string(), "q1".to_string())
            .expect("registers");
        assert!(waiter.fail_pending("Disconnected while waiting for reply: socket closed"));
        let reason = rx
            .await
            .expect("resolved immediately")
            .expect_err("failed, not delivered");
        assert_eq!(
            reason,
            "Disconnected while waiting for reply: socket closed"
        );
        assert!(!waiter.is_waiting(), "slot freed by the failure");
        assert!(!waiter.fail_pending("again"), "nothing left to fail");
    }

    #[test]
    fn outbound_waiter_ignores_non_matching_reply() {
        let waiter = OutboundReplyWaiter::new();
        let _rx = waiter
            .register("supervisor".to_string(), "q1".to_string())
            .expect("registers");
        let wrong = Message {
            id: "r1".to_string(),
            timestamp: 0u64.into(),
            reply_to: Some("q-other".to_string()),
            expects_reply: None,
            content: MessageContent {
                text: "answer".to_string(),
                attachments: None,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!waiter.try_deliver(&session("supervisor", None), &wrong));
        assert!(
            waiter.is_waiting(),
            "slot stays occupied on a non-matching reply"
        );
    }
}
