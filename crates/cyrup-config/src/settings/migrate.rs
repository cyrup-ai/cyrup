//! Legacy settings-shape migrations, run on every parse (Pi `migrateSettings`,
//! settings-manager.ts:376-435).

use serde_json::{Map, Value};

/// Migrate legacy settings shapes in place (Pi `migrateSettings`, settings-manager.ts:376-435):
/// 1. `queueMode` → `steeringMode`
/// 2. legacy `websockets` boolean → `transport` enum (`websocket`/`sse`)
/// 3. old `skills` object (`{enableSkillCommands, customDirectories}`) → array form
/// 4. `retry.maxDelayMs` → `retry.provider.maxRetryDelayMs`
pub fn migrate_settings(settings: &mut Map<String, Value>) {
    // 1. queueMode -> steeringMode (only when steeringMode is absent; otherwise leave as-is).
    if settings.contains_key("queueMode")
        && !settings.contains_key("steeringMode")
        && let Some(v) = settings.remove("queueMode")
    {
        settings.insert("steeringMode".to_string(), v);
    }

    // 2. websockets boolean -> transport enum
    if !settings.contains_key("transport")
        && let Some(Value::Bool(b)) = settings.get("websockets").cloned()
    {
        settings.insert(
            "transport".to_string(),
            Value::String(if b {
                "websocket".to_string()
            } else {
                "sse".to_string()
            }),
        );
        settings.remove("websockets");
    }

    // 3. skills object -> array
    if let Some(Value::Object(skills)) = settings.get("skills").cloned() {
        if let Some(enable) = skills.get("enableSkillCommands")
            && !settings.contains_key("enableSkillCommands")
        {
            settings.insert("enableSkillCommands".to_string(), enable.clone());
        }
        match skills.get("customDirectories") {
            Some(Value::Array(dirs)) if !dirs.is_empty() => {
                settings.insert("skills".to_string(), Value::Array(dirs.clone()));
            }
            _ => {
                settings.remove("skills");
            }
        }
    }

    // 4. retry.maxDelayMs -> retry.provider.maxRetryDelayMs
    if let Some(Value::Object(retry)) = settings.get_mut("retry") {
        if let Some(Value::Number(max_delay)) = retry.get("maxDelayMs").cloned() {
            let provider_has_max = retry
                .get("provider")
                .and_then(Value::as_object)
                .and_then(|p| p.get("maxRetryDelayMs"))
                .is_some_and(|v| !v.is_null());
            if !provider_has_max {
                let mut provider = retry
                    .get("provider")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                provider.insert("maxRetryDelayMs".to_string(), Value::Number(max_delay));
                retry.insert("provider".to_string(), Value::Object(provider));
            }
        }
        retry.remove("maxDelayMs");
    }
}
