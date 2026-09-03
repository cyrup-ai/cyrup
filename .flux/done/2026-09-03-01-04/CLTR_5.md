---
stage: qa
status: completed
updated: 2026-09-03 23:20
aug_against: branch 6c927ad (CLTR_1..4 landed) — uncapped sweep of stream_assistant/started/empty_assistant/errored_assistant/the two terminal strings across all 23 crates incl. tests; pi agent-loop.ts:281-372 read for the message_start rule
---

# CLTR_5 — `AssistantStream`: `stream_assistant` as a functional core (F4)

OBJECTIVE: Extract the "`MessageStart` exactly once, before `MessageEnd`, on every exit path"
invariant — today a `let mut started = false` checked by hand at two exits and set at a third —
into a pure, private accumulator whose consuming `settle_*` decides start-once in one place. The
shell (`stream_assistant`) keeps hooks, request assembly, the `select!`, and **one** emission
tail. Internal to `cyrup-agent/src/agent/run/`; zero tendrils. Source: research §3 F4
([`.flux/research/CORE_LOOP_TYPE_REVIEW.md`](../research/CORE_LOOP_TYPE_REVIEW.md)).

## Aug findings that change the plan (read first)

**Sweep — the shell has exactly one caller and nothing outside `run/` names its internals.**
`tmp/cltr5_sweep.txt` (829 hits, nearly all unrelated `started` locals elsewhere): `stream_assistant`
is called only at [`run/turn.rs:45`](../../crates/cyrup-agent/src/agent/run/turn.rs) —
`let asst = Arc::new(self.stream_assistant().await?);` — plus two doc mentions (`state.rs:46`,
`loop_fn.rs:137`, comments only). `empty_assistant`/`errored_assistant` are `pub(super)` in
[`agent/message.rs:18-31`](../../crates/cyrup-agent/src/agent/message.rs) (visible to the whole
`agent` tree). The two synthesised strings are pinned by tests that read the EMITTED message
(`cyrup-agent/src/tests/untracked_misses.rs:303-307`, `cyrup-tui/src/tests/stop_reason.rs:136-139`)
— they must stay byte-identical, and they do because they move, not change.

**Finding 1 — the shell's caller wants an `Arc`; the accumulator can hand it one for free.**
`turn.rs:45` immediately wraps the returned `AssistantMessage` in `Arc::new`, and the shell today
deep-clones the terminal up to three times (`stream.rs:206/210` out of the event, `:242` and
`:247` into fresh `Arc`s). Every terminal the provider delivers is already an
`Arc<AssistantMessage>` (`StreamEvent::Done { message: Arc<_> }` / `Error { error: Arc<_> }`,
[`cyrup-provider/src/stream.rs:563-575`](../../crates/cyrup-provider/src/stream.rs)). So
`Step::Terminal` carries that `Arc` through unchanged, `Settled.end` is an `Arc`, and
`stream_assistant` returns `Result<Arc<AssistantMessage>, RunFailure>`; `turn.rs:45` drops its
`Arc::new`. Zero clones of the terminal on the happy path. One-line caller change inside `run/`.

**Finding 2 — the abort-before-start `message_start` payload is a cyrup delta from pi; the
accumulator fixes it by construction.** Today the three unstarted exits disagree about what
`MessageStart` carries: abort emits the un-stamped `Pending` partial (`stream.rs:158-163`) while
`Done`/`Error`/EOF emit the settled terminal (`:240-244`). pi has no partial before `start`
(`partialMessage = null`, [`agent-loop.ts:314-315`](../../tmp/pi/packages/agent/src/agent-loop.ts))
and on EVERY unstarted exit emits `message_start` with the FINAL message (`:354-355` on the
`done`/`error` terminal, `:367-368` after the loop). `Settled` therefore always carries the
settled message as the start payload when one is owed — one rule, decided in `settle_*`. No test
pins the old abort payload (`agent_loop.rs::a_02_7_abort_closing_sequence_and_idle_settlement`
checks only `MessageEnd`/`TurnEnd` stop reasons; `untracked_misses.rs:288-309` checks the
aborted message's content and `error_message`; `pending_containment.rs` asserts `Pending` on the
STARTED path's `message_start` only).

**Finding 3 — `Step::Update` can own the event.** Today the shell matches on `&e` and then
`Box::new(e.clone())`s the whole `StreamEvent` (with its `Arc` partial) into `MessageUpdate`
(`stream.rs:221`). `on_event(&mut self, ev: StreamEvent)` takes it by value and returns it inside
`Step::Update { partial, event }` — no clone. Terminal events likewise move their `Arc` out.

**Finding 4 — the accumulator owns the `ModelRef`, so `settle_eof` needs no argument.** The shell
already clones `model` once per turn (`stream.rs:36`); `AssistantStream::new(&model)` stores a
clone and both `empty_assistant` (seed) and `errored_assistant` (EOF) read it. A pure sequence
test then needs nothing but events.

**Finding 5 — three phases, not two.** `on_event` after a terminal must return `Ignore` (pi
returns on `done`/`error`, `agent-loop.ts:358`; cyrup's `untracked_misses.rs:343`
`miss6_no_message_update_after_terminal` pins that no stray `message_update` and no overwrite of
the final message occurs). The shell breaks on `Terminal` so it never calls `on_event` afterwards,
but the guarantee belongs to the accumulator: `Phase::{Unstarted, Started, Terminated}`, and in
`Terminated` the partial is NOT refreshed. A second `Start` after the first is also `Ignore`
(the invariant is exactly-once; a conforming provider sends one `Start`, `stream.rs:502-504`).

## SUBTASK1 — the accumulator: `agent/run/assistant_stream.rs`

Create [`crates/cyrup-agent/src/agent/run/assistant_stream.rs`](../../crates/cyrup-agent/src/agent/run/assistant_stream.rs)
and register it in [`run/mod.rs`](../../crates/cyrup-agent/src/agent/run/mod.rs) as
`mod assistant_stream;` (first in the list, alphabetical, before `mod stream;`).

```rust
//! The assistant-stream accumulator — the functional core of `stream_assistant`. No tokio, no
//! emit, no hooks: it turns each provider [`StreamEvent`] into the one [`Step`] the shell must
//! take, and its consuming `settle_*` constructors decide, in ONE place, whether the shell still
//! owes a `message_start` — the invariant that used to live in a `started` flag checked by hand
//! at three exits.

use crate::agent::message::{empty_assistant, errored_assistant};
use cyrup_core::{AssistantMessage, ModelRef, StopReason};
use cyrup_provider::StreamEvent;
use std::sync::Arc;

/// Where the stream is: pi's `partialMessage === null` / non-null (`agent-loop.ts:314-315`), plus
/// the returned-on-terminal state (`:358`) that makes post-terminal strays inert.
enum Phase {
    Unstarted,
    Started,
    Terminated,
}

/// What the shell must do with one event.
pub(super) enum Step {
    /// Emit `MessageStart(partial)` — yielded at most once.
    Start(Arc<AssistantMessage>),
    /// Emit `MessageUpdate { partial, event }` — only after `Start` (pi `if (partialMessage)`,
    /// `agent-loop.ts:335`).
    Update { partial: Arc<AssistantMessage>, event: StreamEvent },
    /// Stop consuming; hand this to [`AssistantStream::settle`].
    Terminal(Arc<AssistantMessage>),
    /// A pre-start block event, a second `Start`, or a post-terminal stray: nothing to emit.
    Ignore,
}

/// What the shell must emit to close the message. `start` is `Some` iff no [`Step::Start`] was
/// ever yielded, and then carries the settled message — pi emits `message_start` with the FINAL
/// message on every unstarted exit (`agent-loop.ts:354-355`, `:367-368`).
pub(super) struct Settled {
    pub(super) start: Option<Arc<AssistantMessage>>,
    pub(super) end: Arc<AssistantMessage>,
}

pub(super) struct AssistantStream {
    model: ModelRef,
    /// The structured partial assistant message, kept in lockstep with the provider's per-event
    /// `partial` snapshot (Pi `event.partial`, agent-loop.ts:313-340): distinct text / thinking /
    /// toolCall content blocks (with signatures) and streaming tool-call args — NOT a single
    /// collapsed text block. Held as a SHARED handle: refreshing it from each event, and
    /// re-emitting it on `message_update`, were three deep copies of the whole message per delta
    /// (PERF-001).
    partial: Arc<AssistantMessage>,
    phase: Phase,
}

impl AssistantStream {
    /// Seeds the `StopReason::Pending` partial (`empty_assistant`) before the first `start`.
    pub(super) fn new(model: &ModelRef) -> Self {
        Self { model: model.clone(), partial: Arc::new(empty_assistant(model)), phase: Phase::Unstarted }
    }

    pub(super) fn on_event(&mut self, ev: StreamEvent) -> Step {
        if matches!(self.phase, Phase::Terminated) {
            // Pi RETURNS from `streamAssistantResponse` on the `done`/`error` terminal
            // (agent-loop.ts:358): a (non-conforming) post-terminal event can neither emit a stray
            // `message_update` nor overwrite the final partial.
            return Step::Ignore;
        }
        // Refresh the structured partial from the event's own snapshot for every non-terminal
        // event (Pi assigns `partialMessage = event.partial`), so an abort carries whatever
        // content has been streamed so far.
        if let Some(p) = ev.partial() {
            self.partial = Arc::clone(p);
        }
        match ev {
            StreamEvent::Start { .. } => match self.phase {
                Phase::Unstarted => {
                    self.phase = Phase::Started;
                    Step::Start(Arc::clone(&self.partial))
                }
                // Exactly once: a second `start` from a non-conforming provider refreshes the
                // partial (above) and emits nothing.
                Phase::Started | Phase::Terminated => Step::Ignore,
            },
            StreamEvent::Done { message, .. } => {
                self.phase = Phase::Terminated;
                Step::Terminal(message)
            }
            StreamEvent::Error { error, .. } => {
                self.phase = Phase::Terminated;
                Step::Terminal(error)
            }
            // Every other event is a content-block start/delta/end (text, thinking, OR
            // tool-call): re-emit the refreshed partial on `message_update` once the partial
            // exists (Pi emits `message_update` for all nine block events after `start`,
            // agent-loop.ts:326-344).
            event => match self.phase {
                Phase::Started => Step::Update { partial: Arc::clone(&self.partial), event },
                Phase::Unstarted | Phase::Terminated => Step::Ignore,
            },
        }
    }

    /// The `done`/`error` terminal the provider delivered.
    pub(super) fn settle(self, terminal: Arc<AssistantMessage>) -> Settled {
        let start = self.owes_start().then(|| Arc::clone(&terminal));
        Settled { start, end: terminal }
    }

    /// Cancelled mid-stream. Pi returns the stream's own `result()` terminal on abort
    /// (agent-loop.ts:344), which carries the ACCUMULATED partial content with
    /// `stopReason:"aborted"` — NOT a fresh empty message. Reuse the structured partial and only
    /// stamp the terminal reason, so a subscriber/transcript sees the streamed text/thinking/
    /// tool-call blocks rather than `[]`. The terminal's `errorMessage` is Pi's uniform abort
    /// string `"Request was aborted"` — every provider throws `new Error("Request was aborted")`
    /// on `signal.aborted` and the catch sets `output.errorMessage = error.message`
    /// (anthropic-messages.ts:718,733-734; the faux provider's `createAbortedMessage` uses the
    /// same string, faux.ts:291-297) — NOT the bare `"aborted"`.
    pub(super) fn settle_aborted(self) -> Settled {
        let mut aborted = (*self.partial).clone();
        aborted.stop_reason = StopReason::Aborted;
        aborted.error_message = Some("Request was aborted".to_string());
        let end = Arc::new(aborted);
        let start = self.owes_start().then(|| Arc::clone(&end));
        Settled { start, end }
    }

    /// The stream ended without a `done`/`error` terminal.
    pub(super) fn settle_eof(self) -> Settled {
        let end = Arc::new(errored_assistant(
            self.model.provider.clone(),
            self.model.model.as_str(),
            self.model.api.clone(),
            StopReason::Error,
            "stream ended without a terminal event",
        ));
        let start = self.owes_start().then(|| Arc::clone(&end));
        Settled { start, end }
    }

    fn owes_start(&self) -> bool {
        matches!(self.phase, Phase::Unstarted)
    }
}
```

Notes for exec: `StreamEvent::partial()` returns `Option<&Arc<AssistantMessage>>`
(`cyrup-provider/src/stream.rs:665-679`) and is `None` for both terminals, so the refresh never
sees a terminal. `ModelRef` is `Clone` (the shell already clones it at `stream.rs:36`).
`errored_assistant`'s signature is `(ProviderId, &str, Option<ApiId>, StopReason, impl Into<String>)`
(`message.rs:18-24`) — the call above is today's `stream.rs:232-238` moved verbatim. The two
string literals are wire-visible `error_message` text and must not change by a byte.

## SUBTASK2 — the shell: `stream.rs:141-251`

- Imports: `use super::assistant_stream::{AssistantStream, Step};`; the
  `crate::agent::message::{empty_assistant, errored_assistant}` import goes (both now used only
  by the accumulator); `use cyrup_core::{AssistantMessage, StopReason};` → `use cyrup_core::AssistantMessage;`
  (`StopReason` was only used by the two synthesised terminals). `StreamEvent` stays imported
  only if still referenced — it is not (the shell no longer matches on variants), so drop it from
  `use cyrup_provider::{Context, StreamEvent, StreamOptions};`. `Arc` stays (the `MessageEnd`
  emit clones the handle).
- Signature (`:29`): `pub(super) async fn stream_assistant(&mut self) -> Result<Arc<AssistantMessage>, RunFailure>`.
  The doc comment above it is unchanged.
- Replace `:141-250` (from `let mut stream = …` through `Ok(final_msg)`) with:

```rust
        let mut stream = self.stream_fn.stream(&model, &ctx, &opts);
        let cancel_tok = self.cancel.token();
        let mut acc = AssistantStream::new(&model);

        let settled = 'consume: loop {
            tokio::select! {
                biased;
                _ = cancel_tok.cancelled() => break 'consume acc.settle_aborted(),
                ev = stream.next() => {
                    let Some(e) = ev else { break 'consume acc.settle_eof() };
                    match acc.on_event(e) {
                        Step::Start(partial) => {
                            self.emit(AgentEvent::MessageStart {
                                message: AgentMessage::Assistant(partial),
                            })
                            .await?;
                        }
                        Step::Update { partial, event } => {
                            self.emit(AgentEvent::MessageUpdate {
                                message: AgentMessage::Assistant(partial),
                                assistant_message_event: Box::new(event),
                            })
                            .await?;
                        }
                        Step::Terminal(terminal) => break 'consume acc.settle(terminal),
                        Step::Ignore => {}
                    }
                }
            }
        };

        // The one emission tail. `settled.start` is `Some` iff the stream never yielded a
        // `Start` — the exactly-once decision is the accumulator's, not this function's.
        if let Some(first) = settled.start {
            self.emit(AgentEvent::MessageStart { message: AgentMessage::Assistant(first) }).await?;
        }
        self.emit(AgentEvent::MessageEnd {
            message: AgentMessage::Assistant(Arc::clone(&settled.end)),
        })
        .await?;
        Ok(settled.end)
```

  The comments at today's `:144-150` (PERF-001 partial handle), `:164-173` (abort terminal),
  `:188-189`, `:201-204` (return on terminal) and `:213-216` (nine block events) now live in the
  accumulator's docs above — do not leave copies behind in the shell.
- Value-carrying `break 'consume …` inside a `tokio::select!` arm is the same construct the
  shell already uses (`break 'consume;` at `:207/:211`, `return Ok(aborted)` at `:181`); the
  moves of `acc` in the `break` expressions are accepted because no iteration follows a `break`.

## SUBTASK3 — the caller: `run/turn.rs:45`

`let asst = Arc::new(self.stream_assistant().await?);` → `let asst = self.stream_assistant().await?;`.
The comment above it (`:41-44`, "build the handle ONCE and clone the pointer") remains true and
stays. Nothing else in `turn.rs` changes (`asst` is already used as an `Arc<AssistantMessage>`).

## Definition of done

- `stream.rs` contains no `started` identifier; exactly one `AgentEvent::MessageStart {` and one
  `AgentEvent::MessageEnd {` site; no `MessageUpdate` outside the `Step::Update` arm; no
  `"Request was aborted"` / `"stream ended without a terminal event"` literal (both live in
  `assistant_stream.rs`, byte-identical).
- `assistant_stream.rs` contains no `tokio`, no `emit`, no `hooks`, no `select`; `Settled` and
  the three `settle_*` are the only producers of the start decision; `Phase` has three variants.
- `stream_assistant` returns `Arc<AssistantMessage>`; `turn.rs` has no `Arc::new(self.stream_assistant`.
- No file outside `crates/cyrup-agent/src/agent/run/` changes; no test file changes. The abort /
  EOF / post-terminal tests (`untracked_misses.rs::miss6_no_message_update_after_terminal`,
  `agent_loop.rs::a_02_7_abort_closing_sequence_and_idle_settlement`, `pending_containment.rs`,
  the 18 `model_boundary.rs` tests, `cyrup-tui/src/tests/turn_interleaving.rs` (12)) pass
  unedited.
- `cargo check --workspace --all-targets --features test-fixtures` clean;
  `cargo test --workspace --features test-fixtures --no-fail-fast` all green (8466 baseline);
  `cargo clippy --workspace --all-targets --features test-fixtures` adds no warning (the one
  pre-existing `cyrup-tui` `question_mark` warning is not this task's);
  `cargo doc -p cyrup-agent --no-deps` exits 0 (the intra-doc links `[`StreamEvent`]`,
  `[`Step`]`, `[`Step::Start`]`, `[`AssistantStream::settle`]` resolve in the new module).

## Research notes

Research §3 F4, §2 row 7. The provider seam `self.stream_fn.stream(&model, &ctx, &opts)`
(`stream.rs:141`, boundary B3) is untouched. If `StopReason::Deferred` later needs a settled
terminal, it is one more `Step`/`settle_*` arm here, not another `if !started`. pi anchors:
`streamAssistantResponse` [`agent-loop.ts:281-372`](../../tmp/pi/packages/agent/src/agent-loop.ts)
— `partialMessage`/`addedPartial` at `:314-315`, `start` at `:319-323`, the nine block events at
`:326-344`, the terminal return at `:346-358`, the post-loop EOF tail at `:363-370`.

No tests to be written — another team owns tests. No benchmarks to be written.
