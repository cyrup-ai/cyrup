//! Request encoding — the thinking lowering: `thinkingConfig` construction, the discrete
//! `thinkingLevel` vs token-`thinkingBudget` split and the budget table (Pi `streamSimple` +
//! `getGoogleBudget`, google-generative-ai.ts:314-344,430-469, and the shared level resolution in
//! google-shared.ts:48-111, both @v0.87.1).
//!
//! The supported rungs are DATA, not a per-family table: `resolve_google_thinking_level` reads
//! `model.thinking_level_map` exactly as Pi's `resolveGoogleThinkingLevel` does
//! (google-shared.ts:48-65), so a catalog row alone decides which Google level a pi rung sends.
//! The id probes survive only to select the WIRE FORMAT (`usesGoogleThinkingLevel`,
//! google-shared.ts:72-83).

use super::capabilities::uses_google_thinking_level;
use super::options::{GoogleThinking, GoogleThinkingLevel};
use crate::api::compat::thinking_level_key;
use crate::collection::clamp_thinking_level;
use crate::model::Model;
use crate::utils::simple_options::ThinkingBudgets;
use cyrup_core::ModelThinkingLevel;
use serde_json::{Map, Value, json};
use std::fmt;

/// A pi rung resolved through `model.thinkingLevelMap` to one of Google's four real levels (Pi
/// `ResolvedGoogleThinkingLevel = Exclude<ThinkingLevel, "xhigh" | "max">`, google-shared.ts:36).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResolvedGoogleThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
}

/// A rung whose `thinkingLevelMap` entry does not name a Google level.
///
/// `[CYRUP-DELTA]` — google-shared.ts:61-63 `throw new Error(...)`. cyrup has no unwinding path
/// through the request encoder, so the same condition is a typed `Err` that the driver turns into a
/// failed turn. The `Display` text is byte-identical to pi's thrown message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnsupportedThinkingLevel(pub String);

impl fmt::Display for UnsupportedThinkingLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Resolve a pi rung to a Google level through `model.thinkingLevelMap` (Pi
/// `resolveGoogleThinkingLevel`, google-shared.ts:48-65). An absent entry falls back to the rung's
/// own name; a mapped string is lowercased; anything else is unsupported.
pub(super) fn resolve_google_thinking_level(
    model: &Model,
    level: ModelThinkingLevel,
) -> Result<ResolvedGoogleThinkingLevel, UnsupportedThinkingLevel> {
    let key = thinking_level_key(level);
    // `Option<&Option<String>>`: outer = entry present, inner = non-null value. Pi's `mapped` is
    // `string | null | undefined` and all three cases differ (google-shared.ts:52-53).
    let entry = model.thinking_level_map.as_ref().and_then(|m| m.get(key));
    let resolved = match entry {
        Some(Some(v)) => v.to_lowercase(),
        // `typeof mapped === "string"` is false for both `null` and `undefined` → the rung's own
        // name is resolved instead (google-shared.ts:53).
        _ => key.to_string(),
    };
    match resolved.as_str() {
        "minimal" => Ok(ResolvedGoogleThinkingLevel::Minimal),
        "low" => Ok(ResolvedGoogleThinkingLevel::Low),
        "medium" => Ok(ResolvedGoogleThinkingLevel::Medium),
        "high" => Ok(ResolvedGoogleThinkingLevel::High),
        _ => {
            // `String(mapped)` (google-shared.ts:62) renders the two non-string cases verbatim.
            let mapped = match entry {
                Some(Some(v)) => v.as_str(),
                Some(None) => "null",
                None => "undefined",
            };
            Err(UnsupportedThinkingLevel(format!(
                "Unsupported Google thinking level mapping for {}/{}: {key} -> {mapped}",
                model.provider, model.id
            )))
        }
    }
}

/// The Google wire level for a resolved rung (Pi `toGoogleThinkingLevel`, google-shared.ts:85-96).
pub(super) fn to_google_thinking_level(level: ResolvedGoogleThinkingLevel) -> GoogleThinkingLevel {
    match level {
        ResolvedGoogleThinkingLevel::Minimal => GoogleThinkingLevel::Minimal,
        ResolvedGoogleThinkingLevel::Low => GoogleThinkingLevel::Low,
        ResolvedGoogleThinkingLevel::Medium => GoogleThinkingLevel::Medium,
        ResolvedGoogleThinkingLevel::High => GoogleThinkingLevel::High,
    }
}

/// Build `thinkingConfig` (Pi `streamSimple` thinking lowering + `buildParams` thinking branch,
/// google-generative-ai.ts:317-344,402-412). `None` omits the field entirely.
pub(super) fn thinking_config(
    model: &Model,
    reasoning: ModelThinkingLevel,
) -> Result<Option<Value>, UnsupportedThinkingLevel> {
    if !reasoning.is_on() {
        // streamSimple `!options.reasoning` path → `thinking: { enabled: false }`, which lowers to
        // the model's disabled-thinking config (google-generative-ai.ts:317-319,410-412).
        return Ok(Some(disabled_thinking_config(model)?));
    }

    // streamSimple reasoning path: clamp, and a rung that clamps all the way to `off` disables
    // thinking rather than climbing back up (google-generative-ai.ts:321-324).
    let clamped = clamp_thinking_level(model, reasoning);
    if !clamped.is_on() {
        return Ok(Some(disabled_thinking_config(model)?));
    }
    let resolved = resolve_google_thinking_level(model, clamped)?;

    let mut cfg = Map::new();
    cfg.insert("includeThoughts".to_string(), json!(true));
    if uses_google_thinking_level(model) {
        cfg.insert(
            "thinkingLevel".to_string(),
            json!(to_google_thinking_level(resolved).as_wire()),
        );
    } else {
        cfg.insert(
            "thinkingBudget".to_string(),
            json!(google_budget(model, resolved, None)),
        );
    }
    Ok(Some(Value::Object(cfg)))
}

/// Lower a direct `GoogleOptions.thinking` override to `thinkingConfig` (1:1 with Pi `buildParams`,
/// google-generative-ai.ts:402-412). When `enabled`, `level` wins over `budgetTokens`; otherwise the
/// model's disabled-thinking config. The outer `model.reasoning` guard is applied by the caller,
/// mirroring Pi's `options.thinking?.enabled && model.reasoning` / `model.reasoning && … !enabled`.
/// `level` here is already resolved — pi's `buildParams` likewise reads the level `streamSimple`
/// computed, not a pi rung.
pub(super) fn thinking_config_override(
    model: &Model,
    thinking: &GoogleThinking,
) -> Result<Option<Value>, UnsupportedThinkingLevel> {
    if thinking.enabled {
        let mut cfg = Map::new();
        cfg.insert("includeThoughts".to_string(), json!(true));
        if let Some(level) = thinking.level {
            cfg.insert("thinkingLevel".to_string(), json!(level.as_wire()));
        } else if let Some(budget) = thinking.budget_tokens {
            cfg.insert("thinkingBudget".to_string(), json!(budget));
        }
        Ok(Some(Value::Object(cfg)))
    } else {
        Ok(Some(disabled_thinking_config(model)?))
    }
}

/// The disabled-thinking config for a reasoning model (Pi `getDisabledGoogleThinkingConfig`,
/// google-shared.ts:102-111). A token-budget model disables with `thinkingBudget: 0`; a level model
/// whose catalog row keeps `off` supported does too; otherwise the lowest rung the row still
/// supports is sent as a level.
fn disabled_thinking_config(model: &Model) -> Result<Value, UnsupportedThinkingLevel> {
    if !uses_google_thinking_level(model) {
        // Gemini 2.x and friends support disabling via thinkingBudget = 0.
        return Ok(json!({ "thinkingBudget": 0 }));
    }
    let fallback = clamp_thinking_level(model, ModelThinkingLevel::Off);
    if !fallback.is_on() {
        return Ok(json!({ "thinkingBudget": 0 }));
    }
    let resolved = resolve_google_thinking_level(model, fallback)?;
    Ok(json!({ "thinkingLevel": to_google_thinking_level(resolved).as_wire() }))
}

/// The token thinking-budget for a RESOLVED level (Pi `getGoogleBudget`,
/// google-generative-ai.ts:430-469). Post-resolution every rung has a budget, so there is no
/// `Option`: a model outside the three Gemini-2.5 families gets `-1` (dynamic), as at `:469`.
fn google_budget(
    model: &Model,
    level: ResolvedGoogleThinkingLevel,
    custom: Option<&ThinkingBudgets>,
) -> i64 {
    if let Some(c) = custom {
        let v = match level {
            ResolvedGoogleThinkingLevel::Minimal => c.minimal,
            ResolvedGoogleThinkingLevel::Low => c.low,
            ResolvedGoogleThinkingLevel::Medium => c.medium,
            ResolvedGoogleThinkingLevel::High => c.high,
        };
        if let Some(v) = v {
            return v as i64;
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
        Some([minimal, low, medium, high]) => match level {
            ResolvedGoogleThinkingLevel::Minimal => minimal,
            ResolvedGoogleThinkingLevel::Low => low,
            ResolvedGoogleThinkingLevel::Medium => medium,
            ResolvedGoogleThinkingLevel::High => high,
        },
        None => -1,
    }
}
