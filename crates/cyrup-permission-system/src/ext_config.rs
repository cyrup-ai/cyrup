//! The extension config `config.json` (port of pi `extension-config.ts:10-31`). Two toggles
//! (`debug`, `yoloMode`) plus the forwarded-prompt timeout. JSONC (trailing commas / comments
//! allowed). A missing file is the all-defaults case (not an error); a malformed file falls back to
//! defaults (matching the subagents `config.json` convention in this binary).

use std::path::Path;

use crate::jsonc;

/// pi `PermissionSystemExtensionConfig` (`extension-config.ts:10-14`); defaults `{false, false,
/// Some(30)}` (`:27-31`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionConfig {
    /// pi `debug` — `strict === true` (`:81`).
    pub debug: bool,
    /// pi `yoloMode` — `strict === true` (`:82`). (Auto-approve of `ask` is P-1/P-3 territory; carried
    /// now for shape parity.)
    pub yolo_mode: bool,
    /// pi `forwardedPromptTimeoutSeconds`: `null`/`false` → `None` (indefinite); a finite `> 0`
    /// number → that; else `Some(30)` (`:74-78`). Consumed by forwarding (P-4).
    pub forwarded_prompt_timeout_seconds: Option<u64>,
}

impl Default for ExtensionConfig {
    fn default() -> Self {
        // pi `DEFAULT_EXTENSION_CONFIG` (`extension-config.ts:27-31`).
        ExtensionConfig { debug: false, yolo_mode: false, forwarded_prompt_timeout_seconds: Some(30) }
    }
}

impl ExtensionConfig {
    /// Load from `path` (JSONC). Absent → defaults; malformed → defaults (with a stderr warning, so a
    /// hand-edited typo is discoverable — matching `subagent_config.rs`).
    #[must_use]
    pub fn load(path: &Path) -> ExtensionConfig {
        let Ok(text) = std::fs::read_to_string(path) else {
            return ExtensionConfig::default();
        };
        match jsonc::parse(&text) {
            Ok(value) => Self::normalize(&value),
            Err(err) => {
                eprintln!(
                    "cyrup-permission-system: warning: {} is not valid config JSON ({err}); using defaults",
                    path.display()
                );
                ExtensionConfig::default()
            }
        }
    }

    /// pi `normalizePermissionSystemConfig` (`extension-config.ts:69-85`).
    #[must_use]
    pub fn normalize(value: &serde_json::Value) -> ExtensionConfig {
        let default = ExtensionConfig::default();
        let debug = value.get("debug").and_then(serde_json::Value::as_bool).unwrap_or(false);
        let yolo_mode = value.get("yoloMode").and_then(serde_json::Value::as_bool).unwrap_or(false);

        let forwarded = match value.get("forwardedPromptTimeoutSeconds") {
            // `null` / `false` → indefinite.
            Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::Bool(false)) => None,
            Some(v) => match v.as_f64() {
                // finite, > 0 → that value (floored to whole seconds).
                Some(n) if n.is_finite() && n > 0.0 => Some(n as u64),
                _ => default.forwarded_prompt_timeout_seconds,
            },
            None => default.forwarded_prompt_timeout_seconds,
        };

        ExtensionConfig { debug, yolo_mode, forwarded_prompt_timeout_seconds: forwarded }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn absent_is_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(ExtensionConfig::load(&dir.path().join("nope.json")), ExtensionConfig::default());
    }

    #[test]
    fn null_timeout_is_indefinite_and_bools_parse() {
        let v = serde_json::json!({"debug": true, "yoloMode": true, "forwardedPromptTimeoutSeconds": null});
        let c = ExtensionConfig::normalize(&v);
        assert!(c.debug && c.yolo_mode);
        assert_eq!(c.forwarded_prompt_timeout_seconds, None);
    }

    #[test]
    fn finite_positive_timeout_kept_else_default() {
        assert_eq!(
            ExtensionConfig::normalize(&serde_json::json!({"forwardedPromptTimeoutSeconds": 45}))
                .forwarded_prompt_timeout_seconds,
            Some(45)
        );
        assert_eq!(
            ExtensionConfig::normalize(&serde_json::json!({"forwardedPromptTimeoutSeconds": -5}))
                .forwarded_prompt_timeout_seconds,
            Some(30)
        );
    }
}
