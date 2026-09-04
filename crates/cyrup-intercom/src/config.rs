//! [`IntercomConfig`] + `load_config` + `ask_timeout_ms` — a 1:1 port of `pi-intercom/config.ts`.
//!
//! The load-bearing behavior: `load_config` validates every key and, from **v0.10.0**, raises
//! `Failed to load intercom config at {path}: {message}` on ANY parse/validation error
//! (`v0.10.1 config.ts:153-155`). It used to fail CLOSED to `inbound_trigger = Never`; upstream
//! removed that fallback because it was indistinguishable from a deliberate restrictive setting and
//! never named the file.

use std::path::Path;

use crate::identity::{ENV_INTERCOM_ASK_TIMEOUT_MS, ENV_INTERCOM_SCOPE_ID};
use crate::transport::protocol::ScopeId;

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
    /// `stableId` (`v0.10.1 config.ts:38-39`) — "Optional stable intercom session ID for
    /// restart-stable addressing". Preferred over the host's per-process session id at registration
    /// (`v0.10.1 index.ts:435` `resolveConfiguredIntercomSessionId`), so a restarted worker keeps
    /// the address its peers already hold. Trimmed non-empty or a hard error; never a silent
    /// default (`v0.10.1 config.ts:141-150`).
    pub stable_id: Option<String>,
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
            stable_id: None,
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

/// `loadConfig` (`v0.10.1 config.ts:58-156`): read `<intercomDir>/config.json` and validate each
/// key. A missing file returns the defaults unchanged; ANY parse/validation error is a hard error
/// naming the path.
///
/// v0.10.0 **replaced** the fail-closed fallback (`console.error(...); return { ...defaults,
/// inboundTrigger: "never" }`) with a throw (`v0.10.1 config.ts:153-155`):
///
/// ```text
/// const message = error instanceof Error ? error.message : String(error);
/// throw new Error(`Failed to load intercom config at ${configPath}: ${message}`, { cause: error });
/// ```
///
/// CHANGELOG 0.10.0: "Surface malformed intercom config errors with path context instead of silently
/// falling back to defaults." The old behaviour was indistinguishable from a deliberate
/// `inboundTrigger: "never"` — intercom connected, listed, sent and asked normally but never
/// auto-triggered, and the only diagnostic was a `tracing::warn!` invisible in the TUI.
///
/// # Errors
/// `Failed to load intercom config at {path}: {message}` for an unreadable or invalid config.
pub fn load_config(intercom_dir: &Path) -> Result<IntercomConfig, String> {
    let path = config_path(intercom_dir);
    if !path.exists() {
        return Ok(IntercomConfig::default());
    }
    // pi's `readFileSync` throw is caught by the same `catch`, so a read failure carries the same
    // path-prefixed message a parse failure does.
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to load intercom config at {}: {e}", path.display()))?;
    parse_config(&raw)
        .map_err(|e| format!("Failed to load intercom config at {}: {e}", path.display()))
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
        let s = v
            .as_str()
            .ok_or_else(|| "\"brokerCommand\" must be a string".to_string())?;
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err("\"brokerCommand\" must not be empty".to_string());
        }
        config.broker_command = trimmed.to_string();
    }
    if let Some(v) = obj.get("brokerArgs") {
        let arr = v
            .as_array()
            .ok_or_else(|| "\"brokerArgs\" must be an array".to_string())?;
        let mut args = Vec::with_capacity(arr.len());
        for arg in arr {
            let s = arg
                .as_str()
                .ok_or_else(|| "\"brokerArgs\" items must be strings".to_string())?;
            args.push(s.to_string());
        }
        config.broker_args = args;
    }
    if let Some(v) = obj.get("confirmSend") {
        config.confirm_send = v
            .as_bool()
            .ok_or_else(|| "\"confirmSend\" must be a boolean".to_string())?;
    }
    if let Some(v) = obj.get("enabled") {
        config.enabled = v
            .as_bool()
            .ok_or_else(|| "\"enabled\" must be a boolean".to_string())?;
    }
    if let Some(v) = obj.get("inboundTrigger") {
        config.inbound_trigger = match v.as_str() {
            Some("always") => InboundTrigger::Always,
            Some("replies") => InboundTrigger::Replies,
            Some("never") => InboundTrigger::Never,
            _ => {
                return Err(
                    "\"inboundTrigger\" must be \"always\", \"replies\", or \"never\"".to_string(),
                );
            }
        };
    }
    if let Some(v) = obj.get("replyHint") {
        config.reply_hint = v
            .as_bool()
            .ok_or_else(|| "\"replyHint\" must be a boolean".to_string())?;
    }
    if let Some(v) = obj.get("status") {
        let s = v
            .as_str()
            .ok_or_else(|| "\"status\" must be a string".to_string())?;
        config.status = Some(s.to_string());
    }
    // `v0.10.1 config.ts:141-150`. Fail-closed on both halves: a non-string is an error, and a
    // present-but-blank value is an error too rather than a silent `undefined`.
    if let Some(v) = obj.get("stableId") {
        let s = v
            .as_str()
            .ok_or_else(|| "\"stableId\" must be a string".to_string())?;
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err("\"stableId\" must not be empty".to_string());
        }
        config.stable_id = Some(trimmed.to_string());
    }
    Ok(config)
}

/// `getAskTimeoutMs` (`config.ts:7-18`): `CYRUP_INTERCOM_ASK_TIMEOUT_MS` if a positive integer, else
/// [`DEFAULT_ASK_TIMEOUT_MS`]. An empty/unset value uses the default. A present-but-invalid value
/// (non-integer, non-positive, e.g. `"abc"`, `"0"`, `"-5"`, `"5000.5"`) is a hard `Err` — matching
/// pi's uncaught `throw new Error("PI_INTERCOM_ASK_TIMEOUT_MS must be a positive integer number of
/// milliseconds")` (`config.ts:14-16`), which crashes every call site rather than silently
/// substituting a default.
///
/// # Errors
/// Returns `Err` when `CYRUP_INTERCOM_ASK_TIMEOUT_MS` is set to a value that is not a positive
/// integer number of milliseconds.
pub fn ask_timeout_ms() -> Result<u64, String> {
    ask_timeout_ms_from(|k| std::env::var(k).ok())
}

/// The pure core of [`ask_timeout_ms`].
///
/// # Errors
/// See [`ask_timeout_ms`].
pub fn ask_timeout_ms_from(env: impl Fn(&str) -> Option<String>) -> Result<u64, String> {
    match env(ENV_INTERCOM_ASK_TIMEOUT_MS) {
        None => Ok(DEFAULT_ASK_TIMEOUT_MS),
        Some(raw) if raw.trim().is_empty() => Ok(DEFAULT_ASK_TIMEOUT_MS),
        Some(raw) => match raw.trim().parse::<u64>() {
            Ok(v) if v > 0 => Ok(v),
            _ => Err(format!(
                "{ENV_INTERCOM_ASK_TIMEOUT_MS} must be a positive integer number of milliseconds"
            )),
        },
    }
}

/// `getIntercomScopeId()` (`v0.13.0 config.ts:21-24`):
///
/// ```text
/// const scopeId = env[INTERCOM_SCOPE_ID_ENV]?.trim();
/// return scopeId ? scopeId : undefined;
/// ```
///
/// Trimmed; blank is UNSCOPED, not an error — unlike [`ask_timeout_ms`], a scope has no malformed
/// *shape* to reject. Fatality for a bad scope lives on the BROKER side, where a non-string
/// `scopeId` on the register frame is a protocol error (`normalizeScopeId`,
/// `v0.13.0 broker/broker.ts:133-142`).
///
/// Deliberately **not** an [`IntercomConfig`] key: upstream reads it from the environment only,
/// because `config.json` is machine-global and a scope stored there would apply to every session on
/// the box.
#[must_use]
pub fn intercom_scope_id() -> Option<ScopeId> {
    intercom_scope_id_from(|k| std::env::var(k).ok())
}

/// The pure core of [`intercom_scope_id`] — the same `_from(env)` seam
/// [`ask_timeout_ms_from`] uses, and upstream's own (`env: NodeJS.ProcessEnv = process.env`,
/// `v0.13.0 config.ts:21`).
#[must_use]
pub fn intercom_scope_id_from(env: impl Fn(&str) -> Option<String>) -> Option<ScopeId> {
    env(ENV_INTERCOM_SCOPE_ID)
        .as_deref()
        .and_then(ScopeId::parse)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    /// ICOM-055 — `getIntercomScopeId` (`v0.13.0 config.ts:21-24`). Unset and whitespace-only are
    /// the SAME answer (unscoped); a value is trimmed. Nothing here can fail: a bad scope is the
    /// broker's problem, not the resolver's.
    #[test]
    fn intercom_scope_id_trims_and_treats_blank_as_unscoped() {
        let of = |v: Option<&str>| {
            intercom_scope_id_from(|k| {
                assert_eq!(k, ENV_INTERCOM_SCOPE_ID);
                v.map(str::to_string)
            })
        };
        assert_eq!(of(None), None);
        assert_eq!(of(Some("")), None);
        assert_eq!(of(Some("  \t ")), None);
        assert_eq!(
            of(Some("  alpha  ")).as_ref().map(ScopeId::as_str),
            Some("alpha")
        );
    }

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

    /// `v0.10.1 config.ts:153-155` (v0.10.0). A malformed config must NAME ITS PATH and fail, not
    /// silently become `inboundTrigger: "never"` — that fallback made a typo indistinguishable from
    /// a deliberate restrictive setting.
    #[test]
    fn load_config_errors_with_the_path_on_a_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_path(dir.path());
        std::fs::write(&path, "{ this is not valid json").unwrap();
        let err = load_config(dir.path()).expect_err("a corrupt config is a hard error");
        assert!(
            err.starts_with(&format!(
                "Failed to load intercom config at {}: ",
                path.display()
            )),
            "must name the config path: {err}"
        );
        assert!(
            err.contains("valid JSON"),
            "must carry the underlying parse message: {err}"
        );
    }

    #[test]
    fn load_config_missing_file_is_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load_config(dir.path()).expect("missing file is not an error"),
            IntercomConfig::default()
        );
    }

    /// `v0.10.1 config.ts:141-150`. `stableId` used to be accepted and silently ignored, which reads
    /// as a working feature.
    #[test]
    fn stable_id_is_parsed_trimmed_and_fails_closed() {
        assert_eq!(
            parse_config(r#"{"stableId":"  worker-a  "}"#)
                .expect("valid")
                .stable_id
                .as_deref(),
            Some("worker-a")
        );
        assert!(parse_config("{}").expect("valid").stable_id.is_none());
        assert_eq!(
            parse_config(r#"{"stableId":7}"#).expect_err("non-string"),
            "\"stableId\" must be a string"
        );
        assert_eq!(
            parse_config(r#"{"stableId":"   "}"#).expect_err("blank"),
            "\"stableId\" must not be empty"
        );
    }

    #[test]
    fn ask_timeout_default_and_override() {
        assert_eq!(
            ask_timeout_ms_from(|_| None).unwrap(),
            DEFAULT_ASK_TIMEOUT_MS
        );
        assert_eq!(
            ask_timeout_ms_from(|_| Some("   ".to_string())).unwrap(),
            DEFAULT_ASK_TIMEOUT_MS
        );
        assert_eq!(
            ask_timeout_ms_from(|_| Some("5000".to_string())).unwrap(),
            5000
        );
    }

    #[test]
    fn ask_timeout_invalid_value_is_a_hard_error_not_a_silent_default() {
        // config.ts:14-16 — pi throws on a non-positive/non-safe-integer value; it must never be
        // silently swallowed into DEFAULT_ASK_TIMEOUT_MS (the pre-fix cyrup behavior). This test
        // fails against that pre-fix behavior because `ask_timeout_ms_from` used to return a plain
        // `u64` that silently defaulted instead of an `Err`.
        assert!(ask_timeout_ms_from(|_| Some("-3".to_string())).is_err());
        assert!(ask_timeout_ms_from(|_| Some("0".to_string())).is_err());
        assert!(ask_timeout_ms_from(|_| Some("abc".to_string())).is_err());
        assert!(ask_timeout_ms_from(|_| Some("5000.5".to_string())).is_err());
    }
}
