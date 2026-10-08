//! Usage accounting from `message_start` / `message_delta`.

use super::*;

/// PROV-122 — pi `api/anthropic-messages.ts:841-847` @ce950d78f: `message_delta` reads
/// `usage.cache_creation.ephemeral_1h_input_tokens` under a `!= null` guard, so a one-hour cache
/// write reported only in the delta is priced at 2x input, and a delta without the breakdown
/// leaves the `message_start` value intact.
fn stream(start_usage: &str, delta_usage: &str) -> String {
    format!(
        concat!(
            "event: message_start\n",
            "data: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_1\",\"usage\":{}}}}}\n\n",
            "event: message_delta\n",
            "data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{}}}\n\n",
            "event: message_stop\n",
            "data: {{\"type\":\"message_stop\"}}\n\n",
        ),
        start_usage, delta_usage
    )
}

async fn terminal(raw: &str) -> AssistantMessage {
    let events = collect(raw.as_bytes().to_vec(), &model()).await;
    match events.last() {
        Some(StreamEvent::Done { message, .. }) => (**message).clone(),
        other => panic!("expected a done terminal, got {other:?}"),
    }
}

#[tokio::test]
async fn prov122_a_delta_only_breakdown_is_priced_at_the_one_hour_rate() {
    let msg = terminal(&stream(
        "{\"input_tokens\":10,\"output_tokens\":1}",
        "{\"output_tokens\":5,\"cache_creation_input_tokens\":1500,\"cache_creation\":{\"ephemeral_1h_input_tokens\":1000}}",
    ))
    .await;
    assert_eq!(msg.usage.cache_write, 1500);
    assert_eq!(msg.usage.cache_write_1h, Some(1000));
    // 500 short @ $6.25/1e6 + 1000 long @ 2 x $5/1e6 (`usage.rs` `compute_cost`).
    let expected = (500.0 * 6.25 + 1000.0 * 5.0 * 2.0) / 1e6;
    assert!(
        (msg.usage.cost.cache_write - expected).abs() < 1e-12,
        "cost.cache_write was {}",
        msg.usage.cost.cache_write
    );
}

#[tokio::test]
async fn prov122_a_delta_without_a_breakdown_keeps_the_start_value() {
    let msg = terminal(&stream(
        "{\"input_tokens\":10,\"output_tokens\":1,\"cache_creation_input_tokens\":1500,\"cache_creation\":{\"ephemeral_1h_input_tokens\":1000}}",
        "{\"output_tokens\":5,\"cache_creation_input_tokens\":1500}",
    ))
    .await;
    assert_eq!(msg.usage.cache_write_1h, Some(1000));
}

#[tokio::test]
async fn prov122_a_delta_with_an_empty_breakdown_keeps_the_start_value() {
    let msg = terminal(&stream(
        "{\"input_tokens\":10,\"output_tokens\":1,\"cache_creation\":{\"ephemeral_1h_input_tokens\":1000}}",
        "{\"output_tokens\":5,\"cache_creation\":{}}",
    ))
    .await;
    assert_eq!(msg.usage.cache_write_1h, Some(1000));
}
