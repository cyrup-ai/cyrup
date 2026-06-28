//! Summary generation (arch-05 §3.6/§6.3, R-05-008/012/013/014). The summarization model call is an
//! injected `Summarizer` seam; the production impl wraps a `cyrup-provider` `Provider`.

use std::sync::Arc;

use cyrup_core::{
    AssistantMessage, CancelToken, Content, Message, ModelRef, StopReason, ThinkingLevel,
};
use cyrup_provider::{collect_message, Context, Model, Provider, StreamOptions};

use crate::compaction::error::CompactionError;
use crate::compaction::files::format_file_operations;
use crate::compaction::prepare::CompactionPreparation;
use crate::compaction::serialize::serialize_conversation;

/// System prompt steering the model to summarize rather than continue (R-05-012).
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a summarization assistant. You are given a \
transcript of a coding conversation and must produce a concise, structured summary that preserves \
all information needed to continue the work. Do not continue the conversation; only summarize it.";

/// Required-section structure (R-05-013).
const FORMAT_INSTRUCTIONS: &str = "Produce a structured markdown summary with EXACTLY these \
sections:\n\n## Goal\n## Constraints & Preferences\n## Progress\n### Done\n### In Progress\n### \
Blocked\n## Key Decisions\n## Next Steps\n## Critical Context\n\nBe specific and preserve file \
paths, identifiers, and decisions with their rationale.";

/// Initial summarization prompt (R-05-008).
pub const SUMMARIZATION_PROMPT: &str = FORMAT_INSTRUCTIONS;

/// Iterative-update prompt when a previous summary exists (R-05-008/012).
pub const UPDATE_SUMMARIZATION_PROMPT: &str = "Update the previous summary to incorporate the new \
conversation below, keeping the same section structure.\n\nProduce a structured markdown summary \
with EXACTLY these sections:\n\n## Goal\n## Constraints & Preferences\n## Progress\n### Done\n### \
In Progress\n### Blocked\n## Key Decisions\n## Next Steps\n## Critical Context";

/// Prompt for the turn-prefix half of a split-turn compaction (R-05-006).
pub const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = "Summarize the following partial turn concisely, \
capturing its goal, what was attempted, and any important context for continuing it.";

/// A single non-streaming summarization request (arch-05 §3.6).
pub struct SummarizationRequest<'a> {
    pub system_prompt: &'a str,
    pub prompt_text: String,
    pub max_tokens: u32,
    pub model: ModelRef,
    pub thinking: ThinkingLevel,
}

/// The summarization seam: a single completion used for summaries (R-05-008).
#[allow(async_fn_in_trait)]
pub trait Summarizer: Send + Sync {
    /// Resolve to the final `AssistantMessage`. Transport failure arrives as a `stop_reason`
    /// Error/Aborted message, not as `Err`.
    async fn complete(
        &self,
        req: SummarizationRequest<'_>,
        cancel: CancelToken,
    ) -> Result<AssistantMessage, CompactionError>;
}

/// Production `Summarizer`: a thin wrapper over a `cyrup-provider` `Provider` + `Model` (arch-01).
pub struct ProviderSummarizer<P: Provider> {
    provider: Arc<P>,
    model: Model,
}

impl<P: Provider> ProviderSummarizer<P> {
    pub fn new(provider: Arc<P>, model: Model) -> Self {
        Self { provider, model }
    }
}

impl<P: Provider> Summarizer for ProviderSummarizer<P> {
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

fn model_ref(model: &Model) -> ModelRef {
    ModelRef {
        provider: model.provider.clone(),
        api: Some(model.api.clone()),
        model: model.id.clone(),
    }
}

/// `min(floor(0.8*reserve), model.max_tokens)` (treating a zero `max_tokens` as unbounded).
fn compute_max_tokens(reserve: u32, model_max: u32) -> u32 {
    let from_reserve = (u64::from(reserve) * 4 / 5) as u32;
    if model_max == 0 {
        from_reserve.max(1)
    } else {
        from_reserve.min(model_max).max(1)
    }
}

fn join_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Generate a summary for `msgs` via the model with the structured format, previous summary, and
/// custom instructions (R-05-008/012/014).
pub async fn generate_summary<S: Summarizer>(
    summarizer: &S,
    msgs: &[Message],
    model: &Model,
    reserve: u32,
    instructions: Option<&str>,
    previous_summary: Option<&str>,
    cancel: CancelToken,
) -> Result<String, CompactionError> {
    let mut base = if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT.to_string()
    } else {
        SUMMARIZATION_PROMPT.to_string()
    };
    if let Some(instr) = instructions {
        base.push_str(&format!("\n\nAdditional focus: {instr}"));
    }

    let transcript = serialize_conversation(msgs);
    let mut prompt = format!("<conversation>\n{transcript}\n</conversation>\n\n");
    if let Some(prev) = previous_summary {
        prompt.push_str(&format!("<previous-summary>\n{prev}\n</previous-summary>\n\n"));
    }
    prompt.push_str(&base);

    let req = SummarizationRequest {
        system_prompt: SUMMARIZATION_SYSTEM_PROMPT,
        prompt_text: prompt,
        max_tokens: compute_max_tokens(reserve, u32::try_from(model.max_tokens).unwrap_or(u32::MAX)),
        model: model_ref(model),
        thinking: ThinkingLevel::Off,
    };
    let resp = summarizer.complete(req, cancel).await?;
    match resp.stop_reason {
        StopReason::Error => {
            Err(CompactionError::Summarization(resp.error_message.unwrap_or_default()))
        }
        StopReason::Aborted => Err(CompactionError::Aborted),
        _ => Ok(join_text(&resp.content)),
    }
}

/// Generate the turn-prefix summary for a split turn (R-05-006).
pub async fn generate_turn_prefix_summary<S: Summarizer>(
    summarizer: &S,
    msgs: &[Message],
    model: &Model,
    reserve: u32,
    cancel: CancelToken,
) -> Result<String, CompactionError> {
    let transcript = serialize_conversation(msgs);
    let prompt =
        format!("<conversation>\n{transcript}\n</conversation>\n\n{TURN_PREFIX_SUMMARIZATION_PROMPT}");
    let req = SummarizationRequest {
        system_prompt: SUMMARIZATION_SYSTEM_PROMPT,
        prompt_text: prompt,
        max_tokens: compute_max_tokens(reserve, u32::try_from(model.max_tokens).unwrap_or(u32::MAX)),
        model: model_ref(model),
        thinking: ThinkingLevel::Off,
    };
    let resp = summarizer.complete(req, cancel).await?;
    match resp.stop_reason {
        StopReason::Error => {
            Err(CompactionError::Summarization(resp.error_message.unwrap_or_default()))
        }
        StopReason::Aborted => Err(CompactionError::Aborted),
        _ => Ok(join_text(&resp.content)),
    }
}

/// The default compaction summary (history + optional turn-prefix), with machine file blocks
/// appended (R-05-006/013).
pub async fn compact_default<S: Summarizer>(
    summarizer: &S,
    prep: &CompactionPreparation,
    model: &Model,
    instructions: Option<&str>,
    cancel: CancelToken,
) -> Result<String, CompactionError> {
    let reserve = prep.settings.reserve_tokens;
    let mut summary = if prep.is_split_turn && !prep.turn_prefix_messages.is_empty() {
        let history = if prep.messages_to_summarize.is_empty() {
            "No prior history.".to_string()
        } else {
            generate_summary(
                summarizer,
                &prep.messages_to_summarize,
                model,
                reserve,
                instructions,
                prep.previous_summary.as_deref(),
                cancel.clone(),
            )
            .await?
        };
        let prefix = generate_turn_prefix_summary(
            summarizer,
            &prep.turn_prefix_messages,
            model,
            reserve,
            cancel,
        )
        .await?;
        format!("{history}\n\n---\n\n**Turn Context (split turn):**\n\n{prefix}")
    } else {
        generate_summary(
            summarizer,
            &prep.messages_to_summarize,
            model,
            reserve,
            instructions,
            prep.previous_summary.as_deref(),
            cancel,
        )
        .await?
    };

    let (read, modified) = prep.file_ops.compute_lists();
    summary.push_str(&format_file_operations(&read, &modified));
    Ok(summary)
}
