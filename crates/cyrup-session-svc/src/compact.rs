//! A `Summarizer` over a `dyn Provider` (arch-05 seam). `cyrup_session::ProviderSummarizer` is
//! generic over a sized `P: Provider`, so the facade — which holds an `Arc<dyn Provider>` — supplies
//! its own trait-object-friendly summarizer for compaction.

use std::sync::Arc;

use cyrup_core::{AssistantMessage, CancelToken, Content, Message};
use cyrup_provider::{collect_message, Context, Model, Provider, StreamOptions};
use cyrup_session::compaction::{CompactionError, SummarizationRequest, Summarizer};

/// Production summarizer bound to a `dyn Provider` + the resolved summarization model.
pub(crate) struct DynSummarizer {
    provider: Arc<dyn Provider>,
    model: Model,
}

impl DynSummarizer {
    pub(crate) fn new(provider: Arc<dyn Provider>, model: Model) -> Self {
        Self { provider, model }
    }
}

impl Summarizer for DynSummarizer {
    async fn complete(
        &self,
        req: SummarizationRequest<'_>,
        cancel: CancelToken,
    ) -> Result<AssistantMessage, CompactionError> {
        let ctx = Context {
            system_prompt: Some(req.system_prompt.to_string()),
            messages: vec![Message::User {
                content: vec![Content::text(req.prompt_text)],
                timestamp: 0,
            }],
            tools: Vec::new(),
        };
        let opts = StreamOptions {
            cancel: Some(cancel.clone()),
            max_tokens: Some(u64::from(req.max_tokens)),
            ..StreamOptions::default()
        };
        let stream = self.provider.stream(&self.model, &ctx, &opts);
        match cancel.run_until_cancelled(collect_message(stream)).await {
            Some(msg) => Ok(msg),
            None => Err(CompactionError::Aborted),
        }
    }
}
