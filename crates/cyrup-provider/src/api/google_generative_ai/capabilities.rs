//! Request encoding — the model-id capability probes: Gemini-3 pro/flash and Gemma-4 family
//! detection plus the id-derived feature gates Pi keys off the same strings
//! (google-generative-ai.ts:404-406, google-shared.ts:62-98).

use crate::model::Model;

/// `/gemma-?4/` (Pi `isGemma4Model`, google-generative-ai.ts:404-406).
pub(super) fn is_gemma4(model: &Model) -> bool {
    let id = model.id.as_str().to_lowercase();
    id.contains("gemma4") || id.contains("gemma-4")
}

/// `/gemini-3(?:\.\d+)?-pro/` (Pi `isGemini3ProModel`, google-generative-ai.ts:408-410).
pub(super) fn is_gemini3_pro(model: &Model) -> bool {
    gemini3_variant(&model.id.as_str().to_lowercase(), "-pro")
}

/// `/gemini-3(?:\.\d+)?-flash/` or the two `*-latest` aliases (Pi `isGemini3FlashModel`,
/// google-generative-ai.ts:412-415).
pub(super) fn is_gemini3_flash(model: &Model) -> bool {
    let id = model.id.as_str().to_lowercase();
    gemini3_variant(&id, "-flash")
        || id == "gemini-flash-latest"
        || id == "gemini-flash-lite-latest"
}

/// Match `gemini-3` optionally followed by `.<digits>`, then `suffix` (replicates the
/// `/gemini-3(?:\.\d+)?<suffix>/` regexes without a regex dependency).
fn gemini3_variant(id: &str, suffix: &str) -> bool {
    let needle = "gemini-3";
    let mut from = 0;
    while let Some(pos) = id[from..].find(needle) {
        let abs = from + pos;
        let rest = &id[abs + needle.len()..];
        let after_version = if let Some(stripped) = rest.strip_prefix('.') {
            let digits = stripped.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits == 0 {
                rest // `.` not followed by a digit → optional group does not match
            } else {
                &stripped[digits..]
            }
        } else {
            rest
        };
        if after_version.starts_with(suffix) {
            return true;
        }
        from = abs + 1;
    }
    false
}

/// `modelId.startsWith("claude-") || modelId.startsWith("gpt-oss-") ||
/// (geminiMajorVersion !== undefined && geminiMajorVersion >= 3)` — Pi `requiresToolCallId`,
/// v0.84.1 `ai/src/api/google-shared.ts:72-79`.
///
/// Version lag, not a port bug: at v0.83.0 (`google-shared.ts:71-73`) the function was only the
/// two `startsWith` arms; the Gemini-3 arm was added upstream for v0.84.1 so Gemini 3 models echo
/// explicit tool-call ids in `functionCall`/`functionResponse`.
pub(super) fn requires_tool_call_id(model_id: &str) -> bool {
    model_id.starts_with("claude-")
        || model_id.starts_with("gpt-oss-")
        || gemini_major_version(model_id).is_some_and(|v| v >= 3)
}

/// `getGeminiMajorVersion >= 3` (Pi `supportsMultimodalFunctionResponse`, v0.84.1
/// `ai/src/api/google-shared.ts:87-93`). A non-Gemini id (no major version) returns `true`.
/// Body unchanged v0.83.0 → v0.84.1; only the line span moved (v0.83.0 `:81-87`), because the
/// Gemini-3 arm added to `requiresToolCallId` above pushed everything after it down six lines.
pub(super) fn supports_multimodal_function_response(model_id: &str) -> bool {
    match gemini_major_version(model_id) {
        Some(v) => v >= 3,
        None => true,
    }
}

/// `/^gemini(?:-live)?-(\d+)/` (Pi `getGeminiMajorVersion`, v0.84.1
/// `ai/src/api/google-shared.ts:81-85`; v0.83.0 `:75-79` — same body, shifted).
pub(super) fn gemini_major_version(model_id: &str) -> Option<u32> {
    let id = model_id.to_lowercase();
    let rest = id.strip_prefix("gemini")?;
    let rest = rest.strip_prefix("-live").unwrap_or(rest);
    let rest = rest.strip_prefix('-')?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}
