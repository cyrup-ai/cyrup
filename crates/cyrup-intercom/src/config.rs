//! [`IntercomConfig`] + `load_config` + `ask_timeout_ms` — a 1:1 port of `pi-intercom/config.ts`.
//!
//! The load-bearing behavior (`config.test.ts:63-80`): `load_config` validates every key and
//! **fails CLOSED to `inbound_trigger = Never`** on ANY parse/validation error (`config.ts:139-142`)
//! — a corrupt config must never leave inbound auto-triggering at its permissive default.

use std::path::Path;

use crate::identity::ENV_INTERCOM_ASK_TIMEOUT_MS;

/// `DEFAULT_ASK_TIMEOUT_MS = 10 * 60 * 1000` (`config.ts:5`) — an ask may wait as long as a human
/// takes (10 minutes) before it is pruned.
pub const DEFAULT_ASK_TIMEOUT_MS: u64 = 10 * 60 * 1000;

/// `InboundTriggerPolicy` (`config.ts:20`): whether an inbound broker message may auto-start a model
/// turn. Default `Always` (`config.ts:53`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InboundTrigger {
    /// Every inbound message may trigger a turn.
    #[default]
    Always,
    /// Only inbound replies (to an outstanding ask) may trigger a turn.
    Replies,
    /// No inbound message ever auto-triggers a turn.
    Never,
}

/// `IntercomConfig` (`config.ts:22-43`). `broker_command`/`broker_args` are parsed for wire-parity
/// with pi's `config.json`, but cyrup's broker spawn re-execs `current_exe __intercom-broker`
/// (`transport::spawn`) rather than shelling out to `npx tsx`, so they are informational on cyrup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntercomConfig {
    /// Broker command (`config.ts:24`, default `"npx"`). Informational on cyrup (re-exec path).
    pub broker_command: String,
    /// Broker args (`config.ts:26`, default `["--no-install","tsx"]`). Informational on cyrup.
    pub broker_args: Vec<String>,
    /// Require confirmation before non-reply sends from interactive sessions (`config.ts:52`, false).
    pub confirm_send: bool,
    /// Inbound auto-trigger policy (`config.ts:53`, `Always`).
    pub inbound_trigger: InboundTrigger,
    /// Optional custom status suffix (`config.ts:37`).
    pub status: Option<String>,
    /// Enable/disable intercom (`config.ts:54`, true).
    pub enabled: bool,
    /// Show the reply hint in incoming messages (`config.ts:55`, true).
    pub reply_hint: bool,
}

impl Default for IntercomConfig {
    fn default() -> Self {
        Self {
            broker_command: "npx".to_string(),
            broker_args: vec!["--no-install".to_string(), "tsx".to_string()],
            confirm_send: false,
            inbound_trigger: InboundTrigger::Always,
            status: None,
            enabled: true,
            reply_hint: true,
        }
    }
}

/// `getConfigPath` (`config.ts:45-47`): `<intercomDir>/config.json`.
#[must_use]
pub fn config_path(intercom_dir: &Path) -> std::path::PathBuf {
    intercom_dir.join("config.json")
}

/// `loadConfig` (`config.ts:58-143`): read `<intercomDir>/config.json`, validate each key, and
/// **fail CLOSED to `inbound_trigger = Never`** on ANY parse/validation error. A missing file
/// returns the defaults unchanged.
#[must_use]
pub fn load_config(intercom_dir: &Path) -> IntercomConfig {
    let path = config_path(intercom_dir);
    if !path.exists() {
        return IntercomConfig::default();
    }
    match parse_config(&std::fs::read_to_string(&path).unwrap_or_default()) {
        Ok(cfg) => cfg,
        Err(err) => {
            // pi: `console.error(...); return { ...defaults, inboundTrigger: "never" }` (config.ts:140-141).
            tracing::warn!(path = %path.display(), error = %err, "failed to load intercom config; failing closed to inbound_trigger=never");
            IntercomConfig {
                inbound_trigger: InboundTrigger::Never,
                ..IntercomConfig::default()
            }
        }
    }
}

/// The pure validation core (`config.ts:64-138`), split out so the fail-closed table can be tested
/// without touching the filesystem. Every key that is present must have the correct type, else an
/// `Err` is returned (which `load_config` maps to the fail-closed default).
fn parse_config(raw: &str) -> Result<IntercomConfig, String> {
    let parsed: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("config must be valid JSON: {e}"))?;
    let obj = match &parsed {
        serde_json::Value::Object(map) => map,
        _ => return Err("Config must be a JSON object".to_string()),
    };
    let mut config = IntercomConfig::default();

    if let Some(v) = obj.get("brokerCommand") {
        let s = v.as_str().ok_or_else(|| "\"brokerCommand\" must be a string".to_string())?;
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err("\"brokerCommand\" must not be empty".to_string());
        }
        config.broker_command = trimmed.to_string();
    }
    if let Some(v) = obj.get("brokerArgs") {
        let arr = v.as_array().ok_or_else(|| "\"brokerArgs\" must be an array".to_string())?;
        let mut args = Vec::with_capacity(arr.len());
        for arg in arr {
            let s = arg.as_str().ok_or_else(|| "\"brokerArgs\" items must be strings".to_string())?;
            args.push(s.to_string());
        }
        config.broker_args = args;
    }
    if let Some(v) = obj.get("confirmSend") {
        config.confirm_send = v.as_bool().ok_or_else(|| "\"confirmSend\" must be a boolean".to_string())?;
    }
    if let Some(v) = obj.get("enabled") {
        config.enabled = v.as_bool().ok_or_else(|| "\"enabled\" must be a boolean".to_string())?;
    }
    if let Some(v) = obj.get("inboundTrigger") {
        config.inbound_trigger = match v.as_str() {
            Some("always") => InboundTrigger::Always,
            Some("replies") => InboundTrigger::Replies,
            Some("never") => InboundTrigger::Never,
            _ => return Err("\"inboundTrigger\" must be \"always\", \"replies\", or \"never\"".to_string()),
        };
    }
    if let Some(v) = obj.get("replyHint") {
        config.reply_hint = v.as_bool().ok_or_else(|| "\"replyHint\" must be a boolean".to_string())?;
    }
    if let Some(v) = obj.get("status") {
        let s = v.as_str().ok_or_else(|| "\"status\" must be a string".to_string())?;
        config.status = Some(s.to_string());
    }
    Ok(config)
}

/// `getAskTimeoutMs` (`config.ts:7-18`): `CYRUP_INTERCOM_ASK_TIMEOUT_MS` if a positive integer, else
/// [`DEFAULT_ASK_TIMEOUT_MS`]. An empty/unset value uses the default; a present-but-invalid value
/// falls back to the default (pi throws; cyrup logs + defaults so a bad env never bricks a session).
#[must_use]
pub fn ask_timeout_ms() -> u64 {
    ask_timeout_ms_from(|k| std::env::var(k).ok())
}

/// The pure core of [`ask_timeout_ms`].
#[must_use]
pub fn ask_timeout_ms_from(env: impl Fn(&str) -> Option<String>) -> u64 {
    match env(ENV_INTERCOM_ASK_TIMEOUT_MS) {
        None => DEFAULT_ASK_TIMEOUT_MS,
        Some(raw) if raw.trim().is_empty() => DEFAULT_ASK_TIMEOUT_MS,
        Some(raw) => match raw.trim().parse::<u64>() {
            Ok(v) if v > 0 => v,
            _ => {
                tracing::warn!(value = %raw, "CYRUP_INTERCOM_ASK_TIMEOUT_MS must be a positive integer; using default");
                DEFAULT_ASK_TIMEOUT_MS
            }
        },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn defaults_when_no_keys_present() {
        let cfg = parse_config("{}").expect("empty object is valid");
        assert_eq!(cfg, IntercomConfig::default());
        assert_eq!(cfg.inbound_trigger, InboundTrigger::Always);
    }

    #[test]
    fn valid_keys_parse() {
        let cfg = parse_config(
            r#"{"confirmSend":true,"inboundTrigger":"replies","enabled":false,"status":"busy"}"#,
        )
        .expect("valid");
        assert!(cfg.confirm_send);
        assert_eq!(cfg.inbound_trigger, InboundTrigger::Replies);
        assert!(!cfg.enabled);
        assert_eq!(cfg.status.as_deref(), Some("busy"));
    }

    #[test]
    fn bad_type_is_an_error_that_load_maps_to_never() {
        // config.test.ts:63-80 — any validation error fails closed to Never.
        assert!(parse_config(r#"{"confirmSend":"nope"}"#).is_err());
        assert!(parse_config(r#"{"inboundTrigger":"sometimes"}"#).is_err());
        assert!(parse_config("not json at all").is_err());
        assert!(parse_config("[]").is_err());
    }

    #[test]
    fn load_config_fails_closed_to_never_on_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(config_path(dir.path()), "{ this is not valid json").unwrap();
        let cfg = load_config(dir.path());
        assert_eq!(cfg.inbound_trigger, InboundTrigger::Never);
    }

    #[test]
    fn load_config_missing_file_is_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_config(dir.path()), IntercomConfig::default());
    }

    #[test]
    fn ask_timeout_default_and_override() {
        assert_eq!(ask_timeout_ms_from(|_| None), DEFAULT_ASK_TIMEOUT_MS);
        assert_eq!(ask_timeout_ms_from(|_| Some("   ".to_string())), DEFAULT_ASK_TIMEOUT_MS);
        assert_eq!(ask_timeout_ms_from(|_| Some("5000".to_string())), 5000);
        assert_eq!(ask_timeout_ms_from(|_| Some("-3".to_string())), DEFAULT_ASK_TIMEOUT_MS);
        assert_eq!(ask_timeout_ms_from(|_| Some("0".to_string())), DEFAULT_ASK_TIMEOUT_MS);
    }
}
