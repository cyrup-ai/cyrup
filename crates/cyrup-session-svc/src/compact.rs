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
use cyrup_provider::{Model, Provider, RetryPolicy};
use cyrup_session::compaction::{
    complete_summarization, CompactionError, SummarizationRequest, Summarizer,
};

/// Production summarizer bound to a `dyn Provider` + the resolved summarization model.
pub(crate) struct DynSummarizer {
    provider: Arc<dyn Provider>,
    model: Model,
    /// The session's `retry` settings, threaded into every summarization call exactly as Pi passes
    /// `settingsManager.getRetrySettings()` (`agent-session.ts:1858,2132,2997`).
    retry: RetryPolicy,
}

impl DynSummarizer {
    pub(crate) fn new(provider: Arc<dyn Provider>, model: Model, retry: RetryPolicy) -> Self {
        Self { provider, model, retry }
    }
}

impl Summarizer for DynSummarizer {
    async fn complete(
        &self,
        req: SummarizationRequest<'_>,
        cancel: CancelToken,
    ) -> Result<AssistantMessage, CompactionError> {
        complete_summarization(&*self.provider, &self.model, req, self.retry, None, cancel).await
    }
}
