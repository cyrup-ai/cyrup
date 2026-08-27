//! `thoughtSignature` validation and retention.

use super::*;

/// A valid (multiple-of-4, base64) thought signature for the signed-empty-block tests.
const VALID_SIG: &str = "AAAAAAAAAAAAAAAAAAAAAA==";

/// Pi google-shared.ts:148-151 (commit 6138f5a0): Gemini can attach `thoughtSignature` to a
/// part whose visible text is empty and requires it echoed back. A signed EMPTY thinking block
/// must survive so the reasoning chain is not broken.
#[test]
fn keeps_signed_empty_thinking_block() {
    let m = model_with("gemini-3-pro-preview", true);
    let ctx = signed_block_ctx(
        "google",
        "gemini-3-pro-preview",
        vec![
            Content::Thinking {
                thinking: String::new(),
                thinking_signature: Some(VALID_SIG.to_string()),
                redacted: false,
            },
            a_tool_call(),
        ],
    );
    let contents = convert_messages(&m, &ctx);
    let parts = model_turn_parts(&contents);
    let signed: Vec<&Value> = parts
        .iter()
        .filter(|p| p.get("thoughtSignature").and_then(Value::as_str) == Some(VALID_SIG))
        .collect();
    assert_eq!(signed.len(), 1, "parts: {parts:?}");
    assert_eq!(signed[0]["thought"], true);
    assert_eq!(signed[0]["text"], "");
}

/// Pi google-shared.ts:134-139: the same rule for a signed EMPTY text block.
#[test]
fn keeps_signed_empty_text_block() {
    let m = model_with("gemini-3-pro-preview", true);
    let ctx = signed_block_ctx(
        "google",
        "gemini-3-pro-preview",
        vec![
            Content::Text {
                text: String::new(),
                text_signature: Some(VALID_SIG.to_string()),
            },
            a_tool_call(),
        ],
    );
    let contents = convert_messages(&m, &ctx);
    let parts = model_turn_parts(&contents);
    let signed: Vec<&Value> = parts
        .iter()
        .filter(|p| p.get("thoughtSignature").and_then(Value::as_str) == Some(VALID_SIG))
        .collect();
    assert_eq!(signed.len(), 1, "parts: {parts:?}");
    assert!(signed[0].get("thought").is_none());
    assert_eq!(signed[0]["text"], "");
}

/// The skip is gated on the signature being ABSENT — UNSIGNED empty blocks are still dropped
/// (Pi google-shared.ts:139/151).
#[test]
fn still_drops_unsigned_empty_blocks() {
    let m = model_with("gemini-3-pro-preview", true);
    let ctx = signed_block_ctx(
        "google",
        "gemini-3-pro-preview",
        vec![
            Content::Thinking {
                thinking: String::new(),
                thinking_signature: None,
                redacted: false,
            },
            Content::Text {
                text: "   ".to_string(),
                text_signature: None,
            },
            a_tool_call(),
        ],
    );
    let contents = convert_messages(&m, &ctx);
    let parts = model_turn_parts(&contents);
    assert_eq!(parts.len(), 1, "parts: {parts:?}");
    assert!(parts[0].get("functionCall").is_some());
}

/// An empty text block whose signature is INVALID base64 resolves to no signature, so the
/// unsigned rule applies and it is still dropped.
#[test]
fn still_drops_empty_block_with_invalid_signature() {
    let m = model_with("gemini-3-pro-preview", true);
    let ctx = signed_block_ctx(
        "google",
        "gemini-3-pro-preview",
        vec![
            Content::Text {
                text: String::new(),
                text_signature: Some("not base64!".to_string()),
            },
            a_tool_call(),
        ],
    );
    let contents = convert_messages(&m, &ctx);
    let parts = model_turn_parts(&contents);
    assert_eq!(parts.len(), 1, "parts: {parts:?}");
    assert!(parts[0].get("functionCall").is_some());
}

/// The cross-provider/model `else` branch keeps the OLD unconditional skip — the signature is
/// unusable there, so signed empty blocks are still dropped and the signature never leaks
/// (Pi google-shared.ts:157-162, deliberately retained by 6138f5a0).
#[test]
fn cross_provider_drops_signed_empty_blocks_unconditionally() {
    let m = model_with("gemini-3-pro-preview", true);
    // Assistant turn is attributed to a DIFFERENT model → `same` is false.
    let ctx = signed_block_ctx(
        "google",
        "other-model",
        vec![
            Content::Thinking {
                thinking: String::new(),
                thinking_signature: Some(VALID_SIG.to_string()),
                redacted: false,
            },
            Content::Text {
                text: String::new(),
                text_signature: Some(VALID_SIG.to_string()),
            },
            a_tool_call(),
        ],
    );
    let contents = convert_messages(&m, &ctx);
    let parts = model_turn_parts(&contents);
    assert_eq!(parts.len(), 1, "parts: {parts:?}");
    assert!(parts[0].get("functionCall").is_some());
    assert!(!Value::Array(parts).to_string().contains(VALID_SIG));
}

/// The cross-provider branch still converts a NON-empty thinking block to plain text.
#[test]
fn cross_provider_keeps_non_empty_thinking_as_text() {
    let m = model_with("gemini-3-pro-preview", true);
    let ctx = signed_block_ctx(
        "google",
        "other-model",
        vec![Content::Thinking {
            thinking: "reasoned".to_string(),
            thinking_signature: Some(VALID_SIG.to_string()),
            redacted: false,
        }],
    );
    let contents = convert_messages(&m, &ctx);
    let parts = model_turn_parts(&contents);
    assert_eq!(parts.len(), 1, "parts: {parts:?}");
    assert_eq!(parts[0]["text"], "reasoned");
    assert!(parts[0].get("thought").is_none());
    assert!(parts[0].get("thoughtSignature").is_none());
}

#[test]
fn base64_signature_validation() {
    assert!(is_valid_thought_signature("YWJjZA=="));
    assert!(!is_valid_thought_signature("not base64!"));
    assert!(!is_valid_thought_signature("abc")); // not a multiple of 4
}
