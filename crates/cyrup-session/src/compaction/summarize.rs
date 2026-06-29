//! Summary generation (arch-05 §3.6/§6.3, R-05-008/012/013/014). The summarization model call is an
//! injected `Summarizer` seam; the production impl wraps a `cyrup-provider` `Provider`.

use std::sync::Arc;

use cyrup_core::{
    AssistantMessage, CancelToken, Content, Message, ModelRef, StopReason, ModelThinkingLevel,
};
use cyrup_provider::{collect_message, Context, Model, Provider, StreamOptions};

use crate::compaction::error::CompactionError;
use crate::compaction::files::format_file_operations;
use crate::compaction::prepare::CompactionPreparation;
use crate::compaction::serialize::serialize_conversation;

/// System prompt steering the model to summarize rather than continue (R-05-012). Byte-1:1 with Pi
/// `SUMMARIZATION_SYSTEM_PROMPT` (`utils.ts:168-170`).
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task \
is to read a conversation between a user and an AI assistant, then produce a structured summary \
following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any \
questions in the conversation. ONLY output the structured summary.";

/// Initial summarization prompt (R-05-008). Byte-1:1 with Pi `SUMMARIZATION_PROMPT`
/// (`compaction.ts:460-491`).
pub const SUMMARIZATION_PROMPT: &str = "The messages above are a conversation to summarize. Create a \
structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or \"(none)\" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or \"(none)\" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

/// Iterative-update prompt when a previous summary exists (R-05-008/012). Byte-1:1 with Pi
/// `UPDATE_SUMMARIZATION_PROMPT` (`compaction.ts:493-530`).
pub const UPDATE_SUMMARIZATION_PROMPT: &str = "The messages above are NEW conversation messages to \
incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from \"In Progress\" to \"Done\" when completed
- UPDATE \"Next Steps\" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it

Use this EXACT format:

## Goal
[Preserve existing goals, add new ones if the task expanded]

## Constraints & Preferences
- [Preserve existing, add new ones discovered]

## Progress
### Done
- [x] [Include previously done items AND newly completed items]

### In Progress
- [ ] [Current work - update based on progress]

### Blocked
- [Current blockers - remove if resolved]

## Key Decisions
- **[Decision]**: [Brief rationale] (preserve all previous, add new)

## Next Steps
1. [Update based on current state]

## Critical Context
- [Preserve important context, add new if needed]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

/// Prompt for the turn-prefix half of a split-turn compaction (R-05-006). Byte-1:1 with Pi
/// `TURN_PREFIX_SUMMARIZATION_PROMPT` (`compaction.ts:737-750`).
pub const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = "This is the PREFIX of a turn that was too large \
to keep. The SUFFIX (recent work) is retained.

Summarize the prefix to provide context for the retained suffix:

## Original Request
[What did the user ask for in this turn?]

## Early Progress
- [Key decisions and work done in the prefix]

## Context for Suffix
- [Information needed to understand the retained recent work]

Be concise. Focus on what's needed to understand the kept suffix.";

/// A single non-streaming summarization request (arch-05 §3.6).
pub struct SummarizationRequest<'a> {
    pub system_prompt: &'a str,
    pub prompt_text: String,
    pub max_tokens: u32,
    pub model: ModelRef,
    pub thinking: ModelThinkingLevel,
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

/// `min(floor(frac*reserve), model.max_tokens)` (treating a zero `max_tokens` as unbounded).
/// `frac` is `(num, den)`: history summaries use `0.8` (Pi `compaction.ts:578-581`); the turn-prefix
/// half uses `0.5` (Pi `compaction.ts:863-866`).
fn compute_max_tokens_frac(reserve: u32, model_max: u32, num: u64, den: u64) -> u32 {
    let from_reserve = (u64::from(reserve) * num / den) as u32;
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
        max_tokens: compute_max_tokens_frac(
            reserve,
            u32::try_from(model.max_tokens).unwrap_or(u32::MAX),
            4,
            5,
        ),
        model: model_ref(model),
        thinking: ModelThinkingLevel::Off,
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
        max_tokens: compute_max_tokens_frac(
            reserve,
            u32::try_from(model.max_tokens).unwrap_or(u32::MAX),
            1,
            2,
        ),
        model: model_ref(model),
        thinking: ModelThinkingLevel::Off,
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
