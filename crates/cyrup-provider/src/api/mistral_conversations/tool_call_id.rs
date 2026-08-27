//! Tool-call-id normalization (Pi createMistralToolCallIdNormalizer, :153-183)

use crate::utils::hash::short_hash;
use std::cell::RefCell;
use std::collections::HashMap;
use super::MISTRAL_TOOL_CALL_ID_LENGTH;

/// A deterministic, collision-avoiding 9-char tool-call-id normalizer (Pi
/// `createMistralToolCallIdNormalizer`). Stateful within one request: stable per source id, and
/// distinct source ids never collapse to the same candidate.
#[derive(Default)]
pub(super) struct MistralToolCallIdNormalizer {
    id_map: RefCell<HashMap<String, String>>,
    reverse_map: RefCell<HashMap<String, String>>,
}

impl MistralToolCallIdNormalizer {
    pub(super) fn normalize(&self, id: &str) -> String {
        if let Some(existing) = self.id_map.borrow().get(id) {
            return existing.clone();
        }
        let mut attempt = 0u32;
        loop {
            let candidate = derive_mistral_tool_call_id(id, attempt);
            let owner = self.reverse_map.borrow().get(&candidate).cloned();
            if owner.as_deref().map(|o| o == id).unwrap_or(true) {
                self.id_map
                    .borrow_mut()
                    .insert(id.to_string(), candidate.clone());
                self.reverse_map
                    .borrow_mut()
                    .insert(candidate.clone(), id.to_string());
                return candidate;
            }
            attempt += 1;
        }
    }
}

/// Derive a candidate 9-char id for `id` at `attempt` (Pi `deriveMistralToolCallId`,
/// mistral-conversations.ts:175-183).
pub(super) fn derive_mistral_tool_call_id(id: &str, attempt: u32) -> String {
    let normalized: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if attempt == 0 && normalized.len() == MISTRAL_TOOL_CALL_ID_LENGTH {
        return normalized;
    }
    let seed_base = if normalized.is_empty() {
        id.to_string()
    } else {
        normalized
    };
    let seed = if attempt == 0 {
        seed_base
    } else {
        format!("{seed_base}:{attempt}")
    };
    short_hash(&seed)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(MISTRAL_TOOL_CALL_ID_LENGTH)
        .collect()
}
