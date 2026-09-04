//! A `Summarizer` over a `dyn Provider` (arch-05 seam). `cyrup_session::ProviderSummarizer` is
//! generic over a sized `P: Provider`, so the facade — which holds an `Arc<dyn Provider>` — supplies
//! its own trait-object-friendly summarizer for compaction.
//!
//! Request shaping is NOT duplicated here: both summarizers delegate to
//! `cyrup_session::compaction::complete_summarization`, the single port of Pi
//! `completeSummarization` (`compaction.ts:555-581`), so the cache/routing isolation, the reasoning
//! level and the retry policy cannot drift between the two.

use std::sync::Arc;

use cyrup_core::{AssistantMessage, CancelToken};
use cyrup_provider::{Model, Provider, RetryObserver, RetryPolicy};
use cyrup_session::compaction::{
    CompactionError, SummarizationRequest, Summarizer, complete_summarization,
};
use tokio::sync::mpsc;

use crate::event::{AgentSessionEvent, SummarizationRetrySource};

/// Production summarizer bound to a `dyn Provider` + the resolved summarization model.
pub(crate) struct DynSummarizer {
    provider: Arc<dyn Provider>,
    model: Model,
    /// The session's `retry` settings, threaded into every summarization call exactly as Pi passes
    /// `settingsManager.getRetrySettings()` (`agent-session.ts:1858,2132,2997`).
    retry: RetryPolicy,
    /// The `summarization_retry_*` emitter Pi builds alongside the policy
    /// (`_summarizationRetryCallbacks`, `agent-session.ts:2641-2670`). `None` keeps the retry
    /// silent, which is what an unwired/test summarizer wants.
    observer: Option<Arc<dyn RetryObserver>>,
}

impl DynSummarizer {
    pub(crate) fn new(provider: Arc<dyn Provider>, model: Model, retry: RetryPolicy) -> Self {
        Self {
            provider,
            model,
            retry,
            observer: None,
        }
    }

    /// Attach the `summarization_retry_*` event emitter for this call — Pi passes
    /// `this._summarizationRetryCallbacks({...})` as the argument right after the retry settings
    /// at every one of its three summarization call sites (`agent-session.ts:1858-1859,
    /// 2132-2133, 2996-2997`).
    pub(crate) fn with_observer(mut self, observer: Arc<dyn RetryObserver>) -> Self {
        self.observer = Some(observer);
        self
    }
}

impl Summarizer for DynSummarizer {
    async fn complete(
        &self,
        req: SummarizationRequest<'_>,
        cancel: CancelToken,
    ) -> Result<AssistantMessage, CompactionError> {
        complete_summarization(
            &*self.provider,
            &self.model,
            req,
            self.retry,
            self.observer.as_deref(),
            cancel,
        )
        .await
    }
}

/// A [`RetryObserver`] that turns Pi's three `_summarizationRetryCallbacks` hooks into seam events
/// (`agent-session.ts:2645-2668`).
///
/// Pi's `_emit` is synchronous; cyrup's fan-out is `async` (it awaits per-subscriber backpressure),
/// and [`RetryObserver`] is deliberately synchronous because every Pi implementation is a plain
/// emit. The gap is bridged with an unbounded channel drained by
/// [`crate::AgentSession::spawn_event_pump`] — unbounded so the retry loop is never itself blocked
/// by a slow subscriber, which is safe because a BOUNDED retry emits at most
/// `2 * max_retries + 1` events per summarization call — seven at Pi's default `maxRetries: 3`.
/// An unbounded retry policy would make this queue unbounded too, which is one more reason the
/// attempt bound is load-bearing.
pub(crate) struct RetryEventEmitter {
    tx: mpsc::UnboundedSender<AgentSessionEvent>,
    source: SummarizationRetrySource,
}

impl RetryObserver for RetryEventEmitter {
    fn on_retry_scheduled(
        &self,
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        error_message: &str,
    ) {
        let _ = self
            .tx
            .send(AgentSessionEvent::SummarizationRetryScheduled {
                attempt,
                max_attempts,
                delay_ms,
                error_message: error_message.to_string(),
            });
    }

    fn on_retry_attempt_start(&self) {
        let _ = self
            .tx
            .send(AgentSessionEvent::SummarizationRetryAttemptStart {
                source: self.source,
            });
    }

    /// Pi's `onRetryFinished` receives `(success, attempt, finalError)` and discards all three
    /// (`agent-session.ts:2664-2667`), so the emitted event is payload-free.
    fn on_retry_finished(&self, _success: bool, _attempt: u32, _final_error: Option<&str>) {
        let _ = self.tx.send(AgentSessionEvent::SummarizationRetryFinished);
    }
}

/// Build the emitter + its drain queue for one summarization operation — Pi
/// `_summarizationRetryCallbacks(source)` (`agent-session.ts:2641`). The receiver is drained by
/// `AgentSession::spawn_event_pump`.
pub(crate) fn summarization_retry_channel(
    source: SummarizationRetrySource,
) -> (
    Arc<dyn RetryObserver>,
    mpsc::UnboundedReceiver<AgentSessionEvent>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Arc::new(RetryEventEmitter { tx, source }), rx)
}
