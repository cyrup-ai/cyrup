//! Response decoding — the accumulator the SSE loop threads through (Pi's `currentBlock` +
//! `output` pair, google-generative-ai.ts:89-100).

use crate::model::Model;
use crate::usage::compute_cost;
use crate::utils::provider_plumbing::now_millis;
use cyrup_core::{ApiId, AssistantMessage, Content, StopReason, Usage};

/// The in-progress text/thinking block being accumulated (Pi `currentBlock`,
/// google-generative-ai.ts:89).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CurrentKind {
    Text,
    Thinking,
}

/// Streaming-decode state (mirrors Pi's `output` accumulation, google-generative-ai.ts:57-264).
#[derive(Default)]
pub(super) struct Decoder {
    pub(super) blocks: Vec<Content>,
    pub(super) current: Option<CurrentKind>,
    pub(super) usage: Usage,
    pub(super) response_id: Option<String>,
    /// The settled stop reason, or `None` while none has been delivered — cyrup's spelling of Pi's
    /// `output.stopReason = "pending"` seed (google-generative-ai.ts:73), which is where the
    /// `Default` below now starts. Gemini only sets this from a candidate's `finishReason`, so
    /// `None` at EOF means the stream was TRUNCATED. It previously seeded `Stop` (on a misreading of
    /// upstream, which seeds `"pending"`, not `"stop"`), which is what let a truncated Gemini stream
    /// be transcribed as a cleanly completed turn (PROV-010).
    pub(super) stop_reason: Option<StopReason>,
    /// The candidate's own `finishReason`, kept verbatim beside the narrowed [`StopReason`] (pi
    /// `output.rawStopReason = candidate.finishReason`,
    /// `v0.84.1 ai/src/api/google-generative-ai.ts:216`). PORT BUG, not version lag: the write is
    /// present at v0.83.0 too (`v0.83.0 ai/src/api/google-generative-ai.ts:215`) and cyrup never
    /// ported it.
    ///
    /// For Gemini this field is doubly load-bearing: pi READS it back to compose the terminal error
    /// (``output.rawStopReason ? `Provider stopped with: ${output.rawStopReason}` : "An unknown
    /// error occurred"``, `v0.84.1 ai/src/api/google-generative-ai.ts:271-273`). cyrup reaches the
    /// same text from the other end — [`map_stop_reason`](super::stop_reason::map_stop_reason) bakes it in at map time — so the visible
    /// message was NOT degraded; only the recorded field was missing. It is NOT cleared by the
    /// tool-call override below: pi leaves `rawStopReason` set when it rewrites `stopReason` to
    /// `"toolUse"` (`:218-220`).
    pub(super) raw_stop_reason: Option<String>,
    pub(super) error_message: Option<String>,
}

impl Decoder {
    /// Build the live `partial` snapshot. Usage cost is computed without overwriting the
    /// API-reported `total_tokens` (Pi `calculateCost` fills only `usage.cost`,
    /// google-generative-ai.ts:234).
    pub(super) fn snapshot(&self, model: &Model, api: &ApiId) -> AssistantMessage {
        let mut usage = self.usage.clone();
        usage.cost = compute_cost(&model.cost, &usage);
        AssistantMessage {
            content: self.blocks.clone(),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: api.clone(),
            response_model: None,
            response_id: self.response_id.clone(),
            diagnostics: None,
            usage,
            // Pi's live `partial` carries the raw `output.stopReason`, i.e. `"pending"` until a
            // `finishReason` lands (google-generative-ai.ts:73,229). The TERMINAL event never takes
            // this value — it goes through `StreamEvent::end_of_stream`, which routes
            // `None`/`Pending` to the `error` terminal.
            stop_reason: self.stop_reason.unwrap_or(StopReason::Pending),
            deferred: None,
            error_message: self.error_message.clone(),
            raw_stop_reason: self.raw_stop_reason.clone(),
            timestamp: now_millis(),
        }
    }

    pub(super) fn block_index(&self) -> usize {
        self.blocks.len().saturating_sub(1)
    }
}
