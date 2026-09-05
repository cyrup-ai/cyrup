//! The extension control-op drain (SEAM-003 / EXT-005).
//!
//! Pi `ExtensionCommandContextActions` (extensions/types.ts:1652-1672). Guests queue control ops
//! synchronously across the wasm boundary; this applies them at the command tier — routing the
//! runtime-tier ops (`new_session`/`switch`/`fork`/`reload`) to the installed
//! [`crate::RuntimeActions`] and the session-local ones in place.

use std::sync::atomic::Ordering;

use cyrup_agent::AgentMessage;
use cyrup_core::{EntryId, ModelId, ProviderId};
use cyrup_ext::host::ControlOp;

use crate::error::SessionServiceError;

use super::types::NavigateTreeOptions;
use super::{AgentSession, now_ms};

/// Upper bound on a `ControlOp::WaitIdle` drained at the command tier (SEAM-003). Pi's
/// `ctx.waitForIdle()` is a promise resolved by `_resolveIdleWaitIfIdle` and cannot wedge the
/// command path; cyrup's waits on the post-run driver watch, which a CONCURRENT run could hold
/// indefinitely. The op is bounded and its expiry reported rather than hanging the drain.
const WAIT_IDLE_CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// The op's Pi-facing name, for the SEAM-003 failure diagnostic.
fn control_op_name(op: &ControlOp) -> &'static str {
    match op {
        ControlOp::NewSession { .. } => "new_session",
        ControlOp::Switch { .. } => "switch_session",
        ControlOp::Fork { .. } => "fork",
        ControlOp::Navigate { .. } => "navigate_tree",
        ControlOp::Reload => "reload",
        ControlOp::Compact { .. } => "compact",
        ControlOp::WaitIdle => "wait_idle",
        ControlOp::SendMessage { .. } => "send_message",
        ControlOp::SendUserMessage { .. } => "send_user_message",
        ControlOp::SetModel(_) => "set_model",
        ControlOp::SetThinkingLevel(_) => "set_thinking_level",
        ControlOp::Abort => "abort",
        ControlOp::Shutdown => "shutdown",
    }
}

/// Parse a guest `setModel` payload (a `control` capability arg) into `(provider, model)`. Accepts
/// either `"provider/model"` (Pi's `provider/model` id form) or `{ "provider": .., "model": .. }`.
/// Returns `None` for an unparseable payload (degrade, never panic).
fn parse_model_ref(v: &serde_json::Value) -> Option<(ProviderId, ModelId)> {
    if let Some(s) = v.as_str() {
        let (p, m) = s.split_once('/')?;
        if p.is_empty() || m.is_empty() {
            return None;
        }
        return Some((ProviderId::from(p), ModelId::from(m)));
    }
    let p = v.get("provider").and_then(serde_json::Value::as_str)?;
    let m = v.get("model").and_then(serde_json::Value::as_str)?;
    if p.is_empty() || m.is_empty() {
        return None;
    }
    Some((ProviderId::from(p), ModelId::from(m)))
}

impl AgentSession {
    /// Drain + apply control ops a loaded extension queued via its `control` capability (Pi
    /// `createCommandContext`, agent-session.ts:1158; arch-08 §6.3). This is the command-tier-safe
    /// point that bridges the SYNC guest `control()` call to the real ASYNC session effect: a guest
    /// that calls `session.setThinkingLevel(...)` / `setModel(...)` / `sendUserMessage(...)` / a
    /// compaction reaches [`crate::host_services::LiveHostServices`], which queues the op; here it is
    /// applied. Mutating from a command tier respects the deadlock rule (R-08-008): never called
    /// from inside the agent loop.
    ///
    /// SEAM-003: this is now a SINK, not a filter. It used to return the runtime-tier ops
    /// (`new_session`/`switch`/`fork`/`navigate`/`reload`/`wait_idle`/`send_message`) "for the
    /// runtime to act on" — and its single production caller (`try_execute_wasm_command`) dropped
    /// the returned vector, while the NATIVE command route never drained at all. Every op is now
    /// routed here:
    ///
    /// * `NewSession`/`Switch`/`Fork`/`Reload` → the installed [`crate::RuntimeActions`] sink (Pi
    ///   binds these to the real `runtimeHost.*` in every host, rpc-mode.ts:321-346).
    /// * `Navigate`/`WaitIdle`/`SendMessage`/`SendUserMessage`/`Compact` → applied in place; they
    ///   are session-local and need no runtime host.
    /// * `SetModel`/`SetThinkingLevel`/`Abort`/`Shutdown` → the `Send`-safe shared helper
    ///   [`Self::apply_agent_state_op`], so the event-tier drain handles them identically.
    ///
    /// A failure is reported through the extension host's error listener (the same channel a
    /// contained handler fault uses) — never a silent drop, and never a panic.
    pub async fn apply_pending_control(&self) {
        // Fan out the facade events a guest state-mutation queued (entry_appended/session_info_changed):
        // the guest appended/renamed synchronously via `LiveHostServices`; emit here — the same
        // command-tier-safe bridge point the control ops drain at — so listeners observe them.
        for ev in self.services.host_services.take_pending_events() {
            self.fanout_emit(ev).await;
        }
        // Push the tool set a guest `setActiveTools` restricted the session to onto the live agent
        // (Pi `setActiveTools` = `setActiveToolsByName`, agent-session.ts:2283,850-854). The guest
        // updated the authoritative dynamic-tool view synchronously across the wasm-suspended call
        // (so `getActiveTools` already reflects it); the ASYNC agent push lands here — the same
        // command-tier-safe bridge point control ops / pending events drain at — before the next turn.
        // EXT-004: surface any tool an extension registered since the last drain (Pi calls
        // `refreshTools()` from `registerTool` itself; cyrup's registration crosses a SYNC wasm
        // import, so the async agent push lands at this same bridge point). Ordered BEFORE the
        // explicit `setActiveTools` push below so an extension that registered a tool AND then
        // restricted the active set in the same handler gets what it asked for — in Pi the refresh
        // happens inside `registerTool`, i.e. strictly earlier than any later `setActiveTools`, and
        // `setActiveToolsByName` is always the last word.
        self.refresh_extension_tools().await;
        // …and the restriction is RE-RESOLVED here, against the just-refreshed registry, rather than
        // replayed from the pre-refresh pair the synchronous guest call built. `merge_registered`
        // above auto-activates every newly registered name and writes the active set doing it, so
        // replaying a stale pair left the dynamic-tool view holding the refresh's set and the agent
        // holding the restriction's — the guest asked for `["read"]` and the facade answered
        // `["read", <the guest's own tools>]`. Routing through the SAME facade method the host/CLI
        // toggle uses keeps both in step and makes `setActiveToolsByName` the last word for real.
        if let Some(names) = self.services.host_services.take_pending_active_tools() {
            self.set_active_tools_by_name(&names).await;
        }
        let ops = self.services.host_services.take_pending_control();
        for op in ops {
            // Agent-state + lifecycle ops (SetModel/SetThinkingLevel/Abort/Shutdown) apply in place
            // via the shared `Send`-safe helper; it returns `Some(op)` for anything it did not
            // handle so the routing below stays exhaustive.
            let Some(op) = self.apply_agent_state_op(op).await else {
                continue;
            };
            let name = control_op_name(&op);
            let outcome = match op {
                ControlOp::SendUserMessage { content, .. } => {
                    // A guest `sendUserMessage` op re-enters the prompt path (`send_user_message` →
                    // `prompt_accepted` → `prepare` → `try_execute_extension_command`), closing an
                    // `async fn` cycle. Box this cold re-entry edge so the future stays finitely
                    // sized (E0733) without adding indirection to the hot prompt path.
                    Box::pin(self.send_user_message(content, None))
                        .await
                        .map(|_| ())
                }
                // Pi `ctx.compact(options)` (extensions/types.ts:344): `customInstructions`
                // (types.ts:296-300) rides the op through to the summarizer — the same
                // `Option<String>` a `/compact <instructions>` slash command passes.
                ControlOp::Compact {
                    custom_instructions,
                } => self.compact(custom_instructions).await.map(|_| ()),
                // ---- session-local runtime ops (no runtime host needed) ----
                ControlOp::Navigate { entry_id, opts } => {
                    Box::pin(self.control_navigate(&entry_id, &opts)).await
                }
                ControlOp::WaitIdle => {
                    // Pi's `waitForIdle` is a promise that cannot deadlock the command path; cyrup's
                    // waits on the post-run driver watch. This drain normally runs BEFORE
                    // `spawn_run`, so the flag is already false — but a concurrent run would
                    // otherwise block the command path indefinitely, so bound it and surface the
                    // expiry instead of hanging.
                    match tokio::time::timeout(WAIT_IDLE_CONTROL_TIMEOUT, self.wait_for_idle())
                        .await
                    {
                        Ok(()) => Ok(()),
                        Err(_) => Err(SessionServiceError::Io(
                            "control op `wait_idle` timed out waiting for the agent to settle"
                                .into(),
                        )),
                    }
                }
                ControlOp::SendMessage { message, opts } => {
                    Box::pin(self.control_send_message(&message, &opts)).await
                }
                // ---- RUNTIME-tier ops: only a host that installed a `RuntimeActions` can do these ----
                ControlOp::NewSession { opts } => match self.runtime_actions.get() {
                    Some(rt) => rt.new_session(&opts).await,
                    None => Err(SessionServiceError::NoRuntimeHost("new_session")),
                },
                ControlOp::Switch { session_id, opts } => match self.runtime_actions.get() {
                    Some(rt) => rt.switch_session(&session_id, &opts).await,
                    None => Err(SessionServiceError::NoRuntimeHost("switch_session")),
                },
                ControlOp::Fork { entry_id, opts } => match self.runtime_actions.get() {
                    Some(rt) => rt.fork(&entry_id, &opts).await,
                    None => Err(SessionServiceError::NoRuntimeHost("fork")),
                },
                ControlOp::Reload => match self.runtime_actions.get() {
                    Some(rt) => rt.reload().await,
                    None => Err(SessionServiceError::NoRuntimeHost("reload")),
                },
                // Handled by `apply_agent_state_op` above; unreachable, but keep the match total so
                // a future `ControlOp` variant is a compile error rather than a silent drop.
                other => Err(SessionServiceError::Io(format!(
                    "unrouted control op: {other:?}"
                ))),
            };
            if let Err(e) = outcome {
                self.report_control_failure(name, &e);
            }
        }
    }

    /// Apply a `ControlOp::Navigate` (Pi `ctx.navigateTree(targetId, {summarize, customInstructions,
    /// replaceInstructions, label})`, extensions/types.ts:1665-1668, bound to `session.navigateTree`
    /// at rpc-mode.ts:325-337).
    async fn control_navigate(
        &self,
        entry_id: &str,
        opts: &serde_json::Value,
    ) -> Result<(), SessionServiceError> {
        let options = NavigateTreeOptions {
            summarize: opts
                .get("summarize")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            custom_instructions: opts
                .get("customInstructions")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            replace_instructions: opts
                .get("replaceInstructions")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            label: opts
                .get("label")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        };
        self.navigate_tree(EntryId::from(entry_id), options)
            .await
            .map(|_| ())
    }

    /// Apply a `ControlOp::SendMessage` (Pi `ctx.sendMessage(message, {triggerTurn, deliverAs})`,
    /// extensions/types.ts:395-398/1223). `message` is the guest's
    /// `Pick<CustomMessage, "customType"|"content"|"display"|"details">`.
    async fn control_send_message(
        &self,
        message: &serde_json::Value,
        opts: &serde_json::Value,
    ) -> Result<(), SessionServiceError> {
        use serde_json::Value;
        let custom_type = message
            .get("customType")
            .and_then(Value::as_str)
            .unwrap_or("extension")
            .to_string();
        let content = message.get("content").cloned().unwrap_or(Value::Null);
        let display = message
            .get("display")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let details = message.get("details").cloned();
        let deliver_as = match opts.get("deliverAs").and_then(Value::as_str) {
            Some("steer") => Some(crate::event::DeliverAs::Steer),
            Some("followUp") => Some(crate::event::DeliverAs::FollowUp),
            Some("nextTurn") => Some(crate::event::DeliverAs::NextTurn),
            _ => None,
        };
        // Pi's `triggerTurn` runs a fresh turn OVER the custom message when idle
        // (`_runAgentPrompt(appMessage)`); `deliverAs` takes precedence, exactly as in
        // `send_custom_message`/`inject_message`.
        let trigger_turn = opts
            .get("triggerTurn")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // AGENT-030 — pi's `sendMessage` tests `this.isStreaming`, the session latch
        // `_isAgentRunActive` (agent-session.ts:900-901, :1477-1483): a trigger-turn message landing
        // in the post-`agent_end` gap steers the active loop rather than starting a second run.
        if trigger_turn && deliver_as.is_none() && !self.is_run_active() {
            let msg = AgentMessage::Custom {
                kind: custom_type,
                payload: content,
                // The guest's `pi.sendMessage({… details})` payload, read just above and previously
                // discarded on this branch only — the trigger-turn arm now carries it like the
                // `send_custom_message` tail below.
                details: details.clone(),
                // SUBA-094 — likewise `display`, read just above and dropped here until now: pi
                // hands `_runAgentPrompt` the same `appMessage` that carries it
                // (`agent-session.ts:1488-1505` @v0.84.4), so a guest's
                // `sendMessage({display:false}, {triggerTurn:true})` is a model-only message.
                display,
                timestamp: Some(now_ms()),
            };
            return self.spawn_run(vec![msg]).await;
        }
        self.send_custom_message(&custom_type, content, display, details, deliver_as)
            .await
    }

    /// Surface a control-op failure. SEAM-003's contract is that an op is either PERFORMED or
    /// REPORTED — never silently dropped, which is exactly what the old `let _deferred = …` did.
    /// Pi's pre-bind action stubs throw `"Extension runtime not initialized…"`
    /// (extensions/loader.ts:173-176 `notInitialized`) rather than no-op; cyrup cannot throw across
    /// the drain, so it warns.
    fn report_control_failure(&self, op: &str, err: &SessionServiceError) {
        tracing::warn!(op = %op, error = %err, "extension control op failed");
    }

    /// Apply a single AGENT-STATE / LIFECYCLE control op in place, returning `None` when it was one
    /// of those (handled) or `Some(op)` when it is some other op the caller must route itself.
    ///
    /// `SetModel`/`SetThinkingLevel` are pure agent-state mutations the next turn reads (Pi
    /// `setModel`/`setThinkingLevel`, agent-session.ts:1476-1490 / 1541-1572). `Abort`/`Shutdown`
    /// join them because Pi puts BOTH on the base `ExtensionContext` — "Available in all contexts"
    /// (extensions/types.ts:339,344) — so `cyrup-ext`'s `control::Host` deliberately does not
    /// `require_command_tier()` them and they can arrive from an EVENT handler. Handling them here,
    /// in the shared helper, is what makes the event-tier turn-boundary drain
    /// ([`Self::apply_pending_agent_control`]) service them instead of re-queueing them until some
    /// later command happens to run.
    ///
    /// Shared by [`Self::apply_pending_control`] (command-tier drain) and
    /// [`Self::apply_pending_agent_control`] so the two never drift. Note it does NOT touch the
    /// `send_user_message`/`compact` re-entry arms — whose prompt-path futures are `!Send` — so a
    /// caller that needs a `Send` future (the spawned post-run driver) can use it.
    async fn apply_agent_state_op(&self, op: ControlOp) -> Option<ControlOp> {
        match op {
            ControlOp::SetThinkingLevel(level) => {
                if let Some(lv) = crate::builder::thinking_level_from_str(&level) {
                    let _ = self.set_thinking_level(lv).await;
                }
                None
            }
            ControlOp::SetModel(v) => {
                if let Some((provider, model)) = parse_model_ref(&v) {
                    let _ = self.set_model_id(provider, model).await;
                }
                None
            }
            // Pi `ctx.abort()` (types.ts:339): "Abort the current agent run." Bound at
            // agent-session.ts:2405 to `void this.abort()`.
            ControlOp::Abort => {
                self.abort();
                None
            }
            // Pi `ctx.shutdown()` (types.ts:344) → the host's `shutdownHandler`, which in Pi's RPC
            // mode is exactly `() => { shutdownRequested = true }` (rpc-mode.ts:344-346); the host
            // acts on it at the next `agent_settled`.
            ControlOp::Shutdown => {
                self.shutdown_requested.store(true, Ordering::SeqCst);
                None
            }
            other => Some(other),
        }
    }

    /// GAP-11 event-tier turn-boundary drain: apply the AGENT-STATE control ops
    /// (`SetModel`/`SetThinkingLevel`) a guest queued from an EVENT handler (`on_message_end` /
    /// `on_input` / a mid-turn tool hook / `on_agent_end`), at a STORE-FREE point (after a run settles
    /// or after `emit_input_event` returns — every `LiveExtension.inner` store guard released), so the
    /// change takes effect on the SUBSEQUENT turn, matching Pi (which mutates synchronously from any
    /// handler, loader.ts:342-354). The re-emit (`thinking_level_select`/`model_select`) fires here as
    /// a fresh top-level guest call, never a re-entry into the suspended event-hook store.
    ///
    /// This is the `Send`-safe subset of [`Self::apply_pending_control`]: only SetModel/
    /// SetThinkingLevel can reach the queue from an event handler (every other control op stays
    /// command-tier-gated in live.rs), and this never touches the `!Send` `send_user_message`/
    /// `compact` arms — so it runs inside the spawned post-run driver ([`Self::drive_run`]). It also
    /// drains the same pending facade-event / active-tool fan-out `apply_pending_control` does, so a
    /// guest that appended/renamed/restricted tools from the event handler is observed here too. Any
    /// op it does not handle is re-queued (never dropped) for the command-tier drain.
    pub(super) async fn apply_pending_agent_control(&self) {
        for ev in self.services.host_services.take_pending_events() {
            self.fanout_emit(ev).await;
        }
        // EXT-004, event-tier twin of the drain in `apply_pending_control` (same ordering rule:
        // the refresh runs first so an explicit `setActiveTools` still has the last word — and, as
        // there, the restriction is re-resolved AFTER it rather than replayed from the pre-refresh
        // pair, so the dynamic-tool view and the agent cannot disagree about what is active).
        self.refresh_extension_tools().await;
        if let Some(names) = self.services.host_services.take_pending_active_tools() {
            self.set_active_tools_by_name(&names).await;
        }
        for op in self.services.host_services.take_pending_control() {
            if let Some(other) = self.apply_agent_state_op(op).await {
                // Unreachable in practice — live.rs gates every non-base-context control op to the
                // command tier, so only SetModel/SetThinkingLevel/Abort/Shutdown can be queued from
                // an event handler, and `apply_agent_state_op` handles all four. Re-queue (never
                // drop) as a guard so a future gating change can't silently lose a command-tier op;
                // the command-tier drain (`apply_pending_control`) will handle it.
                let _ =
                    cyrup_ext::host::HostServices::control(&*self.services.host_services, other);
            }
        }
    }
}
