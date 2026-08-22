//! Run lifecycle: the entry points that claim the run latch and spawn the run task
//! ([`Agent::prompt`], [`Agent::continue_run`], [`Agent::reset`]), the handle they hand back, and
//! the settlement guard + post-unwind failure emission that close a run out.

use super::message::errored_assistant;
use super::prompt::PromptInput;
use super::run::{EntryStart, RunCtx};
use super::util::{lock, panic_message};
use super::Agent;
use crate::error::{AgentError, BusyEntry, ContinueSurface};
use crate::event::{AgentEvent, AgentMessage};
use crate::state::{reduce, GenerationConfig, StateInner};
use crate::subscriber::EventSubscriber;
use cyrup_core::{Content, RunCancel, StopReason};
use futures::future::FutureExt;
use std::sync::{Arc, Mutex};
use tokio::sync::{oneshot, watch};

/// Resolves to the NEW messages created during the run (func-02 R-02-002).
pub struct RunHandle {
    new_messages: oneshot::Receiver<Vec<AgentMessage>>,
}

impl RunHandle {
    /// Await the run; yields the new messages (empty if the task was dropped).
    pub async fn finished(self) -> Vec<AgentMessage> {
        self.new_messages.await.unwrap_or_default()
    }
}

/// Emit one event without a [`RunCtx`] — the same reduce-then-await-subscribers path as
/// [`RunCtx::emit`], used by the catch-all failure path (Pi `handleRunFailure`, agent.ts:496-511)
/// after the run task has unwound and `RunCtx` is gone. Subscriber panics are contained.
pub(super) async fn emit_standalone(
    subscribers: &Arc<Mutex<Vec<Arc<dyn EventSubscriber>>>>,
    state: &Arc<Mutex<StateInner>>,
    cancel: &RunCancel,
    ev: AgentEvent,
) {
    {
        let mut st = lock(state);
        reduce(&mut st, &ev);
    }
    let subs = { lock(subscribers).clone() };
    for s in subs.iter() {
        // This IS the post-unwind failure path (pi's `handleRunFailure`), so a subscriber that
        // fails here has nowhere further to unwind; the panic is contained deliberately.
        let _ =
            std::panic::AssertUnwindSafe(s.on_event(&ev, cancel.child())).catch_unwind().await;
    }
}

/// Settlement safety-net (func-02 R-02-048): flips the run's settlement signals on scope exit —
/// the happy path AND any unwind (e.g. an uncontained panic on the run task) — so `wait_for_idle()`
/// can NEVER deadlock. The happy path records the run's new messages via [`SettlementGuard::complete`];
/// on an unwind the oneshot resolves to an empty `Vec`.
///
/// The run-active flag it clears is `running_tx` ITSELF, and deliberately not a second bool beside
/// it: `wait_for_idle()` releases on `running_tx` going false, so any separate "is a run in flight"
/// latch cleared AFTERWARDS opens a window in which a caller that has just been woken by this very
/// send is told the agent is idle and is then rejected with [`AgentError::RunActive`] by
/// [`Agent::start_run`]. That window is exactly two statements wide but a preemption between them
/// (routine under a loaded machine) stretches it to milliseconds — long enough for a woken caller
/// to run a full `prompt` preflight — which is how a `prompt(); wait_for_idle(); prompt()` sequence
/// could fail non-deterministically under parallel load.
pub(super) struct SettlementGuard {
    state: Arc<Mutex<StateInner>>,
    cancel_slot: Arc<Mutex<Option<RunCancel>>>,
    running_tx: watch::Sender<bool>,
    result_tx: Option<oneshot::Sender<Vec<AgentMessage>>>,
    new_messages: Vec<AgentMessage>,
}

impl SettlementGuard {
    fn complete(&mut self, new_messages: Vec<AgentMessage>) {
        self.new_messages = new_messages;
    }
}

impl Drop for SettlementGuard {
    fn drop(&mut self) {
        {
            let mut st = lock(&self.state);
            st.is_streaming = false;
            // AGENT-018 — pi resets `pendingToolCalls` in `finishRun()`
            // (`packages/agent/src/agent.ts:514-520` @v0.83.0, the clear at `:517`), called from
            // `runWithLifecycle`'s `finally` at `:491-493` — i.e. AFTER every `agent_end` listener
            // has settled. Clearing it inside the `agent_end` reducer arm instead meant a subscriber
            // reading `pending_tool_calls` on `agent_end` (the diagnostic for calls abandoned by an
            // aborted run) saw an empty set under cyrup and the real set under pi.
            st.pending_tool_calls.clear();
            st.streaming_message = None;
        }
        *lock(&self.cancel_slot) = None;
        // The ONE settlement write. Everything a waiter can observe about "is a run in flight" is
        // this channel, so the instant it reads `false` a fresh `start_run` is guaranteed to be
        // accepted — there is no second flag left set behind it.
        let _ = self.running_tx.send(false);
        if let Some(tx) = self.result_tx.take() {
            let _ = tx.send(std::mem::take(&mut self.new_messages));
        }
    }
}

impl Agent {
    /// Clear transcript, runtime state, and queued messages — REFUSED while a run is in flight (Pi
    /// `reset`, `packages/agent/src/agent.ts:332-345` @v0.84.1, whose first statement is
    /// `if (this.activeRun) { throw new Error("Agent is already processing. Wait for completion
    /// before resetting."); }`). v0.83.0's `reset()` had no guard, so this is upstream drift
    /// (AGENT-023): a `reset()` racing a live run emptied `state.messages` while the loop kept
    /// reducing `message_end` into it, and cleared `pending_tool_calls` while tools were still
    /// executing, so the run resumed writing into a cleared transcript.
    pub async fn reset(&self) -> Result<(), AgentError> {
        if self.is_running() {
            return Err(AgentError::RunActive(BusyEntry::Reset));
        }
        {
            let mut st = lock(&self.state);
            st.messages.clear();
            st.is_streaming = false;
            st.streaming_message = None;
            st.pending_tool_calls.clear();
            st.error_message = None;
        }
        lock(&self.steering).clear();
        lock(&self.follow_up).clear();
        Ok(())
    }

    // --- run entry points (R-02-001..006) ---
    /// Start a new run from a prompt (Pi `prompt`, `packages/agent/src/agent.ts:339-347`
    /// @v0.83.0, `:350-358` @v0.84.1).
    ///
    /// AGENT-034 — pi's `prompt()` carries its **own** run-active guard at `:340-344`, ahead of
    /// `normalizePromptInput` and of the latch claim inside `runWithLifecycle`, and it throws a
    /// message distinct from every other entry point's:
    /// `"Agent is already processing a prompt. Use steer() or followUp() to queue messages, or
    /// wait for completion."` — the one string in the family that tells the caller what to do
    /// instead. Pinned upstream by `packages/agent/test/agent.test.ts:508-547` @v0.83.0. As in
    /// [`Self::continue_run`], the check is a FAST PATH only: pi gets check-then-claim atomicity
    /// from single-threaded JS, so the latch in [`Self::start_run`] stays authoritative and a run
    /// claimed between the two yields [`BusyEntry::Run`].
    pub async fn prompt(&self, input: impl Into<PromptInput>) -> Result<RunHandle, AgentError> {
        if self.is_running() {
            return Err(AgentError::RunActive(BusyEntry::Prompt));
        }
        let input = input.into();
        self.start_run(EntryStart::Prompt(input.messages), false).await
    }

    /// Start a prompt from text plus image attachments (Pi `prompt(input, images?)`,
    /// agent.ts:326,379-383): the images are appended to the user message content after the text.
    ///
    /// This is the same upstream method as [`Self::prompt`] — one overload set behind one guard —
    /// so it carries the identical AGENT-034 fast-path check.
    pub async fn prompt_with_images(
        &self,
        text: impl Into<String>,
        images: Vec<Content>,
    ) -> Result<RunHandle, AgentError> {
        if self.is_running() {
            return Err(AgentError::RunActive(BusyEntry::Prompt));
        }
        self.start_run(EntryStart::Prompt(vec![PromptInput::text_with_images(text, images).into_one()]), false)
            .await
    }

    pub async fn continue_run(&self) -> Result<RunHandle, AgentError> {
        // AGENT-020 — pi's run-active guard is the FIRST statement of `continue()`
        // (`packages/agent/src/agent.ts:351-353` @v0.83.0), ahead of both the "No messages to
        // continue from" throw at `:355-358` and the two `drain()` calls at `:361`/`:367`. Ordering
        // it that way is what makes a rejected continuation leave the queues intact. Hoist it here
        // for the same reason — and note this is only a FAST PATH: pi gets check-then-claim
        // atomicity from single-threaded JS, Rust does not, so a run can still be claimed between
        // this read and the latch CAS in `start_run`. The `push_front` restores below are the half
        // that actually makes the drains lossless.
        if self.is_running() {
            return Err(AgentError::RunActive(BusyEntry::Continue));
        }
        let messages = lock(&self.state).messages.clone();
        if messages.is_empty() {
            // AGENT-034 — `Agent.continue()` says "No messages to continue from" (agent.ts:357
            // @v0.83.0 / :368 @v0.84.1); the low-level `agentLoopContinue` says something else
            // entirely, which is why the variant carries the surface.
            return Err(AgentError::NoMessages(ContinueSurface::Agent));
        }
        let last_is_assistant = messages.last().map(|m| m.is_assistant()).unwrap_or(false);
        if last_is_assistant {
            // R-02-005: drain steering, else follow-up, treat as a fresh prompt; else error.
            // A steering-drain continuation skips the loop's FIRST steering poll so a second queued
            // steering message is not drained a turn too early (Pi `skipInitialSteeringPoll`,
            // agent.ts:349-352); a follow-up-drain continuation does NOT skip (agent.ts:354-357).
            let steering = lock(&self.steering).drain();
            if !steering.is_empty() {
                // Restore on rejection so the drained batch is not dropped on the floor: pi's
                // guard-first ordering leaves `steeringQueue` untouched when the continuation is
                // refused, so the message is still delivered at the loop's next steering poll
                // (`agent-loop.ts:259`). Clone only what the restore needs.
                return match self.start_run(EntryStart::Prompt(steering.clone()), true).await {
                    Ok(h) => Ok(h),
                    Err(e) => {
                        lock(&self.steering).push_front(steering);
                        Err(e)
                    }
                };
            }
            let follow = lock(&self.follow_up).drain();
            if follow.is_empty() {
                return Err(AgentError::ContinueFromAssistant);
            }
            return match self.start_run(EntryStart::Prompt(follow.clone()), false).await {
                Ok(h) => Ok(h),
                Err(e) => {
                    lock(&self.follow_up).push_front(follow);
                    Err(e)
                }
            };
        }
        self.start_run(EntryStart::Continue, false).await
    }

    async fn start_run(
        &self,
        entry: EntryStart,
        skip_initial_steering_poll: bool,
    ) -> Result<RunHandle, AgentError> {
        // Claim the run-in-flight latch with an atomic compare-and-set on the very channel
        // `wait_for_idle`/`is_running` observe (Pi's `_isAgentRunActive` guard, agent.ts:398-400 —
        // single-threaded JS gets this atomicity for free; Rust has to ask for it). `send_if_modified`
        // runs the closure under the channel's own write lock and notifies receivers only when it
        // returns `true`, so this both rejects a concurrent second run and publishes "running" in
        // one indivisible step. Using a SEPARATE bool here (as this did) meant a caller woken by
        // `SettlementGuard`'s `send(false)` could reach this guard before the guard's next statement
        // cleared that bool, and get a spurious `RunActive`.
        let claimed = self.running_tx.send_if_modified(|running| {
            if *running {
                false
            } else {
                *running = true;
                true
            }
        });
        if !claimed {
            // AGENT-034 — pi's own latch guard (`runWithLifecycle`, agent.ts:472-474 @v0.83.0)
            // carries the bare `"Agent is already processing."`; the entry-point-specific texts
            // belong to the guards in `prompt`/`continue`/`reset`, which on a single JS thread
            // always fire first. Here they are only a fast path, so this string is reachable —
            // exactly on the check-then-claim race they cannot close.
            return Err(AgentError::RunActive(BusyEntry::Run));
        }
        let cancel = RunCancel::new();
        *lock(&self.cancel_slot) = Some(cancel.clone());
        // A clone kept for the catch-all failure path so it can distinguish an aborted run from a
        // genuine error after `RunCtx` (which owns the run's `cancel`) has unwound (Pi
        // `handleRunFailure(error, signal.aborted)`, agent.ts:490,496-511).
        let fail_cancel = cancel.clone();

        let (system_prompt, model, thinking_level, tools, messages, transport) = {
            let mut st = lock(&self.state);
            st.error_message = None;
            st.is_streaming = true;
            // Pi `createContextSnapshot` hands the loop a `.slice()` COPY of `messages`
            // (agent.ts:424-429); the loop mutates only that copy while the agent's observable
            // `state.messages` grows independently via the reducer on `message_end`.
            (
                st.system_prompt.clone(),
                st.model.clone(),
                st.thinking_level,
                st.tools.clone(),
                st.messages.clone(),
                st.transport,
            )
        };
        // `transport` is LIVE state, not a build-time constant: pi reads `this.transport` when it
        // assembles the loop config at RUN START (`createLoopConfig`, agent.ts:442) and the
        // `/settings` row mutates that field on the running agent (`interactive-mode.ts:4215`).
        // Overlaying it here — rather than reading it per-turn inside the loop — reproduces pi's
        // snapshot semantics exactly: a `set_transport` between runs takes effect on the next run
        // and never re-targets an in-flight one.
        let gen_config = GenerationConfig { transport, ..self.gen_config.clone() };

        let mut rc = RunCtx::new(
            self.state.clone(),
            self.subscribers.clone(),
            self.steering.clone(),
            self.follow_up.clone(),
            self.hooks.clone(),
            self.stream_fn.clone(),
            self.key_resolver.clone(),
            self.tool_execution,
            self.session_id.clone(),
            system_prompt,
            model,
            thinking_level,
            gen_config,
            tools,
            messages,
            cancel,
            skip_initial_steering_poll,
        )
        .with_header_fn(lock(&self.header_fn).clone());

        let (tx, rx) = oneshot::channel();
        let state = self.state.clone();
        let running_tx = self.running_tx.clone();
        let cancel_slot = self.cancel_slot.clone();
        // Independent handles for the catch-all failure path (Pi `handleRunFailure`,
        // agent.ts:496-511): they must outlive the unwound `RunCtx`.
        let fail_state = self.state.clone();
        let fail_subs = self.subscribers.clone();

        tokio::spawn(async move {
            // The guard settles on scope exit no matter how this task ends (normal return OR an
            // unwind), so `wait_for_idle()` can never deadlock (func-02 R-02-048).
            let mut guard = SettlementGuard {
                state,
                cancel_slot,
                running_tx,
                result_tx: Some(tx),
                new_messages: Vec::new(),
            };
            // Run the loop; if its task UNWINDS (an uncontained panic in a hook/executor), synthesize
            // Pi's closing sequence — an error assistant message + `message_start/message_end/
            // turn_end/agent_end` — so subscribers always see a complete, well-formed termination
            // (Pi `handleRunFailure`, agent.ts:496-511), then settle with that message.
            match std::panic::AssertUnwindSafe(rc.run(entry)).catch_unwind().await {
                Ok(new) => guard.complete(new),
                Err(payload) => {
                    let model = { lock(&fail_state).model.clone() };
                    // Pi: `stopReason = aborted ? "aborted" : "error"` (agent.ts:504). An aborted run
                    // that unwinds is reported as aborted, everything else as error.
                    let aborted = fail_cancel.is_cancelled();
                    let stop_reason =
                        if aborted { StopReason::Aborted } else { StopReason::Error };
                    // Pi: `errorMessage = error instanceof Error ? error.message : String(error)`
                    // (agent.ts:505). Rust `catch_unwind` cannot recover an arbitrary error value,
                    // but a `panic!`/`unwrap` payload is a `&str`/`String` we can downcast to recover
                    // the real message; otherwise fall back to a generic string.
                    let error_message = panic_message(payload.as_ref());
                    // Pi `handleRunFailure` failure message: one empty text block + `Date.now()`
                    // (agent.ts:497-506), NOT empty content / a zero timestamp.
                    let failure = errored_assistant(
                        model.provider.clone(),
                        model.model.as_str(),
                        model.api.clone(),
                        stop_reason,
                        error_message,
                    );
                    let fm = AgentMessage::Assistant(failure);
                    emit_standalone(
                        &fail_subs,
                        &fail_state,
                        &fail_cancel,
                        AgentEvent::MessageStart { message: fm.clone() },
                    )
                    .await;
                    emit_standalone(
                        &fail_subs,
                        &fail_state,
                        &fail_cancel,
                        AgentEvent::MessageEnd { message: fm.clone() },
                    )
                    .await;
                    emit_standalone(
                        &fail_subs,
                        &fail_state,
                        &fail_cancel,
                        AgentEvent::TurnEnd { message: fm.clone(), tool_results: Vec::new() },
                    )
                    .await;
                    emit_standalone(
                        &fail_subs,
                        &fail_state,
                        &fail_cancel,
                        AgentEvent::AgentEnd { messages: vec![fm.clone()] },
                    )
                    .await;
                    guard.complete(vec![fm]);
                }
            }
        });

        Ok(RunHandle { new_messages: rx })
    }
}
