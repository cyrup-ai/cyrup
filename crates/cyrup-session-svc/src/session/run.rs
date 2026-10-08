//! Prompting and the run driver — `prompt`/`steer`/`follow_up` in, an assembled agent run out.
//!
//! Pi `agent-session.ts` `prompt`/`_runAgentPrompt`/`_handlePostAgentRun`. Covers preflight
//! (`prepare`, the `input` extension event, queue routing), input expansion, run-message assembly
//! and the spawned post-run driver loop that owns retry / auto-compaction / queued continuations.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use cyrup_agent::AgentMessage;
use cyrup_core::{AssistantMessage, Content, EventStream, Message};
use cyrup_ext::{HostEvent, InputEventSource, InputReduction, InputStreamingBehavior};

use crate::error::SessionServiceError;
use crate::event::{
    AgentSessionEvent, InputSource, PromptAccepted, PromptOptions, StreamingBehavior, UserInput,
    core_message_to_agent,
};

use super::AgentSession;

/// What came of offering a batch of injected messages to the agent.
///
/// # `AgentBusy` is an outcome, not an error
///
/// A background completion is never *refused*: nothing about it can be invalid, and no policy
/// rejects it. The only thing that can happen other than acceptance is that another run holds the
/// latch at this instant — a scheduling state that resolves on its own. Naming it here keeps it
/// out of [`SessionServiceError`], where it would be indistinguishable from a fault, and keeps the
/// caller's `match` exhaustive over the two things that can actually happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InjectionOffer {
    /// The agent claimed the latch with these messages as its run input. They are on the run's
    /// transcript, so the `message_end` that persists them necessarily follows.
    Taken,
    /// Another run owns the latch. Nothing was consumed and nothing was persisted twice; the
    /// caller still owns the batch.
    AgentBusy,
}

/// A submission made while `agent_settled` was being emitted — one entry of pi's
/// `_deferredSettledActions` (`agent-session.ts:379` @v0.87.1). Pi pushes a closure; cyrup names
/// the three closures pi pushes.
pub(crate) enum DeferredSettled {
    /// `prompt(text, options)` (`:1607-1609`) reached through [`AgentSession::prompt_run`]. The
    /// caller already holds the returned run-scoped stream; its sender rides here and is adopted
    /// when the deferred prompt runs.
    PromptRun {
        input: UserInput,
        stream: tokio::sync::mpsc::Sender<AgentSessionEvent>,
    },
    /// `prompt(text, options)` reached through [`AgentSession::prompt_with`].
    Prompt {
        input: UserInput,
        options: PromptOptions,
    },
    /// `sendMessage(…, {triggerTurn: true})`'s `_runAgentPrompt(appMessage)` (`:1956-1958`).
    Run(Vec<AgentMessage>),
}

/// The disposition of the `input` extension event (Pi `InputEventResult.action`, runner.ts:1100).
/// A `transform` outcome rewrites the in-flight [`UserInput`] in place (via `EventPatch::Input`) and
/// then reports `Continue`, exactly as Pi folds `currentText`/`currentImages` before continuing.
enum InputDisposition {
    /// A handler fully serviced the submission (`handled`); no run or queue follows.
    Handled,
    /// No handler claimed it; proceed with expansion + run/queue (text/images may have been
    /// rewritten by a `transform` handler already applied to the [`UserInput`]).
    Continue,
}

/// Collapse the host-side [`InputSource`] onto Pi's three handler-visible `InputSource` values
/// (`"interactive" | "rpc" | "extension"`, extensions/types.ts:789). cyrup's richer provenance
/// (`Cli`/`Stdin`/`Sdk`/`Tui`) all present as `interactive` to a handler, exactly as Pi's host
/// passes `"interactive"` for any non-rpc submission (agent-session.ts:1021).
fn input_event_source(source: InputSource) -> InputEventSource {
    match source {
        InputSource::Rpc => InputEventSource::Rpc,
        InputSource::Cli | InputSource::Stdin | InputSource::Sdk | InputSource::Tui => {
            InputEventSource::Interactive
        }
    }
}

/// Map the queue selector onto the handler-visible `streamingBehavior` (Pi `"steer" | "followUp"`).
fn input_streaming_behavior(behavior: StreamingBehavior) -> InputStreamingBehavior {
    match behavior {
        StreamingBehavior::Steer => InputStreamingBehavior::Steer,
        StreamingBehavior::FollowUp => InputStreamingBehavior::FollowUp,
    }
}

/// What [`AgentSession::prepare`] resolved a submission to (the shared `prompt` preflight outcome).
enum Prepared {
    /// Assembled run input to dispatch to the agent.
    Run(Vec<AgentMessage>),
    /// An `input` handler serviced it; nothing to run.
    Handled,
    /// The agent is streaming; the (expanded) submission is queued via the carried behavior.
    Queued(StreamingBehavior, UserInput),
}

impl AgentSession {
    /// Submit a user prompt and observe the run as a stream of [`AgentSessionEvent`] (R-11-005/007).
    ///
    /// The returned stream terminates after the run's `agent_end`. Errors only if the prompt could
    /// not be *accepted* (e.g. the agent is already streaming — use [`Self::steer`]/[`Self::follow_up`]).
    pub async fn prompt(
        &self,
        input: impl Into<UserInput>,
    ) -> Result<EventStream<AgentSessionEvent>, SessionServiceError> {
        self.prompt_run(input)
            .await
            .map(|(_accepted, stream)| stream)
    }

    /// [`Self::prompt`] **plus the preflight's own answer**.
    ///
    /// The distinction this returns is load-bearing for any front-end that parks a request on the
    /// stream: `PromptAccepted::Handled` means an `input` extension handler fully serviced the
    /// submission and **no run was started**, so no `agent_settled` will ever reach the returned
    /// stream. [`Self::prompt`] discards that fact — a caller that awaits the settle then waits
    /// forever, with no timeout anywhere, which is what an ACP `session/prompt` does
    /// (gap-analysis 15 `ACP-153`).
    ///
    /// `PromptAccepted::Queued` is unreachable here: this refuses with
    /// [`SessionServiceError::StreamingNeedsBehavior`] when a run is active, and that is the only
    /// state that queues. Use [`Self::prompt_with`] to queue deliberately.
    ///
    /// The stream is the **run-scoped** one and it is registered before the run starts, so no event
    /// is missed; `Fanout::end_run` clears it right after `emit_agent_settled`, which makes the
    /// settle its last event.
    ///
    /// # Errors
    ///
    /// [`SessionServiceError::StreamingNeedsBehavior`] when a run is already active, or whatever
    /// the preflight itself fails with.
    pub async fn prompt_run(
        &self,
        input: impl Into<UserInput>,
    ) -> Result<(PromptAccepted, EventStream<AgentSessionEvent>), SessionServiceError> {
        // SEAM-129 — pi `prompt()` opens with `if (this._isEmittingAgentSettled) {
        // this._deferredSettledActions.push(…); return; }` (`agent-session.ts:1607-1609` @v0.87.1):
        // a prompt made from an `agent_settled` handler runs right after the emit, not refused.
        if self.is_emitting_settled() {
            let (tx, stream) = self.fanout.detached_run();
            self.defer_settled(DeferredSettled::PromptRun {
                input: input.into(),
                stream: tx,
            });
            return Ok((PromptAccepted::Started, stream));
        }
        // AGENT-030: the session-level run latch, not the agent's per-run streaming flag — pi's
        // `prompt()` consults `this.isStreaming`, which IS `_isAgentRunActive`
        // (agent-session.ts:876-877 / :1159 @v0.83.0). See [`Self::is_run_active`].
        if self.is_run_active() {
            return Err(SessionServiceError::StreamingNeedsBehavior);
        }
        // Register the run-scoped subscription BEFORE starting the run so no event is missed.
        let stream = self.fanout.subscribe_run();
        match self.prepare(input.into(), PromptOptions::default()).await? {
            Prepared::Run(messages) => {
                self.spawn_run(messages).await?;
                Ok((PromptAccepted::Started, stream))
            }
            // An `input` handler serviced the submission (no run started); the stream stays idle.
            Prepared::Handled => Ok((PromptAccepted::Handled, stream)),
            // Unreachable — see the doc. Reported rather than assumed, so a future change to the
            // guard above surfaces here instead of silently parking a caller on a dead stream.
            Prepared::Queued(behavior, _) => Ok((PromptAccepted::Queued(behavior), stream)),
        }
    }

    /// Submit a prompt, resolving only to the preflight acceptance (mirrors Pi). The run is observed
    /// via [`Self::subscribe`]. Used by adapters that manage their own persistent subscription.
    pub async fn prompt_accepted(
        &self,
        input: impl Into<UserInput>,
    ) -> Result<PromptAccepted, SessionServiceError> {
        self.prompt_with(input, PromptOptions::default()).await
    }

    /// Submit a prompt with per-call [`PromptOptions`] (Pi `prompt(text, options)`,
    /// agent-session.ts:998). Closes the in-`prompt` `streamingBehavior` seam (gap `#13`): while the
    /// agent is streaming, the (template-expanded) text is queued via steer/follow-up per
    /// `streaming_behavior` instead of being rejected, exactly as Pi does at agent-session.ts:1043-
    /// 1056. The `Result` itself is the `preflightResult` callback (`Ok` = accepted, `Err` = the
    /// preflight throw). An `input` extension handler may fully service the submission, yielding
    /// [`PromptAccepted::Handled`].
    pub async fn prompt_with(
        &self,
        input: impl Into<UserInput>,
        options: PromptOptions,
    ) -> Result<PromptAccepted, SessionServiceError> {
        // SEAM-129 — the same deferral as [`Self::prompt_run`] (`agent-session.ts:1607-1609`).
        if self.is_emitting_settled() {
            self.defer_settled(DeferredSettled::Prompt {
                input: input.into(),
                options,
            });
            return Ok(PromptAccepted::Started);
        }
        match self.prepare(input.into(), options).await? {
            Prepared::Handled => Ok(PromptAccepted::Handled),
            // SEAM-121 — pi's `prompt` calls the PRIVATE `_queueFollowUp`/`_queueSteer` here
            // (agent-session.ts:1661-1663 @v0.87.1), never the public `followUp`/`steer`:
            // `prepare` has already run the `input` handlers and the skill/template expansion, and
            // the public entry points do both. Routing through them re-dispatched `input` for one
            // submission and expanded the text a second time.
            Prepared::Queued(behavior, ui) => Ok(match behavior {
                StreamingBehavior::FollowUp => self.queue_follow_up(ui).await,
                StreamingBehavior::Steer => self.queue_steer(ui).await,
            }),
            Prepared::Run(messages) => {
                self.spawn_run(messages).await?;
                Ok(PromptAccepted::Started)
            }
        }
    }

    /// Dispatch an assembled run. A BOUND session (via [`Self::into_shared`]) spawns the post-run
    /// driver task so auto-retry / post-run auto-compaction / queued continuations actually fire from
    /// the completed turn (Pi `_runAgentPrompt`, agent-session.ts:973-985). An unbound by-value session
    /// keeps the legacy behavior: start the run and let the subscriber terminate the run-scoped streams
    /// on `agent_end` (the post-run loop does not run).
    pub(super) async fn spawn_run(
        &self,
        messages: Vec<AgentMessage>,
    ) -> Result<(), SessionServiceError> {
        // Clear the run-abort latch where the run STARTS — Pi does this once, at the head of
        // `_runAgentPrompt` (`this._agentRunAbortRequested = false;`, `agent-session.ts:1469`
        // @v0.87.1). cyrup has TWO run entry points where Pi has one ([`Self::spawn_run`] and
        // [`Self::run_injection`]), so that single clear point is split across both; they are the
        // same point, not two policies.
        self.set_abort_requested(false);
        // pi's very next statement, with its own comment: "Compaction before the prompt may have
        // scheduled a retry; the new prompt replaces it." (`this._failedResponse = undefined;`,
        // `agent-session.ts:1810-1811` @v1.0.4.) A leaked stash would route the first request of
        // this prompt as a retry of a message the transcript no longer contains.
        self.clear_failed_response();
        // pi's NEXT statement again (`this._recordSelection();`, `agent-session.ts:1812` @v1.0.4),
        // in pi's order: abort latch, failed stash, record, pending tools. `SESS-067` — tree
        // navigation can leave the branch implying a selection other than the live one, and a
        // response can never record a VIRTUAL selection because it names the physical model that
        // answered; this writes the `model_change` a resume restores from. Same "two entry points,
        // one point" note as the clears above.
        self.record_selection().await;
        // …and the restored tools that have not registered by now are dropped (pi
        // `_pendingToolNames.clear()`, `agent-session.ts:1782` @v1.0.1).
        self.clear_pending_tools();
        match self.handle.get() {
            Some(this) => {
                // Flag the loop active BEFORE returning so an immediate `wait_for_idle` waits for the
                // WHOLE loop, not just the first `agent_end`.
                self.claim_run_latch();
                tokio::spawn(this.drive_run(messages));
                Ok(())
            }
            None => {
                self.agent.prompt(messages).await?;
                Ok(())
            }
        }
    }

    /// The post-run execution loop (Pi `_runAgentPrompt` + `_handlePostAgentRun`,
    /// agent-session.ts:973-1022). Runs the prompt, then — for as long as the post-run handler asks —
    /// drives `agent.continue()` for an auto-retry, a threshold/overflow auto-compaction, or an
    /// `agent_end`-queued continuation. Spawned by [`Self::spawn_run`] on a bound session.
    ///
    /// EXT-087, and the reason this is a `fn` returning a boxed future rather than an `async fn`:
    /// it is the CUT POINT for an auto-trait inference cycle, and the cut has to be a signature.
    ///
    /// Since `settle_run` drains the control queue, the call graph contains a loop —
    /// `settle_run -> apply_pending_control -> prompt_with -> spawn_run -> tokio::spawn(drive_run)
    /// -> drive_accepted_run -> settle_run`. While every edge of it was an `async fn`, every edge
    /// was an opaque type whose `Send`-ness depended on its own, and rustc declines that question:
    /// `tokio::spawn` below failed with `cannot satisfy impl Future<..>: Send`. That message reads
    /// exactly like a genuinely non-`Send` value held across an await, and it is NOT one — there is
    /// no `Rc`, `RefCell`, `LocalSet` or `spawn_local` anywhere in this path, and an `assert_send`
    /// probe over `apply_pending_control()` compiles cleanly once the loop is cut.
    ///
    /// Naming the return type here removes one opaque type from the loop, so proving the inner
    /// block `Send` terminates at a trait object that is `Send` by declaration. Cutting instead at
    /// a CALL SITE — coercing the `Box::pin` in `apply_pending_control`'s send arm to
    /// `Pin<Box<dyn Future + Send>>` — does not work: the coercion is part of the very opaque-type
    /// computation it depends on, and fails as E0391 `cycle detected` rather than resolving.
    /// `tests::ext_087_send_from_event::ext_087_post_settle_drain_future_is_send` pins the result,
    /// so re-opening the loop is a named assertion failure instead of a wall of E0277 here.
    fn drive_run(
        self: Arc<Self>,
        messages: Vec<AgentMessage>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(async move {
            // The refusal used to be silent (`if let Ok`): a `RunActive`/`Empty` here left the
            // session with no run, no event, and no log line. It cannot be returned — this is the
            // spawned driver — so it is logged at the one place that knows it happened. A caller
            // that CAN act on the refusal uses [`Self::run_injection`] instead, which awaits the
            // claim itself.
            match self.agent.prompt(messages).await {
                Ok(handle) => self.drive_accepted_run(handle).await,
                Err(e) => {
                    tracing::warn!(error = %e, "prompt refused inside the run driver");
                    self.settle_run().await;
                }
            }
        })
    }

    /// Start a run and REPORT whether the agent accepted it, then drive the post-run loop in the
    /// background.
    ///
    /// # Why this exists beside `spawn_run`
    ///
    /// [`Self::spawn_run`] is fire-and-forget: it returns once the driver task is spawned, so its
    /// caller cannot distinguish "the run started" from "`Agent::prompt` refused and the driver
    /// logged it". That is tolerable for a user submission (the user is present, and the refusal
    /// path is guarded by the preflight) and intolerable for an injected background completion,
    /// whose producer holds the only copy of the announced result and deletes it once delivery is
    /// reported. Awaiting the claim here is what turns that report into a fact.
    ///
    /// Only the CLAIM is awaited, not the turn: the post-run loop (retry / auto-compaction /
    /// queued continuation) and the settle tail run on a spawned task exactly as they do for
    /// `spawn_run`, so the injection pump is never blocked for the length of a model response.
    ///
    /// # Errors
    ///
    /// Only a genuine fault — no model selected, an empty run input, a hook or core failure.
    /// A busy agent is [`InjectionOffer::AgentBusy`], not an error.
    pub(super) async fn run_injection(
        &self,
        messages: Vec<AgentMessage>,
    ) -> Result<InjectionOffer, SessionServiceError> {
        // Clear the run-abort latch where the run STARTS — Pi does this once, at the head of
        // `_runAgentPrompt` (`this._agentRunAbortRequested = false;`, `agent-session.ts:1469`
        // @v0.87.1). cyrup has TWO run entry points where Pi has one ([`Self::spawn_run`] and
        // [`Self::run_injection`]), so that single clear point is split across both; they are the
        // same point, not two policies.
        self.set_abort_requested(false);
        // pi's very next statement, with its own comment: "Compaction before the prompt may have
        // scheduled a retry; the new prompt replaces it." (`this._failedResponse = undefined;`,
        // `agent-session.ts:1810-1811` @v1.0.4.) A leaked stash would route the first request of
        // this prompt as a retry of a message the transcript no longer contains.
        self.clear_failed_response();
        // pi's NEXT statement again (`this._recordSelection();`, `agent-session.ts:1812` @v1.0.4),
        // in pi's order: abort latch, failed stash, record, pending tools. `SESS-067` — tree
        // navigation can leave the branch implying a selection other than the live one, and a
        // response can never record a VIRTUAL selection because it names the physical model that
        // answered; this writes the `model_change` a resume restores from. Same "two entry points,
        // one point" note as the clears above.
        self.record_selection().await;
        // …and the restored tools that have not registered by now are dropped (pi
        // `_pendingToolNames.clear()`, `agent-session.ts:1782` @v1.0.1).
        self.clear_pending_tools();
        let Some(this) = self.handle.get() else {
            // An unbound by-value session has no post-run driver; the run is still claimed here,
            // so the acceptance report stays truthful.
            return Self::classify_claim(self.agent.prompt(messages).await.map(|_| ()));
        };
        // Flag the loop active BEFORE the claim, for the same reason `spawn_run` does: an
        // immediate `wait_for_idle` must wait for the WHOLE loop, not just the first `agent_end`.
        self.claim_run_latch();
        match this.agent.prompt(messages).await {
            Ok(handle) => {
                tokio::spawn(async move { this.drive_accepted_run(handle).await });
                Ok(InjectionOffer::Taken)
            }
            Err(e) => {
                // Release the latch claimed for a run that never started. Without this the session
                // reads "active" forever, `wait_for_idle` never returns, and the injection pump
                // parks permanently — turning a momentary overlap into a permanent stall.
                let _ = self.driver_tx.send(false);
                Self::classify_claim(Err(e))
            }
        }
    }

    /// Split what `Agent::prompt` reports into the ONE outcome that is expected and the rest,
    /// which are faults.
    ///
    /// # Why `RunActive` must not stay in `Err`
    ///
    /// [`cyrup_agent::AgentError::RunActive`] is the ordinary consequence of a user submission
    /// owning the run latch — it recurs, it resolves by itself, and the only sane response is to
    /// offer the messages again at the next idle edge. Every other variant
    /// (`NoModelSelected`, `NoMessages`, `Hook`, `Core`, …) describes a session that will still be
    /// unable to accept the same messages a moment later. A caller that receives both through one
    /// channel cannot tell "wait" from "stop", and the natural catch-all — retry — turns a
    /// permanent fault into an unbounded spin that answers nobody.
    fn classify_claim(
        claimed: Result<(), cyrup_agent::AgentError>,
    ) -> Result<InjectionOffer, SessionServiceError> {
        match claimed {
            Ok(()) => Ok(InjectionOffer::Taken),
            Err(cyrup_agent::AgentError::RunActive(_)) => Ok(InjectionOffer::AgentBusy),
            Err(fault) => Err(SessionServiceError::from(fault)),
        }
    }

    /// The post-run half of the driver, shared by [`Self::drive_run`] and [`Self::run_injection`]
    /// so the settle tail exists exactly once.
    async fn drive_accepted_run(self: Arc<Self>, handle: cyrup_agent::RunHandle) {
        {
            let _ = handle.finished().await;
            // GAP-11: apply the event-tier control ops (set_model / set_thinking_level) a guest queued
            // from `on_message_end` / a mid-turn tool hook / `on_agent_end`. This runs at a STORE-FREE
            // point — the whole run's ordered subscriber dispatch has returned, so every
            // `LiveExtension.inner` store guard is released and the drain's `thinking_level_select` /
            // `model_select` re-emit is a fresh top-level guest call, never a re-entry into the
            // suspended event-hook store (see live.rs `set_thinking_level`). This is the "before the
            // next turn" point the control queue promises, so the SUBSEQUENT `continue_run` (and the
            // next user turn) reads the new `agent.model` / `thinking_level`. Uses the `Send`-safe
            // focused drain (not the full `apply_pending_control`) because this future is spawned:
            // only SetModel/SetThinkingLevel can reach the queue from an event handler.
            self.apply_pending_agent_control().await;
            // Pi's continuation loop is `while (!this._agentRunAbortRequested)`
            // (`agent-session.ts:1473` @v0.87.1), so an abort landing DURING a `continue_run` also
            // stops the loop — not only one that lands before the post-run step.
            while !self.abort_requested() && self.handle_post_agent_run().await {
                match self.agent.continue_run().await {
                    Ok(h) => {
                        let _ = h.finished().await;
                        // Same store-free turn-boundary drain after each continuation settles.
                        self.apply_pending_agent_control().await;
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "continue_run refused after a post-run step said to continue"
                        );
                        break;
                    }
                }
            }
        }
        self.settle_run().await;
    }

    /// Pi `_runAgentPrompt`'s `finally` (agent-session.ts:1063-1072), in its exact order.
    async fn settle_run(&self) {
        // SESS-062 — pi's `finally` opens with `if (this._agentRunAbortRequested)
        // this._finishCancelledRetry();` (`agent-session.ts:1484` @v0.87.1), AHEAD of the
        // system-prompt reset, both flushes and `_emitAgentSettled()`.
        //
        // It is the only latch site that is reachable when the abort lands DURING a continuation,
        // and therefore the one that actually closes the retry sequence. The loop above is
        // `while !abort_requested() && handle_post_agent_run()`, so an abort arriving while
        // `continue_run()` is awaited exits on the loop's OWN condition —
        // `handle_post_agent_run` is never re-entered and none of its four
        // `finish_cancelled_retry()` calls run.
        //
        // Without this, a retry that an abort cut short leaks `retry_attempt` permanently: neither
        // side resets the counter at run START (pi zeroes `_retryAttempt` only at :960, :1519 and
        // :3366; cyrup only in `prepare_retry`, `handle_post_agent_run` and
        // `finish_cancelled_retry`). The leaked attempt survives into every later run until
        // `retry_attempt() >= retry_max_retries` refuses auto-retry for the life of the session,
        // and no `auto_retry_end` ever closes the "retrying" state a consumer opened on
        // `auto_retry_start` / the `will_retry` field of `agent_end`.
        //
        // `finish_cancelled_retry` is a no-op when no retry is in flight, which is why pi guards
        // only on the latch and not on the attempt count.
        if self.abort_requested() {
            self.finish_cancelled_retry().await;
        }
        // pi's `finally` clears the retry stash next (`this._failedResponse = undefined;`,
        // `agent-session.ts:1831` @v1.0.4), ahead of the system-prompt reset. This is the only
        // clear that runs when an abort lands DURING a continuation, so without it an aborted
        // retry sequence leaks its failed response into the next prompt's first request.
        self.clear_failed_response();
        // Pi `_runAgentPrompt`'s `finally` continues with `this._systemPromptOverride = undefined;`
        // (agent-session.ts:1069 @v0.83.0), BEFORE the bash flush and the settle emit — a
        // `before_agent_start` replacement is scoped to its own run and must not survive into the
        // next one (DRIFT-033).
        *Self::lock(&self.system_prompt_override) = None;
        // Pi `finally` (agent-session.ts:982-984): flush deferred bash messages from this turn.
        self.flush_pending_bash_messages().await;
        // …and, immediately after it, the deferred custom messages — pi's `finally` calls the two
        // back to back (agent-session.ts:1486-1487 @v0.87.1), so a `triggerTurn: false` message
        // queued during a run that ends without reaching a `turn_end` is never stranded. SEAM-127.
        self.flush_pending_custom_messages().await;
        // SEAM-005: the run has FULLY settled — the post-run loop above is done, so no retry,
        // compaction or queued continuation will follow. This is exactly Pi's `_emitAgentSettled()`
        // call site: the `finally` of `_runAgentPrompt` (agent-session.ts:1063-1072), AFTER
        // `_flushPendingBashMessages()` and BEFORE the idle wait resolves.
        //
        // SEAM-129 — `_emitAgentSettled` (`agent-session.ts:870-891` @v0.87.1) clears
        // `_isAgentRunActive` FIRST and emits under `_isEmittingAgentSettled`, so a handler that
        // submits a prompt is neither refused nor raced: the submission is deferred and run right
        // after the emit, before the idle wait resolves.
        self.run_released.store(true, Ordering::SeqCst);
        self.emitting_settled.store(true, Ordering::SeqCst);
        self.emit_agent_settled().await;
        // Terminate the run-scoped subscriptions returned by `prompt` now the whole loop has
        // settled. Ordered AFTER the settle emit so a run-scoped subscriber (what `prompt` hands
        // back) actually observes `agent_settled` as its last event — and BEFORE the emitting flag
        // drops, so a prompt that lands in between cannot register a stream this then closes.
        self.fanout.end_run();
        self.emitting_settled.store(false, Ordering::SeqCst);
        // EXT-087 — HOLD the settled-drain latch across the deferred work below. `wait_for_run_settled`
        // honours it, so a `wait_for_idle` cannot return between the release at the end of this
        // function and a queued send reaching `spawn_run` — pi's `_deferredSettledActions.length > 0`
        // keeping the idle wait pending (`core/agent-session.ts:881-890` @v0.87.1).
        let _ = self.settled_drain_tx.send(true);
        // SEAM-129 — pi's `for (const action of deferred) await action();` (`:881-887`): the prompts a
        // native `agent_settled` handler submitted while the emit ran.
        let deferred = std::mem::take(&mut *Self::lock(&self.deferred_settled));
        let started = self.run_deferred_settled(deferred).await;
        // EXT-087 — the POST-SETTLE drain: apply the control ops an `agent_settled` handler queued,
        // now that the dispatch above has fully returned. A guest's `send-user-message` crosses a wasm
        // import onto the control queue instead of reaching `prompt()` inside the dispatch, so the
        // QUEUE is its deferral and this is its splice. It runs STORE-FREE (`emit_agent_settled` has
        // returned, so a run it starts dispatches `agent_start` as a fresh top-level guest call) and
        // with the run already released above (`run_released`), so `is_run_active` is false and
        // `prompt_with` starts a run rather than refusing a send with no `deliverAs` as
        // `StreamingNeedsBehavior`. `take_pending_control` is take-once and this runs once per
        // settle, so a send queued from `agent_settled` starts exactly ONE run (pi's `splice(0)`,
        // `:881`). When a deferred native submission already started a run, the queue is left for
        // that run's own settle instead of being routed onto a run that is now streaming.
        if !started {
            self.apply_pending_control().await;
        }
        let _ = self.settled_drain_tx.send(false);
        // Pi's `_resolveIdleWaitIfIdle()` runs after the emit and the deferred actions — i.e. the
        // idle wait releases only after the event has been delivered. `driver_tx` is cyrup's idle
        // latch, so it drops last, and only if no run has claimed it since it was released.
        self.driver_tx.send_if_modified(|active| {
            let release = self.run_released.swap(false, Ordering::SeqCst);
            if release {
                *active = false;
            }
            release
        });
    }

    /// Mark a run as owning the session: `driver_tx` up and the SEAM-129 release cleared, in one
    /// step under the watch's lock so a settling run's tail cannot interleave between them.
    fn claim_run_latch(&self) {
        self.driver_tx.send_modify(|active| {
            self.run_released.store(false, Ordering::SeqCst);
            *active = true;
        });
    }

    /// Pi `_isEmittingAgentSettled` (`agent-session.ts:378` @v0.87.1).
    pub(crate) fn is_emitting_settled(&self) -> bool {
        self.emitting_settled.load(Ordering::SeqCst)
    }

    /// Pi `this._deferredSettledActions.push(…)`.
    pub(crate) fn defer_settled(&self, action: DeferredSettled) {
        Self::lock(&self.deferred_settled).push(action);
    }

    /// Pi `for (const action of deferred) await action();` (`agent-session.ts:881-887` @v0.87.1).
    /// Pi's `await this.prompt(…)` resolves only once that run has settled, so each later action
    /// waits for the run the previous one started. Returns whether the LAST action left a run
    /// owning the latch. A failed action ends the loop, as pi's throw leaves its `for`: the actions
    /// after it are dropped (a dropped [`DeferredSettled::PromptRun`] sender ends its caller's
    /// stream). The failure is logged, because it has no caller left to return to — pi's rejects
    /// the settling run's own `prompt()`, which cyrup's detached driver has already answered.
    ///
    /// Boxed as an explicitly `Send` future: an action starts a run through [`Self::spawn_run`],
    /// whose spawned driver ends in [`Self::settle_run`], which awaits this — an `async fn` cycle
    /// the compiler cannot prove `Send` through opaque types.
    fn run_deferred_settled(
        &self,
        deferred: Vec<DeferredSettled>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        Box::pin(self.run_deferred_settled_inner(deferred))
    }

    async fn run_deferred_settled_inner(&self, deferred: Vec<DeferredSettled>) -> bool {
        let mut started = false;
        for action in deferred {
            if started {
                self.wait_for_run_settled().await;
            }
            let outcome = match action {
                DeferredSettled::PromptRun { input, stream } => {
                    self.run_deferred_prompt(input, stream).await
                }
                DeferredSettled::Prompt { input, options } => {
                    Box::pin(self.prompt_with(input, options))
                        .await
                        .map(|accepted| matches!(accepted, PromptAccepted::Started))
                }
                DeferredSettled::Run(messages) => self.spawn_run(messages).await.map(|()| true),
            };
            match outcome {
                Ok(action_started) => started = action_started,
                Err(e) => {
                    tracing::warn!(error = %e, "a submission deferred past agent_settled failed");
                    return false;
                }
            }
        }
        started
    }

    /// [`Self::prompt_run`]'s body for a deferred submission whose stream was handed out already:
    /// the sender joins the run-scoped set exactly where `prompt_run` subscribes, and is withdrawn
    /// again when no run follows, which ends the caller's stream.
    async fn run_deferred_prompt(
        &self,
        input: UserInput,
        stream: tokio::sync::mpsc::Sender<AgentSessionEvent>,
    ) -> Result<bool, SessionServiceError> {
        if self.is_run_active() {
            return Err(SessionServiceError::StreamingNeedsBehavior);
        }
        self.fanout.adopt_run(stream.clone());
        let prepared = Box::pin(self.prepare(input, PromptOptions::default())).await;
        match prepared {
            Ok(Prepared::Run(messages)) => self.spawn_run(messages).await.map(|()| true),
            other => {
                self.fanout.release_run(&stream);
                other.map(|_| false)
            }
        }
    }

    /// Emit `agent_settled` (Pi `_emitAgentSettled`, agent-session.ts:581-588) — to the EXTENSION
    /// RUNNER first, then to the session subscribers, matching Pi's order exactly
    /// (`await this._extensionRunner.emit(...)` then `this._emit(...)`).
    ///
    /// Fires once per RUN, not once per agent loop: a turn that auto-retries produces two
    /// `agent_end`s and exactly one `agent_settled`. That is the whole reason the event exists —
    /// `agent_end` cannot tell a consumer whether more work is coming, which is why Pi's RPC host
    /// checks its shutdown request here and nowhere else (rpc-mode.ts:355-358).
    pub(crate) async fn emit_agent_settled(&self) {
        let cancel = self.session_cancel.child_token();
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(&HostEvent::AgentSettled, &cancel)
            .await;
        self.fanout_emit(AgentSessionEvent::AgentSettled).await;
    }

    /// Decide whether the just-finished run needs a continuation (Pi `_handlePostAgentRun`,
    /// agent-session.ts:986-1013): retry a transient error after backoff, close a spent retry
    /// sequence, run a post-run threshold/overflow compaction, or continue for `agent_end`-queued
    /// messages. Returns `true` when the driver should `agent.continue()`.
    async fn handle_post_agent_run(&self) -> bool {
        // Pi checks `_agentRunAbortRequested` FOUR times inside `_handlePostAgentRun`
        // (`agent-session.ts:1497-1500,1504-1505,1507-1510,1522-1523` @v0.87.1) plus once more in
        // its tail (:1528). Each one turns a decision that would have CONTINUED the run into a
        // stop, and each closes a real window: an abort can land between `agent_end` and this
        // function, during `prepare_retry`'s backoff, or during `check_compaction`'s summary call.
        if self.abort_requested() {
            self.finish_cancelled_retry().await;
            return false;
        }
        let Some(msg) = Self::lock(&self.last_assistant).take() else {
            return false;
        };
        // Retryable transient error → backoff + continue (Pi :991-993). The backoff is awaited
        // inside `prepare_retry`, so an abort can land there: Pi re-reads the latch straight after
        // (`if (this._agentRunAbortRequested) this._finishCancelledRetry(); return !latch;`, :1504).
        if self.is_retryable_error(&msg) && self.prepare_retry(&msg).await {
            if self.abort_requested() {
                self.finish_cancelled_retry().await;
                return false;
            }
            // pi `this._failedResponse = message;` immediately after the abort-latch re-read
            // (`agent-session.ts:1850-1853` @v1.0.4). Under a virtual selection this is what makes
            // the retried request route as `Retry` and hands the router the failed PHYSICAL
            // request — `prepare_retry` has already dropped that message from the transcript, so
            // nothing else can supply it.
            self.stash_failed_response(&msg);
            return true;
        }
        if self.abort_requested() {
            self.finish_cancelled_retry().await;
            return false;
        }
        // A terminal error with a spent / non-retryable budget closes the retry sequence (Pi :995-1003).
        if msg.stop_reason == cyrup_core::StopReason::Error && self.retry_attempt() > 0 {
            let attempt = std::mem::replace(&mut *Self::lock(&self.retry_attempt), 0);
            self.fanout_emit(AgentSessionEvent::AutoRetryEnd {
                success: false,
                attempt,
                final_error: msg.error_message.clone(),
            })
            .await;
        }
        if self.abort_requested() {
            self.finish_cancelled_retry().await;
            return false;
        }
        // Threshold / overflow post-run compaction → continue (Pi :1005-1007). The summary call is
        // awaited inside, and `abort()` cancels it, so the latch is re-read on the way out
        // (Pi `return !this._agentRunAbortRequested;`, :1522).
        match self.check_compaction(&msg, true).await {
            Ok(true) => return !self.abort_requested(),
            Ok(false) => {}
            // SESS-055 — the one throw `_checkCompaction` has is `getCompactionSettings(model)`
            // (`agent-session.ts:2604` @v0.87.1). It leaves `_runAgentPrompt`'s `while` for the
            // `finally`, so the run settles with NO continuation, even for messages an `agent_end`
            // handler queued. pi's `prompt()` then rejects with it; this driver runs detached from
            // the prompt caller, so the log is where the error goes, and the next prompt's
            // pre-send check refuses with the same message.
            Err(e) => {
                tracing::warn!(error = %e, "post-run compaction check failed");
                return false;
            }
        }
        // Messages queued by `agent_end` extension handlers need a continuation (Pi :1009-1012),
        // unless the run was aborted (Pi :1528).
        !self.abort_requested() && self.agent.has_queued_messages()
    }

    /// Close a retry sequence that an abort cut short — Pi `_finishCancelledRetry`
    /// (`agent-session.ts:3363-3374` @v0.87.1):
    ///
    /// ```ts
    /// private _finishCancelledRetry(): void {
    ///     if (this._retryAttempt === 0) return;
    ///     const attempt = this._retryAttempt;
    ///     this._retryAttempt = 0;
    ///     this._emit({ type: "auto_retry_end", success: false, attempt, finalError: "Retry cancelled" });
    /// }
    /// ```
    ///
    /// A no-op when no retry was in flight, which is why every abort path can call it
    /// unconditionally.
    async fn finish_cancelled_retry(&self) {
        let attempt = {
            let mut a = Self::lock(&self.retry_attempt);
            if *a == 0 {
                return;
            }
            std::mem::replace(&mut *a, 0)
        };
        self.fanout_emit(AgentSessionEvent::AutoRetryEnd {
            success: false,
            attempt,
            final_error: Some("Retry cancelled".to_string()),
        })
        .await;
    }

    /// The persist+fan-out subscriber's `message_start` handler for a USER message (Pi
    /// `_handleAgentEvent` head, agent-session.ts:514-535): reset the overflow-recovery latch and, when
    /// the message text matches a queued steer/follow-up mirror entry, drop it and emit `queue_update`
    /// as the agent drains the queue.
    pub(crate) async fn on_user_message_start(&self, message: &AgentMessage) {
        *Self::lock(&self.overflow_recovery_attempted) = false;
        let Some(text) = agent_user_text(message) else {
            return;
        };
        let mut drained = false;
        {
            let mut steer = Self::lock(&self.steering_messages);
            if let Some(pos) = steer.iter().position(|m| *m == text) {
                steer.remove(pos);
                drained = true;
            }
        }
        if !drained {
            let mut fu = Self::lock(&self.follow_up_messages);
            if let Some(pos) = fu.iter().position(|m| *m == text) {
                fu.remove(pos);
                drained = true;
            }
        }
        if drained {
            self.emit_queue_update().await;
        }
    }

    /// The subscriber's `message_end` handler for an ASSISTANT message (Pi `_handleAgentEvent` tail,
    /// agent-session.ts:673-694 @v0.84.3): track the last assistant message (drives the post-run
    /// loop), clear the overflow latch on a response that is neither an error NOR a length stop, and
    /// reset the retry counter on any non-error response, emitting `auto_retry_end{success:true}` if
    /// a retry sequence was in flight.
    ///
    /// SEAM-112 — the two clears carry DIFFERENT predicates upstream and must not be fused into one
    /// early return. pi guards the latch with `stopReason !== "error" && stopReason !== "length"`
    /// (`agent-session.ts:678`) and the retry counter with `stopReason !== "error"` alone (`:684`).
    /// cyrup kept only the shared arm, so every `Length` message cleared the latch here — i.e.
    /// immediately BEFORE `check_compaction` reads it (`auto_compaction.rs:85`) for the overflow
    /// case a `Length` message triggers (`is_context_overflow` case 3, `overflow.rs:101-109`). The
    /// read was therefore always `false`, the one-shot brake at `:85-98` was unreachable, and
    /// overflow recovery re-compacted and re-drove the interrupted turn without bound — re-running
    /// the same tool call on every pass. `Length` is a retriable, NOT-completed response here for
    /// the same reason it is one at `auto_compaction.rs:400-407`.
    pub(crate) async fn on_assistant_message_end(&self, assistant: &AssistantMessage) {
        *Self::lock(&self.last_assistant) = Some(assistant.clone());
        if assistant.stop_reason == cyrup_core::StopReason::Error {
            return;
        }
        if assistant.stop_reason != cyrup_core::StopReason::Length {
            *Self::lock(&self.overflow_recovery_attempted) = false;
        }
        let attempt = {
            let mut at = Self::lock(&self.retry_attempt);
            let v = *at;
            if v > 0 {
                *at = 0;
            }
            v
        };
        if attempt > 0 {
            self.fanout_emit(AgentSessionEvent::AutoRetryEnd {
                success: true,
                attempt,
                final_error: None,
            })
            .await;
        }
    }

    /// The shared preflight Pi's `prompt` performs before either running or queueing
    /// (agent-session.ts:1003-1142): emit the `input` extension event (which may fully service the
    /// submission), then — if the agent is streaming — expand templates and route to the steer/
    /// follow-up queue per `streaming_behavior` (erroring when none is given), else assemble the run
    /// input. Returns the disposition the caller acts on.
    async fn prepare(
        &self,
        mut ui: UserInput,
        options: PromptOptions,
    ) -> Result<Prepared, SessionServiceError> {
        // AGENT-030: pi's whole preflight reads `this.isStreaming` == `_isAgentRunActive`
        // (agent-session.ts:1022 for the `input` event's `streamingBehavior`, `:1159` for the
        // queue routing) — the latch that spans `_handlePostAgentRun` and every `agent.continue()`,
        // not a per-run flag. See [`Self::is_run_active`].
        let streaming = self.is_run_active();
        // 0. Slash extension-command exec FIRST (Pi `_tryExecuteExtensionCommand`,
        //    agent-session.ts:1004-1013): for `expandPromptTemplates && text.startsWith("/")`, if a
        //    registered command name matches, run its handler and short-circuit (no prompt sent).
        //    Matches Pi's order: tried BEFORE the `input` event + before skill/template expansion.
        if ui.expand_templates
            && ui.text.starts_with('/')
            && self.try_execute_extension_command(&ui.text).await
        {
            return Ok(Prepared::Handled);
        }
        // 1. `input` extension event, emitted BEFORE expansion (Pi agent-session.ts:1015-1033). A
        //    handler that returns `handled` fully services the submission — no run, no queue; a
        //    `transform` handler rewrites `ui` (text/images) in place before continuing. The handler
        //    sees `streamingBehavior` only while streaming (Pi `this.isStreaming ? ... : undefined`,
        //    agent-session.ts:1022).
        let handler_behavior = if streaming {
            options.streaming_behavior
        } else {
            None
        };
        if matches!(
            self.emit_input_event(&mut ui, handler_behavior).await,
            InputDisposition::Handled
        ) {
            return Ok(Prepared::Handled);
        }
        // GAP-11: apply any event-tier control op (set_model / set_thinking_level) an `on_input`
        // handler just queued, at this STORE-FREE point — `emit_input_event` has returned, releasing
        // every `LiveExtension.inner` guard, so the drain's re-emit is a fresh top-level guest call
        // (never a re-entry). This makes an `on_input` `setModel`/`setThinkingLevel` take effect on
        // the turn now being assembled, matching Pi, whose synchronous `on_input` mutation lands
        // before the dispatched turn (agent-session.ts:1015-1033). The focused drain never re-enters
        // `prepare` (unlike the full `apply_pending_control`'s `SendUserMessage` arm), keeping this
        // hot path free of the boxed async-recursion edge.
        self.apply_pending_agent_control().await;
        // 2. While streaming, expand then queue per `streamingBehavior` (Pi agent-session.ts:1043-
        //    1056). Without a behavior the submission is rejected (Pi throws at :1044).
        if streaming {
            let behavior = options
                .streaming_behavior
                .ok_or(SessionServiceError::StreamingNeedsBehavior)?;
            let mut queued = ui;
            if queued.expand_templates {
                queued.text = self.expand_input_text(&queued.text);
            }
            return Ok(Prepared::Queued(behavior, queued));
        }
        // 3. Not streaming: run the full pre-send sequence + assemble the run input.
        Ok(Prepared::Run(self.prepare_and_assemble(ui).await?))
    }

    /// Emit the `input` extension event (Pi `emitInput`, runner.ts:1095). A handler may fully
    /// service the submission (`HookOutcome::Handled`/`Block` ⇒ [`InputDisposition::Handled`]) or
    /// *transform* it (`HookOutcome::Mutate(EventPatch::Input{..})`, Pi `action:"transform"`,
    /// runner.ts:1116-1119): the folded text/images flow back into `ui` and the submission continues
    /// with the rewritten content (Pi agent-session.ts:1029-1032).
    async fn emit_input_event(
        &self,
        ui: &mut UserInput,
        streaming_behavior: Option<StreamingBehavior>,
    ) -> InputDisposition {
        if self
            .services
            .ext_host
            .dispatcher()
            .no_subscribers(cyrup_ext::EventKind::Input)
        {
            return InputDisposition::Continue;
        }
        let cancel = self.session_cancel.child_token();
        // Deliver the `source` (Pi `InputEvent.source`, agent-session.ts:1021) + the in-flight
        // `streamingBehavior` (`undefined` when idle, :1022) so a handler can branch on
        // interactive-vs-queued / steer-vs-follow-up before deciding (#13c).
        //
        // EXT-025: the ONE `input` emitter is the extension host's, as pi's is the runner's
        // `emitInput` (`core/extensions/runner.ts` @v0.87.1) — this used to be a second, inline copy
        // of the same reduction.
        let reduced = self
            .services
            .ext_host
            .emit_input(
                &ui.text,
                ui.images.clone(),
                input_event_source(ui.source),
                streaming_behavior.map(input_streaming_behavior),
                &cancel,
            )
            .await;
        match reduced {
            InputReduction::Handled | InputReduction::Blocked { .. } => InputDisposition::Handled,
            // Apply the `transform` the handler chain folded (Pi agent-session.ts:1029-1032:
            // `currentText`/`currentImages` adopt the result).
            InputReduction::Transform { text, images } => {
                ui.text = text;
                ui.images = images;
                InputDisposition::Continue
            }
            InputReduction::Continue => InputDisposition::Continue,
        }
    }

    /// Run the pre-send sequence Pi's `prompt` performs before dispatching the run
    /// (agent-session.ts:1037-1083): expand skill/prompt-template commands, flush any pending bash
    /// messages, run the `hasConfiguredAuth` precheck, and perform the pre-send compaction check
    /// (which catches an aborted last response). Then assemble the run input (`before_agent_start`
    /// hook + ordering). Returns the assembled run messages. Errors before any persistence on an auth
    /// miss (Pi `_getRequiredRequestAuth` throw → `preflightResult?.(false)`).
    async fn prepare_and_assemble(
        &self,
        mut input: UserInput,
    ) -> Result<Vec<AgentMessage>, SessionServiceError> {
        // 1. Skill (`/skill:name`) + prompt-template (`/name args`) expansion (agent-session.ts:1037).
        if input.expand_templates {
            input.text = self.expand_input_text(&input.text);
        }
        // 2. Flush deferred bash AND custom messages so ordering is intact — pi's two adjacent
        //    calls at the head of a new prompt (agent-session.ts:1670-1671 @v0.87.1, "Flush any
        //    pending bash and custom messages before the new prompt"). SEAM-127.
        self.flush_pending_bash_messages().await;
        self.flush_pending_custom_messages().await;
        // 3. Model + auth precheck. Pi validates the MODEL first —
        // `if (!this.model) { throw new Error(formatNoModelSelectedMessage()); }`
        // (agent-session.ts:1177-1180) — and only then the credential (`:1182-1195`). This is the
        // first turn of a modelless first run (SEAM-075): the answer is the `/login` → `/model`
        // instruction, surfaced as an error on the turn, never a process exit.
        {
            let model = Self::lock(&self.compaction_model)
                .clone()
                .ok_or(SessionServiceError::NoModelSelected)?;
            // PROV-037 — pi's refusal is a THREE-branch decision, not one
            // (`agent-session.ts:1182-1195` @v0.83.0):
            //
            //   const hasConfiguredAuth =
            //       this._modelRuntime.hasConfiguredAuth(this.model.provider) ||
            //       (await this._modelRuntime.checkAuth(this.model.provider)) !== undefined;
            //   if (!hasConfiguredAuth) {
            //       if (this._modelRuntime.isUsingOAuth(...)) throw new Error(`Authentication failed…`);
            //       throw new Error(formatNoApiKeyFoundMessage(this.model.provider));
            //   }
            //
            // cyrup consulted only the cached `has_configured_auth` and reported its own
            // `no configured auth for model: p/m`. Two consequences, both user-visible: a provider
            // whose credential is present but outside the cached configured set was refused where
            // pi re-checks and PROCEEDS, and an expired OAuth token produced a message that named
            // neither the provider nor `/login`.
            if !self.has_configured_auth(&model) && !self.recheck_provider_auth(&model).await {
                let provider = model.provider.as_str();
                return Err(SessionServiceError::AuthPreflightRefused(
                    if self.provider_is_oauth_backed(&model.provider).await {
                        crate::auth_guidance::format_oauth_reauthenticate_message(provider)
                    } else {
                        crate::auth_guidance::format_no_api_key_found_message(provider)
                    },
                ));
            }
        }
        // 4. Pre-send compaction check on the last assistant turn (agent-session.ts:1080-1083).
        // Pi calls `_checkCompaction` whenever there is a last assistant, with no enable gate of its
        // own (`agent-session.ts:1695-1698` @v0.87.1): the enable check lives inside, AFTER the
        // settings read that refuses an invalid compaction budget (SESS-055).
        if let Some(last) = self.last_assistant_message().await {
            let _ = self.check_compaction(&last, false).await?;
        }
        // 5. Assemble (before_agent_start hook + ordering).
        Ok(self.assemble_run_messages(input).await)
    }

    /// Expand a `/skill:name args` command to the skill block + args, or a `/name args` prompt
    /// template, leaving any other text unchanged (Pi `_expandSkillCommand` + `expandPromptTemplate`,
    /// agent-session.ts:1174-1204,1037-1041).
    fn expand_input_text(&self, text: &str) -> String {
        let expanded = self.expand_skill_command(text);
        let templates: Vec<_> = self.prompt_templates().winners().collect();
        cyrup_resources::expand_prompt_template(&expanded, templates)
    }

    /// `/skill:name args` → the skill block (Pi `_expandSkillCommand`, agent-session.ts:1174). Unknown
    /// skills / read failures pass the text through unchanged.
    fn expand_skill_command(&self, text: &str) -> String {
        let Some(rest) = text.strip_prefix("/skill:") else {
            return text.to_string();
        };
        let (name, args) = match rest.find(char::is_whitespace) {
            Some(i) => (&rest[..i], rest[i..].trim()),
            None => (rest, ""),
        };
        let Some(skill) = self
            .services
            .resources
            .skills
            .winners()
            .find(|s| s.name == name)
        else {
            return text.to_string();
        };
        let Ok(content) = std::fs::read_to_string(&skill.skill_md) else {
            return text.to_string();
        };
        let body = strip_frontmatter(&content).trim().to_string();
        let block = format!(
            "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
            skill.name,
            skill.skill_md.display(),
            skill.dir.display(),
            body
        );
        if args.is_empty() {
            block
        } else {
            format!("{block}\n\n{args}")
        }
    }

    /// The most recent assistant message on the current branch as a full [`AssistantMessage`] (for
    /// the compaction/retry checks), or `None`.
    async fn last_assistant_message(&self) -> Option<AssistantMessage> {
        self.messages()
            .await
            .into_iter()
            .rev()
            .find_map(|m| match m {
                Message::Assistant(a) => Some(a),
                _ => None,
            })
    }

    /// The messages a run starts with: [`Self::assemble_run_inputs`]'s, led by the system message
    /// that brings the transcript's prompt up to date when it is not (pi `messages.unshift(
    /// updateMessage)`, `agent-session.ts:2058-2060` @v1.0.0).
    ///
    /// The comparison is against the transcript as the run starts, after the handlers ran: a
    /// handler's `setActiveTools` has been applied by then, so the prompt describes the tools the
    /// run has.
    async fn assemble_run_messages(&self, input: UserInput) -> Vec<AgentMessage> {
        let mut messages = self.assemble_run_inputs(input).await;
        let transcript = self.agent.snapshot().await.messages;
        if let Some(update) = self.prompt_update(&transcript) {
            messages.insert(0, AgentMessage::System(update));
        }
        messages
    }

    /// Run the `before_agent_start` extension hook and assemble the run's input messages (R-06-014;
    /// Pi agent-session.ts:1105-1131). The hook chain may (a) **replace** the system prompt — kept in
    /// its own slot and projected onto the request for this run (pi `forceSystemPrompt`), and reset
    /// when no handler replaced it — and (b) **inject** additional messages, which are appended after
    /// the user message. Without this the assembled prompt was never offered to extensions (the gap
    /// the facade closes).
    async fn assemble_run_inputs(&self, input: UserInput) -> Vec<AgentMessage> {
        let user_text = input.text.clone();
        let images = input.images.clone();
        let user_msg = input.into_agent_message();
        // Drain any messages staged for this turn (Pi `_pendingNextTurnMessages`,
        // agent-session.ts:1099-1103); they are injected AFTER the user message in the run input.
        let pending: Vec<AgentMessage> = std::mem::take(&mut *Self::lock(&self.pending_next_turn));

        // The LIVE base (Pi reads the mutable field: `this._baseSystemPrompt`, agent-session.ts:1228
        // into `emitBeforeAgentStart`, :1252 for the reset) — NOT the frozen builder-assembled
        // `services.system_prompt`, which predates every `set_active_tools_by_name` /
        // `refresh_extension_tools` rebuild this session performed.
        let base = self.base_system_prompt();
        // Fast path: no extension listens for `before_agent_start` — keep the assembled base prompt.
        if self
            .services
            .ext_host
            .dispatcher()
            .no_subscribers(cyrup_ext::EventKind::BeforeAgentStart)
        {
            // No handler ran, so there is nothing to override with — pi's `else` branch
            // (agent-session.ts:1251 @v0.83.0) clears the slot for exactly this reason, and a stale
            // override from a PREVIOUS run must not leak into this one.
            *Self::lock(&self.system_prompt_override) = None;
            let mut messages = vec![user_msg];
            messages.extend(pending);
            return messages;
        }

        // EXT-025: the ONE `before_agent_start` emitter is the extension host's (pi's runner
        // `emitBeforeAgentStart`); it answers `None` for an unchanged prompt with nothing injected
        // and for a blocked/handled chain alike, both of which keep the base below.
        let cancel = self.session_cancel.child_token();
        let reduced = self
            .services
            .ext_host
            .emit_before_agent_start(
                &user_text,
                serde_json::to_value(&images).unwrap_or(serde_json::Value::Null),
                &base,
                serde_json::Value::Null,
                &cancel,
            )
            .await;

        let mut messages = vec![user_msg];
        messages.extend(pending);
        // Pi `setActiveTools` (pi-permission-system index.ts:2155): a `before_agent_start` handler may
        // have RESTRICTED the active tool set via `HostServices::set_active_tools` (the permission
        // companion's `shouldExposeTool` shaping), which stages the requested NAMES. Drain + apply
        // it IN-TURN here — before `spawn_run` — so the restriction shapes THIS turn (turn 1), not the
        // next turn boundary where `apply_pending_agent_control` would otherwise pick it up. The
        // prompt rebuilt for the restricted set becomes the base (pi `setActiveTools` →
        // `_rebuildSystemPrompt`), so the tools the handler hid are not listed in the prompt the model
        // is sent. Draining it here also leaves `pending_active_tools` empty for the later
        // `apply_pending_agent_control` drains, so the restriction is applied exactly once.
        if let Some(names) = self.services.host_services.take_pending_active_tools() {
            let (loadout, prompt) = { Self::lock(&self.dynamic_tools).set_active(&names) };
            self.push_active_tools(loadout, prompt).await;
        }
        if let Some(cyrup_ext::BeforeAgentStartReduction {
            system_prompt,
            injected,
        }) = reduced
        {
            // Record the (possibly handler-replaced / sanitized) system prompt in the override slot,
            // or clear it. Pi's two branches are `if (result?.systemPrompt !== undefined) {
            // this._systemPromptOverride = result.systemPrompt; … } else {
            // this._systemPromptOverride = undefined; … }` (agent-session.ts:1246-1252 @v0.83.0) —
            // the slot is written on both, so a replacement never outlives its run and a rebuild
            // cannot undo one (DRIFT-033).
            //
            // The slot is a FORCED prompt (pi `forceSystemPrompt`, `core/system-prompt.ts` @v1.0.0).
            // It is not written to the transcript, which keeps the structured sections whatever a
            // handler returned; it is projected onto each request instead, replacing the transcript's
            // system messages with one holding this text (`PolicyHooks::transform_context`, pi
            // `_installAgentForcedPromptProjection`, `agent-session.ts:1715-1731`).
            //
            // CYRUP-DELTA on the discriminator only: pi distinguishes "handler returned no prompt"
            // (`undefined`) from "handler returned one"; cyrup's `HostEvent::BeforeAgentStart`
            // carries the prompt as a mutated-in-place `String`, so a handler that returns the base
            // verbatim is indistinguishable from one that returns nothing. The reduction reports a
            // prompt only when it differs from `base`, so an unchanged one is read as "no override",
            // which agrees with pi on the resulting prompt for every input and differs only in
            // which slot holds the identical text.
            *Self::lock(&self.system_prompt_override) = system_prompt;
            messages.extend(injected.iter().map(core_message_to_agent));
        } else {
            // Nothing changed, or the chain was blocked/handled (no Pi analogue here): keep the
            // base prompt, no injection.
            *Self::lock(&self.system_prompt_override) = None;
        }
        messages
    }

    /// Await [`Self::is_idle`]: full settlement of the in-flight run AND its post-run loop
    /// (R-11-005), AND of any running compaction or branch summary (SEAM-125, pi v0.85.1's
    /// `isIdle = !_isAgentRunActive && !isCompacting`, whose manual-compaction and branch-summary
    /// `finally` blocks resolve the idle wait — `_clearManualCompactionState`,
    /// `agent-session.ts:2386-2388` @v0.87.1). A `print`/SDK caller that triggered `/compact`, an
    /// extension's `ctx.waitForIdle()`, and the injection pump therefore all wait a manual
    /// compaction out instead of starting work that races it.
    pub async fn wait_for_idle(&self) {
        loop {
            // Subscribe BEFORE reading the slots: a clear that lands between the read and the await
            // is then still observed as a change, so the wait cannot miss its own wake-up.
            let mut settled = self.compaction_settled.subscribe();
            self.wait_for_run_settled().await;
            if !self.is_compacting() {
                return;
            }
            if settled.changed().await.is_err() {
                return;
            }
        }
    }

    /// Await settlement of the in-flight run AND its post-run loop only — the two run latches
    /// [`Self::is_run_active`] reads. On a bound session the agent goes briefly idle BETWEEN a
    /// completed turn and a retry/compaction continuation, so this first awaits the post-run driver
    /// (`driver_tx` is `true` for the whole loop) and only then the agent — otherwise a one-shot
    /// caller would resume mid-loop.
    ///
    /// [`Self::abort_and_settle`] waits on this rather than [`Self::wait_for_idle`]: `abort()` does
    /// not yet cancel a compaction (that is area 03's `SESS-062`), so waiting for one there would
    /// hold a teardown or a `/compact` preflight for the whole summarization call.
    ///
    /// EXT-087 — it waits on a SECOND latch, `settled_drain_tx`, and the loop around the pair is
    /// the point. `settle_run` drops `driver_tx` before draining the control queue (it must: the
    /// same latch is the `is_run_active` routing predicate, and a drain that read RUNNING would
    /// have `prompt_with` refuse a `deliverAs`-less send instead of starting a run). Without the
    /// second latch there is a window where `driver_tx` reads idle while a queued send has not yet
    /// reached `spawn_run`, and a caller would watch the session go idle and then watch a run start
    /// under it. pi has no such window because it holds two pieces of state where cyrup held one —
    /// `_isAgentRunActive` cleared early (`core/agent-session.ts:872` @v0.87.1) and the idle wait
    /// resolved late and conditionally (`:881-890`).
    ///
    /// Both receivers are subscribed BEFORE either is read, so a raise that lands between the two
    /// reads is still observed as a change rather than missed.
    pub(super) async fn wait_for_run_settled(&self) {
        loop {
            let mut drain_rx = self.settled_drain_tx.subscribe();
            let mut rx = self.driver_tx.subscribe();
            while *rx.borrow_and_update() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
            if !*drain_rx.borrow_and_update() {
                break;
            }
            // A post-settle drain is in flight; it may be about to start a run. Wait for it to
            // finish, then re-check `driver_tx` — which that run's `spawn_run` will have raised.
            if drain_rx.changed().await.is_err() {
                break;
            }
        }
        self.agent.wait_for_idle().await;
    }

    /// Enqueue a steering message (delivered after the current tool batch, func-02 §9) — pi
    /// `steer(text, images, options)`, agent-session.ts:1858-1862 @v0.87.1, one line onto
    /// `_queueUserInput`. The caller's `UserInput::source` is pi's `options?.source ?? "interactive"`
    /// (cyrup already carries the provenance on the input itself, so the RPC arm's `user_input`
    /// helper supplies `InputSource::Rpc` with no extra parameter).
    pub async fn steer(
        &self,
        input: impl Into<UserInput>,
    ) -> Result<PromptAccepted, SessionServiceError> {
        self.queue_user_input(input.into(), StreamingBehavior::Steer)
            .await
    }

    /// Enqueue a follow-up message (delivered after the agent goes idle, func-02 §9) — pi
    /// `followUp(text, images, options)`, agent-session.ts:1871-1875 @v0.87.1.
    pub async fn follow_up(
        &self,
        input: impl Into<UserInput>,
    ) -> Result<PromptAccepted, SessionServiceError> {
        self.queue_user_input(input.into(), StreamingBehavior::FollowUp)
            .await
    }

    /// Pi's private `_queueUserInput` (agent-session.ts:1823-1848 @v0.87.1), step for step. Both
    /// public queue entry points are one line onto this.
    ///
    /// SEAM-121 — `steer`/`follow_up` previously went straight from the extension-command check to
    /// the expansion and the queue, skipping the `input` event entirely. Only `prepare` emitted it,
    /// so a queued submission — every mid-run steer, every RPC `steer`/`followUp` frame, every
    /// `sendUserMessage` landing on a live run — reached the transcript without an `on_input`
    /// handler ever seeing it: no `transform` applied, no `handled` honoured, and the handler could
    /// not observe the `rpc` source it is given upstream.
    async fn queue_user_input(
        &self,
        mut ui: UserInput,
        behavior: StreamingBehavior,
    ) -> Result<PromptAccepted, SessionServiceError> {
        // 1. pi :1829-1831 — the extension-command check is FIRST, ahead of the handlers.
        if ui.expand_templates {
            self.throw_if_extension_command(&ui.text)?;
        }
        // 2. pi :1833-1839 — `_runInputHandlers(text, images, source, this.isStreaming ? behavior :
        //    undefined)`. `is_run_active()` is cyrup's documented analogue of `this.isStreaming`
        //    (see the AGENT-030 note on [`Self::prepare`]), so the handler sees the queue selector
        //    only while a run is live. `undefined` (i.e. `handled`) queues NOTHING and emits no
        //    `queue_update`.
        let handler_behavior = self.is_run_active().then_some(behavior);
        if matches!(
            self.emit_input_event(&mut ui, handler_behavior).await,
            InputDisposition::Handled
        ) {
            return Ok(PromptAccepted::Handled);
        }
        // GAP-11, same reasoning as in [`Self::prepare`]: drain any control op an `on_input`
        // handler just queued at this store-free point, so a `setModel`/`setThinkingLevel` from a
        // handler that also steered takes effect rather than waiting for the next turn boundary.
        self.apply_pending_agent_control().await;
        // 3. pi :1840-1841 — expansion runs on the handler's OUTPUT, not on the raw text.
        if ui.expand_templates {
            ui.text = self.expand_input_text(&ui.text);
        }
        // 4. pi :1843-1847 — the private queue.
        Ok(match behavior {
            StreamingBehavior::Steer => self.queue_steer(ui).await,
            StreamingBehavior::FollowUp => self.queue_follow_up(ui).await,
        })
    }

    /// Pi's private `_queueSteer` (agent-session.ts:1879-1893 @v0.87.1): mirror the text into the
    /// facade queue, hand the message to the agent, emit `queue_update`. Deliberately does NO
    /// extension-command check, NO expansion and NO `input` dispatch — every caller has already run
    /// whichever of those apply, and doing them here is what made one submission dispatch `input`
    /// twice.
    pub(super) async fn queue_steer(&self, ui: UserInput) -> PromptAccepted {
        Self::lock(&self.steering_messages).push(ui.text.clone());
        self.agent.steer(ui.into_agent_message());
        self.emit_queue_update().await;
        PromptAccepted::Queued(StreamingBehavior::Steer)
    }

    /// Pi's private `_queueFollowUp` (agent-session.ts:1896-1910 @v0.87.1). See
    /// [`Self::queue_steer`].
    pub(super) async fn queue_follow_up(&self, ui: UserInput) -> PromptAccepted {
        Self::lock(&self.follow_up_messages).push(ui.text.clone());
        self.agent.follow_up(ui.into_agent_message());
        self.emit_queue_update().await;
        PromptAccepted::Queued(StreamingBehavior::FollowUp)
    }

    /// Error if `text` is a registered extension command (Pi `_throwIfExtensionCommand`,
    /// agent-session.ts:1312-1321): extension commands cannot be queued via `steer`/`follow_up`.
    /// Only `/`-prefixed text is checked; the registry covers native + wasm commands.
    fn throw_if_extension_command(&self, text: &str) -> Result<(), SessionServiceError> {
        let Some(body) = text.strip_prefix('/') else {
            return Ok(());
        };
        let name = body.split_once(' ').map_or(body, |(n, _)| n);
        if self
            .services
            .ext_host
            .registry()
            .has_command(name)
            .unwrap_or(false)
        {
            return Err(SessionServiceError::ExtensionCommandNotQueueable(
                name.to_string(),
            ));
        }
        Ok(())
    }
}

/// Strip a leading `---\n…\n---` YAML frontmatter block (Pi `stripFrontmatter`); returns the body
/// after it, or the original text when no frontmatter is present.
fn strip_frontmatter(content: &str) -> &str {
    let Some(rest) = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
    else {
        return content;
    };
    // Find the closing `---` line.
    if let Some(idx) = rest.find("\n---") {
        let after = &rest[idx + 4..];
        after
            .strip_prefix('\n')
            .or_else(|| after.strip_prefix("\r\n"))
            .unwrap_or(after)
    } else {
        content
    }
}

/// The concatenated text of a `user` agent message, or `None` for any other role (Pi
/// `_getUserMessageText`, agent-session.ts:589-595). Used to match a streaming user message against
/// the facade steer/follow-up queue mirrors so they drain in lockstep with the agent.
fn agent_user_text(m: &AgentMessage) -> Option<String> {
    match m {
        AgentMessage::User { content, .. } => Some(
            content
                .iter()
                .filter_map(|c| match c {
                    Content::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        ),
        _ => None,
    }
}
