//! The `cyrup-ext-subagents` seam impls (`tui::intercom::{ClarifyChannel, DeliveryChannel}`) backed
//! by the broker client — the port doc §8.2/§8.3.
//!
//! These CLOSE R-SA-119/120/123/124/125's `NoOp`/`NoTransport` stubs. They are constructed by
//! [`crate::extension::IntercomExtension`] and exposed via its `clarify_channel()`/
//! `delivery_channel()` accessors.
//!
//! HANDOFF (WIRED, the port doc §8.4 item 1 + §5 Phase 5): the two `Arc<dyn …Channel>` these
//! produce are handed to `SubagentsExtension::with_channels(config, cwd, delivery, clarify)` at the
//! `crates/cyrup/src/main.rs` session-build sites (all three modes), replacing
//! `cyrup-ext-subagents`' `NoTransportChannel`/no-live-`AskLock` degrade defaults with these real
//! broker-backed impls. The subagents run driver then consumes them: `deliver_group_out_of_band`
//! (extension.rs) invokes the `DeliveryChannel`, and the exec detach-trigger arm (exec/mod.rs)
//! fires the `ClarifyChannel` via `spawn_clarify`.
//!
//! HUMAN SURFACE (P4, the port doc §4.1 Route B + §5 Phase 4): [`IntercomClarifyChannel::ask`] now
//! obtains a human answer for the child's ask through the live `HostServices` late-bound via P-1
//! (`SharedIntercomState::host_services`, bound by `IntercomExtension::set_host_services` before
//! `init`). It (1) correlates the `ClarifyRequest` to the `(child_target, question_id)` this
//! orchestrator RECEIVED over the broker, (2) surfaces `request.prompt` to the human via
//! `HostServices::input` (the push dialog; the inbound loop already pull-surfaced the ask via
//! `append_entry`), and (3) routes the answer back to the STILL-ALIVE child over the broker
//! (`client.send(child, { reply_to: question_id, text: answer })`, `index.ts:1295-1373`). When no
//! `HostServices` is bound (headless/degraded) or the human declines, it returns `Err` →
//! `ClarifyOutcome::NoLiveChannel` (the documented graceful fallback, R-SA-119).
//!
//! HANDOFF (WIRED, the port doc §8.4 item 1): the `Arc<dyn ClarifyChannel>` this produces is handed
//! to the subagents run driver via `SubagentsExtension::with_channels(.., clarify)` at the
//! `crates/cyrup/src/main.rs` sites; the exec detach-trigger arm fires it (`spawn_clarify`) when a
//! child's blocking `contact_supervisor` ask surfaces. The channel's human-answer leg is also
//! exercised end-to-end by the `tests/human_surface.rs` integration proof.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use cyrup_ext::DialogOptions;
use cyrup_ext_subagents::tui::intercom::{ClarifyChannel, ClarifyRequest, DeliveryChannel, IntercomPayload, SteerChannel};

use crate::relay::format_result_relay;
use crate::session_state::SharedIntercomState;
use crate::transport::client::SendOptions;

/// The placeholder shown in the clarify input dialog (pi's supervisor reply prompt).
const CLARIFY_INPUT_PLACEHOLDER: &str = "Reply to the subagent";

/// Broker-backed out-of-band result delivery (closes R-SA-123/124/125). Relays an allowlisted
/// [`IntercomPayload`] to this orchestrator's own supervisor over the broker (`index.ts:969-1027`).
pub struct IntercomDeliveryChannel {
    state: Arc<SharedIntercomState>,
    /// This orchestrator's supervisor target (its own `orchestrator_target`), when it is itself a
    /// child. `None` for a top-level orchestrator — which surfaces the result LOCALLY instead
    /// (`deliverLocalSubagentRelayMessage`, `index.ts:889-910`) via the live `HostServices`
    /// (`append_entry` + a `inject_message` trigger-turn), see [`IntercomDeliveryChannel::send`].
    supervisor_target: Option<String>,
}

impl IntercomDeliveryChannel {
    /// Build the channel over the shared state + this orchestrator's supervisor target (if any).
    #[must_use]
    pub fn new(state: Arc<SharedIntercomState>, supervisor_target: Option<String>) -> Self {
        Self { state, supervisor_target }
    }
}

impl DeliveryChannel for IntercomDeliveryChannel {
    fn send(&self, payload: IntercomPayload) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
        Box::pin(async move {
            // Format ONLY the allowlisted fields into the relay body (R-SA-124 preserved).
            let text = format_result_relay(&payload);
            let Some(target) = self.supervisor_target.clone() else {
                // Top-level orchestrator: no supervisor to relay to, so surface the result LOCALLY
                // (`deliverLocalSubagentRelayMessage`, `index.ts:889-910` → `sendIncomingMessage(entry,
                // "trigger", …, true)`) through the live `HostServices` bound via P-1 — append a visible
                // transcript entry (matching the inbound surface) AND inject a turn-triggering message
                // over it. When no `HostServices` is bound (headless/degraded), degrade to `Ok(false)`
                // so `cyrup-ext-subagents` keeps the full inline payload.
                let Some(services) = self.state.host_services() else {
                    return Ok(false);
                };
                let entry = serde_json::json!({
                    "content": text,
                    "runId": payload.run_id.as_str(),
                    "agent": &payload.agent,
                    "success": payload.success,
                });
                // Best-effort surface + trigger-turn: neither leg's failure changes "delivered
                // locally" (a bound live session IS the local delivery target, pi's own semantics).
                let _ = services.append_entry("subagent-result", &entry);
                let _ = services.inject_message(&text, Some("subagent-result"), true, true);
                return Ok(true);
            };
            let Some(client) = self.state.client() else {
                return Ok(false); // not connected → degrade (keep full inline).
            };
            let resolved = match self.state.resolve_target(&client, &target).await {
                Ok(Some(id)) => id,
                _ => target,
            };
            match client.send(&resolved, SendOptions { text, ..Default::default() }).await {
                Ok(result) => Ok(result.delivered),
                Err(e) => Err(e.to_string()),
            }
        })
    }
}

/// Broker-backed live-child steer channel (closes R-SA-086's follow-up delivery). Delivers an
/// UNSOLICITED steer message to an already-registered subagent child, addressed by its deterministic
/// broker presence target (`resolve_subagent_intercom_target`). This is the transport
/// [`cyrup_ext_subagents`]'s `control_resume` `SteerRunning` arm drives — pi
/// `deliverSubagentIntercomMessageEvent(events, target.intercomTarget, …)`
/// (`subagent-executor.ts:860-878`). Distinct from [`IntercomDeliveryChannel`] (fixed supervisor
/// target) and [`IntercomClarifyChannel`] (reply-only): only this seam sends a fresh, unsolicited
/// message to an ARBITRARY resolved child target.
pub struct IntercomSteerChannel {
    state: Arc<SharedIntercomState>,
}

impl IntercomSteerChannel {
    /// Build the channel over the shared state.
    #[must_use]
    pub fn new(state: Arc<SharedIntercomState>) -> Self {
        Self { state }
    }
}

impl SteerChannel for IntercomSteerChannel {
    fn steer(&self, target: String, text: String) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
        Box::pin(async move {
            // Not connected → no registered receiver reachable (the genuine delivery-failed fallback,
            // pi's "intercom target is not registered" precondition), NOT a transport error.
            let Some(client) = self.state.client() else {
                return Ok(false);
            };
            // Resolve the deterministic target (name) to a live session id when one is registered;
            // fall back to the raw target so the broker's own id/name/prefix resolution still runs.
            let resolved = match self.state.resolve_target(&client, &target).await {
                Ok(Some(id)) => id,
                _ => target,
            };
            // Unsolicited send (no `reply_to`/`expects_reply`) — same call shape as
            // `IntercomDeliveryChannel::send`. `delivered` is pi's `delivered === true` signal: a
            // registered child took delivery.
            match client.send(&resolved, SendOptions { text, ..Default::default() }).await {
                Ok(result) => Ok(result.delivered),
                Err(e) => Err(e.to_string()),
            }
        })
    }
}

/// Broker-backed clarify/ask channel (closes R-SA-119/120). Correlates a child's blocking ask —
/// received by this orchestrator over the broker — to its `(child_target, question_id)`, surfaces the
/// prompt to a human via `HostServices` (under the shared human-interaction lock), then routes the
/// answer back to the still-alive child. See the module "HUMAN SURFACE" doc + [`Self::ask`].
pub struct IntercomClarifyChannel {
    state: Arc<SharedIntercomState>,
}

impl IntercomClarifyChannel {
    /// Build the channel over the shared state.
    #[must_use]
    pub fn new(state: Arc<SharedIntercomState>) -> Self {
        Self { state }
    }

    /// Correlate a `ClarifyRequest` to the inbound child ask this orchestrator received over the
    /// broker, returning `(child_target_id, question_id)` — the address + edge the P4 reply-routing
    /// leg uses. Matches the pending ask whose body carries `Run: <run_id>`.
    #[must_use]
    pub fn correlate(&self, request: &ClarifyRequest) -> Option<(String, String)> {
        let needle = format!("Run: {}", request.run_id.as_str());
        let ctx = self
            .state
            .tracker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .find_pending_containing(&needle)?;
        Some((ctx.from.id, ctx.message.id))
    }
}

impl ClarifyChannel for IntercomClarifyChannel {
    fn ask(&self, request: ClarifyRequest) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
        Box::pin(async move {
            // (1) Correlate to the child ask this orchestrator RECEIVED over the broker.
            let (child_target, question_id) = self.correlate(&request).ok_or_else(|| {
                format!("no pending child ask correlates to run {}", request.run_id.as_str())
            })?;

            // The live human source (P-1) + the broker client to route the answer back.
            let services = self
                .state
                .host_services()
                .ok_or_else(|| "no live host services (P-1 unbound): cannot surface the ask".to_string())?;
            let client = self
                .state
                .client()
                .ok_or_else(|| "intercom not connected: cannot route the human answer to the child".to_string())?;

            // C3 (reconciliation §1 / §4 step 6): acquire the ONE host-owned, session-scoped human-
            // interaction lock BEFORE surfacing the prompt, WAITING if the permission gate's `ask`
            // dialog (or another in-flight clarify) currently holds it. Both companions share this SAME
            // lock (`HostServices::human_interaction_lock`), so an intercom clarify and a permission
            // approval can never prompt the same human simultaneously — it replaces the two former
            // private single-slot locks (this crate's per-session `AskLock` slot, permission's
            // `Semaphore(1)`). Held (RAII, owned permit) across the blocking `input` below; released
            // before the answer is routed back to the child (which never prompts the human). Absent
            // (headless / no live backend) ⇒ nothing to serialize (there is no human to double-prompt).
            let human_guard = match services.human_interaction_lock() {
                Some(lock) => Some(lock.acquire().await),
                None => None,
            };

            // (2) Surface `request.prompt` to the human and block on their reply. The sync
            //     `HostServices` dialog bridge uses `block_in_place`+`block_on` (host_services.rs:404),
            //     so drive it on a blocking thread rather than this async task.
            let prompt = request.prompt.clone();
            let answer = tokio::task::spawn_blocking(move || {
                services.input(&prompt, Some(CLARIFY_INPUT_PLACEHOLDER), &DialogOptions::default())
            })
            .await
            .map_err(|e| format!("clarify input task failed: {e}"))?;
            // The human has answered — release the shared human-interaction lock so the permission gate
            // (or the next clarify) may prompt while this answer is routed back over the broker.
            drop(human_guard);
            let answer = answer
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .ok_or_else(|| "human declined to answer the subagent's ask".to_string())?;

            // (3) Route the answer back to the STILL-ALIVE child over the broker (reply_to =
            //     questionId), so the child's blocking `contact_supervisor` call unblocks.
            client
                .send(&child_target, SendOptions {
                    text: answer.clone(),
                    reply_to: Some(question_id.clone()),
                    ..Default::default()
                })
                .await
                .map_err(|e| e.to_string())?;

            // The ask is answered — dismiss it (pi `replyTracker.markReplied`, index.ts:1349).
            self.state
                .tracker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .mark_replied(&question_id);
            Ok(answer)
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use crate::config::IntercomConfig;
    use crate::identity::{ChildMessageKind, ChildOrchestratorMetadata, format_child_orchestrator_message};
    use crate::transport::protocol::{Message, MessageContent, SessionInfo, now_ms};

    fn meta() -> ChildOrchestratorMetadata {
        ChildOrchestratorMetadata {
            orchestrator_target: "supervisor".to_string(),
            orchestrator_session_id: None,
            run_id: "run-xyz".to_string(),
            agent: "researcher".to_string(),
            index: "0".to_string(),
            session_name: Some("subagent-chat-1".to_string()),
        }
    }

    #[test]
    fn clarify_correlates_child_ask_by_run_id() {
        let state = Arc::new(SharedIntercomState::new(IntercomConfig::default(), 600_000, std::path::PathBuf::from("/w")));
        // Simulate the orchestrator having received the child's ask over the broker.
        let body = format_child_orchestrator_message(ChildMessageKind::Ask, &meta(), "Which DB?");
        let from = SessionInfo {
            id: "child-session".to_string(),
            name: Some("subagent-chat-1".to_string()),
            cwd: "/w".to_string(),
            model: "m".to_string(),
            pid: 1,
            started_at: 0,
            last_activity: 0,
            status: None,
            peer_uid: None,
            trusted_local: None,
        };
        let msg = Message {
            id: "question-123".to_string(),
            timestamp: 0,
            reply_to: None,
            expects_reply: Some(true),
            content: MessageContent { text: body, attachments: None },
        };
        state.tracker.lock().unwrap().record_incoming_message(from, msg, now_ms());

        let channel = IntercomClarifyChannel::new(state);
        let request = ClarifyRequest {
            run_id: cyrup_ext_subagents::background::RunId::from_token("run-xyz"),
            step_index: Some(0),
            prompt: "Which DB?".to_string(),
        };
        let (child_target, question_id) = channel.correlate(&request).expect("correlates the child ask");
        assert_eq!(child_target, "child-session");
        assert_eq!(question_id, "question-123");
    }

    #[tokio::test]
    async fn clarify_ask_degrades_to_no_live_channel_without_host_services() {
        let state = Arc::new(SharedIntercomState::new(IntercomConfig::default(), 600_000, std::path::PathBuf::from("/w")));
        // Record a matching pending child ask so correlation SUCCEEDS — the Err below is then
        // unambiguously the "no host services bound" (P-1 unbound) degrade, not a correlation miss.
        let body = format_child_orchestrator_message(ChildMessageKind::Ask, &meta(), "Which DB?");
        let from = SessionInfo {
            id: "child-session".to_string(),
            name: Some("subagent-chat-1".to_string()),
            cwd: "/w".to_string(),
            model: "m".to_string(),
            pid: 1,
            started_at: 0,
            last_activity: 0,
            status: None,
            peer_uid: None,
            trusted_local: None,
        };
        let msg = Message {
            id: "question-123".to_string(),
            timestamp: 0,
            reply_to: None,
            expects_reply: Some(true),
            content: MessageContent { text: body, attachments: None },
        };
        state.tracker.lock().unwrap().record_incoming_message(from, msg, now_ms());

        let channel = IntercomClarifyChannel::new(state);
        let request = ClarifyRequest {
            run_id: cyrup_ext_subagents::background::RunId::from_token("run-xyz"),
            step_index: Some(0),
            prompt: "ok?".to_string(),
        };
        // No HostServices bound → Err (→ ClarifyOutcome::NoLiveChannel); never blocks/panics.
        let err = channel.ask(request).await.expect_err("no human source (P4 unbound) → NoLiveChannel");
        assert!(err.contains("host services"), "degrade Err names the unbound host services: {err}");
    }

    #[tokio::test]
    async fn steer_without_a_connected_client_reports_not_delivered() {
        // No broker client bound → the steer degrades to `Ok(false)` (the genuine "not registered"
        // delivery-failed fallback `control_resume` surfaces as pi's guidance), never `Err`/panic.
        let state = Arc::new(SharedIntercomState::new(IntercomConfig::default(), 600_000, std::path::PathBuf::from("/w")));
        let channel = IntercomSteerChannel::new(state);
        assert_eq!(
            channel.steer("subagent-worker-run-1-1".to_string(), "Follow-up".to_string()).await,
            Ok(false)
        );
    }

    #[tokio::test]
    async fn delivery_without_supervisor_target_reports_not_delivered() {
        let state = Arc::new(SharedIntercomState::new(IntercomConfig::default(), 600_000, std::path::PathBuf::from("/w")));
        let channel = IntercomDeliveryChannel::new(state, None);
        let payload = IntercomPayload {
            run_id: cyrup_ext_subagents::background::RunId::from_token("run00000000000001"),
            agent: "researcher".to_string(),
            success: true,
            outputs: vec!["done".to_string()],
            status: cyrup_ext_subagents::tui::intercom::SubagentResultStatus::Completed,
            summary: "1 completed".to_string(),
            child_statuses: vec![cyrup_ext_subagents::tui::intercom::SubagentResultStatus::Completed],
            total_tokens: 10,
        };
        // No supervisor + no host services → Ok(false) (degrade, keep full inline).
        assert_eq!(channel.send(payload).await, Ok(false));
    }

    /// One captured `inject_message` call: `(content, custom_type, display, trigger_turn)`.
    type InjectedCall = (String, Option<String>, bool, bool);

    /// A `HostServices` recorder capturing the top-level LOCAL surface — `append_entry` +
    /// `inject_message` (D: `deliverLocalSubagentRelayMessage`).
    #[derive(Default)]
    struct SurfaceRecorder {
        entries: std::sync::Mutex<Vec<(String, serde_json::Value)>>,
        injected: std::sync::Mutex<Vec<InjectedCall>>,
    }
    impl cyrup_ext::HostServices for SurfaceRecorder {
        fn append_entry(&self, custom_type: &str, data: &serde_json::Value) -> std::result::Result<String, String> {
            self.entries.lock().unwrap().push((custom_type.to_string(), data.clone()));
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

    #[tokio::test]
    async fn top_level_delivery_surfaces_locally_and_reports_delivered() {
        // D: a top-level orchestrator (no supervisor_target) with a live HostServices surfaces the
        // relay LOCALLY (append_entry + a trigger-turn inject_message) and reports delivered=true.
        let state = Arc::new(SharedIntercomState::new(IntercomConfig::default(), 600_000, std::path::PathBuf::from("/w")));
        let rec = Arc::new(SurfaceRecorder::default());
        state.set_host_services(rec.clone());
        let channel = IntercomDeliveryChannel::new(state, None);
        let payload = IntercomPayload {
            run_id: cyrup_ext_subagents::background::RunId::from_token("run00000000000002"),
            agent: "researcher".to_string(),
            success: true,
            outputs: vec!["the answer".to_string()],
            total_tokens: 42,
            status: cyrup_ext_subagents::tui::intercom::SubagentResultStatus::Completed,
            summary: "1 completed".to_string(),
            child_statuses: vec![cyrup_ext_subagents::tui::intercom::SubagentResultStatus::Completed],
        };
        assert_eq!(channel.send(payload).await, Ok(true));

        let entries = rec.entries.lock().unwrap();
        assert_eq!(entries.len(), 1, "the relay is appended locally once");
        assert_eq!(entries[0].0, "subagent-result");
        assert_eq!(entries[0].1["runId"], "run00000000000002");

        let injected = rec.injected.lock().unwrap();
        assert_eq!(injected.len(), 1, "a turn is triggered over the relay");
        assert!(injected[0].0.contains("the answer"), "the relay body carries the output");
        assert!(injected[0].3, "trigger_turn is true (pi's sendIncomingMessage(entry, \"trigger\"))");
    }
}
