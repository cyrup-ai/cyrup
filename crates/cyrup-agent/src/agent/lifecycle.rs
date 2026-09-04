//! Run lifecycle: the entry points that claim the run latch and spawn the run task
//! ([`Agent::prompt`], [`Agent::continue_run`], [`Agent::reset`]), the handle they hand back, and
//! the settlement guard + post-unwind failure emission that close a run out.

use super::Agent;
use super::message::errored_assistant;
use super::prompt::PromptInput;
use super::run::{PromptSource, ResumePoint, RunBaseline, RunCtx, RunEntry, RunShared};
use super::util::{lock, panic_message};
use crate::error::{AgentError, BusyEntry, ContinueSurface};
use crate::event::{AgentEvent, AgentMessage};
use crate::state::{GenerationConfig, StateInner, reduce};
use crate::subscriber::EventSubscriber;
use cyrup_core::{Content, RunCancel, SharedStr, StopReason};
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
        let _ = std::panic::AssertUnwindSafe(s.on_event(&ev, cancel.child()))
            .catch_unwind()
            .await;
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
/// [`Agent::claim_and_snapshot`]. That window is exactly two statements wide but a preemption between them
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
        // this channel, so the instant it reads `false` a fresh `claim_and_snapshot` is guaranteed to be
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
    /// from single-threaded JS, so the latch in [`Self::claim_and_snapshot`] stays authoritative and a run
    /// claimed between the two yields [`BusyEntry::Run`].
    pub async fn prompt(&self, input: impl Into<PromptInput>) -> Result<RunHandle, AgentError> {
        if self.is_running() {
            return Err(AgentError::RunActive(BusyEntry::Prompt));
        }
        let input = input.into();
        let (baseline, guard, cancel) = self.claim_and_snapshot()?;
        self.spawn_run(
            RunEntry::Prompt {
                messages: input.messages,
                source: PromptSource::Fresh,
            },
            baseline,
            guard,
            cancel,
        )
    }

    /// `prompt` with images attached to the single user message (Pi `prompt(text, images)`,
    /// agent.ts:352).
    pub async fn prompt_with_images(
        &self,
        text: impl Into<SharedStr>,
        images: Vec<Content>,
    ) -> Result<RunHandle, AgentError> {
        if self.is_running() {
            return Err(AgentError::RunActive(BusyEntry::Prompt));
        }
        let (baseline, guard, cancel) = self.claim_and_snapshot()?;
        self.spawn_run(
            RunEntry::Prompt {
                messages: vec![PromptInput::text_with_images(text, images).into_one()],
                source: PromptSource::Fresh,
            },
            baseline,
            guard,
            cancel,
        )
    }

    /// Continue the run from the current transcript WITHOUT adding a new message (Pi
    /// `continue()`, agent.ts:374-410). If the transcript ends in an assistant message, a queued
    /// steering message (then a queued follow-up) is drained and run as a prompt instead — pi's
    /// order at `:381-401` — and only if neither is queued does this reject with
    /// [`AgentError::ContinueFromAssistant`].
    ///
    /// Every branch runs against the SAME baseline: the latch is claimed first, then the
    /// transcript is snapshotted, validated and run, under one lock — see
    /// [`Self::claim_and_snapshot`]. A rejection after the claim releases the latch through the
    /// guard's drop, and a requeue-on-failure puts a drained queue back exactly as before.
    pub async fn continue_run(&self) -> Result<RunHandle, AgentError> {
        // AGENT-034 — pi's own guard text for this entry point (agent.ts:376-378); a FAST PATH
        // only, the latch claim below stays authoritative.
        if self.is_running() {
            return Err(AgentError::RunActive(BusyEntry::Continue));
        }
        let (baseline, guard, cancel) = self.claim_and_snapshot()?;
        if baseline.messages.is_empty() {
            // `guard` drops here and releases the latch.
            return Err(AgentError::NoMessages(ContinueSurface::Agent));
        }
        let last_is_assistant = baseline.messages.last().is_some_and(|m| m.is_assistant());
        if last_is_assistant {
            // Pi `:381-390`: drain steering first, running it as the prompt with the loop's
            // first steering poll skipped so the SECOND queued message lands on the next turn.
            let steering = lock(&self.steering).drain();
            if !steering.is_empty() {
                let entry = RunEntry::Prompt {
                    messages: steering.clone(),
                    source: PromptSource::SteeringDrain,
                };
                return self
                    .spawn_run(entry, baseline, guard, cancel)
                    .inspect_err(|_| {
                        lock(&self.steering).push_front(steering);
                    });
            }
            // Pi `:391-401`: then follow-up; with neither queued, the continuation is refused.
            let follow = lock(&self.follow_up).drain();
            if follow.is_empty() {
                return Err(AgentError::ContinueFromAssistant);
            }
            let entry = RunEntry::Prompt {
                messages: follow.clone(),
                source: PromptSource::FollowUpDrain,
            };
            return self
                .spawn_run(entry, baseline, guard, cancel)
                .inspect_err(|_| {
                    lock(&self.follow_up).push_front(follow);
                });
        }
        let proof = ResumePoint::check(&baseline.messages, ContinueSurface::Agent)?;
        self.spawn_run(RunEntry::Continue(proof), baseline, guard, cancel)
    }

    /// The handles a run shares with the agent for its whole lifetime.
    fn shared(&self) -> RunShared {
        RunShared {
            state: self.state.clone(),
            subscribers: self.subscribers.clone(),
            steering: self.steering.clone(),
            follow_up: self.follow_up.clone(),
            hooks: self.hooks.clone(),
            stream_fn: self.stream_fn.clone(),
            key_resolver: self.key_resolver.clone(),
            tool_execution: self.tool_execution,
            session_id: self.session_id.clone(),
        }
    }

    /// Claim the run latch, then — under ONE state lock — take the run-start baseline and
    /// perform the two run-start writes. The caller validates against the very transcript the
    /// run will use, and the returned [`SettlementGuard`] releases the latch on drop, so a
    /// rejection after the claim unwinds exactly as a finished run would.
    ///
    /// The latch is an atomic compare-and-set on the very channel `wait_for_idle`/`is_running`
    /// observe (Pi's `_isAgentRunActive` guard, agent.ts:398-400 — single-threaded JS gets this
    /// atomicity for free; Rust has to ask for it). `send_if_modified` runs the closure under the
    /// channel's own write lock and notifies receivers only when it returns `true`, so this both
    /// rejects a concurrent second run and publishes "running" in one indivisible step.
    ///
    /// Why the claim comes FIRST: the two state writes below must never be performed by a caller
    /// that is about to be rejected, and the snapshot must never be taken before the claim — a
    /// `set_messages` in that gap would leave the run on a transcript that was validated but is
    /// no longer the agent's. Claim, then read-validate-write under one lock, closes both.
    fn claim_and_snapshot(&self) -> Result<(RunBaseline, SettlementGuard, RunCancel), AgentError> {
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
            // belong to the fast-path guards in `prompt`/`continue`/`reset`, which on a single JS
            // thread always fire first. Here they are only a fast path, so this string is
            // reachable — exactly on the check-then-claim race they cannot close.
            return Err(AgentError::RunActive(BusyEntry::Run));
        }
        let cancel = RunCancel::new();
        *lock(&self.cancel_slot) = Some(cancel.clone());
        // Built here, before any state is touched, so every early `Err` below releases the latch,
        // clears the cancel slot and releases the latch through its drop.
        let guard = SettlementGuard {
            state: self.state.clone(),
            cancel_slot: self.cancel_slot.clone(),
            running_tx: self.running_tx.clone(),
            result_tx: None,
            new_messages: Vec::new(),
        };
        let baseline = {
            let mut st = lock(&self.state);
            // Resolved FIRST, before the run-start write below, so a modelless agent
            // performs no state write and — through `guard`'s drop — never holds the latch
            // after this returns. Checked under the same lock as the snapshot (not before the
            // claim) for the reason the claim itself comes first: a `set_model(None)` in a
            // check-then-claim gap would otherwise start a run with no model.
            let Some(model) = st.model.clone() else {
                return Err(AgentError::NoModelSelected);
            };
            st.error_message = None;
            // Pi `createContextSnapshot` hands the loop a `.slice()` COPY of `messages`
            // (agent.ts:424-429); the reducer grows `state.messages` independently.
            //
            // `transport` is LIVE state, not a build-time constant: pi reads `this.transport`
            // when it assembles the loop config at RUN START (`createLoopConfig`, agent.ts:442)
            // and the `/settings` row mutates that field on the running agent
            // (`interactive-mode.ts:4215`). Overlaying it here reproduces pi's snapshot semantics
            // exactly: a `set_transport` between runs takes effect on the next run and never
            // re-targets an in-flight one.
            RunBaseline {
                system_prompt: st.system_prompt.clone(),
                model,
                thinking_level: st.thinking_level,
                gen_config: GenerationConfig {
                    transport: st.transport,
                    ..self.gen_config.clone()
                },
                tools: st.tools.clone(),
                messages: st.messages.clone(),
            }
        };
        Ok((baseline, guard, cancel))
    }

    /// Build the run context from a claimed latch and its baseline, and spawn the run task. Reads
    /// no agent state: everything the run needs is in `baseline`, taken under the lock that
    /// validated it. Infallible — the `Result` exists so a caller's requeue-on-failure reads
    /// cleanly.
    fn spawn_run(
        &self,
        entry: RunEntry,
        baseline: RunBaseline,
        mut guard: SettlementGuard,
        cancel: RunCancel,
    ) -> Result<RunHandle, AgentError> {
        // A clone kept for the catch-all failure path so it can distinguish an aborted run from a
        // genuine error after `RunCtx` (which owns the run's `cancel`) has unwound (Pi
        // `handleRunFailure(error, signal.aborted)`, agent.ts:490,496-511).
        let fail_cancel = cancel.clone();
        let fail_model = baseline.model.clone();
        let mut rc = RunCtx::new(self.shared(), baseline, cancel)
            .with_header_fn(lock(&self.header_fn).clone());

        let (tx, rx) = oneshot::channel();
        guard.result_tx = Some(tx);
        // The failure twin below emits through the same subscribers and state the run would have.
        let fail_state = self.state.clone();
        let fail_subs = self.subscribers.clone();

        tokio::spawn(async move {
            // `guard` settles the run on EVERY exit — normal completion, a `RunFailure` the loop
            // converted into a terminal assistant message, or a panic caught below.
            match std::panic::AssertUnwindSafe(rc.run(entry))
                .catch_unwind()
                .await
            {
                Ok(new) => guard.complete(new),
                Err(payload) => {
                    // Pi reads `this._state.model` (agent.ts:500-502); with `Option` the run's own
                    // baseline is the fallback for a model cleared mid-run — never an empty address.
                    let model = { lock(&fail_state).model.clone() }.unwrap_or(fail_model);
                    let aborted = fail_cancel.is_cancelled();
                    let stop_reason = if aborted {
                        StopReason::Aborted
                    } else {
                        StopReason::Error
                    };
                    // Pi `handleRunFailure` synthesizes an errored assistant message and emits the
                    // full terminal sequence so no subscriber is left mid-turn.
                    let error_message = panic_message(payload.as_ref());
                    let failure = errored_assistant(
                        model.provider.clone(),
                        model.model.as_str(),
                        model.api.clone(),
                        stop_reason,
                        error_message,
                    );
                    let fm = AgentMessage::Assistant(Arc::new(failure));
                    emit_standalone(
                        &fail_subs,
                        &fail_state,
                        &fail_cancel,
                        AgentEvent::MessageStart {
                            message: fm.clone(),
                        },
                    )
                    .await;
                    emit_standalone(
                        &fail_subs,
                        &fail_state,
                        &fail_cancel,
                        AgentEvent::MessageEnd {
                            message: fm.clone(),
                        },
                    )
                    .await;
                    emit_standalone(
                        &fail_subs,
                        &fail_state,
                        &fail_cancel,
                        AgentEvent::TurnEnd {
                            message: fm.clone(),
                            tool_results: Vec::new(),
                        },
                    )
                    .await;
                    emit_standalone(
                        &fail_subs,
                        &fail_state,
                        &fail_cancel,
                        AgentEvent::AgentEnd {
                            messages: vec![Arc::new(fm.clone())],
                        },
                    )
                    .await;
                    guard.complete(vec![fm]);
                }
            }
        });

        Ok(RunHandle { new_messages: rx })
    }
}
