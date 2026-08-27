//! Event mapping.

use super::*;

#[test]
fn codex_error_events_carry_upstream_text() {
    // pi `Codex error: ${message || code || JSON.stringify(event)}` (:728).
    assert_eq!(
        map_codex_event(&json!({ "type": "error", "message": "boom" }), None),
        MappedCodexEvent::Fail("Codex error: boom".to_string())
    );
    // Nested error object (:709-718).
    assert_eq!(
        map_codex_event(
            &json!({ "type": "error", "error": { "code": "websocket_connection_limit_reached" } }),
            None
        ),
        MappedCodexEvent::Fail("Codex error: websocket_connection_limit_reached".to_string())
    );
    // Neither code nor message: the serialized event.
    let MappedCodexEvent::Fail(text) = map_codex_event(&json!({ "type": "error" }), None)
    else {
        panic!("expected Fail");
    };
    assert!(text.starts_with("Codex error: {"), "{text}");
}

#[test]
fn response_failed_uses_its_error_message() {
    // pi `message || "Codex response failed"` (:738).
    assert_eq!(
        map_codex_event(
            &json!({ "type": "response.failed", "response": { "error": { "message": "nope" } } }),
            None
        ),
        MappedCodexEvent::Fail("nope".to_string())
    );
    assert_eq!(
        map_codex_event(&json!({ "type": "response.failed" }), None),
        MappedCodexEvent::Fail("Codex response failed".to_string())
    );
}

#[test]
fn terminal_events_are_rewritten_to_response_completed() {
    // pi :741-748 — all three terminals collapse to `response.completed`.
    for etype in ["response.done", "response.completed", "response.incomplete"] {
        let ev = json!({ "type": etype, "response": { "id": "r1", "status": "incomplete" } });
        let MappedCodexEvent::Terminal(mapped) = map_codex_event(&ev, None) else {
            panic!("expected Terminal for {etype}");
        };
        assert_eq!(mapped["type"], json!("response.completed"));
        assert_eq!(mapped["response"]["status"], json!("incomplete"));
    }
}

#[test]
fn unknown_status_is_normalized_away() {
    // pi normalizeCodexStatus (:754-757): an out-of-set status becomes `undefined`, which the
    // shared `mapStopReason(undefined)` reads as `stop`.
    let ev = json!({ "type": "response.done", "response": { "status": "weird" } });
    let MappedCodexEvent::Terminal(mapped) = map_codex_event(&ev, None) else {
        panic!("expected Terminal");
    };
    assert!(mapped["response"].get("status").is_none());
    // MIRROR: an in-set status survives.
    let ev = json!({ "type": "response.done", "response": { "status": "queued" } });
    let MappedCodexEvent::Terminal(mapped) = map_codex_event(&ev, None) else {
        panic!("expected Terminal");
    };
    assert_eq!(mapped["response"]["status"], json!("queued"));
}

#[test]
fn untyped_events_are_skipped_and_others_pass_through() {
    assert_eq!(
        map_codex_event(&json!({ "no_type": true }), None),
        MappedCodexEvent::Skip
    );
    let ev = json!({ "type": "response.output_text.delta", "delta": "hi" });
    assert_eq!(
        map_codex_event(&ev, None),
        MappedCodexEvent::Pass(ev.clone())
    );
}

#[test]
fn service_tier_resolution_matches_upstream() {
    // pi resolveCodexServiceTier (:627-635).
    assert_eq!(
        resolve_codex_service_tier(Some("default"), Some("priority")).as_deref(),
        Some("priority")
    );
    assert_eq!(
        resolve_codex_service_tier(Some("default"), Some("flex")).as_deref(),
        Some("flex")
    );
    // A non-`default` response tier always wins.
    assert_eq!(
        resolve_codex_service_tier(Some("flex"), Some("priority")).as_deref(),
        Some("flex")
    );
    // `default` with no requested tier stays `default`.
    assert_eq!(
        resolve_codex_service_tier(Some("default"), None).as_deref(),
        Some("default")
    );
    // Absent response tier falls back to the requested one.
    assert_eq!(
        resolve_codex_service_tier(None, Some("flex")).as_deref(),
        Some("flex")
    );
    assert_eq!(resolve_codex_service_tier(None, None), None);
}

