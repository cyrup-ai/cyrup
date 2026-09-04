//! The run context — one run's working state, owned by the run task, plus the plumbing every
//! phase of the loop shares: event emission, the failure path, queue polls, tool lookup.

mod assistant_stream;
mod stream;
mod tools;
mod turn;

use super::HeaderFn;
use super::message::errored_assistant;
use super::util::{lock, panic_message};
use crate::error::{AgentError, ContinueSurface};
use crate::event::{AgentEvent, AgentMessage};
use crate::hooks::Hooks;
use crate::queue::{PendingQueue, ToolExecution};
use crate::state::{GenerationConfig, StateInner, reduce};
use crate::stream_fn::{ApiKeyResolver, StreamFn};
use crate::subscriber::EventSubscriber;
use cyrup_core::{ModelRef, ModelThinkingLevel, RunCancel, SessionId, StopReason, Tool};
use futures::future::FutureExt;
use std::sync::{Arc, Mutex};

/// Where a `RunEntry::Prompt`'s messages came from. Only a steering drain skips the loop's first
/// steering poll (pi `skipInitialSteeringPoll`, agent.ts:351,440-446), so that the SECOND queued
/// steering message is not jammed into the same turn as the drained prompt.
pub(crate) enum PromptSource {
    Fresh,
    SteeringDrain,
    FollowUpDrain,
}

/// How a run starts. `Continue` cannot be built without a [`ResumePoint`] — the proof that the
/// transcript may be resumed without a new message — so the precondition has one home and cannot
/// be skipped.
pub(crate) enum RunEntry {
    Prompt {
        messages: Vec<AgentMessage>,
        source: PromptSource,
    },
    Continue(ResumePoint),
}

impl RunEntry {
    pub(crate) fn skip_initial_steering_poll(&self) -> bool {
        matches!(
            self,
            RunEntry::Prompt {
                source: PromptSource::SteeringDrain,
                ..
            }
        )
    }
}

/// Proof that a transcript may be resumed without a new message: non-empty, and not ending in an
/// assistant message (the provider would otherwise reject the request). Zero-sized, private
/// field, ONE constructor — the single home of a rule that used to be written out three times.
pub(crate) struct ResumePoint(());

impl ResumePoint {
    pub(crate) fn check(
        messages: &[AgentMessage],
        surface: ContinueSurface,
    ) -> Result<Self, AgentError> {
        if messages.is_empty() {
            return Err(AgentError::NoMessages(surface));
        }
        if messages.last().is_some_and(|m| m.is_assistant()) {
            return Err(AgentError::ContinueFromAssistant);
        }
        Ok(ResumePoint(()))
    }
}

/// Handles that live for the whole run — cloned out of [`super::Agent`] (or built by
/// `crate::loop_fn`) exactly once per run.
pub(crate) struct RunShared {
    pub state: Arc<Mutex<StateInner>>,
    pub subscribers: Arc<Mutex<Vec<Arc<dyn EventSubscriber>>>>,
    pub steering: Arc<Mutex<PendingQueue>>,
    pub follow_up: Arc<Mutex<PendingQueue>>,
    pub hooks: Arc<dyn Hooks>,
    pub stream_fn: Arc<dyn StreamFn>,
    pub key_resolver: Option<Arc<dyn ApiKeyResolver>>,
    pub tool_execution: ToolExecution,
    pub session_id: Option<SessionId>,
}

/// The run-start `.slice()` baseline — pi `createContextSnapshot` — taken under the state lock,
/// after the run latch is claimed, so it is the transcript the run actually uses.
pub(crate) struct RunBaseline {
    pub system_prompt: String,
    pub model: ModelRef,
    pub thinking_level: ModelThinkingLevel,
    pub gen_config: GenerationConfig,
    pub tools: Vec<Arc<dyn Tool>>,
    pub messages: Vec<AgentMessage>,
}

/// A run-aborting failure travelling up to [`RunCtx::run`], which converts it into Pi's
/// `handleRunFailure` quartet (`packages/agent/src/agent.ts:496-512` @v0.83.0).
///
/// This is cyrup's stand-in for the JS exception that unwinds out of `runLoop` into
/// `runWithLifecycle`'s catch (`agent.ts:489-490`). Pi has exactly three producers of it inside the
/// loop and all three are bare `await`s: `transformContext` / `convertToLlm`
/// (`agent-loop.ts:288-295`, AGENT-025), the two post-turn hooks (`agent-loop.ts:231`, `:246-252`),
/// and a throwing event listener (`agent.ts:573-575`, AGENT-033). The payload is the thrown value's
/// own text — `error instanceof Error ? error.message : String(error)` (`agent.ts:505`).
struct RunFailure(String);

// ---------------------------------------------------------------------------
// The run context (owns one run's working state; lives on the run task)
// ---------------------------------------------------------------------------

pub(crate) struct RunCtx {
    state: Arc<Mutex<StateInner>>,
    subscribers: Arc<Mutex<Vec<Arc<dyn EventSubscriber>>>>,
    steering: Arc<Mutex<PendingQueue>>,
    follow_up: Arc<Mutex<PendingQueue>>,
    hooks: Arc<dyn Hooks>,
    stream_fn: Arc<dyn StreamFn>,
    key_resolver: Option<Arc<dyn ApiKeyResolver>>,
    tool_execution: ToolExecution,
    session_id: Option<SessionId>,
    system_prompt: String,
    /// Running model baseline; a `prepare_next_turn` model override updates it stickily
    /// (Pi `config.model`, agent.ts:425 / agent-loop.ts:228-238).
    model: ModelRef,
    /// Running thinking level; a `prepare_next_turn` `thinking_level` override updates it stickily
    /// (Pi `config.reasoning`, agent.ts:426 / agent-loop.ts:228-238).
    thinking_level: ModelThinkingLevel,
    /// Generation params + telemetry forwarded into `StreamOptions` (Pi `AgentLoopConfig`).
    gen_config: GenerationConfig,
    tools: Vec<Arc<dyn Tool>>,
    cancel: RunCancel,
    new_messages: Vec<Arc<AgentMessage>>,
    /// The loop's OWN working transcript — Pi `currentContext.messages`, a `.slice()` SNAPSHOT of the
    /// agent's `messages` taken at run start, NOT the live `Arc` (agent.ts:424-429; agent-loop.ts:104-107).
    /// This is the array the loop reads to build each LLM payload and that a `prepare_next_turn`
    /// context override replaces. The agent's observable `state.messages` grows INDEPENDENTLY via the
    /// reducer on `message_end` (agent.ts:519-522), so neither a context override nor a mid-run
    /// external `set_messages` leaks between the two — exactly as in Pi.
    ///
    /// `Vec<Arc<AgentMessage>>`, not `Vec<AgentMessage>`, so that snapshot is what Pi's `.slice()`
    /// actually is: a copy of the POINTERS. Cloning it per turn used to deep-copy every message —
    /// after PERF-001 that is O(1) for text, thinking and tool arguments, but still O(bytes) for
    /// `Content::Image` base64 (capped at 4.5 MB each) and for `Custom`/`App` JSON payloads. The
    /// element type is what makes the snapshot O(n) pointer bumps regardless of what it holds.
    messages: Vec<Arc<AgentMessage>>,
    turn_index: usize,
    /// On continue-from-assistant, the first `getSteeringMessages` poll returns `[]` so a second
    /// queued steering message is not drained a turn too early (Pi `skipInitialSteeringPoll`,
    /// agent.ts:351,440-446).
    skip_initial_steering_poll: bool,
    /// AGENT-029 — pi's per-request `transformHeaders` (`sdk.ts:318-327` @v0.83.0), consulted with
    /// THIS turn's model. `None` keeps the pre-existing static-overlay read.
    header_fn: Option<Arc<HeaderFn>>,
}

impl RunCtx {
    /// Assemble a run context from already-built shared handles. Used by [`super::Agent::spawn_run`] and by
    /// the low-level free-function loop (`crate::loop_fn`) so both drive the identical, tested loop.
    pub(crate) fn new(shared: RunShared, baseline: RunBaseline, cancel: RunCancel) -> Self {
        let RunShared {
            state,
            subscribers,
            steering,
            follow_up,
            hooks,
            stream_fn,
            key_resolver,
            tool_execution,
            session_id,
        } = shared;
        let RunBaseline {
            system_prompt,
            model,
            thinking_level,
            gen_config,
            tools,
            messages,
        } = baseline;
        Self {
            state,
            subscribers,
            steering,
            follow_up,
            hooks,
            stream_fn,
            key_resolver,
            tool_execution,
            session_id,
            system_prompt,
            model,
            thinking_level,
            gen_config,
            tools,
            cancel,
            new_messages: Vec::new(),
            // The run-start snapshot arrives owned (§6.5 keeps the entry surface
            // `Vec<AgentMessage>`); wrap each message once, here, so every per-turn snapshot of
            // this vector downstream is a pointer copy.
            messages: messages.into_iter().map(Arc::new).collect(),
            turn_index: 0,
            // Derived from the entry in `run`, never supplied by a caller.
            skip_initial_steering_poll: false,
            header_fn: None,
        }
    }

    pub(crate) fn with_header_fn(mut self, f: Option<Arc<HeaderFn>>) -> Self {
        self.header_fn = f;
        self
    }

    /// The header overlay for `model` — pi's `transformHeaders` result for the request it is about
    /// to make (`sdk.ts:318-327` @v0.83.0). Falls back to the static [`super::Agent::set_headers`] overlay
    /// when no resolver is installed or the resolver has no opinion about this model.
    fn headers_for(&self, model: &ModelRef) -> Option<cyrup_provider::HeaderMap> {
        if let Some(f) = &self.header_fn
            && let Some(h) = f(model)
        {
            return Some(h);
        }
        lock(&self.state).headers.clone()
    }

    /// The sole emission path (arch-02 §5.1): reduce managed state (lock released BEFORE awaiting),
    /// then await each subscriber in registration order before returning.
    ///
    /// AGENT-033 — a FAILING subscriber fails the RUN. Pi's `processEvents`
    /// (`packages/agent/src/agent.ts:573-575` @v0.83.0, `:588-590` @v0.84.1) awaits each listener
    /// bare inside `runWithLifecycle`'s try (`:487-490`), so a throwing listener stops the listener
    /// loop, unwinds out of the loop, and produces the full `handleRunFailure` quartet with the
    /// listener's own message as `errorMessage`. cyrup used to `catch_unwind` and DISCARD, hiding a
    /// broken observer entirely; now the panic message is returned as a [`RunFailure`] that the
    /// caller propagates with `?` up to [`Self::run`], which routes it into
    /// [`Self::emit_run_failure`] — pi's `handleRunFailure`.
    ///
    /// The `catch_unwind` itself is kept: a Rust panic cannot cross an `await` boundary the way a JS
    /// rejection propagates, so it must be caught to be turned into a value at all. `AssertUnwindSafe`
    /// is sound because emission is the sole writer of managed state and the lock is released before
    /// this await, so no broken invariant can leak across the boundary (keeps the crate
    /// `#![forbid(unsafe_code)]`).
    async fn emit(&self, ev: AgentEvent) -> Result<(), RunFailure> {
        {
            let mut st = lock(&self.state);
            reduce(&mut st, &ev);
        }
        let subs = { lock(&self.subscribers).clone() };
        for s in subs.iter() {
            if let Err(payload) = std::panic::AssertUnwindSafe(s.on_event(&ev, self.cancel.child()))
                .catch_unwind()
                .await
            {
                // Pi stops iterating the listener set on the first throw; so do we.
                return Err(RunFailure(panic_message(payload.as_ref())));
            }
        }
        Ok(())
    }

    /// Pi `handleRunFailure` (agent.ts:496-511) reached from INSIDE the loop: the post-turn hooks
    /// (`prepareNextTurn`, agent-loop.ts:231; `shouldStopAfterTurn`, agent-loop.ts:246-252) are
    /// awaited with no try/catch, so a throw unwinds out of `runLoop` into `runWithLifecycle`'s
    /// catch (agent.ts:489-490) and is reported as a run FAILURE: one synthetic errored assistant
    /// message (empty text block, wall-clock timestamp, `stopReason` aborted-vs-error, the thrown
    /// `error.message`) followed by `message_start` → `message_end` → `turn_end` (with NO tool
    /// results) → `agent_end` carrying `[failureMessage]` and nothing else (agent.ts:508-511).
    ///
    /// The post-unwind twin of this path lives at [`super::Agent::spawn_run`]'s `catch_unwind` arm, which must
    /// synthesize the same quartet through [`super::lifecycle::emit_standalone`] because its `RunCtx` is already gone;
    /// here the live `RunCtx` is intact, so emission goes through the ordinary [`RunCtx::emit`] and
    /// the reducer records `error_message`/`stop_reason` exactly as it does for a streamed message.
    ///
    /// `new_messages` is REPLACED by the failure message so the run's returned value matches
    /// `agent_end.messages` — Pi's failed run resolves its promise without the loop-local
    /// `newMessages` accumulator (the throw at agent.ts:488 never reaches `runLoop`'s return), and the
    /// `catch_unwind` twin settles the same single-element vector.
    async fn emit_run_failure(&mut self, error_message: String) {
        // Pi reads `this._state.model` (agent.ts:500-502) — the agent's state model, not the loop's
        // possibly-overridden running baseline; `self.model` is the fallback for a model cleared
        // mid-run.
        let model = { lock(&self.state).model.clone() }.unwrap_or_else(|| self.model.clone());
        // Pi `stopReason: aborted ? "aborted" : "error"` (agent.ts:504).
        let stop_reason = if self.cancel.is_cancelled() {
            StopReason::Aborted
        } else {
            StopReason::Error
        };
        let failure = errored_assistant(
            model.provider.clone(),
            model.model.as_str(),
            model.api.clone(),
            stop_reason,
            error_message,
        );
        let fm = AgentMessage::Assistant(Arc::new(failure));
        // This IS Pi's catch handler, so a subscriber that fails while it runs has nowhere further
        // to unwind (pi's throw would escape `runWithLifecycle` entirely and reach the caller of
        // `prompt()`); the closing quartet is emitted best-effort.
        let _ = self
            .emit(AgentEvent::MessageStart {
                message: fm.clone(),
            })
            .await;
        let _ = self
            .emit(AgentEvent::MessageEnd {
                message: fm.clone(),
            })
            .await;
        let _ = self
            .emit(AgentEvent::TurnEnd {
                message: fm.clone(),
                tool_results: Vec::new(),
            })
            .await;
        let _ = self
            .emit(AgentEvent::AgentEnd {
                messages: vec![Arc::new(fm.clone())],
            })
            .await;
        self.new_messages = vec![Arc::new(fm)];
    }

    fn poll_steering(&self) -> Vec<AgentMessage> {
        lock(&self.steering).drain()
    }

    fn poll_follow_up(&self) -> Vec<AgentMessage> {
        lock(&self.follow_up).drain()
    }

    fn find_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }

    /// Pi `runWithLifecycle` (`packages/agent/src/agent.ts:480-494` @v0.83.0): drive the loop and,
    /// on any thrown value, hand it to `handleRunFailure` (`:489-490`). Every in-loop failure
    /// (`transformContext`/`convertToLlm`, the two post-turn hooks, a throwing listener) reaches
    /// this one catch, which is why they all share [`RunFailure`].
    pub(crate) async fn run(&mut self, entry: RunEntry) -> Vec<AgentMessage> {
        // The only place the flag is set: a property of the entry, so it cannot disagree with it.
        self.skip_initial_steering_poll = entry.skip_initial_steering_poll();
        if let Err(RunFailure(msg)) = self.run_entry(entry).await {
            self.emit_run_failure(msg).await;
        }
        // §6.5: the public return type stays `Vec<AgentMessage>`, so the handles are unwrapped
        // exactly once per run. Off the hot path, and every in-tree caller discards the value.
        self.new_messages.iter().map(|m| (**m).clone()).collect()
    }

    async fn run_entry(&mut self, entry: RunEntry) -> Result<(), RunFailure> {
        self.emit(AgentEvent::AgentStart).await?;
        match entry {
            RunEntry::Prompt {
                messages: prompts, ..
            } => {
                self.emit(AgentEvent::TurnStart).await?;
                for p in prompts {
                    self.emit(AgentEvent::MessageStart { message: p.clone() })
                        .await?;
                    self.emit(AgentEvent::MessageEnd { message: p.clone() })
                        .await?;
                    // Pi appends each prompt to the loop's working copy (`currentContext.messages`,
                    // agent-loop.ts:106/187) — the observable `state.messages` grows separately via
                    // the reducer on the `message_end` above.
                    let p = Arc::new(p);
                    self.messages.push(Arc::clone(&p));
                    self.new_messages.push(p);
                }
                self.run_loop(true).await
            }
            RunEntry::Continue(_) => {
                self.emit(AgentEvent::TurnStart).await?;
                self.run_loop(true).await
            }
        }
    }
}
