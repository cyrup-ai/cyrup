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
use crate::event::{InputSource, PromptOptions, StreamingBehavior, UserInput};

use super::types::NavigateTreeOptions;
use super::{AgentSession, now_ms};

/// pi's `sendUserMessage` options bag — `options?: { deliverAs?: "steer" | "followUp";
/// expandPromptTemplates?: boolean }` (`core/agent-session.ts:2008` @v0.87.1), the typed shape of
/// [`ControlOp::SendUserMessage`]'s `opts` and the host counterpart of
/// `cyrup-ext-sdk`'s `SendUserMessageOptions`.
///
/// `#[serde(default)]` on the container, not just the fields, so `null` / `{}` / a bag carrying only
/// one key all parse — a guest that passes no options at all sends `Value::Null` over the WIT import
/// (`cyrup-ext/src/host/live.rs::send_user_message`), and upstream's `options?.` reads that as
/// "absent" rather than as an error. Unknown keys are ignored for the same reason: TypeScript's
/// structural typing accepts them silently, so refusing them here would be cyrup inventing a
/// failure pi does not have.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SendUserMessageOpts {
    /// pi `options.deliverAs`, handed to `prompt` as `streamingBehavior` (`:2032`).
    deliver_as: Option<StreamingBehavior>,
    /// pi `options.expandPromptTemplates`, read as `?? false` (`:2031`) — **opt IN**. Its own doc
    /// (`:2004`) says what it covers: "Whether to dispatch extension commands and expand skill
    /// commands and prompt templates. Default: false."
    expand_prompt_templates: Option<bool>,
}

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

/// EXT-087 — whether this op is one of the two SENDS [`AgentSession::apply_send_op`] services.
/// Kept beside [`control_op_name`] so the two move together when a `ControlOp` variant is added.
fn is_send_op(op: &ControlOp) -> bool {
    matches!(
        op,
        ControlOp::SendUserMessage { .. } | ControlOp::SendMessage { .. }
    )
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
            // EXT-087 — the two SEND arms live in the shared helper so the MID-RUN drain
            // ([`Self::apply_pending_agent_control`]) applies them with byte-identical option
            // handling instead of a second copy that can drift.
            if is_send_op(&op) {
                if let Err(e) = self.apply_send_op(op).await {
                    self.report_control_failure(name, &e);
                }
                continue;
            }
            let outcome = match op {
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

    /// EXT-087 — the two SEND arms of the control drain, shared by the COMMAND-tier drain
    /// ([`Self::apply_pending_control`]) and the MID-RUN turn-boundary drain
    /// ([`Self::apply_pending_agent_control`]).
    ///
    /// Upstream both sends are applied AT CALL TIME, from whatever handler called them, and
    /// `prompt`/`sendCustomMessage` then branch on `this.isStreaming`: while a run is live the
    /// message is STEERED or FOLLOWED-UP into that run (`core/agent-session.ts:1653-1665` for
    /// `prompt`, `:1949-1954` for `sendCustomMessage`, both @v0.87.1) — it never starts a second
    /// run. cyrup queues instead, so the only way to reproduce that is to drain the send at a
    /// point where the run latch is still raised; that is what the mid-run caller does.
    ///
    /// Callers gate on [`is_send_op`] rather than having this hand the op back: returning a
    /// `ControlOp` in an `Err` variant trips `clippy::result_large_err` (the enum is 144 bytes).
    /// The final arm stays total so a new `ControlOp` variant is a compile error here, and reports
    /// rather than panics if it is ever reached without the gate.
    async fn apply_send_op(&self, op: ControlOp) -> Result<(), SessionServiceError> {
        match op {
            ControlOp::SendUserMessage { content, opts } => {
                // EXT-083 — the options bag is HONOURED here, not dropped. pi's
                // `sendUserMessage` is a thin wrapper over `prompt`:
                //
                // ```ts
                // await this.prompt(text, {
                //     expandPromptTemplates: options?.expandPromptTemplates ?? false,
                //     streamingBehavior: options?.deliverAs,
                //     images,
                //     source: "extension",
                // });
                // ```
                // (`core/agent-session.ts:2030-2035` @v0.87.1)
                //
                // so [`Self::prompt_with`] — cyrup's `prompt(text, options)` — is the call, and
                // both of pi's option reads land on it: `expand_prompt_templates` becomes
                // `UserInput::expand_templates`, `deliver_as` becomes
                // `PromptOptions::streaming_behavior`.
                //
                // NOT [`Self::send_user_message`], which is `sendUserMessage`'s name but not its
                // body: that helper defaults a missing behaviour to a STEER and queues through
                // the PUBLIC `steer`/`follow_up`. pi does neither — it throws "Agent is already
                // processing. Specify streamingBehavior…" for a missing behaviour while
                // streaming (`:1655-1659`, cyrup's
                // [`SessionServiceError::StreamingNeedsBehavior`]) and queues through the
                // PRIVATE `_queueSteer`/`_queueFollowUp` after `prepare` has already run the
                // `input` handlers and the expansion once (SEAM-121). The helper stays as it is
                // for its other caller, the ICOM injection pump.
                //
                // The `expand_templates: false` default is the whole behavioural point of the
                // row: `UserInput::text` sets it TRUE, so a relayed `"/deploy"` used to dispatch
                // a registered extension command (`run.rs`'s step 0) or expand a skill/template
                // before the model ever saw the text. Upstream a chat bridge, a macro or a
                // subagent summary that happens to start with `/` is sent AS TEXT unless the
                // guest asks otherwise.
                let opts: SendUserMessageOpts = serde_json::from_value(opts).unwrap_or_default();
                let input = UserInput {
                    text: content,
                    images: Vec::new(),
                    source: InputSource::Sdk,
                    expand_templates: opts.expand_prompt_templates.unwrap_or(false),
                };
                // A guest `sendUserMessage` op re-enters the prompt path (`prompt_with` →
                // `prepare` → `try_execute_extension_command`), closing an `async fn` cycle. Box
                // this cold re-entry edge so the future stays finitely sized (E0733) without
                // adding indirection to the hot prompt path.
                //
                // EXT-087 — this arm is now reachable from the POST-SETTLE drain in
                // `settle_run` as well as from the command tier, which closes a second loop on
                // top of the E0733 one:
                // `settle_run -> apply_pending_control -> prompt_with -> spawn_run ->
                // tokio::spawn(drive_run) -> drive_accepted_run -> settle_run`. Every edge of
                // that loop used to be an `impl Future` opaque type, and asking whether an
                // opaque type is `Send` when the answer depends on itself is a question rustc
                // declines — it reports `cannot satisfy impl Future<..>: Send`, which reads
                // exactly like a genuinely non-`Send` value held across an await but is NOT
                // one. There is no `Rc`, `RefCell`, `LocalSet` or `spawn_local` anywhere in
                // this path; the failure was inference, not fact.
                //
                // The loop is cut at [`super::AgentSession::drive_run`], whose SIGNATURE now
                // names `Pin<Box<dyn Future<Output = ()> + Send>>` instead of returning an
                // opaque one — see the note there for why the cut has to be a signature and not
                // a coercion at a call site (a coercion re-enters the same opaque-type
                // computation and fails as E0391 instead).
                Box::pin(self.prompt_with(
                    input,
                    PromptOptions {
                        streaming_behavior: opts.deliver_as,
                    },
                ))
                .await
                .map(|_| ())
            }
            ControlOp::SendMessage { message, opts } => {
                Box::pin(self.control_send_message(&message, &opts)).await
            }
            other => Err(SessionServiceError::Io(format!(
                "apply_send_op reached without `is_send_op`: {other:?}"
            ))),
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
        // SEAM-127 — kept as the RAW `Option<bool>`: pi's `sendCustomMessage` tests
        // `options?.triggerTurn !== false` in its streaming branch (agent-session.ts:1949
        // @v0.87.1) and plain truthiness in the branch below it, so an ABSENT key and an explicit
        // `false` are different inputs. `unwrap_or(false)` collapsed them, and the explicit
        // `triggerTurn: false` then took the streaming steer arm — the one shape it exists to
        // avoid.
        let trigger_turn = opts.get("triggerTurn").and_then(Value::as_bool);
        // AGENT-030 — pi's `sendMessage` tests `this.isStreaming`, the session latch
        // `_isAgentRunActive` (agent-session.ts:900-901, :1477-1483): a trigger-turn message landing
        // in the post-`agent_end` gap steers the active loop rather than starting a second run.
        if trigger_turn == Some(true) && deliver_as.is_none() && !self.is_run_active() {
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
        self.send_custom_message(
            &custom_type,
            content,
            display,
            details,
            deliver_as,
            trigger_turn,
        )
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
    /// [`Self::apply_pending_agent_control`] so the two never drift. It does NOT touch the
    /// `send_user_message`/`compact` re-entry arms; the sends have their own shared helper
    /// ([`Self::apply_send_op`]), which the mid-loop drain calls only while the RUN LATCH is
    /// raised — the condition under which `prepare` can answer nothing but
    /// `Prepared::Queued`/`StreamingNeedsBehavior`, so nothing there can start a second run while
    /// the continuation loop is still deciding whether to continue this one. `compact` stays
    /// command-tier in `live.rs`, so it cannot reach the mid-loop drain at all.
    ///
    /// EXT-087, CORRECTING THIS DOC: that exclusion used to be justified as "whose prompt-path
    /// futures are `!Send`", so a caller needing a `Send` future had to avoid them. THAT CLAIM IS
    /// FALSE, and it was load-bearing enough to be worth saying so rather than quietly deleting.
    /// It was checked empirically — an `assert_send` probe over
    /// `AgentSession::apply_pending_control()`'s future compiles, with a deliberate `Rc`-holding
    /// negative control alongside it that does not — so the whole drain, send and compact arms
    /// included, is `Send`. The `Box::pin` at the send arm is for E0733 (a finitely sized future
    /// across the recursive `apply_pending_control -> prompt_with -> prepare ->
    /// try_execute_extension_command -> apply_pending_control` cycle), which is what its own
    /// comment at that arm says; boxing a concrete future PRESERVES `Send`, and a recursive async
    /// cycle can defeat auto-trait INFERENCE without the future actually being `!Send`. The
    /// exclusion here is therefore about RUN ORDERING, not about `Send`, and
    /// [`super::AgentSession::settle_run`] calls the full [`Self::apply_pending_control`] inside
    /// the spawned post-run driver on the strength of that. `ext_087_post_settle_drain_future_is_send`
    /// pins it.
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
    /// EXT-087 — it also services the two SENDS, through the same
    /// [`Self::apply_send_op`] the command-tier drain uses, whenever the RUN LATCH is still raised.
    /// That is the mid-run half of the row: upstream applies `sendMessage`/`sendUserMessage`
    /// synchronously from the handler and `prompt` steers or follows-up into the live run
    /// (`core/agent-session.ts:1653-1665`, `:1949-1954` @v0.87.1), so deferring them to the
    /// post-settle drain started a SECOND run instead of joining the current one. See the arm
    /// itself for why the latch makes that safe. With the latch down, and for every other op, the
    /// op is re-queued (never dropped) for the command-tier / post-settle drain.
    ///
    /// The whole future stays `Send` — `apply_pending_control`'s is (`apply_agent_state_op` and
    /// `apply_send_op` are its only awaits that can recurse), which
    /// `ext_087_post_settle_drain_future_is_send` pins — so this still runs inside the spawned
    /// post-run driver ([`Self::drive_run`]). It also drains the same pending facade-event /
    /// active-tool fan-out `apply_pending_control` does, so a guest that appended/renamed/restricted
    /// tools from the event handler is observed here too.
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
            let Some(op) = self.apply_agent_state_op(op).await else {
                continue;
            };
            // EXT-087, the MID-RUN half — `send-message`/`send-user-message` queued from an event
            // handler that fired DURING a run (`tool_call`, `tool_result`, `message_end`,
            // `agent_end`, an `input` handler while streaming) are applied HERE, while the run
            // latch is still raised, and not carried forward to the post-settle drain.
            //
            // That is what upstream does and the whole of what it does. `pi.sendUserMessage` runs
            // synchronously from the handler into `prompt`, which branches on `this.isStreaming`
            // and routes to `_queueSteer`/`_queueFollowUp` (`core/agent-session.ts:1653-1665`
            // @v0.87.1) — or throws "Agent is already processing. Specify streamingBehavior…"
            // (`:1655-1658`) when the call carried no `deliverAs`. `pi.sendMessage` branches the
            // same way at `:1949-1954`. NEITHER starts a run: the message joins the run that is
            // already going.
            //
            // Draining at the post-settle point instead — which is what this arm used to do —
            // turned every one of those into a SECOND run after the first settled: a second
            // `agent_start`/`agent_end`/`agent_settled` pair, a steer that arrived a whole turn
            // late, and a `deliverAs` that was read and then could not matter because nothing was
            // streaming by the time it was read. That is a behavioural difference, not a timing
            // one, and the `deliverAs`-less case silently ran a turn where upstream reports an
            // error to the extension error channel (`core/agent-session.ts:3057-3064` binds the
            // rejection to `runner.emitError`; cyrup's twin is `report_control_failure`).
            //
            // The latch is the guard that makes this safe: with `is_run_active()` true, `prepare`
            // can only answer `Prepared::Queued` or `StreamingNeedsBehavior`
            // (`session/run.rs::prepare` step 2), never `Prepared::Run` — so this cannot start a
            // second run and cannot race the continuation loop that has not yet decided whether to
            // continue this one. `queue_steer`/`queue_follow_up` make `agent.has_queued_messages()`
            // true, which is exactly how `handle_post_agent_run` (`run.rs:543`, pi `:1009-1012`)
            // already decides to continue — so the message is delivered by the CURRENT run.
            //
            // With the latch down (this drain also runs from `prepare`, before a run exists) the
            // op is re-queued exactly as before, so a send from an idle-time handler still lands on
            // the post-settle / command-tier drain that starts its run.
            let name = control_op_name(&op);
            if is_send_op(&op) && self.is_run_active() {
                if let Err(e) = Box::pin(self.apply_send_op(op)).await {
                    self.report_control_failure(name, &e);
                }
                continue;
            }
            // Anything else: re-queue (never drop) for the command-tier / post-settle drain. It
            // remains the guard it always was — a future gating change cannot silently lose a
            // command-tier op.
            let _ = cyrup_ext::host::HostServices::control(&*self.services.host_services, op);
        }
    }
}
