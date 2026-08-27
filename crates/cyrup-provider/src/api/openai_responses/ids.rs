//! Message + tool conversion (Pi openai-responses-shared.ts): item-id normalization.

use crate::utils::hash::short_hash;

/// Sanitize one id part to `^[a-zA-Z0-9_-]{1,64}$` with trailing `_` trimmed (Pi `normalizeIdPart`,
/// openai-responses-shared.ts:98-102).
pub(super) fn normalize_id_part(part: &str) -> String {
    let sanitized: String = part
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let truncated: String = sanitized.chars().take(64).collect();
    truncated.trim_end_matches('_').to_string()
}

/// `fc_<shortHash>` clamped to 64 chars (Pi `buildForeignResponsesItemId`,
/// openai-responses-shared.ts:104-107).
pub(super) fn build_foreign_responses_item_id(item_id: &str) -> String {
    let normalized = format!("fc_{}", short_hash(item_id));
    normalized.chars().take(64).collect()
}
