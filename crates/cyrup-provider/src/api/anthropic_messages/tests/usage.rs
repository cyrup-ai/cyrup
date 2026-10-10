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

// ---- PROV-149: the prices a turn is actually billed at, from the shipped catalog ----------------

/// The embedded `anthropic` row the provider serves for `id` — the model object a request is
/// built and costed with, not a hand-written fixture.
fn shipped(id: &str) -> Model {
    use crate::provider::Provider;
    crate::providers::anthropic::anthropic_provider()
        .get_model(id)
        .cloned()
        .unwrap_or_else(|| panic!("the embedded anthropic catalog has no {id}"))
}

fn approx(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-12,
        "{what}: priced at {actual}, expected {expected}"
    );
}

/// PROV-149 — pi.dev serves `claude-haiku-5-5` at 0.1/0.5/0.01/0.125 per 1M tokens with one tier,
/// `inputTokensAbove: 100000` at 0.5/2.5/0.05/0.625 (pi `f76c1db66`; Anthropic's pricing page
/// agrees). A 150k-input turn decoded from the wire is billed at the tier rate; a 90k one at base.
#[tokio::test]
async fn prov149_a_150k_input_haiku_55_turn_is_billed_at_the_long_context_tier() {
    let haiku = shipped("claude-haiku-5-5");
    let decode = |input: u64| {
        let haiku = haiku.clone();
        async move {
            let raw = stream(
                &format!("{{\"input_tokens\":{input},\"output_tokens\":1}}"),
                "{\"output_tokens\":2000}",
            );
            let events = collect(raw.as_bytes().to_vec(), &haiku).await;
            match events.last() {
                Some(StreamEvent::Done { message, .. }) => message.usage.clone(),
                other => panic!("expected a done terminal, got {other:?}"),
            }
        }
    };

    let long = decode(150_000).await;
    assert_eq!(long.input, 150_000);
    approx(long.cost.input, 150_000.0 * 0.5 / 1e6, "150k input");
    approx(
        long.cost.output,
        2_000.0 * 2.5 / 1e6,
        "output above the tier",
    );
    approx(
        long.cost.total,
        (150_000.0 * 0.5 + 2_000.0 * 2.5) / 1e6,
        "total",
    );

    // MIRROR: under the threshold the same model bills the base rate.
    let short = decode(90_000).await;
    approx(short.cost.input, 90_000.0 * 0.1 / 1e6, "90k input");
    approx(
        short.cost.output,
        2_000.0 * 0.5 / 1e6,
        "output below the tier",
    );
}

/// Anthropic's pricing page: the >100K threshold counts ALL input tokens, cache reads and writes
/// included. cyrup's `select_rates` keys on `input + cacheRead + cacheWrite`, as pi's
/// `calculateCost` does (`models.ts:1200-1209` @f1b2e77f5), so 20k fresh plus 90k cache-read
/// tokens is a long-context turn and every component moves to the tier.
#[tokio::test]
async fn prov149_cache_reads_count_toward_the_haiku_55_tier_threshold() {
    let raw = stream(
        "{\"input_tokens\":20000,\"cache_read_input_tokens\":90000,\"output_tokens\":1}",
        "{\"output_tokens\":1000}",
    );
    let events = collect(raw.as_bytes().to_vec(), &shipped("claude-haiku-5-5")).await;
    let usage = match events.last() {
        Some(StreamEvent::Done { message, .. }) => message.usage.clone(),
        other => panic!("expected a done terminal, got {other:?}"),
    };
    assert_eq!(usage.input, 20_000);
    assert_eq!(usage.cache_read, 90_000);
    approx(
        usage.cost.input,
        20_000.0 * 0.5 / 1e6,
        "fresh input at the tier",
    );
    approx(
        usage.cost.cache_read,
        90_000.0 * 0.05 / 1e6,
        "cache read at the tier",
    );
    approx(usage.cost.output, 1_000.0 * 2.5 / 1e6, "output at the tier");
}

/// PROV-149 — pi `ce950d78f` dropped the hand-written 5.5 fallbacks, so models.dev's Sonnet 5.5
/// `cacheRead: 0.1` wins (Anthropic: $0.10/MTok, 0.05x the $2 base). The embedded row said 0.2.
#[tokio::test]
async fn prov149_sonnet_55_cache_reads_are_billed_at_ten_cents_per_million() {
    let raw = stream(
        "{\"input_tokens\":1000,\"cache_read_input_tokens\":500000,\"output_tokens\":1}",
        "{\"output_tokens\":100}",
    );
    let events = collect(raw.as_bytes().to_vec(), &shipped("claude-sonnet-5-5")).await;
    let usage = match events.last() {
        Some(StreamEvent::Done { message, .. }) => message.usage.clone(),
        other => panic!("expected a done terminal, got {other:?}"),
    };
    assert_eq!(usage.cache_read, 500_000);
    approx(
        usage.cost.cache_read,
        500_000.0 * 0.1 / 1e6,
        "Sonnet 5.5 cache read",
    );
    approx(usage.cost.input, 1_000.0 * 2.0 / 1e6, "Sonnet 5.5 input");
    approx(usage.cost.output, 100.0 * 10.0 / 1e6, "Sonnet 5.5 output");
}
