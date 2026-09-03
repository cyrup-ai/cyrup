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
    /// The terminal has been yielded. `owes_start` remembers whether a `Start` ever was — a
    /// terminal with no prior `start` (pi's `!addedPartial` branch) still owes `message_start`.
    Terminated { owes_start: bool },
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
        if matches!(self.phase, Phase::Terminated { .. }) {
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
                Phase::Started | Phase::Terminated { .. } => Step::Ignore,
            },
            StreamEvent::Done { message, .. } => {
                self.phase = Phase::Terminated { owes_start: self.owes_start() };
                Step::Terminal(message)
            }
            StreamEvent::Error { error, .. } => {
                self.phase = Phase::Terminated { owes_start: self.owes_start() };
                Step::Terminal(error)
            }
            // Every other event is a content-block start/delta/end (text, thinking, OR
            // tool-call): re-emit the refreshed partial on `message_update` once the partial
            // exists (Pi emits `message_update` for all nine block events after `start`,
            // agent-loop.ts:326-344).
            event => match self.phase {
                Phase::Started => Step::Update { partial: Arc::clone(&self.partial), event },
                Phase::Unstarted | Phase::Terminated { .. } => Step::Ignore,
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
        matches!(self.phase, Phase::Unstarted | Phase::Terminated { owes_start: true })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use cyrup_core::{Content, StopReason};
    use cyrup_provider::faux::{faux_assistant_message, faux_text};

    fn model() -> ModelRef {
        ModelRef { provider: "faux".into(), api: Some("faux".into()), model: "faux-1".into() }
    }

    fn partial(text: &str) -> Arc<AssistantMessage> {
        Arc::new(faux_assistant_message(vec![faux_text(text)], StopReason::Pending))
    }

    fn start(p: &Arc<AssistantMessage>) -> StreamEvent {
        StreamEvent::Start { partial: Arc::clone(p) }
    }

    fn delta(p: &Arc<AssistantMessage>) -> StreamEvent {
        StreamEvent::TextDelta { content_index: 0, delta: "x".into(), partial: Arc::clone(p) }
    }

    fn done(text: &str) -> StreamEvent {
        StreamEvent::terminal(faux_assistant_message(vec![faux_text(text)], StopReason::Stop))
    }

    fn texts(m: &AssistantMessage) -> Vec<String> {
        m.content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect()
    }

    /// The conforming sequence: `Start` yields the one `Start` step, every block event after it
    /// yields an `Update` carrying that event's own partial, the terminal ends consumption, and a
    /// started stream owes no `message_start` at settle time.
    #[test]
    fn start_updates_terminal_owes_no_start() {
        let mut acc = AssistantStream::new(&model());
        let p0 = partial("");
        let p1 = partial("he");
        assert!(matches!(acc.on_event(start(&p0)), Step::Start(p) if Arc::ptr_eq(&p, &p0)));
        match acc.on_event(delta(&p1)) {
            Step::Update { partial, event } => {
                assert!(Arc::ptr_eq(&partial, &p1));
                assert!(matches!(event, StreamEvent::TextDelta { .. }));
            }
            _ => panic!("a post-start block event must be an Update"),
        }
        let terminal = match acc.on_event(done("hello")) {
            Step::Terminal(t) => t,
            _ => panic!("done must be Terminal"),
        };
        let settled = acc.settle(Arc::clone(&terminal));
        assert!(settled.start.is_none(), "a started stream owes no message_start");
        assert!(Arc::ptr_eq(&settled.end, &terminal), "the provider's own Arc passes through");
    }

    /// A terminal with no prior `Start` (pi's `!addedPartial` branch) owes a `message_start`, and
    /// its payload is the SETTLED message — never the `Pending` seed.
    #[test]
    fn unstarted_terminal_owes_start_with_the_settled_message() {
        let mut acc = AssistantStream::new(&model());
        let terminal = match acc.on_event(done("only")) {
            Step::Terminal(t) => t,
            _ => panic!("done must be Terminal"),
        };
        let settled = acc.settle(terminal);
        let first = settled.start.expect("unstarted stream owes a message_start");
        assert!(Arc::ptr_eq(&first, &settled.end));
        assert_eq!(settled.end.stop_reason, StopReason::Stop);
    }

    /// A block event BEFORE `Start` emits nothing but still refreshes the partial, so an abort
    /// that follows carries the content streamed so far (pi returns the stream's own `result()`).
    #[test]
    fn pre_start_block_event_is_ignored_but_an_abort_keeps_its_content() {
        let mut acc = AssistantStream::new(&model());
        assert!(matches!(acc.on_event(delta(&partial("hello"))), Step::Ignore));
        let settled = acc.settle_aborted();
        assert_eq!(settled.end.stop_reason, StopReason::Aborted);
        assert_eq!(settled.end.error_message.as_deref(), Some("Request was aborted"));
        assert_eq!(texts(&settled.end), vec!["hello".to_string()]);
        let first = settled.start.expect("unstarted abort owes a message_start");
        assert!(Arc::ptr_eq(&first, &settled.end), "the start payload is the settled message");
    }

    /// After `Start`, an abort owes no `message_start` and stamps only the terminal reason.
    #[test]
    fn started_abort_owes_no_start() {
        let mut acc = AssistantStream::new(&model());
        let _ = acc.on_event(start(&partial("partial text")));
        let settled = acc.settle_aborted();
        assert!(settled.start.is_none());
        assert_eq!(settled.end.stop_reason, StopReason::Aborted);
        assert_eq!(texts(&settled.end), vec!["partial text".to_string()]);
    }

    /// A stream that ends without a terminal synthesises pi's error terminal, addressed to the
    /// run's model, and owes a `message_start` iff it never started.
    #[test]
    fn eof_synthesises_the_error_terminal() {
        let m = model();
        let acc = AssistantStream::new(&m);
        let settled = acc.settle_eof();
        assert_eq!(settled.end.stop_reason, StopReason::Error);
        assert_eq!(
            settled.end.error_message.as_deref(),
            Some("stream ended without a terminal event")
        );
        assert_eq!(settled.end.provider, m.provider);
        assert_eq!(settled.end.model, m.model.to_string());
        assert!(settled.start.is_some());

        let mut started = AssistantStream::new(&m);
        let _ = started.on_event(start(&partial("")));
        assert!(started.settle_eof().start.is_none());
    }

    /// Exactly once: a second `Start` from a non-conforming provider emits nothing.
    #[test]
    fn second_start_is_ignored() {
        let mut acc = AssistantStream::new(&model());
        assert!(matches!(acc.on_event(start(&partial(""))), Step::Start(_)));
        assert!(matches!(acc.on_event(start(&partial("again"))), Step::Ignore));
    }

    /// pi returns on the terminal: whatever a provider sends afterwards can neither emit a stray
    /// `message_update` nor overwrite the partial an abort would settle with.
    #[test]
    fn post_terminal_events_are_inert() {
        let mut acc = AssistantStream::new(&model());
        let _ = acc.on_event(start(&partial("first")));
        assert!(matches!(acc.on_event(done("final")), Step::Terminal(_)));
        assert!(matches!(acc.on_event(delta(&partial("LEAK"))), Step::Ignore));
        assert!(matches!(acc.on_event(start(&partial("LEAK"))), Step::Ignore));
        let settled = acc.settle_aborted();
        assert_eq!(texts(&settled.end), vec!["first".to_string()], "the stray never landed");
    }
}
