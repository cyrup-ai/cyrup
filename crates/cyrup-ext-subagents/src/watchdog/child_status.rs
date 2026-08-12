//! The child-watchdog control plane — a 1:1 port of `pi-subagents/src/watchdog/child-status.ts`
//! (205 lines @v0.43.0).
//!
//! A subagent runs as its own OS process, so the parent cannot see the child's watchdog state
//! directly. This module is the whole of that seam, and it is two channels in opposite directions:
//!
//! * **Parent -> child, once, at spawn**: [`resolve_child_watchdog_config`] (`:44-73`) projects the
//!   parent's [`ResolvedWatchdogConfig`] onto the flat [`ChildWatchdogConfig`] the child needs,
//!   [`encode_child_watchdog_config`] (`:75-77`) serializes it, and it travels as the single env var
//!   [`CHILD_WATCHDOG_CONFIG_ENV`]. The child decodes it with
//!   [`decode_child_watchdog_config`] (`:135-166`), which is deliberately STRICT — a malformed value
//!   raises rather than silently disabling the watchdog, because a silently-disabled reviewer is
//!   indistinguishable from a clean review.
//! * **Child -> parent, continuously**: the child writes [`ChildWatchdogStatusEvent`] records to its
//!   own stdout (`register-child.ts:48-56`), the parent filters them out of the NDJSON stream with
//!   [`is_child_watchdog_status_event`] (`:168-181`) and folds them with
//!   [`accept_child_watchdog_event`] (`:188-205`). That fold is what keeps a settled child open: the
//!   parent's `watchdogTailTimer` (`execution.ts:608-621`, `subagent-runner.ts:860-871`) will not
//!   close a run while [`child_watchdog_is_active`] (`:183-186`) is true.
//!
//! Two rules in the fold are easy to lose and are asserted below. **Identity filtering is per-field
//! and only when the parent asked for it** (`:194-198`): a parent that passes `run_id: None` accepts
//! events from any run, but a parent that passes `Some` rejects a mismatch outright — including an
//! event that carries no `runId` at all. **Sequence numbers are strictly increasing** (`:199`):
//! `seq <= current.seq` is dropped, so a re-delivered or out-of-order event can never walk the phase
//! backwards.
//!
//! [CYRUP-DELTA] `enabled: false` decodes to `None` — "no child watchdog" — rather than to a
//! disabled config, exactly as upstream's `if (parsed.enabled === false) return undefined` (`:138`)
//! does; the distinction matters because `permission-arbiter.ts:71` treats `None` as
//! "arbiter unavailable, fail closed".

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{ResolvedWatchdogConfig, ThinkingSetting, WatchdogLspConfig};

/// `CHILD_WATCHDOG_CONFIG_ENV` (`child-status.ts:3`), rebranded `PI_SUBAGENT_` ->
/// `CYRUP_SUBAGENT_` like the rest of this crate's 39-var child env family.
pub const CHILD_WATCHDOG_CONFIG_ENV: &str = "CYRUP_SUBAGENT_WATCHDOG_CHILD_CONFIG";

/// `CHILD_WATCHDOG_STATUS_EVENT` (`child-status.ts:4`) — the `type` discriminator of the child's
/// stdout status records. Not rebranded: it is a wire value both ends compare against.
pub const CHILD_WATCHDOG_STATUS_EVENT: &str = "subagent.watchdog.status";

/// `CHILD_WATCHDOG_PHASES` (`child-status.ts:6-7`), in upstream order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChildWatchdogPhase {
    /// No review in flight.
    #[serde(rename = "idle")]
    Idle,
    /// A review model call is running.
    #[serde(rename = "reviewing")]
    Reviewing,
    /// An auto-follow turn is running.
    #[serde(rename = "autofollow")]
    Autofollow,
    /// The child is finishing up after its last review.
    #[serde(rename = "settling")]
    Settling,
    /// A review missed the agent-end catch-up window.
    #[serde(rename = "stale")]
    Stale,
    /// A review failed.
    #[serde(rename = "failed")]
    Failed,
}

impl ChildWatchdogPhase {
    /// Every phase, in upstream's tuple order.
    pub const ALL: &'static [ChildWatchdogPhase] = &[
        ChildWatchdogPhase::Idle,
        ChildWatchdogPhase::Reviewing,
        ChildWatchdogPhase::Autofollow,
        ChildWatchdogPhase::Settling,
        ChildWatchdogPhase::Stale,
        ChildWatchdogPhase::Failed,
    ];

    /// The wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ChildWatchdogPhase::Idle => "idle",
            ChildWatchdogPhase::Reviewing => "reviewing",
            ChildWatchdogPhase::Autofollow => "autofollow",
            ChildWatchdogPhase::Settling => "settling",
            ChildWatchdogPhase::Stale => "stale",
            ChildWatchdogPhase::Failed => "failed",
        }
    }

    /// `(CHILD_WATCHDOG_PHASES as readonly string[]).includes(value)` (`child-status.ts:181`).
    #[must_use]
    pub fn parse(value: &str) -> Option<ChildWatchdogPhase> {
        ChildWatchdogPhase::ALL.iter().copied().find(|phase| phase.as_str() == value)
    }
}

/// `ChildWatchdogConfig` (`child-status.ts:9-23`) — the flat, self-contained config a child needs.
///
/// Serialization omits every absent optional (upstream's `...(x ? { x } : {})` spreads), so the
/// encoded env value is byte-identical to upstream's for the same inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildWatchdogConfig {
    /// Always `true` once decoded — `false` decodes to `None` instead.
    pub enabled: bool,
    /// The parent's run id, for event correlation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The persona the child runs as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The child's index within a fanout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_index: Option<u64>,
    /// How long the parent holds the run open for the child's watchdog tail.
    pub watchdog_tail_timeout_ms: u64,
    /// The child's own agent-end review budget.
    pub agent_end_timeout_ms: u64,
    /// Emission-guard ceiling (`null` upstream = unbounded).
    pub max_warnings: Option<u32>,
    /// An explicit review model for this child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// An explicit reasoning level for this child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingSetting>,
    /// The LSP policy, copied whole.
    pub lsp: WatchdogLspConfig,
    /// Auto-follow blockers inside the child.
    pub auto_follow_blockers: bool,
    /// Attempt ceiling (`null` upstream = unbounded).
    pub auto_follow_max_attempts: Option<u32>,
    /// Consecutive identical blockers that declare a stalemate.
    pub stalemate_repeats: u32,
}

/// `ChildWatchdogStatusEvent` (`child-status.ts:25-35`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildWatchdogStatusEvent {
    /// Always [`CHILD_WATCHDOG_STATUS_EVENT`].
    #[serde(rename = "type")]
    pub event_type: String,
    /// The run this child belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The persona.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The fanout index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_index: Option<u64>,
    /// The same index under the chain-step spelling (`register-child.ts:52` emits both).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_index: Option<u64>,
    /// Strictly increasing per child.
    pub seq: u64,
    /// The child's current phase.
    pub phase: ChildWatchdogPhase,
    /// Epoch milliseconds.
    pub ts: i64,
    /// Whether an auto-follow turn is queued but not yet started.
    pub follow_up_pending: bool,
    /// Free text for `failed`/`stale`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `ChildWatchdogStateSnapshot` (`child-status.ts:37-44`) — the parent's folded view of one child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildWatchdogStateSnapshot {
    /// Last accepted phase.
    pub phase: ChildWatchdogPhase,
    /// Last accepted sequence number.
    pub seq: u64,
    /// The `ts` of the last accepted event.
    pub last_update: i64,
    /// Last accepted `followUpPending`.
    pub follow_up_pending: bool,
    /// Last accepted `reason`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Set by the parent when its own tail timer fired (`execution.ts:614-616`), not by the child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timed_out: Option<bool>,
}

/// The identity a parent filters a child's status events against
/// (`acceptChildWatchdogEvent`'s `runId`/`agent`/`childIndex` arguments).
#[derive(Debug, Clone, Default)]
pub struct ChildWatchdogIdentity {
    /// Require this run id when `Some`.
    pub run_id: Option<String>,
    /// Require this agent name when `Some`.
    pub agent: Option<String>,
    /// Require this child index when `Some`.
    pub child_index: Option<u64>,
}

/// `resolveChildWatchdogConfig` (`child-status.ts:46-73`).
///
/// The enable decision is `config.enabled && (override?.enabled ?? config.children.enabled)`
/// (`:52`) — the MASTER switch gates everything, and a per-agent override can only decide the
/// children layer beneath it, never re-enable a watchdog the master switch turned off.
#[must_use]
pub fn resolve_child_watchdog_config(
    config: &ResolvedWatchdogConfig,
    agent: Option<&str>,
    run_id: Option<&str>,
    child_index: Option<u64>,
) -> Option<ChildWatchdogConfig> {
    let override_config = agent.and_then(|agent| config.children.overrides.get(agent));
    let enabled = config.enabled
        && override_config
            .and_then(|o| o.enabled)
            .unwrap_or(config.children.enabled);
    if !enabled {
        return None;
    }
    let model = override_config
        .and_then(|o| o.model.clone())
        .or_else(|| config.children.model.clone());
    let thinking = override_config
        .and_then(|o| o.thinking.clone())
        .or_else(|| config.children.thinking.clone());
    Some(ChildWatchdogConfig {
        enabled: true,
        run_id: run_id.map(str::to_string),
        agent: agent.map(str::to_string),
        child_index,
        watchdog_tail_timeout_ms: config.children.watchdog_tail_timeout_ms,
        agent_end_timeout_ms: config.agent_end_timeout_ms,
        max_warnings: config.max_warnings,
        model,
        thinking,
        lsp: config.lsp.clone(),
        auto_follow_blockers: config.children.auto_follow.blockers,
        auto_follow_max_attempts: config.children.auto_follow.max_attempts,
        stalemate_repeats: config.children.auto_follow.stalemate_repeats,
    })
}

/// `encodeChildWatchdogConfig` (`child-status.ts:75-77`) — `None` in, `None` out, so the caller
/// simply omits the env var when there is no child watchdog.
#[must_use]
pub fn encode_child_watchdog_config(config: Option<&ChildWatchdogConfig>) -> Option<String> {
    config.and_then(|config| serde_json::to_string(config).ok())
}

/// The strict-decode failure, carrying upstream's verbatim message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildWatchdogConfigError(pub String);

impl std::fmt::Display for ChildWatchdogConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ChildWatchdogConfigError {}

fn invalid(message: impl Into<String>) -> ChildWatchdogConfigError {
    ChildWatchdogConfigError(message.into())
}

/// `childConfigObject` (`child-status.ts:79-82`).
fn config_object<'a>(
    value: Option<&'a Value>,
    field: &str,
) -> Result<&'a serde_json::Map<String, Value>, ChildWatchdogConfigError> {
    match value {
        Some(Value::Object(map)) => Ok(map),
        _ => Err(invalid(format!(
            "Invalid child watchdog config: {field} must be an object."
        ))),
    }
}

/// `childConfigOptionalString` (`child-status.ts:84-90`): absent is fine, present-but-blank is not.
fn optional_string(
    input: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<String>, ChildWatchdogConfigError> {
    let Some(value) = input.get(field) else {
        return Ok(None);
    };
    match value {
        Value::String(s) if !s.trim().is_empty() => Ok(Some(s.clone())),
        _ => Err(invalid(format!(
            "Invalid child watchdog config: {field} must be a non-empty string."
        ))),
    }
}

/// `childConfigOptionalIndex` (`child-status.ts:92-98`).
fn optional_index(
    input: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<u64>, ChildWatchdogConfigError> {
    let Some(value) = input.get(field) else {
        return Ok(None);
    };
    value.as_u64().ok_or_else(|| {
        invalid(format!(
            "Invalid child watchdog config: {field} must be a non-negative integer."
        ))
    })
    .map(Some)
}

/// `childConfigPositiveInteger` (`child-status.ts:100-105`).
fn positive_integer(
    input: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<u64, ChildWatchdogConfigError> {
    input
        .get(field)
        .and_then(Value::as_u64)
        .filter(|value| *value >= 1)
        .ok_or_else(|| {
            invalid(format!(
                "Invalid child watchdog config: {field} must be a positive integer."
            ))
        })
}

/// `childConfigNullableNonNegativeInteger` (`child-status.ts:107-113`): an explicit JSON `null` is
/// "unbounded"; an ABSENT key is a validation failure, not a default.
fn nullable_non_negative_integer(
    input: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<u32>, ChildWatchdogConfigError> {
    match input.get(field) {
        Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .map(Some)
            .ok_or_else(|| {
                invalid(format!(
                    "Invalid child watchdog config: {field} must be null or a non-negative integer."
                ))
            }),
        None => Err(invalid(format!(
            "Invalid child watchdog config: {field} must be null or a non-negative integer."
        ))),
    }
}

/// `childConfigBoolean` (`child-status.ts:115-120`).
fn config_boolean(
    input: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<bool, ChildWatchdogConfigError> {
    input.get(field).and_then(Value::as_bool).ok_or_else(|| {
        invalid(format!(
            "Invalid child watchdog config: {field} must be a boolean."
        ))
    })
}

/// `childConfigLsp` (`child-status.ts:122-133`) — the four LSP fields, each with its own message.
fn config_lsp(value: Option<&Value>) -> Result<WatchdogLspConfig, ChildWatchdogConfigError> {
    let input = config_object(value, "lsp")?;
    let enabled = input.get("enabled").and_then(Value::as_bool).ok_or_else(|| {
        invalid("Invalid child watchdog config: lsp.enabled must be a boolean.")
    })?;
    let timeout_ms = input
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .filter(|v| *v >= 1)
        .ok_or_else(|| {
            invalid("Invalid child watchdog config: lsp.timeoutMs must be a positive integer.")
        })?;
    let max_files = input
        .get("maxFiles")
        .and_then(Value::as_u64)
        .filter(|v| *v >= 1)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| {
            invalid("Invalid child watchdog config: lsp.maxFiles must be a positive integer.")
        })?;
    let max_diagnostics = input
        .get("maxDiagnostics")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| {
            invalid(
                "Invalid child watchdog config: lsp.maxDiagnostics must be a non-negative integer.",
            )
        })?;
    Ok(WatchdogLspConfig {
        enabled,
        timeout_ms,
        max_files,
        max_diagnostics,
    })
}

/// `decodeChildWatchdogConfig` (`child-status.ts:135-166`).
///
/// An absent/empty raw value is `Ok(None)` ("no child watchdog"), `enabled: false` is likewise
/// `Ok(None)`, and anything else that does not validate is an `Err` carrying upstream's verbatim
/// message — which `permission-arbiter.ts:66-70` surfaces as a fail-closed denial reason.
///
/// # Errors
///
/// Returns [`ChildWatchdogConfigError`] when the value is not JSON, is not an object, or any field
/// fails its own validation.
pub fn decode_child_watchdog_config(
    raw: Option<&str>,
) -> Result<Option<ChildWatchdogConfig>, ChildWatchdogConfigError> {
    let Some(raw) = raw.filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    let parsed: Value = serde_json::from_str(raw)
        .map_err(|e| invalid(format!("Invalid child watchdog config: {e}")))?;
    let parsed = config_object(Some(&parsed), "root")?;
    match parsed.get("enabled") {
        Some(Value::Bool(false)) => return Ok(None),
        Some(Value::Bool(true)) => {}
        _ => {
            return Err(invalid(
                "Invalid child watchdog config: enabled must be true or false.",
            ));
        }
    }
    let thinking = match parsed.get("thinking") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(ThinkingSetting::Level(s.clone())),
        Some(Value::Bool(false)) => Some(ThinkingSetting::Off),
        Some(_) => {
            return Err(invalid(
                "Invalid child watchdog config: thinking must be a string or false.",
            ));
        }
    };
    Ok(Some(ChildWatchdogConfig {
        enabled: true,
        run_id: optional_string(parsed, "runId")?,
        agent: optional_string(parsed, "agent")?,
        child_index: optional_index(parsed, "childIndex")?,
        watchdog_tail_timeout_ms: positive_integer(parsed, "watchdogTailTimeoutMs")?,
        agent_end_timeout_ms: positive_integer(parsed, "agentEndTimeoutMs")?,
        max_warnings: nullable_non_negative_integer(parsed, "maxWarnings")?,
        model: optional_string(parsed, "model")?,
        thinking,
        lsp: config_lsp(parsed.get("lsp"))?,
        auto_follow_blockers: config_boolean(parsed, "autoFollowBlockers")?,
        auto_follow_max_attempts: nullable_non_negative_integer(parsed, "autoFollowMaxAttempts")?,
        stalemate_repeats: u32::try_from(positive_integer(parsed, "stalemateRepeats")?)
            .map_err(|_| {
                invalid("Invalid child watchdog config: stalemateRepeats must be a positive integer.")
            })?,
    }))
}

/// `isChildWatchdogStatusEvent` (`child-status.ts:168-181`) — the parent's NDJSON filter.
///
/// **No non-test caller yet, and the missing caller is not in this module.** Upstream consumes this
/// predicate, [`child_watchdog_is_active`] and [`accept_child_watchdog_event`] together, in the two
/// places that read a child's stdout: `runs/foreground/execution.ts:846-864` and
/// `runs/background/subagent-runner.ts:626-645` (again at `:2711`). Both fold the event into a
/// `childWatchdogState`, then use `childWatchdogIsActive` to arm a WATCHDOG TAIL timer that holds
/// the run open while the child is still reviewing (`execution.ts:584-587`,
/// `subagent-runner.ts:831`) instead of letting the final-drain timer terminate it. cyrup's
/// counterparts are `crate::exec` and `crate::background`, and neither reads a child watchdog
/// status event today, so an armed child that is mid-review can still be drained out from under
/// itself. That is unported wiring in those modules; the three predicates here are faithful ports
/// of `child-status.ts:167-205` and are what it will call.
///
/// Every one of upstream's seven predicates is reproduced: the `type` discriminator, an integral
/// non-negative `seq`, a finite numeric `ts`, a boolean `followUpPending`, and a `phase` that is a
/// string in [`ChildWatchdogPhase::ALL`].
#[must_use]
pub fn is_child_watchdog_status_event(value: &Value) -> bool {
    let Some(event) = value.as_object() else {
        return false;
    };
    event.get("type").and_then(Value::as_str) == Some(CHILD_WATCHDOG_STATUS_EVENT)
        && event.get("seq").and_then(Value::as_u64).is_some()
        && event.get("ts").and_then(Value::as_f64).is_some_and(f64::is_finite)
        && event.get("followUpPending").and_then(Value::as_bool).is_some()
        && event
            .get("phase")
            .and_then(Value::as_str)
            .is_some_and(|phase| ChildWatchdogPhase::parse(phase).is_some())
}

/// `childWatchdogIsActive` (`child-status.ts:183-186`) — the predicate the parent's tail timer
/// consults. `stale` and `failed` are terminal and do NOT hold the run open.
#[must_use]
pub fn child_watchdog_is_active(snapshot: Option<&ChildWatchdogStateSnapshot>) -> bool {
    let Some(snapshot) = snapshot else {
        return false;
    };
    snapshot.follow_up_pending
        || matches!(
            snapshot.phase,
            ChildWatchdogPhase::Reviewing
                | ChildWatchdogPhase::Autofollow
                | ChildWatchdogPhase::Settling
        )
}

/// `acceptChildWatchdogEvent` (`child-status.ts:188-205`) — fold one event, or reject it.
///
/// `None` means "not for us, or not newer"; the caller keeps its existing snapshot unchanged. The
/// index comparison uses `event.childIndex ?? event.stepIndex` (`:196`), so a chain step that only
/// reports `stepIndex` still matches a parent watching `child_index`.
#[must_use]
pub fn accept_child_watchdog_event(
    current: Option<&ChildWatchdogStateSnapshot>,
    event: &ChildWatchdogStatusEvent,
    identity: &ChildWatchdogIdentity,
) -> Option<ChildWatchdogStateSnapshot> {
    if identity.run_id.is_some() && event.run_id != identity.run_id {
        return None;
    }
    if identity.agent.is_some() && event.agent != identity.agent {
        return None;
    }
    let event_index = event.child_index.or(event.step_index);
    if identity.child_index.is_some() && event_index != identity.child_index {
        return None;
    }
    if let Some(current) = current
        && event.seq <= current.seq
    {
        return None;
    }
    Some(ChildWatchdogStateSnapshot {
        phase: event.phase,
        seq: event.seq,
        last_update: event.ts,
        follow_up_pending: event.follow_up_pending,
        reason: event.reason.clone().filter(|reason| !reason.is_empty()),
        timed_out: None,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use crate::watchdog::settings::default_watchdog_config;
    use crate::watchdog::types::WatchdogChildOverrideConfig;
    use serde_json::json;

    fn enabled_parent() -> ResolvedWatchdogConfig {
        let mut config = default_watchdog_config();
        config.enabled = true;
        config.children.enabled = true;
        config
    }

    fn event(seq: u64, phase: ChildWatchdogPhase) -> ChildWatchdogStatusEvent {
        ChildWatchdogStatusEvent {
            event_type: CHILD_WATCHDOG_STATUS_EVENT.to_string(),
            run_id: Some("run-1".into()),
            agent: Some("reviewer".into()),
            child_index: Some(0),
            step_index: Some(0),
            seq,
            phase,
            ts: 1_000 + i64::try_from(seq).unwrap_or(0),
            follow_up_pending: false,
            reason: None,
        }
    }

    #[test]
    fn the_master_switch_gates_a_per_agent_override() {
        let mut config = enabled_parent();
        config.enabled = false;
        config
            .children
            .overrides
            .insert("reviewer".into(), WatchdogChildOverrideConfig {
                enabled: Some(true),
                ..Default::default()
            });
        assert!(
            resolve_child_watchdog_config(&config, Some("reviewer"), None, None).is_none(),
            "an override must not re-enable a watchdog the master switch turned off"
        );
    }

    #[test]
    fn a_per_agent_override_can_disable_one_agent_only() {
        let mut config = enabled_parent();
        config
            .children
            .overrides
            .insert("reviewer".into(), WatchdogChildOverrideConfig {
                enabled: Some(false),
                ..Default::default()
            });
        assert!(resolve_child_watchdog_config(&config, Some("reviewer"), None, None).is_none());
        assert!(resolve_child_watchdog_config(&config, Some("worker"), None, None).is_some());
    }

    #[test]
    fn an_override_model_and_thinking_beat_the_children_defaults() {
        let mut config = enabled_parent();
        config.children.model = Some("anthropic/base".into());
        config.children.thinking = Some(ThinkingSetting::Level("low".into()));
        config
            .children
            .overrides
            .insert("reviewer".into(), WatchdogChildOverrideConfig {
                model: Some("openai/strong".into()),
                thinking: Some(ThinkingSetting::Off),
                ..Default::default()
            });
        let resolved =
            resolve_child_watchdog_config(&config, Some("reviewer"), Some("r1"), Some(2)).unwrap();
        assert_eq!(resolved.model.as_deref(), Some("openai/strong"));
        assert_eq!(resolved.thinking, Some(ThinkingSetting::Off));
        assert_eq!(resolved.run_id.as_deref(), Some("r1"));
        assert_eq!(resolved.child_index, Some(2));
        // An unnamed agent falls back to the children block wholesale.
        let plain = resolve_child_watchdog_config(&config, None, None, None).unwrap();
        assert_eq!(plain.model.as_deref(), Some("anthropic/base"));
        assert_eq!(plain.thinking, Some(ThinkingSetting::Level("low".into())));
    }

    #[test]
    fn encode_decode_round_trips_and_omits_absent_optionals() {
        let config = resolve_child_watchdog_config(&enabled_parent(), None, None, None).unwrap();
        let encoded = encode_child_watchdog_config(Some(&config)).unwrap();
        assert!(!encoded.contains("runId"), "absent optionals are omitted, not null");
        assert!(!encoded.contains("\"agent\""));
        assert_eq!(decode_child_watchdog_config(Some(&encoded)).unwrap(), Some(config));
        assert_eq!(encode_child_watchdog_config(None), None);
    }

    #[test]
    fn an_absent_or_disabled_config_decodes_to_none() {
        assert_eq!(decode_child_watchdog_config(None).unwrap(), None);
        assert_eq!(decode_child_watchdog_config(Some("")).unwrap(), None);
        assert_eq!(
            decode_child_watchdog_config(Some("{\"enabled\":false}")).unwrap(),
            None
        );
    }

    #[test]
    fn strict_decode_reports_upstreams_verbatim_messages() {
        let cases: &[(&str, &str)] = &[
            ("[]", "Invalid child watchdog config: root must be an object."),
            ("{}", "Invalid child watchdog config: enabled must be true or false."),
            (
                "{\"enabled\":true,\"thinking\":1}",
                "Invalid child watchdog config: thinking must be a string or false.",
            ),
            (
                "{\"enabled\":true,\"runId\":\"  \"}",
                "Invalid child watchdog config: runId must be a non-empty string.",
            ),
            (
                "{\"enabled\":true,\"childIndex\":-1}",
                "Invalid child watchdog config: childIndex must be a non-negative integer.",
            ),
            (
                "{\"enabled\":true,\"watchdogTailTimeoutMs\":0}",
                "Invalid child watchdog config: watchdogTailTimeoutMs must be a positive integer.",
            ),
        ];
        for (raw, expected) in cases {
            let err = decode_child_watchdog_config(Some(raw)).unwrap_err();
            assert_eq!(err.0, *expected, "for {raw}");
        }
    }

    #[test]
    fn an_absent_max_warnings_is_a_failure_not_a_default() {
        let raw = json!({
            "enabled": true,
            "watchdogTailTimeoutMs": 1,
            "agentEndTimeoutMs": 1,
        })
        .to_string();
        assert_eq!(
            decode_child_watchdog_config(Some(&raw)).unwrap_err().0,
            "Invalid child watchdog config: maxWarnings must be null or a non-negative integer."
        );
    }

    #[test]
    fn the_status_event_filter_rejects_every_malformed_shape() {
        let good = json!({
            "type": CHILD_WATCHDOG_STATUS_EVENT,
            "seq": 1, "ts": 5, "followUpPending": false, "phase": "idle",
        });
        assert!(is_child_watchdog_status_event(&good));
        for bad in [
            json!("string"),
            json!({ "type": "other", "seq": 1, "ts": 5, "followUpPending": false, "phase": "idle" }),
            json!({ "type": CHILD_WATCHDOG_STATUS_EVENT, "seq": -1, "ts": 5, "followUpPending": false, "phase": "idle" }),
            json!({ "type": CHILD_WATCHDOG_STATUS_EVENT, "seq": 1, "ts": 5, "followUpPending": "no", "phase": "idle" }),
            json!({ "type": CHILD_WATCHDOG_STATUS_EVENT, "seq": 1, "ts": 5, "followUpPending": false, "phase": "unknown" }),
            json!({ "type": CHILD_WATCHDOG_STATUS_EVENT, "seq": 1, "followUpPending": false, "phase": "idle" }),
        ] {
            assert!(!is_child_watchdog_status_event(&bad), "{bad}");
        }
    }

    #[test]
    fn only_reviewing_autofollow_settling_or_a_pending_follow_up_hold_a_run_open() {
        assert!(!child_watchdog_is_active(None));
        for (phase, active) in [
            (ChildWatchdogPhase::Idle, false),
            (ChildWatchdogPhase::Reviewing, true),
            (ChildWatchdogPhase::Autofollow, true),
            (ChildWatchdogPhase::Settling, true),
            (ChildWatchdogPhase::Stale, false),
            (ChildWatchdogPhase::Failed, false),
        ] {
            let snapshot = ChildWatchdogStateSnapshot {
                phase,
                seq: 1,
                last_update: 0,
                follow_up_pending: false,
                reason: None,
                timed_out: None,
            };
            assert_eq!(child_watchdog_is_active(Some(&snapshot)), active, "{phase:?}");
        }
        // followUpPending overrides an otherwise-terminal phase.
        let pending = ChildWatchdogStateSnapshot {
            phase: ChildWatchdogPhase::Idle,
            seq: 1,
            last_update: 0,
            follow_up_pending: true,
            reason: None,
            timed_out: None,
        };
        assert!(child_watchdog_is_active(Some(&pending)));
    }

    #[test]
    fn the_fold_rejects_a_stale_or_equal_sequence_number() {
        let identity = ChildWatchdogIdentity::default();
        let first =
            accept_child_watchdog_event(None, &event(2, ChildWatchdogPhase::Reviewing), &identity)
                .unwrap();
        assert_eq!(first.seq, 2);
        assert!(
            accept_child_watchdog_event(
                Some(&first),
                &event(2, ChildWatchdogPhase::Idle),
                &identity
            )
            .is_none(),
            "an equal seq must not walk the phase backwards"
        );
        assert!(
            accept_child_watchdog_event(
                Some(&first),
                &event(1, ChildWatchdogPhase::Idle),
                &identity
            )
            .is_none()
        );
        assert_eq!(
            accept_child_watchdog_event(
                Some(&first),
                &event(3, ChildWatchdogPhase::Idle),
                &identity
            )
            .unwrap()
            .phase,
            ChildWatchdogPhase::Idle
        );
    }

    #[test]
    fn identity_filtering_applies_only_to_the_fields_the_parent_pinned() {
        let anonymous = accept_child_watchdog_event(
            None,
            &event(1, ChildWatchdogPhase::Idle),
            &ChildWatchdogIdentity::default(),
        );
        assert!(anonymous.is_some(), "an unpinned parent accepts any child");
        let wrong_run = accept_child_watchdog_event(
            None,
            &event(1, ChildWatchdogPhase::Idle),
            &ChildWatchdogIdentity {
                run_id: Some("other".into()),
                ..Default::default()
            },
        );
        assert!(wrong_run.is_none());
        let right_run = accept_child_watchdog_event(
            None,
            &event(1, ChildWatchdogPhase::Idle),
            &ChildWatchdogIdentity {
                run_id: Some("run-1".into()),
                agent: Some("reviewer".into()),
                child_index: Some(0),
            },
        );
        assert!(right_run.is_some());
    }

    #[test]
    fn step_index_stands_in_for_child_index() {
        let mut chain_event = event(1, ChildWatchdogPhase::Reviewing);
        chain_event.child_index = None;
        chain_event.step_index = Some(3);
        assert!(
            accept_child_watchdog_event(
                None,
                &chain_event,
                &ChildWatchdogIdentity {
                    child_index: Some(3),
                    ..Default::default()
                }
            )
            .is_some()
        );
        assert!(
            accept_child_watchdog_event(
                None,
                &chain_event,
                &ChildWatchdogIdentity {
                    child_index: Some(4),
                    ..Default::default()
                }
            )
            .is_none()
        );
    }
}
