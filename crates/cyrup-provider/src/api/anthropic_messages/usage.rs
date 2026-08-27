//! Response decoding — usage accounting.

use cyrup_core::Usage;
use serde_json::Value;

/// Apply `message_start` usage (Pi anthropic-messages.ts:551-558): seeds input/output/cache counts.
pub(super) fn apply_message_start_usage(usage: &mut Usage, raw: &Value) {
    usage.input = raw.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
    usage.output = raw
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    usage.cache_read = raw
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    usage.cache_write = raw
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let long = raw
        .get("cache_creation")
        .and_then(|c| c.get("ephemeral_1h_input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    usage.cache_write_1h = Some(long);
}

/// Apply `message_delta` usage (Pi anthropic-messages.ts:690-709): only present fields update,
/// preserving `input_tokens` from `message_start` when a proxy omits it.
pub(super) fn apply_message_delta_usage(usage: &mut Usage, raw: &Value) {
    if let Some(v) = raw.get("input_tokens").and_then(Value::as_u64) {
        usage.input = v;
    }
    if let Some(v) = raw.get("output_tokens").and_then(Value::as_u64) {
        usage.output = v;
    }
    if let Some(v) = raw.get("cache_read_input_tokens").and_then(Value::as_u64) {
        usage.cache_read = v;
    }
    if let Some(v) = raw
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
    {
        usage.cache_write = v;
    }
    if let Some(v) = raw
        .get("output_tokens_details")
        .and_then(|d| d.get("thinking_tokens"))
        .and_then(Value::as_u64)
    {
        usage.reasoning = Some(v);
    }
}
