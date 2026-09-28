//! `reasoning_details` — streaming capture, thinking-signature serialization and replay.
//!
//! A 1:1 translation of pi `test/openai-completions-reasoning-details.test.ts` @v0.87.1 (all four
//! cases). cyrup drives `decode_stream` over raw SSE instead of mocking the `openai` client, and
//! reads the replayed payload through `build_body`, but the chunk sequences, the details and every
//! assertion are upstream's.

use super::*;

/// pi's `reasoningDetail` (`test/openai-completions-reasoning-details.test.ts:40`).
fn encrypted_detail() -> Value {
    json!({ "type": "reasoning.encrypted", "id": "call_1", "data": "encrypted-signature" })
}

/// pi's `signedReasoningTextDetail` (`:41-48`).
fn signed_text_detail() -> Value {
    json!({
        "type": "reasoning.text",
        "text": "I should call the read tool.",
        "signature": "sha256:signed-text",
        "id": "reasoning-text-1",
        "format": "anthropic-claude-v1",
        "index": 0,
    })
}

/// pi's `reasoningSummaryDetail` (`:49-55`).
fn summary_detail() -> Value {
    json!({
        "type": "reasoning.summary",
        "summary": "Decided to inspect the requested file.",
        "id": "reasoning-summary-1",
        "format": "anthropic-claude-v1",
        "index": 1,
    })
}

/// The terminal message of a decoded stream.
async fn stream_message(raw: &'static str) -> AssistantMessage {
    let events = collect_events(raw).await;
    match events.last() {
        Some(StreamEvent::Done { message, .. }) => (**message).clone(),
        other => panic!("expected a Done terminal, got {other:?}"),
    }
}

/// The thinking block's `(thinking, signature)` pair, as pi's `content.find(type === "thinking")`.
fn thinking_of(am: &AssistantMessage) -> (String, Option<String>) {
    am.content
        .iter()
        .find_map(|c| match c {
            Content::Thinking {
                thinking,
                thinking_signature,
                ..
            } => Some((thinking.to_string(), thinking_signature.clone())),
            _ => None,
        })
        .expect("a thinking block")
}

/// `getAssistantPayload` (`:104-109`): the assistant message of the replayed request body.
fn replayed_assistant(am: AssistantMessage) -> Value {
    let ctx = Context {
        system_prompt: None,
        messages: vec![Message::Assistant(am)],
        tools: vec![],
    };
    let body = build_body(&model(), &ctx, &StreamOptions::default());
    body["messages"]
        .as_array()
        .and_then(|ms| ms.iter().find(|m| m["role"] == "assistant").cloned())
        .expect("an assistant message in the replayed payload")
}

/// One `reasoning.encrypted` detail plus a tool call: the detail lands on the THINKING block's
/// signature, the tool call carries none, and replay emits `reasoning_details`
/// ("preserves reasoning_details in the thinking signature", `:115-137`).
#[tokio::test]
async fn preserves_reasoning_details_in_the_thinking_signature() {
    let raw = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.encrypted\",\"id\":\"call_1\",\"data\":\"encrypted-signature\"}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"README.md\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let am = stream_message(raw).await;
    let (thinking, signature) = thinking_of(&am);
    assert_eq!(thinking, "");
    assert_eq!(
        serde_json::from_str::<Value>(&signature.expect("a signature")).unwrap(),
        json!([encrypted_detail()])
    );

    let tc = am
        .content
        .iter()
        .find_map(|c| match c {
            Content::ToolCall(tc) => Some(tc.clone()),
            _ => None,
        })
        .expect("a tool call");
    assert_eq!(tc.id.as_str(), "call_1");
    assert_eq!(tc.name, "read");
    assert_eq!(tc.thought_signature, None);

    let assistant = replayed_assistant(am);
    assert_eq!(assistant["reasoning_details"], json!([encrypted_detail()]));
}

/// The legacy shape: no thinking block, the single encrypted detail sits on the tool call's
/// `thoughtSignature` — replay still emits `reasoning_details`
/// ("falls back to encrypted tool-call signatures for older stored assistant messages", `:139-153`).
#[tokio::test]
async fn falls_back_to_encrypted_tool_call_signatures() {
    let raw = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.encrypted\",\"id\":\"call_1\",\"data\":\"encrypted-signature\"}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"README.md\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let mut am = stream_message(raw).await;
    // Strip the thinking block and stamp the detail onto the tool call, as pi's test does.
    am.content
        .retain(|c| !matches!(c, Content::Thinking { .. }));
    for c in am.content.iter_mut() {
        if let Content::ToolCall(tc) = c {
            tc.thought_signature = Some(encrypted_detail().to_string());
        }
    }

    let assistant = replayed_assistant(am.clone());
    assert_eq!(assistant["reasoning_details"], json!([encrypted_detail()]));

    // A thought signature that is NOT a valid encrypted detail is not replayed at all: pi validates
    // the parsed value (`parseLegacyEncryptedReasoningDetail`, openai-completions.ts:225-241) rather
    // than forwarding whatever JSON the slot held.
    let mut junk = am;
    for c in junk.content.iter_mut() {
        if let Content::ToolCall(tc) = c {
            tc.thought_signature = Some("\"an-opaque-gemini-signature\"".to_string());
        }
    }
    let assistant = replayed_assistant(junk);
    assert!(
        assistant.get("reasoning_details").is_none(),
        "an unparseable thought signature must not become a reasoning detail: {assistant}"
    );
}

/// A `reasoning` text field AND `reasoning_details` carrying text/encrypted/summary: all three are
/// preserved in order, the thinking text comes from the raw field, and the replayed payload carries
/// `reasoning_details` and NO raw `reasoning` key
/// ("preserves signed text and summary reasoning_details in their original sequence", `:155-177`).
#[tokio::test]
async fn preserves_text_and_summary_details_in_sequence() {
    let raw = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning\":\"I should call the read tool.\",\"reasoning_details\":[{\"type\":\"reasoning.text\",\"text\":\"I should call the read tool.\",\"signature\":\"sha256:signed-text\",\"id\":\"reasoning-text-1\",\"format\":\"anthropic-claude-v1\",\"index\":0}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.encrypted\",\"id\":\"call_1\",\"data\":\"encrypted-signature\"},{\"type\":\"reasoning.summary\",\"summary\":\"Decided to inspect the requested file.\",\"id\":\"reasoning-summary-1\",\"format\":\"anthropic-claude-v1\",\"index\":1}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"README.md\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let expected = json!([signed_text_detail(), encrypted_detail(), summary_detail()]);
    let am = stream_message(raw).await;
    let (thinking, signature) = thinking_of(&am);
    assert_eq!(thinking, "I should call the read tool.");
    assert_eq!(
        serde_json::from_str::<Value>(&signature.expect("a signature")).unwrap(),
        expected
    );

    let assistant = replayed_assistant(am);
    assert_eq!(assistant["reasoning_details"], expected);
    assert!(
        assistant.get("reasoning").is_none(),
        "a raw reasoning field must not be sent beside reasoning_details: {assistant}"
    );
}

/// Six deltas (text, text+signature, summary, summary+format, encrypted, summary) collapse to four
/// entries with concatenated text/summary and filled `signature`/`format`/`index`
/// ("merges consecutive text and summary reasoning_details deltas before replay", `:179-246`).
#[tokio::test]
async fn merges_consecutive_text_and_summary_deltas() {
    let raw = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.text\",\"text\":\"The\",\"index\":0}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.text\",\"text\":\" user wants the time.\",\"signature\":\"sha256:text-signature\",\"format\":\"openai-responses-v1\",\"index\":0}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.summary\",\"summary\":\"Looked\",\"index\":0}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.summary\",\"summary\":\" up time.\",\"format\":\"openai-responses-v1\",\"index\":0}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.encrypted\",\"id\":\"call_1\",\"data\":\"encrypted-signature\"}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.summary\",\"summary\":\"After encrypted block.\",\"format\":\"openai-responses-v1\",\"index\":0}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"README.md\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let expected = json!([
        {
            "type": "reasoning.text",
            "text": "The user wants the time.",
            "index": 0,
            "signature": "sha256:text-signature",
            "format": "openai-responses-v1",
        },
        {
            "type": "reasoning.summary",
            "summary": "Looked up time.",
            "index": 0,
            "format": "openai-responses-v1",
        },
        encrypted_detail(),
        {
            "type": "reasoning.summary",
            "summary": "After encrypted block.",
            "format": "openai-responses-v1",
            "index": 0,
        },
    ]);
    let am = stream_message(raw).await;
    let (thinking, signature) = thinking_of(&am);
    assert_eq!(thinking, "");
    assert_eq!(
        serde_json::from_str::<Value>(&signature.expect("a signature")).unwrap(),
        expected
    );

    let assistant = replayed_assistant(am);
    assert_eq!(assistant["reasoning_details"], expected);
}
