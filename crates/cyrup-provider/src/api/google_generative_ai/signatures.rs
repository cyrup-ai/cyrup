//! Request encoding — `thoughtSignature` validation and retention (Pi
//! `isValidThoughtSignature` / `retainThoughtSignature`, google-shared.ts:53-60,88-92).

/// Thought signatures must be valid base64 (`TYPE_BYTES`) — Pi `isValidThoughtSignature`,
/// v0.84.1 `ai/src/api/google-shared.ts:53-60` (the `base64SignaturePattern` const plus the fn);
/// v0.83.0 `:52-59` — same body, shifted.
pub(super) fn is_valid_thought_signature(sig: &str) -> bool {
    if sig.is_empty() || !sig.len().is_multiple_of(4) {
        return false;
    }
    let body = sig.trim_end_matches('=');
    // At most two `=` padding chars (validated by the length-mod-4 rule above).
    sig.len() - body.len() <= 2
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
}

/// Keep a signature only for the same provider/model and valid base64 (Pi `resolveThoughtSignature`,
/// v0.84.1 `ai/src/api/google-shared.ts:62-67`; v0.83.0 `:61-66` — same body, shifted).
pub(super) fn resolve_thought_signature(same: bool, sig: Option<&str>) -> Option<String> {
    match sig {
        Some(s) if same && is_valid_thought_signature(s) => Some(s.to_string()),
        _ => None,
    }
}
