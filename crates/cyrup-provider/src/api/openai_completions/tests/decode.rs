//! SSE decode: text, reasoning, tool calls and usage.

use super::*;

#[tokio::test]
async fn decodes_text_with_usage_terminal() {
    let raw = concat!(
        "data: {\"id\":\"chatcmpl-1\",\"model\":\"openai/gpt-oss-120b\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect_events(raw).await;

    assert!(matches!(events.first(), Some(StreamEvent::Start { .. })));
    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::TextStart {
            content_index: 0,
            ..
        }
    )));
    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, vec!["Hel", "lo"]);

    match events.last() {
        Some(StreamEvent::Done { message, .. }) => {
            assert_eq!(message.stop_reason, StopReason::Stop);
            assert_eq!(message.content, vec![Content::text("Hello")]);
            assert_eq!(message.response_id.as_deref(), Some("chatcmpl-1"));
            assert_eq!(message.usage.input, 10);
            assert_eq!(message.usage.output, 5);
            assert!(message.usage.cost.total > 0.0);
        }
        other => panic!("expected Done terminal, got {other:?}"),
    }
}

#[tokio::test]
async fn non_terminal_events_carry_running_partial() {
    // Pi parity (R-01-022): every non-terminal event carries a `partial` snapshot that grows
    // as deltas arrive, so consumers never reconstruct from deltas.
    let raw = concat!(
        "data: {\"id\":\"resp-1\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect_events(raw).await;

    // Start's partial is empty; every non-terminal carries a partial (the terminal does not).
    assert!(matches!(&events[0], StreamEvent::Start { partial } if partial.content.is_empty()));
    for ev in &events {
        match ev {
            StreamEvent::Done { .. } | StreamEvent::Error { .. } => {
                assert!(ev.partial().is_none())
            }
            _ => assert!(
                ev.partial().is_some(),
                "non-terminal must carry partial: {ev:?}"
            ),
        }
    }

    // The last text_delta's partial reflects the full accumulated text + the response id.
    let last_delta = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta { partial, .. } => Some(partial),
            _ => None,
        })
        .next_back()
        .expect("a text_delta");
    assert_eq!(last_delta.content, vec![Content::text("Hello")]);
    assert_eq!(last_delta.response_id.as_deref(), Some("resp-1"));
    // PROV-010 / AGENT-014 / DRIFT-012, wire half — CLOSED. Pi's in-flight `partial` carries
    // `stopReason: "pending"` (openai-completions.ts:218), and cyrup now does too. This
    // assertion previously pinned `Stop` and said so in its own comment ("pins the CURRENT wire
    // value; it is NOT a statement that `stop` is meaningful mid-stream") — i.e. it pinned the
    // defect. It is REWRITTEN, not removed: a mid-stream partial that claims a completed turn
    // is the bug, so the correct value is the one Pi emits.
    //
    // The containment invariant it used to gesture at still holds and is enforced elsewhere:
    // `Pending` can never reach a TERMINAL event, because `decode_stream` routes end-of-stream
    // through `StreamEvent::end_of_stream` and `StreamEvent::terminal` normalizes a surviving
    // `Pending` to `Error` (see `api::truncation_parity` and
    // `stream_end_without_finish_reason_is_an_error` below).
    assert_eq!(last_delta.stop_reason, StopReason::Pending);
    assert_eq!(
        serde_json::to_value(last_delta).unwrap()["stopReason"],
        "pending",
        "wire spelling must be Pi's"
    );
}

#[tokio::test]
async fn decodes_multichunk_tool_call() {
    let raw = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_9\",\"function\":{\"name\":\"add\",\"arguments\":\"{\\\"a\\\":\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"1,\\\"b\\\":2}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect_events(raw).await;

    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::ToolCallStart {
            content_index: 0,
            ..
        }
    )));
    let delta_count = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ToolCallDelta { .. }))
        .count();
    assert_eq!(delta_count, 2);

    let tc_end = events.iter().find_map(|e| match e {
        StreamEvent::ToolCallEnd { tool_call, .. } => Some(tool_call.clone()),
        _ => None,
    });
    let tc = tc_end.expect("toolcall_end");
    assert_eq!(tc.id.as_str(), "call_9");
    assert_eq!(tc.name, "add");
    assert_eq!(
        serde_json::Value::Object(tc.arguments),
        json!({ "a": 1, "b": 2 })
    );

    match events.last() {
        Some(StreamEvent::Done { message, .. }) => {
            assert_eq!(message.stop_reason, StopReason::ToolUse);
            assert_eq!(message.content.len(), 1);
        }
        other => panic!("expected Done terminal, got {other:?}"),
    }
}

#[tokio::test]
async fn decodes_encrypted_reasoning_details_onto_tool_calls() {
    // `call_a`: detail arrives AFTER the tool call (matched path).
    // `call_b`: detail arrives BEFORE the tool call (pending path).
    let raw = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.encrypted\",\"id\":\"call_b\",\"data\":\"BBB\"}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"function\":{\"name\":\"f\",\"arguments\":\"{}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.encrypted\",\"id\":\"call_a\",\"data\":\"AAA\"}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_b\",\"function\":{\"name\":\"g\",\"arguments\":\"{}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect_events(raw).await;
    let sigs: std::collections::HashMap<String, Option<String>> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolCallEnd { tool_call, .. } => Some((
                tool_call.id.as_str().to_string(),
                tool_call.thought_signature.clone(),
            )),
            _ => None,
        })
        .collect();
    assert!(
        sigs["call_a"]
            .as_deref()
            .unwrap()
            .contains("reasoning.encrypted")
    );
    assert!(sigs["call_a"].as_deref().unwrap().contains("AAA"));
    assert!(sigs["call_b"].as_deref().unwrap().contains("BBB"));
}

#[tokio::test]
async fn decodes_reasoning_to_thinking() {
    let raw = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think \"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"hard\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"answer\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect_events(raw).await;

    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::ThinkingStart {
            content_index: 0,
            ..
        }
    )));
    let think: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ThinkingDelta { delta, .. } => Some(delta.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(think, "think hard");

    match events.last() {
        Some(StreamEvent::Done { message, .. }) => {
            // The thinking block records which reasoning field carried it (`reasoning_content`).
            assert_eq!(
                message.content,
                vec![
                    Content::Thinking {
                        thinking: "think hard".to_string(),
                        thinking_signature: Some("reasoning_content".to_string()),
                        redacted: false,
                    },
                    Content::text("answer"),
                ]
            );
        }
        other => panic!("expected Done terminal, got {other:?}"),
    }
}
