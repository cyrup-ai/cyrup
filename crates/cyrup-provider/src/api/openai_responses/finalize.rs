//! Stream decoding (Pi processResponsesStream, openai-responses-shared.ts:295-531):
//! the terminal `response.completed` / `response.incomplete` (Pi `finalizeResponse`).

use super::blocks::RBlock;
use super::decoder::RDecoder;
use super::pricing::apply_service_tier_pricing;
use crate::model::Model;
use crate::usage::apply_cost;
use cyrup_core::{StopReason, Usage};
use serde_json::Value;

/// Apply a terminal `response.completed`/`response.incomplete` (Pi `finalizeResponse`).
pub(super) fn finalize_response(response: Option<&Value>, dec: &mut RDecoder, model: &Model) {
    dec.saw_terminal = true;
    if let Some(id) = response.and_then(|r| r.get("id")).and_then(Value::as_str) {
        dec.response_id = Some(id.to_string());
    }
    if let Some(usage) = response.and_then(|r| r.get("usage")) {
        let cached = usage
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let input_tokens = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        dec.usage = Usage {
            input: input_tokens.saturating_sub(cached),
            output: usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_read: cached,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: Some(
                usage
                    .pointer("/output_tokens_details/reasoning_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            ),
            total_tokens: usage
                .get("total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cost: Default::default(),
        };
    }
    apply_cost(&model.cost, &mut dec.usage);
    // Service-tier pricing (Pi applyServiceTierPricing): driven by the response's `service_tier`.
    let service_tier = response
        .and_then(|r| r.get("service_tier"))
        .and_then(Value::as_str);
    apply_service_tier_pricing(&mut dec.usage, service_tier, model);

    // Map status to stop reason. For incomplete responses, retain the provider's specific reason so
    // max-output truncation and content filtering stay distinct (v0.84.1
    // `openai-responses-shared.ts:565-573`).
    let status = response
        .and_then(|r| r.get("status"))
        .and_then(Value::as_str);
    let incomplete_reason = response
        .and_then(|r| r.pointer("/incomplete_details/reason"))
        .and_then(Value::as_str);
    // `output.rawStopReason = incompleteReason ? `${status}.${incompleteReason}` : status`
    // (v0.84.1 `openai-responses-shared.ts:570`). The `(None, Some(_))` arm reproduces the JS
    // template literal's `"undefined"` stringification of a missing `status` verbatim; a frame with
    // `incomplete_details.reason` and no `status` is not something the API emits, but guessing a
    // different answer here would be a divergence rather than a cleanup.
    dec.raw_stop_reason = match (status, incomplete_reason) {
        (Some(s), Some(r)) => Some(format!("{s}.{r}")),
        (Some(s), None) => Some(s.to_string()),
        (None, Some(r)) => Some(format!("undefined.{r}")),
        (None, None) => None,
    };
    let (stop_reason, error_message) = map_stop_reason(status, incomplete_reason);
    dec.stop_reason = stop_reason;
    dec.error_message = error_message;
    let has_tool = dec.blocks.iter().any(|b| matches!(b, RBlock::Tool { .. }));
    if has_tool && dec.stop_reason == StopReason::Stop {
        dec.stop_reason = StopReason::ToolUse;
    }
}

/// Pi `mapStopReason` (v0.84.1 `openai-responses-shared.ts:742-772`) — returns the stop reason
/// *and* the companion `errorMessage`, which is what Pi assigns at `:572-573`.
///
/// The `incomplete` arm is the load-bearing change against the ported v0.83.0 baseline
/// (`v0.83.0 openai-responses-shared.ts:737-756`, `case "incomplete": return "length";`): only
/// `incomplete_details.reason === "max_output_tokens"` is a clean `length` stop (`:750-753`).
/// Every other incomplete reason — `content_filter` above all — is an **error** terminal upstream
/// (`:754-759`), so mapping them all to `length` reported a blocked response as a successful,
/// merely-truncated turn.
///
/// The unknown-status arm is Pi's `default:` branch, whose `never` exhaustiveness check *throws*
/// `Unhandled stop reason: <status>` at runtime for a status the SDK union does not name
/// (`:767-770`); the throw lands in the caller's catch and becomes an `error` terminal
/// (`openai-responses.ts:184-190`), never a clean stop.
fn map_stop_reason(
    status: Option<&str>,
    incomplete_reason: Option<&str>,
) -> (StopReason, Option<String>) {
    match status {
        None => (StopReason::Stop, None),
        Some("completed") => (StopReason::Stop, None),
        Some("incomplete") => match incomplete_reason {
            Some("max_output_tokens") => (StopReason::Length, None),
            Some(reason) => (
                StopReason::Error,
                Some(format!("Response incomplete: {reason}")),
            ),
            None => (
                StopReason::Error,
                Some("Response incomplete without a provider reason".to_string()),
            ),
        },
        Some("failed") | Some("cancelled") => (StopReason::Error, None),
        Some("in_progress") | Some("queued") => (StopReason::Stop, None),
        Some(other) => (
            StopReason::Error,
            Some(format!("Unhandled stop reason: {other}")),
        ),
    }
}
