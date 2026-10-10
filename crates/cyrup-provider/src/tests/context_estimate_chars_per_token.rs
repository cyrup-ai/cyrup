//! PROV-142 — port of pi `packages/ai/test/context-estimate.test.ts` @f1b2e77f5 (v1.1.0+11).
//!
//! `27075fe07` (#10497) raised the request-context estimate from 4 to 3.5 characters per token, so
//! `clampMaxTokensToContext` (`api/simple-options.ts:18`) leaves more room for large new inputs.
//! Every expectation below is pi's literal number; each one moved when the divisor changed, and each
//! asserts the `max_tokens` the request is actually built with, not just the estimate.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::context::Context;
use crate::model::{Modality, Model, ModelCost};
use crate::utils::estimate::{ContextUsageEstimate, estimate_context_tokens};
use crate::utils::simple_options::{SimpleStreamOptions, build_base_options};
use cyrup_core::{AssistantMessage, Content, Message, ProviderId, StopReason, Usage};

/// pi `createAssistant(timestamp, totalTokens)`: one `"kept"` text block, `input = totalTokens`.
fn assistant(timestamp: i64, total_tokens: u64) -> Message {
    let mut m = AssistantMessage::errored(
        ProviderId::from("openai"),
        "test-model",
        None,
        StopReason::Stop,
        "",
    );
    m.error_message = None;
    m.content = vec![Content::text("kept")];
    m.usage = Usage {
        input: total_tokens,
        total_tokens,
        ..Usage::default()
    };
    m.timestamp = timestamp;
    Message::Assistant(m)
}

fn user(text: &str, timestamp: i64) -> Message {
    Message::User {
        content: vec![Content::text(text)],
        timestamp,
    }
}

/// pi's test `model`: `contextWindow: 10_000`, `maxTokens: 8_000`.
fn model() -> Model {
    Model {
        id: "test-model".into(),
        name: "Test Model".into(),
        api: "openai-responses".into(),
        provider: "openai".into(),
        base_url: "https://api.openai.com/v1".into(),
        reasoning: false,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        input_limits: None,
        prompt_cache: None,
        context_window: 10_000,
        max_tokens: 8_000,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

fn context(system_prompt: Option<&str>, messages: Vec<Message>) -> Context {
    Context {
        system_prompt: system_prompt.map(str::to_string),
        messages,
        tools: Vec::new(),
    }
}

fn request_max_tokens(ctx: &Context) -> Option<u64> {
    build_base_options(&model(), ctx, &SimpleStreamOptions::default(), None).max_tokens
}

/// "reserves 3.5 characters per token for new text when limiting output" (the #10497 regression).
/// At `/ 4` the 3500-char prompt was 875 tokens and the request asked for 3029.
#[test]
fn reserves_three_and_a_half_chars_per_token_when_limiting_output() {
    let ctx = context(
        None,
        vec![assistant(100, 2_000), user(&"x".repeat(3_500), 200)],
    );
    // The request value first: at `/ 4` it was 10000 - 2875 - 4096 = 3029.
    assert_eq!(request_max_tokens(&ctx), Some(2_904));
    assert_eq!(
        estimate_context_tokens(&ctx),
        ContextUsageEstimate {
            tokens: 3_000,
            usage_tokens: 2_000,
            trailing_tokens: 1_000,
            last_usage_index: Some(0),
        }
    );
}

/// "ignores stale assistant usage after a newer message is inserted before it". With no applicable
/// usage the whole prefix is estimated: system 6 + "summary" 7 + "kept" 4 + 4000 chars, each
/// rounded up separately: 2 + 2 + 2 + 1143 = 1149 (1005 at `/ 4`, which asked for 4899).
#[test]
fn stale_usage_falls_back_to_the_three_and_a_half_estimate() {
    let ctx = context(
        Some("system"),
        vec![
            user("summary", 200),
            assistant(100, 9_500),
            user(&"x".repeat(4_000), 300),
        ],
    );
    // The request value first: at `/ 4` it was 10000 - 1005 - 4096 = 4899.
    assert_eq!(request_max_tokens(&ctx), Some(4_755));
    assert_eq!(
        estimate_context_tokens(&ctx),
        ContextUsageEstimate {
            tokens: 1_149,
            usage_tokens: 0,
            trailing_tokens: 1_149,
            last_usage_index: None,
        }
    );
}

/// "uses assistant usage again after a response to the inserted context": the 4-char `"tail"`
/// trails at ceil(4 / 3.5) = 2 tokens (1 at `/ 4`).
#[test]
fn fresh_usage_counts_the_trailing_text_at_three_and_a_half() {
    let ctx = context(
        None,
        vec![
            user("summary", 200),
            assistant(100, 9_500),
            user("new prompt", 300),
            assistant(400, 2_000),
            user("tail", 500),
        ],
    );
    assert_eq!(
        estimate_context_tokens(&ctx),
        ContextUsageEstimate {
            tokens: 2_002,
            usage_tokens: 2_000,
            trailing_tokens: 2,
            last_usage_index: Some(3),
        }
    );
}
