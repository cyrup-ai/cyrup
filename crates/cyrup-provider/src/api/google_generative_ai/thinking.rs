//! Request encoding — the thinking lowering: `thinkingConfig` construction, the Gemini-3 /
//! Gemma-4 `thinkingLevel` split and the token-budget table (Pi `streamSimple` +
//! `getGoogleBudget`, google-generative-ai.ts:294-318,408-455).

use crate::collection::clamp_thinking_level;
use crate::model::Model;
use crate::utils::simple_options::ThinkingBudgets;
use cyrup_core::{ModelThinkingLevel, ThinkingLevel};
use serde_json::{Map, Value, json};
use super::capabilities::{is_gemini3_flash, is_gemini3_pro, is_gemma4};
use super::options::GoogleThinking;

/// Build `thinkingConfig` (Pi `buildParams` thinking branch + `streamSimple`,
/// google-generative-ai.ts:373-384,294-318). `None` omits the field entirely.
pub(super) fn thinking_config(model: &Model, reasoning: ModelThinkingLevel) -> Option<Value> {
    if !reasoning.is_on() {
        // streamSimple `!options.reasoning` path → `thinking: { enabled: false }`, which lowers to
        // the model's disabled-thinking config (google-generative-ai.ts:294-296,382-384).
        return Some(disabled_thinking_config(model));
    }

    // streamSimple reasoning path: clamp to a supported level, then `off → high`
    // (google-generative-ai.ts:298-299).
    let clamped = clamp_thinking_level(model, reasoning);
    let effort = clamped.level().unwrap_or(ThinkingLevel::High);

    let mut cfg = Map::new();
    cfg.insert("includeThoughts".to_string(), json!(true));

    if is_gemini3_pro(model) || is_gemini3_flash(model) || is_gemma4(model) {
        if let Some(level) = thinking_level(effort, model) {
            cfg.insert("thinkingLevel".to_string(), json!(level));
        }
    } else if let Some(budget) = google_budget(model, effort, None) {
        cfg.insert("thinkingBudget".to_string(), json!(budget));
    }
    Some(Value::Object(cfg))
}

/// Lower a direct `GoogleOptions.thinking` override to `thinkingConfig` (1:1 with Pi `buildParams`,
/// google-generative-ai.ts:373-384). When `enabled`, `level` wins over `budgetTokens`; otherwise the
/// model's disabled-thinking config. The outer `model.reasoning` guard is applied by the caller,
/// mirroring Pi's `options.thinking?.enabled && model.reasoning` / `model.reasoning && … !enabled`.
pub(super) fn thinking_config_override(model: &Model, thinking: &GoogleThinking) -> Option<Value> {
    if thinking.enabled {
        let mut cfg = Map::new();
        cfg.insert("includeThoughts".to_string(), json!(true));
        if let Some(level) = thinking.level {
            cfg.insert("thinkingLevel".to_string(), json!(level.as_wire()));
        } else if let Some(budget) = thinking.budget_tokens {
            cfg.insert("thinkingBudget".to_string(), json!(budget));
        }
        Some(Value::Object(cfg))
    } else {
        Some(disabled_thinking_config(model))
    }
}

/// The disabled-thinking config for a reasoning model (Pi `getDisabledThinkingConfig`,
/// google-generative-ai.ts:417-433).
fn disabled_thinking_config(model: &Model) -> Value {
    if is_gemini3_pro(model) {
        json!({ "thinkingLevel": "LOW" })
    } else if is_gemini3_flash(model) || is_gemma4(model) {
        // Gemini 3 Flash / Flash-Lite and Gemma 4 use the lowest level (Pi: MINIMAL).
        json!({ "thinkingLevel": "MINIMAL" })
    } else {
        // Gemini 2.x supports disabling via thinkingBudget = 0.
        json!({ "thinkingBudget": 0 })
    }
}

/// The Gemini-3 `thinkingLevel` for a clamped effort (Pi `getThinkingLevel`,
/// google-generative-ai.ts:435-466). `None` when the effort has no mapping (e.g. `xhigh` on a
/// Gemini-3-Pro model — Pi's switch returns `undefined`).
fn thinking_level(effort: ThinkingLevel, model: &Model) -> Option<&'static str> {
    if is_gemini3_pro(model) {
        return match effort {
            ThinkingLevel::Minimal | ThinkingLevel::Low => Some("LOW"),
            ThinkingLevel::Medium | ThinkingLevel::High => Some("HIGH"),
            ThinkingLevel::Xhigh | ThinkingLevel::Max => None,
        };
    }
    if is_gemma4(model) {
        return match effort {
            ThinkingLevel::Minimal | ThinkingLevel::Low => Some("MINIMAL"),
            ThinkingLevel::Medium | ThinkingLevel::High => Some("HIGH"),
            ThinkingLevel::Xhigh | ThinkingLevel::Max => None,
        };
    }
    match effort {
        ThinkingLevel::Minimal => Some("MINIMAL"),
        ThinkingLevel::Low => Some("LOW"),
        ThinkingLevel::Medium => Some("MEDIUM"),
        ThinkingLevel::High => Some("HIGH"),
        // Pi types the parameter `ClampedThinkingLevel = Exclude<ThinkingLevel, "xhigh"|"max">`
        // (google-generative-ai.ts:410), so both fall off every switch as `undefined`.
        ThinkingLevel::Xhigh | ThinkingLevel::Max => None,
    }
}

/// The token thinking-budget for a clamped effort (Pi `getGoogleBudget`,
/// google-generative-ai.ts:468-508). `None` when the model/effort pair has no budget (Pi returns
/// `undefined`, so the field is omitted); a non-Gemini-2.5 model returns `Some(-1)` (dynamic).
fn google_budget(
    model: &Model,
    effort: ThinkingLevel,
    custom: Option<&ThinkingBudgets>,
) -> Option<i64> {
    if let Some(c) = custom {
        let v = match effort {
            ThinkingLevel::Minimal => c.minimal,
            ThinkingLevel::Low => c.low,
            ThinkingLevel::Medium => c.medium,
            ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => c.high,
        };
        if let Some(v) = v {
            return Some(v as i64);
        }
    }

    let id = model.id.as_str();
    let table: Option<[i64; 4]> = if id.contains("2.5-pro") {
        Some([128, 2048, 8192, 32768])
    } else if id.contains("2.5-flash-lite") {
        Some([512, 2048, 8192, 24576])
    } else if id.contains("2.5-flash") {
        Some([128, 2048, 8192, 24576])
    } else {
        None
    };
    match table {
        Some([minimal, low, medium, high]) => match effort {
            ThinkingLevel::Minimal => Some(minimal),
            ThinkingLevel::Low => Some(low),
            ThinkingLevel::Medium => Some(medium),
            ThinkingLevel::High => Some(high),
            // Pi `budgets[xhigh]` / `budgets[max]` are `undefined` → omit.
            ThinkingLevel::Xhigh | ThinkingLevel::Max => None,
        },
        None => Some(-1),
    }
}
