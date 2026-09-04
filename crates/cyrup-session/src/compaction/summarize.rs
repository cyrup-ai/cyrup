//! Summary generation (arch-05 §3.6/§6.3, R-05-008/012/013/014). The summarization model call is an
//! injected `Summarizer` seam; the production impl wraps a `cyrup-provider` `Provider`.

use std::sync::Arc;

use cyrup_core::Cost;
use cyrup_core::{
    AssistantMessage, CancelToken, Content, Message, ModelRef, ModelThinkingLevel, StopReason,
    Usage,
};
use cyrup_provider::{
    CacheRetention, Context, Model, Provider, RetryObserver, RetryPolicy, StreamOptions,
    collect_message, retry_assistant_call,
};

use crate::agent_message::{AgentMessage, convert_to_llm};
use crate::compaction::error::CompactionError;
use crate::compaction::files::format_file_operations;
use crate::compaction::prepare::CompactionPreparation;
use crate::compaction::serialize::serialize_conversation;

/// Diagnostic for a summarization whose response never settled ([`StopReason::Pending`]). Used
/// only when the summarizer left no `error_message` of its own — a truncated summarizer stream
/// normally arrives already stamped by `StreamEvent::end_of_stream` with the per-api text Pi
/// throws.
pub const PENDING_SUMMARY: &str = "summarization stream ended without a stop reason";

/// Diagnostic for a summarization that stopped at the model's output token cap
/// ([`StopReason::Length`]). Pi's `getSummarizationFailure` text minus its `${label} failed: `
/// prefix, which [`CompactionError::Summarization`]'s `Display` supplies
/// (`v0.84.4 coding-agent/src/core/compaction/compaction.ts:549-551`).
pub const INCOMPLETE_SUMMARY: &str = "generation hit the token cap and the summary is incomplete";

/// The acceptance gate every summarization response passes through before its text may become a
/// session checkpoint. Pure: a decision over the settled response, no I/O — the four call sites
/// (this module's [`generate_summary`] and [`generate_turn_prefix_summary`], the branch site in
/// `compaction::branch`, and the session service's `/tree` copy of it) share this one function
/// instead of four hand-copied `match`es, which is how they had drifted from pi in the first place.
///
/// Pi v0.84.4 `getSummarizationFailure` (`coding-agent/src/core/compaction/compaction.ts:541-553`):
/// an `error` stop AND a `length` stop are failures — "A length stop contains partial text and
/// must not become a session checkpoint" — followed by the `toolCall` block check every pi call
/// site runs immediately after it (`compaction.ts:715-721`, `:1000-1006`;
/// `branch-summarization.ts:357-363`). Both landed in `97fa14e39` ("reject truncated compaction
/// summaries", #7048), first tagged at v0.84.4; at v0.84.1 every site tested
/// `stopReason === "error"` alone (`compaction.ts:679`, `:961`). The tool-call test is on the
/// content blocks, not the stop reason: a `toolUse` stop with no tool-call block passes, exactly
/// as it does in pi.
///
/// `label` is pi's per-site label — `"Summarization"`, `"Turn prefix summarization"`,
/// `"Branch summarization"` — and names the site in the tool-call refusal.
pub fn check_summarization_response(
    resp: &AssistantMessage,
    label: &str,
) -> Result<(), CompactionError> {
    match resp.stop_reason {
        StopReason::Error => {
            return Err(CompactionError::Summarization(
                resp.error_message.clone().unwrap_or_default(),
            ));
        }
        StopReason::Length => {
            return Err(CompactionError::Summarization(
                INCOMPLETE_SUMMARY.to_string(),
            ));
        }
        StopReason::Aborted => return Err(CompactionError::Aborted),
        // An unsettled response is NOT a summary. A catch-all here would accept a `Pending`
        // message's partial text as a finished summary and compact the transcript against it —
        // silently losing history to a truncated stream. `Deferred` is grouped here for the same
        // reason and NOT with the success arm: a deferred turn is a receipt whose `content` is `[]`
        // (`v0.84.1 ai/src/providers/faux.ts:293-296`), so accepting it would compact the
        // transcript against an EMPTY summary. Pi's own gate special-cases only `"aborted"`,
        // `"error"` and (since v0.84.4) `"length"` and would take the empty text — but it can never
        // reach that state either: compaction never sets `SimpleStreamOptions.deferred`
        // (`v0.84.1 ai/src/types.ts:307`) and every real provider throws for deferred
        // (`v0.84.1 ai/src/models.ts:714,728`). Unreachable on both sides, so this is a strictly
        // safer spelling of the same behaviour.
        StopReason::Pending | StopReason::Deferred => {
            return Err(CompactionError::Summarization(
                resp.error_message
                    .clone()
                    .unwrap_or_else(|| PENDING_SUMMARY.to_string()),
            ));
        }
        StopReason::Stop | StopReason::ToolUse => {}
    }
    if resp
        .content
        .iter()
        .any(|c| matches!(c, Content::ToolCall(_)))
    {
        return Err(CompactionError::Summarization(format!(
            "{label} attempted to call a tool"
        )));
    }
    Ok(())
}

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
pub const UPDATE_SUMMARIZATION_PROMPT: &str =
    "The messages above are NEW conversation messages to \
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
pub const TURN_PREFIX_SUMMARIZATION_PROMPT: &str =
    "This is the PREFIX of a turn that was too large \
to keep. The SUFFIX (recent work) is retained.

Summarize the prefix to provide context for the retained suffix:

## Original Request
[What did the user ask for in this turn?]

## Early Progress
- [Key decisions and work done in the prefix]

## Context for Suffix
- [Information needed to understand the retained recent work]

Be concise. Focus on what's needed to understand the kept suffix.";

/// One summarization call's result: the summary text plus the token spend that produced it — Pi's
/// `{ text, usage }` (`generateSummaryWithUsage` / `generateTurnPrefixSummary`,
/// `compaction.ts:596-616,882-896`). The usage is what lands in `CompactionEntry.usage`, so it must
/// survive all the way from the provider response to the appended entry.
#[derive(Clone, Debug)]
pub struct SummaryOutput {
    pub text: String,
    pub usage: Usage,
}

/// The default compaction's product: the merged summary text and the usage of the LLM call(s) that
/// produced it (`None` only when no call was made at all).
#[derive(Clone, Debug)]
pub struct DefaultCompaction {
    pub summary: String,
    pub usage: Option<Usage>,
}

/// Field-wise sum of two `Usage`s — a 1:1 port of Pi `combineUsage` (`compaction.ts:884-909`).
/// `cacheWrite1h`/`reasoning` stay `None` unless at least one side reports them (Pi spreads the key
/// in conditionally, so an absent value must not materialize as a `0`).
pub fn combine_usage(first: &Usage, second: &Usage) -> Usage {
    Usage {
        input: first.input.saturating_add(second.input),
        output: first.output.saturating_add(second.output),
        cache_read: first.cache_read.saturating_add(second.cache_read),
        cache_write: first.cache_write.saturating_add(second.cache_write),
        cache_write_1h: match (first.cache_write_1h, second.cache_write_1h) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
        },
        reasoning: match (first.reasoning, second.reasoning) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
        },
        total_tokens: first.total_tokens.saturating_add(second.total_tokens),
        cost: Cost {
            input: first.cost.input + second.cost.input,
            output: first.cost.output + second.cost.output,
            cache_read: first.cost.cache_read + second.cost.cache_read,
            cache_write: first.cost.cache_write + second.cost.cache_write,
            total: first.cost.total + second.cost.total,
        },
    }
}

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

/// The shared choke point for EVERY compaction / turn-prefix / branch summarization call — a 1:1
/// port of Pi `completeSummarization` (`compaction.ts:555-581`). Both production `Summarizer`s
/// (this crate's [`ProviderSummarizer`] and the session service's `DynSummarizer`) route through
/// it, so the three request-shaping rules below cannot drift apart:
///
/// 1. **`cache_retention: None`** and **a fresh `session_id`** — "Summaries are standalone
///    requests, so isolate routing and avoid cache writes that cannot be reused"
///    (`compaction.ts:570-575`). Leaving `cache_retention` unset would let the encoder resolve it
///    from `PI_CACHE_RETENTION` (defaulting to `Short`), billing a prompt-cache write on a
///    one-shot request that can never be read back; leaving `session_id` unset would ride the
///    summarization along on the live session's cache-routing affinity.
/// 2. **`reasoning: req.thinking`** — the level the caller already gated through
///    [`summarization_reasoning`] (Pi `createSummarizationOptions`, `compaction.ts:539-553`).
/// 3. **[`retry_assistant_call`]** — so a transient stream drop (`terminated`, socket close)
///    honors the configured [`RetryPolicy`] instead of failing the whole compaction on the first
///    attempt (`compaction.ts:555-560`).
///
/// Cancellation still resolves to [`CompactionError::Aborted`]: the outer race is kept so a
/// cancelled token short-circuits even if the provider has not yet delivered its terminal aborted
/// event, and the token is also handed to the retry loop so its backoff sleep is abortable.
pub async fn complete_summarization(
    provider: &dyn Provider,
    model: &Model,
    req: SummarizationRequest<'_>,
    retry: RetryPolicy,
    callbacks: Option<&dyn RetryObserver>,
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
        cache_retention: Some(CacheRetention::None),
        session_id: Some(crate::ids::gen_session_id()),
        reasoning: req.thinking,
        ..StreamOptions::default()
    };
    let ctx_ref = &ctx;
    let opts_ref = &opts;
    let produce =
        move || async move { collect_message(provider.stream(model, ctx_ref, opts_ref)).await };
    let call = retry_assistant_call(produce, retry, Some(&cancel), callbacks);
    match cancel.run_until_cancelled(call).await {
        Some(msg) => Ok(msg),
        None => Err(CompactionError::Aborted),
    }
}

/// Production `Summarizer`: a thin wrapper over a `cyrup-provider` `Provider` + `Model` (arch-01).
pub struct ProviderSummarizer<P: Provider> {
    provider: Arc<P>,
    model: Model,
    retry: RetryPolicy,
}

impl<P: Provider> ProviderSummarizer<P> {
    /// A summarizer with retries OFF — Pi's `retry?: RetryPolicy` left `undefined`, which returns
    /// the first response unchanged (`retry.ts:159-160`). Production callers must supply the
    /// session's policy via [`Self::with_retry`].
    pub fn new(provider: Arc<P>, model: Model) -> Self {
        Self {
            provider,
            model,
            retry: RetryPolicy::DISABLED,
        }
    }

    /// Bind the session's retry policy, as Pi threads `settingsManager.getRetrySettings()` into
    /// every summarization call (`agent-session.ts:1858,2132,2997`).
    #[must_use]
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }
}

impl<P: Provider> Summarizer for ProviderSummarizer<P> {
    async fn complete(
        &self,
        req: SummarizationRequest<'_>,
        cancel: CancelToken,
    ) -> Result<AssistantMessage, CompactionError> {
        complete_summarization(&*self.provider, &self.model, req, self.retry, None, cancel).await
    }
}

/// Pi `createSummarizationOptions` (`compaction.ts:539-553`): the session thinking level reaches a
/// summarization request only when the model actually supports reasoning AND the level is not
/// `off` — otherwise Pi leaves `options.reasoning` unset, which this port spells `Off`.
pub fn summarization_reasoning(model: &Model, level: ModelThinkingLevel) -> ModelThinkingLevel {
    if model.reasoning && level != ModelThinkingLevel::Off {
        level
    } else {
        ModelThinkingLevel::Off
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
/// `frac` is `(num, den)`: history summaries use `0.8` (Pi `generateSummaryWithUsage`,
/// `compaction.ts:637-640` @v0.83.0); the turn-prefix half uses `0.5` (Pi
/// `generateTurnPrefixSummary`, `compaction.ts:937-940`).
///
/// Pi is verbatim
/// `Math.min(Math.floor(0.8 * reserveTokens), model.maxTokens > 0 ? model.maxTokens : Infinity)`
/// with **no lower bound**: a `reserveTokens` under `den/num` floors to `0` and pi sends
/// `maxTokens: 0`. cyrup previously applied a `.max(1)` floor here, which is a cyrup-original
/// clamp — the one input where the two differ (`reserve_tokens ∈ 1..=1` for the 0.8 fraction,
/// `1` for the 0.5 fraction) is reachable from settings, since `CompactionSettings.reserve_tokens`
/// is a plain deserialized `u32` with no minimum. Parity is the bar, so the floor is gone.
pub(crate) fn compute_max_tokens_frac(reserve: u32, model_max: u32, num: u64, den: u64) -> u32 {
    let from_reserve = (u64::from(reserve) * num / den) as u32;
    if model_max == 0 {
        from_reserve
    } else {
        from_reserve.min(model_max)
    }
}

fn join_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Generate a summary for `msgs` via the model with the structured format, previous summary, and
/// custom instructions (R-05-008/012/014).
///
/// `msgs` are RAW [`AgentMessage`]s; `convert_to_llm` runs here, immediately before
/// `serialize_conversation`, exactly as Pi does
/// (`compaction.ts:650-651`: `const llmMessages = convertToLlm(currentMessages);`). The transcript
/// bytes are therefore identical to rendering earlier — only the extension-visible preparation and
/// the "is there anything to summarize?" test see the raw form.
#[allow(clippy::too_many_arguments)]
pub async fn generate_summary<S: Summarizer>(
    summarizer: &S,
    msgs: &[AgentMessage],
    model: &Model,
    reserve: u32,
    instructions: Option<&str>,
    previous_summary: Option<&str>,
    thinking: ModelThinkingLevel,
    cancel: CancelToken,
) -> Result<SummaryOutput, CompactionError> {
    let mut base = if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT.to_string()
    } else {
        SUMMARIZATION_PROMPT.to_string()
    };
    if let Some(instr) = instructions {
        base.push_str(&format!("\n\nAdditional focus: {instr}"));
    }

    let transcript = serialize_conversation(&convert_to_llm(msgs));
    let mut prompt = format!("<conversation>\n{transcript}\n</conversation>\n\n");
    if let Some(prev) = previous_summary {
        prompt.push_str(&format!(
            "<previous-summary>\n{prev}\n</previous-summary>\n\n"
        ));
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
        thinking: summarization_reasoning(model, thinking),
    };
    let resp = summarizer.complete(req, cancel).await?;
    // Pi `getSummarizationFailure(response, "Summarization")` + the toolCall check
    // (`v0.84.4 compaction.ts:715-721`).
    check_summarization_response(&resp, "Summarization")?;
    Ok(SummaryOutput {
        text: join_text(&resp.content),
        usage: resp.usage,
    })
}

/// Generate the turn-prefix summary for a split turn (R-05-006). `msgs` are RAW
/// [`AgentMessage`]s; `convert_to_llm` runs here, as in Pi (`compaction.ts:941-942`).
pub async fn generate_turn_prefix_summary<S: Summarizer>(
    summarizer: &S,
    msgs: &[AgentMessage],
    model: &Model,
    reserve: u32,
    thinking: ModelThinkingLevel,
    cancel: CancelToken,
) -> Result<SummaryOutput, CompactionError> {
    let transcript = serialize_conversation(&convert_to_llm(msgs));
    let prompt = format!(
        "<conversation>\n{transcript}\n</conversation>\n\n{TURN_PREFIX_SUMMARIZATION_PROMPT}"
    );
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
        thinking: summarization_reasoning(model, thinking),
    };
    let resp = summarizer.complete(req, cancel).await?;
    // Pi `getSummarizationFailure(response, "Turn prefix summarization")` + the toolCall check
    // (`v0.84.4 compaction.ts:1000-1006`).
    check_summarization_response(&resp, "Turn prefix summarization")?;
    Ok(SummaryOutput {
        text: join_text(&resp.content),
        usage: resp.usage,
    })
}

/// The default compaction summary (history + optional turn-prefix), with machine file blocks
/// appended (R-05-006/013).
pub async fn compact_default<S: Summarizer>(
    summarizer: &S,
    prep: &CompactionPreparation,
    model: &Model,
    instructions: Option<&str>,
    thinking: ModelThinkingLevel,
    cancel: CancelToken,
) -> Result<DefaultCompaction, CompactionError> {
    let reserve = prep.settings.reserve_tokens;
    // Usage is threaded out alongside the text so it can be persisted on the compaction entry (Pi
    // `CompactionResult.usage`, `compaction.ts:88-89`). On a split turn BOTH calls are billed and Pi
    // records their sum (`combineUsage(historyUsage, turnPrefixResult.usage)`, `compaction.ts:877`);
    // when the history half is skipped ("No prior history.") only the turn-prefix call is charged.
    let (mut summary, usage) = if prep.is_split_turn && !prep.turn_prefix_messages.is_empty() {
        let history = if prep.messages_to_summarize.is_empty() {
            None
        } else {
            Some(
                generate_summary(
                    summarizer,
                    &prep.messages_to_summarize,
                    model,
                    reserve,
                    instructions,
                    prep.previous_summary.as_deref(),
                    thinking,
                    cancel.clone(),
                )
                .await?,
            )
        };
        let prefix = generate_turn_prefix_summary(
            summarizer,
            &prep.turn_prefix_messages,
            model,
            reserve,
            thinking,
            cancel,
        )
        .await?;
        let history_text = history
            .as_ref()
            .map_or_else(|| "No prior history.".to_string(), |h| h.text.clone());
        let merged = match &history {
            Some(h) => combine_usage(&h.usage, &prefix.usage),
            None => prefix.usage.clone(),
        };
        (
            format!(
                "{history_text}\n\n---\n\n**Turn Context (split turn):**\n\n{}",
                prefix.text
            ),
            Some(merged),
        )
    } else {
        let result = generate_summary(
            summarizer,
            &prep.messages_to_summarize,
            model,
            reserve,
            instructions,
            prep.previous_summary.as_deref(),
            thinking,
            cancel,
        )
        .await?;
        (result.text, Some(result.usage))
    };

    let (read, modified) = prep.file_ops.compute_lists();
    summary.push_str(&format_file_operations(&read, &modified));
    Ok(DefaultCompaction { summary, usage })
}
