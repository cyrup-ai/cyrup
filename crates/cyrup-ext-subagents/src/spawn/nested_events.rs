//! Nested-run event relay + capability-gated control routing (C17) — a faithful port of
//! pi-subagents' `src/runs/shared/nested-events.ts`.
//!
//! # The mechanism
//!
//! A top-level ("root") run mints a [`NestedRoute`]: a private directory under
//! [`nested_events_dir`] holding an `events/` sink, a `controls/` inbox, and a `route.json`
//! carrying the root run id + a random capability token. The route's four coordinates are exported
//! into a fanned-out child's environment (via [`nested_route_env`] / [`nested_child_auth_env`])
//! **only when that child is fanout-authorized**. An authorized descendant — at any depth — appends
//! immutable, capability-token-stamped event files into the sink as it makes progress; the
//! grandparent [`project_nested_events`] folds those files into a [`NestedRegistry`] tree so it can
//! see (and, via the control inbox, interrupt/resume) descendants it never spawned directly.
//!
//! Every boundary is validated: the route must live inside [`nested_events_dir`], every event's
//! `rootRunId`+`capabilityToken` must match the route, every id must pass
//! [`crate::spawn::nested_path::is_safe_nested_path_id_str`], oversized/corrupt/duplicate/stale
//! records are dropped, and terminal state is never regressed by a later stale update.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::SubagentError;
use crate::spawn::nested_path::{
    is_safe_nested_path_id_str, parse_nested_path_env, sanitize_nested_path, NestedPathEntry,
};

// =================================================================================================
// Environment variable names (cyrup equivalents of pi's PI_SUBAGENT_* set)
// =================================================================================================

/// Marks a process as a subagent child (pi `PI_SUBAGENT_CHILD`).
pub const CHILD_ENV: &str = "CYRUP_SUBAGENT_CHILD";
/// `"1"` iff the child is authorized to relay nested events to its grandparent (pi
/// `PI_SUBAGENT_FANOUT_CHILD`).
pub const FANOUT_CHILD_ENV: &str = "CYRUP_SUBAGENT_FANOUT_CHILD";
/// This run's own id (pi `PI_SUBAGENT_RUN_ID`).
pub const RUN_ID_ENV: &str = "CYRUP_SUBAGENT_RUN_ID";
/// This child's index within its parent group (pi `PI_SUBAGENT_CHILD_INDEX`).
pub const CHILD_INDEX_ENV: &str = "CYRUP_SUBAGENT_CHILD_INDEX";
/// Inherited event-sink directory (pi `PI_SUBAGENT_PARENT_EVENT_SINK`).
pub const PARENT_EVENT_SINK_ENV: &str = "CYRUP_SUBAGENT_PARENT_EVENT_SINK";
/// Inherited control-inbox directory (pi `PI_SUBAGENT_PARENT_CONTROL_INBOX`).
pub const PARENT_CONTROL_INBOX_ENV: &str = "CYRUP_SUBAGENT_PARENT_CONTROL_INBOX";
/// Inherited root run id (pi `PI_SUBAGENT_PARENT_ROOT_RUN_ID`).
pub const PARENT_ROOT_RUN_ID_ENV: &str = "CYRUP_SUBAGENT_PARENT_ROOT_RUN_ID";
/// Inherited immediate-parent run id (pi `PI_SUBAGENT_PARENT_RUN_ID`).
pub const PARENT_RUN_ID_ENV: &str = "CYRUP_SUBAGENT_PARENT_RUN_ID";
/// Inherited parent child index (pi `PI_SUBAGENT_PARENT_CHILD_INDEX`).
pub const PARENT_CHILD_INDEX_ENV: &str = "CYRUP_SUBAGENT_PARENT_CHILD_INDEX";
/// Inherited nesting depth (pi `PI_SUBAGENT_PARENT_DEPTH`).
pub const PARENT_DEPTH_ENV: &str = "CYRUP_SUBAGENT_PARENT_DEPTH";
/// Inherited encoded ancestry path (pi `PI_SUBAGENT_PARENT_PATH`).
pub const PARENT_PATH_ENV: &str = "CYRUP_SUBAGENT_PARENT_PATH";
/// Inherited capability token (pi `PI_SUBAGENT_PARENT_CAPABILITY_TOKEN`).
pub const PARENT_CAPABILITY_TOKEN_ENV: &str = "CYRUP_SUBAGENT_PARENT_CAPABILITY_TOKEN";

/// Optional override for the temp root the nested-event directories live under; when unset,
/// defaults to `<temp_dir>/cyrup-subagents`.
pub const TEMP_ROOT_ENV: &str = "CYRUP_SUBAGENTS_TEMP_ROOT";

const ROUTE_FILE: &str = "route.json";
const REGISTRY_FILE: &str = "registry.json";
const MAX_EVENT_BYTES: u64 = 64 * 1024;
const MAX_STEPS: usize = 12;
const MAX_CHILDREN: usize = 16;
const MAX_DEPTH: i64 = 3;

// =================================================================================================
// Directory roots (pi shared/types.ts TEMP_ROOT_DIR / RESULTS_DIR / ASYNC_DIR + NESTED_EVENTS_DIR)
// =================================================================================================

/// The scratch root the NESTED-EVENT tree and the supervisor channels hang off — pi's
/// `TEMP_ROOT_DIR` (`shared/types.ts`). `<temp_dir>/cyrup-subagents`, overridable with
/// [`TEMP_ROOT_ENV`].
///
/// # Not the same root as `background::temp_root_dir`, despite the shared upstream name
///
/// Both render pi's one `TEMP_ROOT_DIR`, but they read different variables and resolve to
/// different directories: that one keys off `CYRUP_HOME` (`<CYRUP_HOME>/.cyrup/subagents`), this
/// one reads [`TEMP_ROOT_ENV`] and never consults `CYRUP_HOME` at all. A test that sandboxes only
/// `CYRUP_HOME` therefore relocates that tree and leaves THIS one on the shared real temp root —
/// which is why the integration tests covering nested events and supervisor channels have to set
/// both variables.
///
/// `pub` because [`crate::native_supervisor`] hangs `supervisor-channels/` off the SAME root
/// upstream does (`native-supervisor-channel.ts:18` — `path.join(TEMP_ROOT_DIR,
/// "supervisor-channels")`); a second, independently-derived root would put the child's request
/// files somewhere the parent's poller never looks.
#[must_use]
pub fn temp_root_dir() -> PathBuf {
    temp_root_dir_from(&|key| std::env::var_os(key), std::env::temp_dir())
}

/// [`temp_root_dir`] with its two ambient inputs supplied — the crate's `_from` convention, which
/// this resolver was the last one in the crate to lack. Its absence is why
/// [`crate::paths::Roots::from_lookup`] could not resolve every root from one lookup.
#[must_use]
pub fn temp_root_dir_from(env: crate::paths::EnvLookup<'_>, os_temp_dir: PathBuf) -> PathBuf {
    env(TEMP_ROOT_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| os_temp_dir.join("cyrup-subagents"))
}

/// The root directory every nested-event route lives under (pi `NESTED_EVENTS_DIR`).
#[must_use]
pub fn nested_events_dir() -> PathBuf {
    temp_root_dir().join("nested-subagent-events")
}

fn results_dir() -> PathBuf {
    temp_root_dir().join("async-subagent-results")
}

fn async_dir() -> PathBuf {
    temp_root_dir().join("async-subagent-runs")
}

fn nested_runs_dir() -> PathBuf {
    temp_root_dir().join("nested-subagent-runs")
}

// =================================================================================================
// Data model (pi NestedRouteInfo / NestedRunSummary / NestedStepSummary / records / registry)
// =================================================================================================

/// A private relay address minted by a root run (pi `NestedRouteInfo`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedRoute {
    /// The root run this route belongs to.
    pub root_run_id: String,
    /// The `events/` directory descendants append relayed events into.
    pub event_sink: PathBuf,
    /// The `controls/` directory the grandparent drops interrupt/resume requests into.
    pub control_inbox: PathBuf,
    /// The capability token every event/control record must carry to be trusted.
    pub capability_token: String,
}

/// Token usage totals (pi `TokenUsage`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TokenUsage {
    /// Input tokens.
    pub input: i64,
    /// Output tokens.
    pub output: i64,
    /// Total tokens.
    pub total: i64,
}

/// Cost totals (pi `CostSummary` subset used by nested summaries).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostSummary {
    /// Billed input tokens.
    pub input_tokens: i64,
    /// Billed output tokens.
    pub output_tokens: i64,
    /// Cost in USD.
    pub cost_usd: f64,
}

/// A per-step summary within a nested run (pi `NestedStepSummary`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedStepSummary {
    /// The step's agent.
    pub agent: String,
    /// The step's status.
    pub status: String,
    /// Optional fields, omitted when absent, mirroring pi's conditional projection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool_started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<NestedRunSummary>>,
}

/// A projected nested run (pi `NestedRunSummary`). Every optional field is omitted when absent so
/// the serialized shape matches pi's conditional-spread projection.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedRunSummary {
    /// The run's id.
    pub id: String,
    /// The id of the run that spawned it.
    pub parent_run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_step_index: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent: Option<String>,
    /// Nesting depth (0-based), clamped to `MAX_DEPTH`.
    pub depth: i64,
    /// The ancestry chain.
    pub path: Vec<NestedPathEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub async_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intercom_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_intercom_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaf_intercom_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_inbox: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// The run's lifecycle state.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_step: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_step_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool_started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost: Option<CostSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<NestedStepSummary>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<NestedRunSummary>>,
}

/// A relayed status event (pi `NestedEventRecord`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedEventRecord {
    #[serde(rename = "type")]
    pub event_type: String,
    pub ts: i64,
    pub root_run_id: String,
    pub parent_run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_step_index: Option<i64>,
    pub capability_token: String,
    pub child: NestedRunSummary,
}

/// The caller-supplied portion of a status event (pi `Omit<NestedEventRecord, "rootRunId" |
/// "capabilityToken">`).
#[derive(Debug, Clone)]
pub struct NestedEventInput {
    pub event_type: String,
    pub ts: i64,
    pub parent_run_id: String,
    pub parent_step_index: Option<i64>,
    pub child: NestedRunSummary,
}

/// A capability-gated interrupt/resume request from the grandparent (pi
/// `NestedControlRequestRecord`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedControlRequestRecord {
    #[serde(rename = "type")]
    pub event_type: String,
    pub ts: i64,
    pub root_run_id: String,
    pub capability_token: String,
    pub request_id: String,
    pub target_run_id: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// The descendant's answer to a control request (pi `NestedControlResultRecord`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedControlResultRecord {
    #[serde(rename = "type")]
    pub event_type: String,
    pub ts: i64,
    pub root_run_id: String,
    pub capability_token: String,
    pub request_id: String,
    pub target_run_id: String,
    pub ok: bool,
    pub message: String,
}

/// The projected registry a grandparent maintains from a route's event stream (pi
/// `NestedRegistry`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedRegistry {
    pub root_run_id: String,
    pub updated_at: i64,
    pub children: Vec<NestedRunSummary>,
    pub processed_events: Vec<String>,
}

/// The resolved ancestry address of the immediate parent (pi
/// `resolveNestedParentAddressFromEnv`'s return). Serializable so it can be carried verbatim through
/// [`crate::background::runner_main::RunnerConfig`]'s one-shot handoff file (the orchestrator resolves
/// it once from its own inherited env, the detached runner never re-resolves it).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedParentAddress {
    pub parent_run_id: String,
    pub parent_step_index: Option<i64>,
    pub depth: i64,
    pub path: Vec<NestedPathEntry>,
}

// =================================================================================================
// Small value helpers (pi clampNumber / stringValue / sanitizeState / sanitizeTokenUsage / ...)
// =================================================================================================

fn finite_number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|f| f.is_finite())
}

/// Preserve the original JSON number (keeping integer-ness) when finite, else `None`.
fn number_value(value: Option<&Value>) -> Option<Value> {
    let v = value?;
    if v.is_i64() || v.is_u64() {
        return Some(v.clone());
    }
    if v.as_f64().is_some_and(f64::is_finite) {
        return Some(v.clone());
    }
    None
}

fn string_value(value: Option<&Value>, max: usize) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(max).collect())
}

fn is_safe_nested_id(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(is_safe_nested_path_id_str)
}

fn sanitize_state(value: Option<&Value>, fallback: &str) -> String {
    match value.and_then(Value::as_str) {
        // G77: `"stopped"` is in upstream's own allowlist (`nested-events.ts:270-273` @v0.43.0);
        // without it a stopped descendant's state is silently rewritten to the `fallback`, so the
        // parent's registry never learns the child was stopped.
        Some(s @ ("queued" | "running" | "complete" | "failed" | "paused" | "stopped")) => {
            s.to_string()
        }
        _ => fallback.to_string(),
    }
}

fn sanitize_step_status(value: Option<&Value>) -> String {
    match value.and_then(Value::as_str) {
        // G77: same allowlist widening for a nested STEP status (`nested-events.ts:281-283`).
        Some(
            s @ ("pending" | "running" | "complete" | "completed" | "failed" | "paused"
            | "stopped"),
        ) => s.to_string(),
        _ => "pending".to_string(),
    }
}

fn set_if_some(map: &mut serde_json::Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        map.insert(key.to_string(), value);
    }
}

fn sanitize_token_usage(value: Option<&Value>) -> Option<Value> {
    let obj = value?.as_object()?;
    let input = number_value(obj.get("input"))?;
    let output = number_value(obj.get("output"))?;
    let total = number_value(obj.get("total"))?;
    Some(serde_json::json!({ "input": input, "output": output, "total": total }))
}

fn sanitize_cost(value: Option<&Value>) -> Option<Value> {
    let obj = value?.as_object()?;
    let input_tokens = number_value(obj.get("inputTokens"))?;
    let output_tokens = number_value(obj.get("outputTokens"))?;
    let cost_usd = number_value(obj.get("costUsd"))?;
    Some(serde_json::json!({
        "inputTokens": input_tokens,
        "outputTokens": output_tokens,
        "costUsd": cost_usd,
    }))
}

// =================================================================================================
// Sanitization (pi sanitizeSummary / sanitizeStep)
// =================================================================================================

fn sanitize_step_map(input: &Value, depth: i64) -> Option<Value> {
    let obj = input.as_object()?;
    let agent = string_value(obj.get("agent"), 128)?;
    let mut map = serde_json::Map::new();
    map.insert("agent".to_string(), Value::String(agent));
    map.insert("status".to_string(), Value::String(sanitize_step_status(obj.get("status"))));
    set_if_some(&mut map, "sessionFile", string_value(obj.get("sessionFile"), 2048).map(Value::String));
    if let Some(s @ ("active_long_running" | "needs_attention")) = obj.get("activityState").and_then(Value::as_str) {
        map.insert("activityState".to_string(), Value::String(s.to_string()));
    }
    set_if_some(&mut map, "lastActivityAt", number_value(obj.get("lastActivityAt")));
    set_if_some(&mut map, "currentTool", string_value(obj.get("currentTool"), 128).map(Value::String));
    set_if_some(&mut map, "currentToolStartedAt", number_value(obj.get("currentToolStartedAt")));
    set_if_some(&mut map, "currentPath", string_value(obj.get("currentPath"), 2048).map(Value::String));
    set_if_some(&mut map, "turnCount", number_value(obj.get("turnCount")));
    set_if_some(&mut map, "toolCount", number_value(obj.get("toolCount")));
    set_if_some(&mut map, "startedAt", number_value(obj.get("startedAt")));
    set_if_some(&mut map, "endedAt", number_value(obj.get("endedAt")));
    set_if_some(&mut map, "error", string_value(obj.get("error"), 1024).map(Value::String));
    if depth < MAX_DEPTH
        && let Some(Value::Array(children)) = obj.get("children")
    {
        let sanitized = sanitize_children(children, depth + 1);
        if !sanitized.is_empty() {
            map.insert("children".to_string(), Value::Array(sanitized));
        }
    }
    Some(Value::Object(map))
}

fn sanitize_children(children: &[Value], depth: i64) -> Vec<Value> {
    children
        .iter()
        .filter_map(|child| sanitize_summary_map(child, depth))
        .take(MAX_CHILDREN)
        .collect()
}

fn sanitize_summary_map(input: &Value, depth: i64) -> Option<Value> {
    let obj = input.as_object()?;
    if !is_safe_nested_id(obj.get("id")) || !is_safe_nested_id(obj.get("parentRunId")) {
        return None;
    }
    let id = obj.get("id").and_then(Value::as_str)?.to_string();
    let parent_run_id = obj.get("parentRunId").and_then(Value::as_str)?.to_string();

    let path = obj.get("path").map_or_else(Vec::new, sanitize_nested_path);
    let depth_value = finite_number(obj.get("depth")).unwrap_or(0.0);
    let clamped_depth = depth_value.max(0.0).min(MAX_DEPTH as f64) as i64;

    let mut map = serde_json::Map::new();
    map.insert("id".to_string(), Value::String(id));
    map.insert("parentRunId".to_string(), Value::String(parent_run_id));
    set_if_some(&mut map, "parentStepIndex", number_value(obj.get("parentStepIndex")));
    set_if_some(&mut map, "parentAgent", string_value(obj.get("parentAgent"), 128).map(Value::String));
    map.insert("depth".to_string(), Value::from(clamped_depth));
    map.insert("path".to_string(), serde_json::to_value(&path).unwrap_or(Value::Array(Vec::new())));
    map.insert("state".to_string(), Value::String(sanitize_state(obj.get("state"), "running")));

    set_if_some(&mut map, "asyncDir", string_value(obj.get("asyncDir"), 2048).map(Value::String));
    // pid must be a positive integer.
    if let Some(pid) = obj.get("pid").and_then(Value::as_i64).filter(|p| *p > 0) {
        map.insert("pid".to_string(), Value::from(pid));
    }
    set_if_some(&mut map, "sessionId", string_value(obj.get("sessionId"), 256).map(Value::String));
    set_if_some(&mut map, "sessionFile", string_value(obj.get("sessionFile"), 2048).map(Value::String));
    set_if_some(&mut map, "intercomTarget", string_value(obj.get("intercomTarget"), 256).map(Value::String));
    set_if_some(&mut map, "ownerIntercomTarget", string_value(obj.get("ownerIntercomTarget"), 256).map(Value::String));
    set_if_some(&mut map, "leafIntercomTarget", string_value(obj.get("leafIntercomTarget"), 256).map(Value::String));
    if let Some(s @ ("live" | "gone" | "unknown")) = obj.get("ownerState").and_then(Value::as_str) {
        map.insert("ownerState".to_string(), Value::String(s.to_string()));
    }
    set_if_some(&mut map, "controlInbox", string_value(obj.get("controlInbox"), 2048).map(Value::String));
    set_if_some(&mut map, "capabilityToken", string_value(obj.get("capabilityToken"), 128).map(Value::String));
    if let Some(s @ ("single" | "parallel" | "chain")) = obj.get("mode").and_then(Value::as_str) {
        map.insert("mode".to_string(), Value::String(s.to_string()));
    }
    set_if_some(&mut map, "agent", string_value(obj.get("agent"), 128).map(Value::String));
    if let Some(Value::Array(agents)) = obj.get("agents") {
        let names: Vec<Value> = agents
            .iter()
            .filter_map(|a| string_value(Some(a), 128).map(Value::String))
            .take(MAX_STEPS)
            .collect();
        map.insert("agents".to_string(), Value::Array(names));
    }
    set_if_some(&mut map, "currentStep", number_value(obj.get("currentStep")));
    set_if_some(&mut map, "chainStepCount", number_value(obj.get("chainStepCount")));
    if let Some(s @ ("active_long_running" | "needs_attention")) = obj.get("activityState").and_then(Value::as_str) {
        map.insert("activityState".to_string(), Value::String(s.to_string()));
    }
    set_if_some(&mut map, "lastActivityAt", number_value(obj.get("lastActivityAt")));
    set_if_some(&mut map, "currentTool", string_value(obj.get("currentTool"), 128).map(Value::String));
    set_if_some(&mut map, "currentToolStartedAt", number_value(obj.get("currentToolStartedAt")));
    set_if_some(&mut map, "currentPath", string_value(obj.get("currentPath"), 2048).map(Value::String));
    set_if_some(&mut map, "turnCount", number_value(obj.get("turnCount")));
    set_if_some(&mut map, "toolCount", number_value(obj.get("toolCount")));
    set_if_some(&mut map, "totalTokens", sanitize_token_usage(obj.get("totalTokens")));
    set_if_some(&mut map, "totalCost", sanitize_cost(obj.get("totalCost")));
    set_if_some(&mut map, "startedAt", number_value(obj.get("startedAt")));
    set_if_some(&mut map, "endedAt", number_value(obj.get("endedAt")));
    set_if_some(&mut map, "lastUpdate", number_value(obj.get("lastUpdate")));
    set_if_some(&mut map, "error", string_value(obj.get("error"), 1024).map(Value::String));

    if let Some(Value::Array(steps)) = obj.get("steps") {
        let sanitized: Vec<Value> = steps
            .iter()
            .filter_map(|step| sanitize_step_map(step, depth + 1))
            .take(MAX_STEPS)
            .collect();
        if !sanitized.is_empty() {
            map.insert("steps".to_string(), Value::Array(sanitized));
        }
    }
    if depth < MAX_DEPTH
        && let Some(Value::Array(children)) = obj.get("children")
    {
        let sanitized = sanitize_children(children, depth + 1);
        if !sanitized.is_empty() {
            map.insert("children".to_string(), Value::Array(sanitized));
        }
    }

    Some(Value::Object(map))
}

/// pi `sanitizeSummary`.
#[must_use]
pub fn sanitize_summary(input: &Value) -> Option<NestedRunSummary> {
    let map = sanitize_summary_map(input, 0)?;
    serde_json::from_value(map).ok()
}

// =================================================================================================
// Route creation + validation (pi createNestedRoute / validateRouteShape / resolve*FromEnv)
// =================================================================================================

fn contained_path(base: &Path, candidate: &Path) -> bool {
    let base = std::path::absolute(base).unwrap_or_else(|_| base.to_path_buf());
    let candidate = std::path::absolute(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    candidate == base || candidate.starts_with(&base)
}

fn common_route_root(event_sink: &Path) -> PathBuf {
    std::path::absolute(event_sink)
        .unwrap_or_else(|_| event_sink.to_path_buf())
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

fn assert_safe_id(label: &str, value: &str) -> Result<(), SubagentError> {
    if is_safe_nested_path_id_str(value) {
        Ok(())
    } else {
        Err(SubagentError::UnsafePathToken(format!(
            "{label} must be a non-empty safe id token."
        )))
    }
}

fn validate_route_shape(route: &NestedRoute) -> Result<(), SubagentError> {
    validate_route_shape_in(&nested_events_dir(), route)
}

/// [`validate_route_shape`] against an explicitly supplied nested-events root.
///
/// The containment check is the point of this function — a route whose sink escapes the tree the
/// caller trusts is rejected — so the TRUSTED ROOT is what varies, never the check. It is a
/// parameter rather than a field on [`NestedRoute`] deliberately: that type is serialized across a
/// process boundary, so a route carrying its own root would let a child nominate the very boundary
/// it is being checked against.
fn validate_route_shape_in(root: &Path, route: &NestedRoute) -> Result<(), SubagentError> {
    assert_safe_id("rootRunId", &route.root_run_id)?;
    assert_safe_id("capabilityToken", &route.capability_token)?;
    if !contained_path(root, &route.event_sink) {
        return Err(SubagentError::UnsafePathToken(
            "Nested event sink is outside the subagent nested event root.".to_string(),
        ));
    }
    if !contained_path(root, &route.control_inbox) {
        return Err(SubagentError::UnsafePathToken(
            "Nested control inbox is outside the subagent nested event root.".to_string(),
        ));
    }
    let control_root = std::path::absolute(&route.control_inbox)
        .unwrap_or_else(|_| route.control_inbox.clone())
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if common_route_root(&route.event_sink) != control_root {
        return Err(SubagentError::UnsafePathToken(
            "Nested event sink and control inbox must share one route root.".to_string(),
        ));
    }
    Ok(())
}

fn create_dir_all_mode(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// pi `createNestedRoute`: mint a fresh route directory (`events/`, `controls/`, `route.json`).
///
/// # Errors
///
/// Returns [`SubagentError`] if `root_run_id` is unsafe or the directories/file cannot be created.
pub fn create_nested_route(root_run_id: &str) -> Result<NestedRoute, SubagentError> {
    create_nested_route_in(&nested_events_dir(), root_run_id)
}

/// [`create_nested_route`] under an explicitly supplied nested-events root.
///
/// The returned [`NestedRoute`] carries absolute `event_sink`/`control_inbox` paths, so everything
/// downstream — `write_nested_event`, `project_nested_events` — follows the route rather than
/// re-deriving from [`TEMP_ROOT_ENV`]. Supplying the root here is therefore enough to scope a whole
/// nested-events tree to a caller's own directory without moving that variable on the process.
///
/// # Errors
///
/// See [`create_nested_route`].
pub fn create_nested_route_in(
    events_root: &Path,
    root_run_id: &str,
) -> Result<NestedRoute, SubagentError> {
    assert_safe_id("rootRunId", root_run_id)?;
    let capability_token = uuid::Uuid::new_v4().to_string();
    let route_root = events_root.join(format!("{root_run_id}-{capability_token}"));
    let event_sink = route_root.join("events");
    let control_inbox = route_root.join("controls");
    create_dir_all_mode(&event_sink).map_err(SubagentError::Spawn)?;
    create_dir_all_mode(&control_inbox).map_err(SubagentError::Spawn)?;
    let metadata = serde_json::json!({
        "rootRunId": root_run_id,
        "capabilityToken": capability_token,
        "createdAt": crate::time::now_epoch_millis(),
    });
    let route_file = route_root.join(ROUTE_FILE);
    std::fs::write(&route_file, format!("{metadata}\n")).map_err(SubagentError::Spawn)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&route_file, std::fs::Permissions::from_mode(0o600));
    }
    Ok(NestedRoute {
        root_run_id: root_run_id.to_string(),
        event_sink,
        control_inbox,
        capability_token,
    })
}

fn read_route_metadata(route: &NestedRoute) -> Result<(Option<String>, Option<String>), SubagentError> {
    let route_file = common_route_root(&route.event_sink).join(ROUTE_FILE);
    let content = std::fs::read_to_string(&route_file).map_err(SubagentError::Spawn)?;
    let parsed: Value = serde_json::from_str(&content)
        .map_err(|err| SubagentError::MalformedSettings(format!("route.json: {err}")))?;
    let root = parsed.get("rootRunId").and_then(Value::as_str).map(str::to_string);
    let token = parsed.get("capabilityToken").and_then(Value::as_str).map(str::to_string);
    Ok((root, token))
}

/// pi `resolveNestedRouteFromEnv`: reconstruct and validate the inherited route from a lookup.
///
/// # Errors
///
/// Returns [`SubagentError`] if the reconstructed route fails shape validation or its `route.json`
/// metadata does not match the provided id/token.
pub fn resolve_nested_route_from_env(
    get: impl Fn(&str) -> Option<String>,
) -> Result<Option<NestedRoute>, SubagentError> {
    let (Some(root_run_id), Some(event_sink), Some(control_inbox), Some(capability_token)) = (
        get(PARENT_ROOT_RUN_ID_ENV).filter(|s| !s.is_empty()),
        get(PARENT_EVENT_SINK_ENV).filter(|s| !s.is_empty()),
        get(PARENT_CONTROL_INBOX_ENV).filter(|s| !s.is_empty()),
        get(PARENT_CAPABILITY_TOKEN_ENV).filter(|s| !s.is_empty()),
    ) else {
        return Ok(None);
    };
    let route = NestedRoute {
        root_run_id: root_run_id.clone(),
        event_sink: PathBuf::from(event_sink),
        control_inbox: PathBuf::from(control_inbox),
        capability_token: capability_token.clone(),
    };
    validate_route_shape(&route)?;
    let (meta_root, meta_token) = read_route_metadata(&route)?;
    if meta_root.as_deref() != Some(root_run_id.as_str())
        || meta_token.as_deref() != Some(capability_token.as_str())
    {
        return Err(SubagentError::UnsafePathToken(
            "Nested event route metadata does not match the provided root id and capability token."
                .to_string(),
        ));
    }
    Ok(Some(route))
}

/// pi `resolveInheritedNestedRouteFromEnv`: like [`resolve_nested_route_from_env`] but swallows a
/// validation error into `None` (logging it), so an invalid inherited route never aborts the child.
pub fn resolve_inherited_nested_route_from_env(
    get: impl Fn(&str) -> Option<String>,
) -> Option<NestedRoute> {
    match resolve_nested_route_from_env(get) {
        Ok(route) => route,
        Err(err) => {
            tracing::warn!(error = %err, "ignoring invalid nested subagent event route");
            None
        }
    }
}

/// pi `resolveNestedParentAddressFromEnv`.
#[must_use]
pub fn resolve_nested_parent_address_from_env(
    get: impl Fn(&str) -> Option<String>,
) -> Option<NestedParentAddress> {
    let parent_run_id = get(PARENT_RUN_ID_ENV)?;
    if !is_safe_nested_path_id_str(&parent_run_id) {
        return None;
    }
    let parent_step_index = get(PARENT_CHILD_INDEX_ENV)
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<i64>().ok());
    let depth_raw = get(PARENT_DEPTH_ENV)
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|f| f.is_finite())
        .unwrap_or(1.0);
    let depth = depth_raw.max(1.0).min(MAX_DEPTH as f64) as i64;

    let parsed_path = parse_nested_path_env(get(PARENT_PATH_ENV).as_deref());
    let path = if parsed_path.is_empty() {
        vec![NestedPathEntry {
            run_id: parent_run_id.clone(),
            step_index: parent_step_index,
            agent: None,
        }]
    } else {
        parsed_path
    };

    Some(NestedParentAddress {
        parent_run_id,
        parent_step_index,
        depth,
        path,
    })
}

/// pi `nestedRouteEnv`: the four env entries a fanout-authorized parent hands to its child.
#[must_use]
pub fn nested_route_env(route: &NestedRoute) -> HashMap<String, String> {
    HashMap::from([
        (PARENT_EVENT_SINK_ENV.to_string(), route.event_sink.to_string_lossy().into_owned()),
        (PARENT_CONTROL_INBOX_ENV.to_string(), route.control_inbox.to_string_lossy().into_owned()),
        (PARENT_ROOT_RUN_ID_ENV.to_string(), route.root_run_id.clone()),
        (PARENT_CAPABILITY_TOKEN_ENV.to_string(), route.capability_token.clone()),
    ])
}

/// The child-ROLE env pair EVERY spawned subagent child carries, and the SINGLE production source
/// of those two entries — pi `augmentChildEnv` (`runs/shared/pi-args.ts:328-330`), where
/// `env[SUBAGENT_CHILD_ENV] = "1"` and `env[SUBAGENT_FANOUT_CHILD_ENV] = fanoutAuthorized ? "1" : "0"`
/// are written unconditionally on every spawn, next to each other, in that order.
///
/// Three independent subsystems key off this pair, which is why it is factored out rather than
/// re-inlined per call site:
/// 1. **Registration** ([`crate::extension::registration_mode_from_env`], pi
///    `extension/index.ts:177` + `extension/fanout-child.ts:132`): a plain child registers NO
///    subagent surface; a fanout-authorized child registers the restricted one.
/// 2. **Parent-session anchoring** (pi `extension/index.ts:552`): only a NON-child overwrites
///    [`crate::exec::PARENT_SESSION_ENV_VAR`] with its own session id.
/// 3. **Permission ask-forwarding** (`cyrup_permission_system::permission_extension_for_env`, this
///    crate's downstream consumer): a child installs the `ForwardingAskChannel` that writes its
///    `ask` into the PARENT's filesystem spool instead of dying with no reachable human.
///
/// `authorized == false` blanks the fanout flag to an explicit `"0"` rather than leaving it absent.
/// That is pi's own wording and it is load-bearing here: a spawn env is an OVERLAY applied over the
/// inherited environment ([`crate::spawn::SpawnedChild::spawn`] never clears it), so an absent entry
/// would let a fanout-authorized process's own `CYRUP_SUBAGENT_FANOUT_CHILD=1` leak down into a
/// grandchild that was never granted the route.
#[must_use]
pub fn child_role_env(authorized: bool) -> [(&'static str, &'static str); 2] {
    [(CHILD_ENV, "1"), (FANOUT_CHILD_ENV, if authorized { "1" } else { "0" })]
}

/// The `fanout-child` authorization env overlay (pi-args `augmentChildEnv`, the nested-route
/// portion): sets the [`child_role_env`] pair and, **only when `authorized`**, propagates the route +
/// parent-address coordinates so the grandchild's events reach the grandparent. When not
/// authorized, the parent coordinates are blanked (empty strings) so the child cannot spoof its way
/// onto a route it was not granted.
#[must_use]
pub fn nested_child_auth_env(
    authorized: bool,
    route: Option<&NestedRoute>,
    address: Option<&NestedParentAddress>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    for (key, value) in child_role_env(authorized) {
        env.insert(key.to_string(), value.to_string());
    }

    let blank = String::new();
    let (event_sink, control_inbox, root_run_id, token) = match (authorized, route) {
        (true, Some(route)) => (
            route.event_sink.to_string_lossy().into_owned(),
            route.control_inbox.to_string_lossy().into_owned(),
            route.root_run_id.clone(),
            route.capability_token.clone(),
        ),
        _ => (blank.clone(), blank.clone(), blank.clone(), blank.clone()),
    };
    let (parent_run_id, child_index, depth, path) = match (authorized, address) {
        (true, Some(addr)) => (
            addr.parent_run_id.clone(),
            addr.parent_step_index.map(|i| i.to_string()).unwrap_or_default(),
            addr.depth.to_string(),
            encode_path(&addr.path),
        ),
        _ => (blank.clone(), blank.clone(), blank.clone(), blank),
    };

    env.insert(PARENT_EVENT_SINK_ENV.to_string(), event_sink);
    env.insert(PARENT_CONTROL_INBOX_ENV.to_string(), control_inbox);
    env.insert(PARENT_ROOT_RUN_ID_ENV.to_string(), root_run_id);
    env.insert(PARENT_CAPABILITY_TOKEN_ENV.to_string(), token);
    env.insert(PARENT_RUN_ID_ENV.to_string(), parent_run_id);
    env.insert(PARENT_CHILD_INDEX_ENV.to_string(), child_index);
    env.insert(PARENT_DEPTH_ENV.to_string(), depth);
    env.insert(PARENT_PATH_ENV.to_string(), path);
    env
}

fn encode_path(path: &[NestedPathEntry]) -> String {
    crate::spawn::nested_path::encode_nested_path_env(path)
}

// =================================================================================================
// Event parse / write (pi parseRecord / parseNestedEventRecords / writeNestedEvent)
// =================================================================================================

fn parse_record(content: &str, route: &NestedRoute) -> Option<NestedEventRecord> {
    if content.len() as u64 > MAX_EVENT_BYTES {
        return None;
    }
    let parsed: Value = serde_json::from_str(content).ok()?;
    let obj = parsed.as_object()?;
    let type_str = obj.get("type").and_then(Value::as_str)?;
    if !matches!(
        type_str,
        "subagent.nested.started" | "subagent.nested.updated" | "subagent.nested.completed"
    ) {
        return None;
    }
    if obj.get("rootRunId").and_then(Value::as_str) != Some(route.root_run_id.as_str())
        || obj.get("capabilityToken").and_then(Value::as_str) != Some(route.capability_token.as_str())
    {
        return None;
    }
    let parent_run_id = obj.get("parentRunId").and_then(Value::as_str)?;
    if !is_safe_nested_path_id_str(parent_run_id) {
        return None;
    }
    let ts = finite_number(obj.get("ts"))? as i64;
    let child = sanitize_summary(obj.get("child")?)?;
    if child.id == route.root_run_id {
        return None;
    }
    let mut routed_child = child;
    routed_child.control_inbox = Some(route.control_inbox.to_string_lossy().into_owned());
    routed_child.capability_token = Some(route.capability_token.clone());
    routed_child.owner_state = Some(routed_child.owner_state.unwrap_or_else(|| "unknown".to_string()));

    Some(NestedEventRecord {
        event_type: type_str.to_string(),
        ts,
        root_run_id: route.root_run_id.clone(),
        parent_run_id: parent_run_id.to_string(),
        parent_step_index: number_value(obj.get("parentStepIndex")).and_then(|v| v.as_i64()),
        capability_token: route.capability_token.clone(),
        child: routed_child,
    })
}

/// pi `parseNestedEventRecords`: parse a single record, or one-per-line JSONL (dropping a trailing
/// partial line that lacks a terminating newline).
#[must_use]
pub fn parse_nested_event_records(content: &str, route: &NestedRoute) -> Vec<NestedEventRecord> {
    if !content.contains('\n') {
        return parse_record(content.trim(), route).into_iter().collect();
    }
    let mut lines: Vec<&str> = content.split('\n').collect();
    // pi `.slice(0, content.endsWith("\n") ? undefined : -1)`: when the buffer ends with a
    // newline pi keeps every segment (the trailing empty segment after the final newline is
    // removed below by the empty-line filter); otherwise it drops the final, unterminated
    // (partial) line. `pop()` reproduces the `-1` slice without range indexing.
    if !content.ends_with('\n') {
        lines.pop();
    }
    lines
        .iter()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                None
            } else {
                parse_record(trimmed, route)
            }
        })
        .collect()
}

fn write_route_record<T: serde::Serialize>(
    dir: &Path,
    ts: i64,
    payload: &T,
) -> Result<PathBuf, SubagentError> {
    let content = format!(
        "{}\n",
        serde_json::to_string(payload)
            .map_err(|err| SubagentError::MalformedSettings(format!("route record: {err}")))?
    );
    if content.len() as u64 > MAX_EVENT_BYTES {
        return Err(SubagentError::UnsafePathToken(
            "Nested route record exceeds the maximum size.".to_string(),
        ));
    }
    create_dir_all_mode(dir).map_err(SubagentError::Spawn)?;
    let name = format!("{ts:013}-{}.json", uuid::Uuid::new_v4());
    let tmp = dir.join(format!(".{name}.tmp"));
    let final_path = dir.join(&name);
    std::fs::write(&tmp, &content).map_err(SubagentError::Spawn)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &final_path).map_err(SubagentError::Spawn)?;
    Ok(final_path)
}

/// pi `writeNestedEvent`: sanitize the caller's event against `route`, then append it to the sink.
///
/// # Errors
///
/// Returns [`SubagentError`] if the route is invalid, the record fails sanitization, or the file
/// cannot be written.
pub fn write_nested_event(route: &NestedRoute, event: &NestedEventInput) -> Result<(), SubagentError> {
    write_nested_event_in(&nested_events_dir(), route, event)
}

/// [`write_nested_event`] validated against an explicitly supplied nested-events root, for a caller
/// that minted its route with [`create_nested_route_in`] and therefore owns the tree.
///
/// # Errors
///
/// See [`write_nested_event`].
pub fn write_nested_event_in(
    events_root: &Path,
    route: &NestedRoute,
    event: &NestedEventInput,
) -> Result<(), SubagentError> {
    validate_route_shape_in(events_root, route)?;
    let record = NestedEventRecord {
        event_type: event.event_type.clone(),
        ts: event.ts,
        root_run_id: route.root_run_id.clone(),
        parent_run_id: event.parent_run_id.clone(),
        parent_step_index: event.parent_step_index,
        capability_token: route.capability_token.clone(),
        child: event.child.clone(),
    };
    let serialized = serde_json::to_string(&record)
        .map_err(|err| SubagentError::MalformedSettings(format!("nested event: {err}")))?;
    let sanitized = parse_record(&serialized, route).ok_or_else(|| {
        SubagentError::MalformedSettings("Nested event record failed validation.".to_string())
    })?;
    write_route_record(&route.event_sink, sanitized.ts, &sanitized)?;
    Ok(())
}

// =================================================================================================
// Registry projection (pi readNestedRegistry / applyNestedEvent / mergeSummary / attachChild /
// projectNestedEvents)
// =================================================================================================

fn registry_path(route: &NestedRoute) -> PathBuf {
    common_route_root(&route.event_sink).join(REGISTRY_FILE)
}

fn terminal(state: &str) -> bool {
    // G77: `"stopped"` is in upstream's own set (`runs/shared/nested-events.ts:420-422` @v0.43.0:
    // `state === "complete" || state === "failed" || state === "paused" || state === "stopped"`).
    // Without it, a later non-terminal event from a stopped descendant would overwrite its terminal
    // record in the nested-run registry — and the cascade's own `is_live_state` would then keep
    // re-targeting a run that has already been stopped.
    matches!(state, "complete" | "failed" | "paused" | "stopped")
}

/// `{ ...existing, ...incoming }`: incoming's present keys win (via JSON map merge, so absent
/// optional fields never clobber existing values), reproducing pi's object spread exactly.
fn overlay(existing: &NestedRunSummary, incoming: &NestedRunSummary) -> NestedRunSummary {
    let mut base = match serde_json::to_value(existing) {
        Ok(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    if let Ok(Value::Object(inc)) = serde_json::to_value(incoming) {
        for (key, value) in inc {
            base.insert(key, value);
        }
    }
    serde_json::from_value(Value::Object(base)).unwrap_or_else(|_| incoming.clone())
}

fn merge_summary(existing: Option<NestedRunSummary>, event: &NestedEventRecord) -> NestedRunSummary {
    let incoming_state = if event.event_type == "subagent.nested.completed"
        && event.child.state == "running"
    {
        "complete".to_string()
    } else {
        event.child.state.clone()
    };
    let mut incoming = event.child.clone();
    incoming.state = incoming_state;
    incoming.last_update = Some(event.child.last_update.unwrap_or(event.ts));

    let Some(existing) = existing else {
        return incoming;
    };
    let existing_update = existing.last_update.unwrap_or(0);
    let incoming_update = incoming.last_update.unwrap_or(event.ts);
    if incoming_update < existing_update {
        return existing;
    }
    if terminal(&existing.state) && !terminal(&incoming.state) {
        return existing;
    }
    if terminal(&existing.state) && terminal(&incoming.state) && incoming_update == existing_update {
        return existing;
    }
    let mut merged = overlay(&existing, &incoming);
    merged.state = incoming.state.clone();
    merged.last_update = Some(existing_update.max(incoming_update));
    merged
}

fn walk_attach(
    items: Vec<NestedRunSummary>,
    event: &NestedEventRecord,
    updated: &mut bool,
) -> Vec<NestedRunSummary> {
    items
        .into_iter()
        .map(|mut item| {
            if item.id == event.parent_run_id {
                let existing = item.children.take().unwrap_or_default();
                let child_pos = existing.iter().position(|c| c.id == event.child.id);
                let next_child = merge_summary(
                    child_pos.and_then(|p| existing.get(p).cloned()),
                    event,
                );
                let mut next_children: Vec<NestedRunSummary> = Vec::with_capacity(existing.len() + 1);
                let mut replaced = false;
                for (idx, child) in existing.into_iter().enumerate() {
                    if Some(idx) == child_pos {
                        next_children.push(next_child.clone());
                        replaced = true;
                    } else {
                        next_children.push(child);
                    }
                }
                if !replaced {
                    next_children.push(next_child);
                }
                next_children.truncate(MAX_CHILDREN);
                item.children = Some(next_children);
                item.last_update = Some(item.last_update.unwrap_or(0).max(event.ts));
                *updated = true;
                item
            } else if item.children.as_ref().is_some_and(|c| !c.is_empty()) {
                let children = item.children.take().unwrap_or_default();
                item.children = Some(walk_attach(children, event, updated));
                item
            } else {
                item
            }
        })
        .collect()
}

fn attach_child(children: Vec<NestedRunSummary>, event: &NestedEventRecord) -> Vec<NestedRunSummary> {
    let mut updated = false;
    let next = walk_attach(children, event, &mut updated);
    if updated {
        return next;
    }
    // Not attached under any known parent: upsert at the top level.
    let child_pos = next.iter().position(|c| c.id == event.child.id);
    let next_child = merge_summary(child_pos.and_then(|p| next.get(p).cloned()), event);
    match child_pos {
        Some(pos) => next
            .into_iter()
            .enumerate()
            .map(|(idx, child)| if idx == pos { next_child.clone() } else { child })
            .collect(),
        None => {
            let mut out = next;
            out.push(next_child);
            out.truncate(MAX_CHILDREN);
            out
        }
    }
}

/// pi `applyNestedEvent`.
#[must_use]
pub fn apply_nested_event(registry: NestedRegistry, event: &NestedEventRecord) -> NestedRegistry {
    NestedRegistry {
        updated_at: registry.updated_at.max(event.ts),
        children: attach_child(registry.children, event),
        ..registry
    }
}

/// pi `readNestedRegistry`.
///
/// # Errors
///
/// Returns [`SubagentError`] if the route is invalid.
pub fn read_nested_registry(route: &NestedRoute) -> Result<NestedRegistry, SubagentError> {
    read_nested_registry_in(&nested_events_dir(), route)
}

/// [`read_nested_registry`] validated against an explicitly supplied nested-events root.
///
/// # Errors
///
/// See [`read_nested_registry`].
pub fn read_nested_registry_in(
    events_root: &Path,
    route: &NestedRoute,
) -> Result<NestedRegistry, SubagentError> {
    validate_route_shape_in(events_root, route)?;
    let empty = NestedRegistry {
        root_run_id: route.root_run_id.clone(),
        updated_at: 0,
        children: Vec::new(),
        processed_events: Vec::new(),
    };
    let content = match std::fs::read_to_string(registry_path(route)) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(empty),
        Err(err) => return Err(SubagentError::Spawn(err)),
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&content) else {
        return Ok(empty);
    };
    let updated_at = parsed.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
    let children = match parsed.get("children") {
        Some(Value::Array(items)) => items.iter().filter_map(sanitize_summary).collect(),
        _ => Vec::new(),
    };
    let processed_events = match parsed.get("processedEvents") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    };
    Ok(NestedRegistry {
        root_run_id: route.root_run_id.clone(),
        updated_at,
        children,
        processed_events,
    })
}

fn write_registry_atomic(path: &Path, registry: &NestedRegistry) -> Result<(), SubagentError> {
    let bytes = serde_json::to_vec_pretty(registry)
        .map_err(|err| SubagentError::MalformedSettings(format!("registry: {err}")))?;
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, &bytes).map_err(SubagentError::Spawn)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            Err(SubagentError::Spawn(err))
        }
    }
}

/// pi `projectNestedEvents`: fold every not-yet-seen event file in the sink into the registry,
/// persisting the updated registry sidecar. The grandparent's single source of descendant truth.
///
/// # Errors
///
/// Returns [`SubagentError`] if the route is invalid or the sink cannot be read.
pub fn project_nested_events(route: &NestedRoute) -> Result<NestedRegistry, SubagentError> {
    project_nested_events_in(&nested_events_dir(), route)
}

/// [`project_nested_events`] validated against an explicitly supplied nested-events root.
///
/// # Errors
///
/// See [`project_nested_events`].
pub fn project_nested_events_in(
    events_root: &Path,
    route: &NestedRoute,
) -> Result<NestedRegistry, SubagentError> {
    validate_route_shape_in(events_root, route)?;
    let mut registry = read_nested_registry_in(events_root, route)?;
    let mut seen: Vec<String> = registry.processed_events.clone();
    let mut changed = false;

    let mut entries: Vec<String> = match std::fs::read_dir(&route.event_sink) {
        Ok(read) => read
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".json") || name.ends_with(".jsonl"))
            .collect(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => return Err(SubagentError::Spawn(err)),
    };
    entries.sort();

    for entry in entries {
        if seen.contains(&entry) {
            continue;
        }
        let event_path = route.event_sink.join(&entry);
        if !contained_path(&route.event_sink, &event_path) {
            continue;
        }
        let content = match std::fs::metadata(&event_path) {
            Ok(meta) if meta.is_file() && meta.len() <= MAX_EVENT_BYTES => {
                match std::fs::read_to_string(&event_path) {
                    Ok(content) => content,
                    Err(_) => continue,
                }
            }
            _ => continue,
        };
        for event in parse_nested_event_records(&content, route) {
            registry = apply_nested_event(registry, &event);
        }
        // Reaching here means a not-yet-seen event file was consumed; the registry sidecar (which
        // records `processedEvents`) must be rewritten even if the file yielded zero valid records.
        seen.push(entry);
        changed = true;
    }

    if changed {
        // keep only the most recent 1000 processed-event names
        if seen.len() > 1000 {
            seen = seen.split_off(seen.len() - 1000);
        }
        registry.processed_events = seen;
        // Best-effort: a failed sidecar write must not lose the freshly folded in-memory registry.
        let _ = write_registry_atomic(&registry_path(route), &registry);
    }
    Ok(registry)
}

// =================================================================================================
// Route discovery (pi findNestedRouteForRootId / listNestedRoutes / projectNestedRegistryForRoot)
// =================================================================================================

fn route_from_root_dir(route_root: &Path) -> Option<NestedRoute> {
    route_from_root_dir_in(&nested_events_dir(), route_root)
}

/// [`route_from_root_dir`] validated against an explicitly supplied nested-events root.
fn route_from_root_dir_in(events_root: &Path, route_root: &Path) -> Option<NestedRoute> {
    let metadata: Value = serde_json::from_str(
        &std::fs::read_to_string(route_root.join(ROUTE_FILE)).ok()?,
    )
    .ok()?;
    let root_run_id = metadata.get("rootRunId").and_then(Value::as_str)?.to_string();
    let capability_token = metadata.get("capabilityToken").and_then(Value::as_str)?.to_string();
    let route = NestedRoute {
        root_run_id,
        event_sink: route_root.join("events"),
        control_inbox: route_root.join("controls"),
        capability_token,
    };
    validate_route_shape_in(events_root, &route).ok()?;
    Some(route)
}

/// pi `findNestedRouteForRootId`.
///
/// # Errors
///
/// Returns [`SubagentError`] if `root_run_id` is unsafe or the events dir cannot be listed.
pub fn find_nested_route_for_root_id(root_run_id: &str) -> Result<Option<NestedRoute>, SubagentError> {
    assert_safe_id("rootRunId", root_run_id)?;
    let dir = nested_events_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(read) => read,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(SubagentError::Spawn(err)),
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&format!("{root_run_id}-")) {
            continue;
        }
        if let Some(route) = route_from_root_dir(&dir.join(&name))
            && route.root_run_id == root_run_id
        {
            return Ok(Some(route));
        }
    }
    Ok(None)
}

/// pi `projectNestedRegistryForRoot`.
///
/// # Errors
///
/// Propagates [`find_nested_route_for_root_id`]/[`project_nested_events`] errors.
pub fn project_nested_registry_for_root(
    root_run_id: &str,
) -> Result<Option<NestedRegistry>, SubagentError> {
    match find_nested_route_for_root_id(root_run_id)? {
        Some(route) => Ok(Some(project_nested_events(&route)?)),
        None => Ok(None),
    }
}

/// pi `listNestedRoutes`.
///
/// # Errors
///
/// Returns [`SubagentError`] if the events dir cannot be listed.
pub fn list_nested_routes() -> Result<Vec<NestedRoute>, SubagentError> {
    list_nested_routes_in(&nested_events_dir())
}

/// [`list_nested_routes`] under an explicitly supplied nested-events root.
///
/// # Errors
///
/// See [`list_nested_routes`].
pub fn list_nested_routes_in(dir: &Path) -> Result<Vec<NestedRoute>, SubagentError> {
    let dir = dir.to_path_buf();
    let entries = match std::fs::read_dir(&dir) {
        Ok(read) => read,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(SubagentError::Spawn(err)),
    };
    Ok(entries
        .filter_map(Result::ok)
        .filter_map(|entry| route_from_root_dir_in(&dir, &dir.join(entry.file_name())))
        .collect())
}

/// pi `findNestedRun`: depth-first search for a run by id across children and step-children.
#[must_use]
pub fn find_nested_run(children: &[NestedRunSummary], id: &str) -> Option<NestedRunSummary> {
    for child in children {
        if child.id == id {
            return Some(child.clone());
        }
        if let Some(nested) = child.children.as_ref().and_then(|c| find_nested_run(c, id)) {
            return Some(nested);
        }
        let step_children = collect_step_children(child);
        if let Some(nested) = find_nested_run(&step_children, id) {
            return Some(nested);
        }
    }
    None
}

fn collect_step_children(run: &NestedRunSummary) -> Vec<NestedRunSummary> {
    run.steps
        .as_ref()
        .map(|steps| {
            steps
                .iter()
                .filter_map(|s| s.children.clone())
                .flatten()
                .collect()
        })
        .unwrap_or_default()
}

/// pi `hasLiveNestedDescendants`.
#[must_use]
pub fn has_live_nested_descendants(children: &[NestedRunSummary]) -> bool {
    for child in children {
        if !terminal(&child.state) {
            return true;
        }
        if let Some(nested) = &child.children
            && has_live_nested_descendants(nested)
        {
            return true;
        }
        if has_live_nested_descendants(&collect_step_children(child)) {
            return true;
        }
    }
    false
}

// =================================================================================================
// Control routing (pi write/read NestedControlRequest / NestedControlResult)
// =================================================================================================

fn parse_control_request(content: &str, route: &NestedRoute) -> Option<NestedControlRequestRecord> {
    if content.len() as u64 > MAX_EVENT_BYTES {
        return None;
    }
    let parsed: Value = serde_json::from_str(content).ok()?;
    let obj = parsed.as_object()?;
    if obj.get("type").and_then(Value::as_str) != Some("subagent.nested.control-request") {
        return None;
    }
    if obj.get("rootRunId").and_then(Value::as_str) != Some(route.root_run_id.as_str())
        || obj.get("capabilityToken").and_then(Value::as_str) != Some(route.capability_token.as_str())
    {
        return None;
    }
    let request_id = obj.get("requestId").and_then(Value::as_str).filter(|s| is_safe_nested_path_id_str(s))?;
    let target_run_id = obj.get("targetRunId").and_then(Value::as_str).filter(|s| is_safe_nested_path_id_str(s))?;
    let action = obj.get("action").and_then(Value::as_str)?;
    if action != "interrupt" && action != "resume" {
        return None;
    }
    let ts = finite_number(obj.get("ts"))? as i64;
    Some(NestedControlRequestRecord {
        event_type: "subagent.nested.control-request".to_string(),
        ts,
        root_run_id: route.root_run_id.clone(),
        capability_token: route.capability_token.clone(),
        request_id: request_id.to_string(),
        target_run_id: target_run_id.to_string(),
        action: action.to_string(),
        message: string_value(obj.get("message"), 16_000),
    })
}

fn parse_control_result(content: &str, route: &NestedRoute) -> Option<NestedControlResultRecord> {
    if content.len() as u64 > MAX_EVENT_BYTES {
        return None;
    }
    let parsed: Value = serde_json::from_str(content).ok()?;
    let obj = parsed.as_object()?;
    if obj.get("type").and_then(Value::as_str) != Some("subagent.nested.control-result") {
        return None;
    }
    if obj.get("rootRunId").and_then(Value::as_str) != Some(route.root_run_id.as_str())
        || obj.get("capabilityToken").and_then(Value::as_str) != Some(route.capability_token.as_str())
    {
        return None;
    }
    let request_id = obj.get("requestId").and_then(Value::as_str).filter(|s| is_safe_nested_path_id_str(s))?;
    let target_run_id = obj.get("targetRunId").and_then(Value::as_str).filter(|s| is_safe_nested_path_id_str(s))?;
    let ts = finite_number(obj.get("ts"))? as i64;
    let ok = obj.get("ok").and_then(Value::as_bool)?;
    let message = string_value(obj.get("message"), 16_000).unwrap_or_else(|| {
        if ok {
            "Control request completed.".to_string()
        } else {
            "Control request failed.".to_string()
        }
    });
    Some(NestedControlResultRecord {
        event_type: "subagent.nested.control-result".to_string(),
        ts,
        root_run_id: route.root_run_id.clone(),
        capability_token: route.capability_token.clone(),
        request_id: request_id.to_string(),
        target_run_id: target_run_id.to_string(),
        ok,
        message,
    })
}

/// The caller portion of a control request (pi `Omit<..., "type" | "rootRunId" |
/// "capabilityToken">`).
#[derive(Debug, Clone)]
pub struct NestedControlRequestInput {
    pub ts: i64,
    pub request_id: String,
    pub target_run_id: String,
    pub action: String,
    pub message: Option<String>,
}

/// pi `writeNestedControlRequest`.
///
/// # Errors
///
/// Returns [`SubagentError`] if the route/ids are invalid or the record fails validation.
pub fn write_nested_control_request(
    route: &NestedRoute,
    request: &NestedControlRequestInput,
) -> Result<PathBuf, SubagentError> {
    validate_route_shape(route)?;
    assert_safe_id("requestId", &request.request_id)?;
    assert_safe_id("targetRunId", &request.target_run_id)?;
    let record = NestedControlRequestRecord {
        event_type: "subagent.nested.control-request".to_string(),
        ts: request.ts,
        root_run_id: route.root_run_id.clone(),
        capability_token: route.capability_token.clone(),
        request_id: request.request_id.clone(),
        target_run_id: request.target_run_id.clone(),
        action: request.action.clone(),
        message: request.message.clone(),
    };
    let serialized = serde_json::to_string(&record)
        .map_err(|err| SubagentError::MalformedSettings(format!("control request: {err}")))?;
    let sanitized = parse_control_request(&serialized, route).ok_or_else(|| {
        SubagentError::MalformedSettings("Nested control request failed validation.".to_string())
    })?;
    write_route_record(&route.control_inbox, sanitized.ts, &sanitized)
}

/// pi `readNestedControlRequests`.
///
/// # Errors
///
/// Returns [`SubagentError`] if the route is invalid or the inbox cannot be read.
pub fn read_nested_control_requests(
    route: &NestedRoute,
) -> Result<Vec<(NestedControlRequestRecord, PathBuf)>, SubagentError> {
    validate_route_shape(route)?;
    let mut entries: Vec<String> = match std::fs::read_dir(&route.control_inbox) {
        Ok(read) => read
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".json"))
            .collect(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => return Err(SubagentError::Spawn(err)),
    };
    entries.sort();
    let mut requests = Vec::new();
    for entry in entries {
        let file_path = route.control_inbox.join(&entry);
        if !contained_path(&route.control_inbox, &file_path) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&file_path) else { continue };
        if !meta.is_file() || meta.len() > MAX_EVENT_BYTES {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&file_path) else { continue };
        if let Some(request) = parse_control_request(&content, route) {
            requests.push((request, file_path));
        }
    }
    Ok(requests)
}

/// The caller portion of a control result (pi `Omit<..., "type" | "rootRunId" |
/// "capabilityToken">`).
#[derive(Debug, Clone)]
pub struct NestedControlResultInput {
    pub ts: i64,
    pub request_id: String,
    pub target_run_id: String,
    pub ok: bool,
    pub message: String,
}

/// pi `writeNestedControlResult`: the descendant answers a request by appending a result into the
/// event sink (so the grandparent's `readNestedControlResults` scan picks it up).
///
/// # Errors
///
/// Returns [`SubagentError`] if the route/ids are invalid or the record fails validation.
pub fn write_nested_control_result(
    route: &NestedRoute,
    result: &NestedControlResultInput,
) -> Result<(), SubagentError> {
    validate_route_shape(route)?;
    assert_safe_id("requestId", &result.request_id)?;
    assert_safe_id("targetRunId", &result.target_run_id)?;
    let record = NestedControlResultRecord {
        event_type: "subagent.nested.control-result".to_string(),
        ts: result.ts,
        root_run_id: route.root_run_id.clone(),
        capability_token: route.capability_token.clone(),
        request_id: result.request_id.clone(),
        target_run_id: result.target_run_id.clone(),
        ok: result.ok,
        message: result.message.clone(),
    };
    let serialized = serde_json::to_string(&record)
        .map_err(|err| SubagentError::MalformedSettings(format!("control result: {err}")))?;
    let sanitized = parse_control_result(&serialized, route).ok_or_else(|| {
        SubagentError::MalformedSettings("Nested control result failed validation.".to_string())
    })?;
    write_route_record(&route.event_sink, sanitized.ts, &sanitized)?;
    Ok(())
}

/// pi `readNestedControlResults`.
///
/// # Errors
///
/// Returns [`SubagentError`] if the route is invalid or the sink cannot be read.
pub fn read_nested_control_results(
    route: &NestedRoute,
) -> Result<Vec<NestedControlResultRecord>, SubagentError> {
    validate_route_shape(route)?;
    let mut entries: Vec<String> = match std::fs::read_dir(&route.event_sink) {
        Ok(read) => read
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".json") || name.ends_with(".jsonl"))
            .collect(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => return Err(SubagentError::Spawn(err)),
    };
    entries.sort();
    let mut results = Vec::new();
    for entry in entries {
        let event_path = route.event_sink.join(&entry);
        if !contained_path(&route.event_sink, &event_path) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&event_path) else { continue };
        if !meta.is_file() || meta.len() > MAX_EVENT_BYTES {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&event_path) else { continue };
        let lines: Vec<&str> = if content.contains('\n') {
            content.split('\n').filter(|l| !l.trim().is_empty()).collect()
        } else {
            vec![content.as_str()]
        };
        for line in lines {
            if let Some(result) = parse_control_result(line, route) {
                results.push(result);
            }
        }
    }
    Ok(results)
}

// =================================================================================================
// Nested storage helpers (pi nestedResultsPath / isTopLevelAsyncDir / resolveNestedAsyncDir /
// nestedArtifactEnv)
// =================================================================================================

/// pi `nestedResultsPath`.
///
/// # Errors
///
/// Returns [`SubagentError`] if `root_run_id`/`id` are unsafe.
pub fn nested_results_path(root_run_id: &str, id: &str) -> Result<PathBuf, SubagentError> {
    assert_safe_id("rootRunId", root_run_id)?;
    assert_safe_id("id", id)?;
    Ok(results_dir().join("nested").join(root_run_id).join(format!("{id}.json")))
}

/// The results-DIRECTORY a nested run's terminal result file (via [`nested_results_path`]) lives
/// under, for the SAME root — i.e. `nested_results_path(root, id)`'s parent, one call before `id` is
/// known (a background spawn needs the directory to hand to [`crate::background::RunPaths::for_run`]
/// as its own `results_dir`, not the final per-run file path).
///
/// # Errors
///
/// Returns [`SubagentError`] if `root_run_id` is unsafe.
pub fn nested_results_dir(root_run_id: &str) -> Result<PathBuf, SubagentError> {
    assert_safe_id("rootRunId", root_run_id)?;
    Ok(results_dir().join("nested").join(root_run_id))
}

/// pi's `path.join(TEMP_ROOT_DIR, "nested-subagent-runs", rootRunId)` (`async-execution.ts:587-589,
/// 828-830`) — the async-ROOT (not one run's own dir) a nested run's `RunPaths` are derived under,
/// for the given root run id. A background spawn resolves this ONCE per inherited route and hands it
/// to [`crate::background::RunPaths::for_run`] as `async_root`, so `RunDir::new` derives the SAME
/// `<TEMP_ROOT>/nested-subagent-runs/<rootRunId>/<id>` pi's own `asyncDir` resolves to.
///
/// # Errors
///
/// Returns [`SubagentError`] if `root_run_id` is unsafe.
pub fn nested_async_root(root_run_id: &str) -> Result<PathBuf, SubagentError> {
    assert_safe_id("rootRunId", root_run_id)?;
    Ok(nested_runs_dir().join(root_run_id))
}

/// pi `isTopLevelAsyncDir`: contained in the async root but not under the nested-runs root.
#[must_use]
pub fn is_top_level_async_dir(async_dir_path: &Path) -> bool {
    let resolved = std::path::absolute(async_dir_path).unwrap_or_else(|_| async_dir_path.to_path_buf());
    contained_path(&async_dir(), &resolved) && !contained_path(&nested_runs_dir(), &resolved)
}

/// pi `resolveNestedAsyncDir`: accept a run's `asyncDir` only when it stays inside its expected
/// nested storage subtree.
#[must_use]
pub fn resolve_nested_async_dir(root_run_id: &str, run: &NestedRunSummary) -> Option<PathBuf> {
    resolve_nested_async_dir_in(&nested_runs_dir(), root_run_id, run)
}

/// [`resolve_nested_async_dir`] against an explicitly supplied nested-runs root.
///
/// As with the route guard, the CHECK is fixed and only the trusted root varies — a descendant
/// whose `async_dir` escapes the tree the caller owns is still refused.
#[must_use]
pub fn resolve_nested_async_dir_in(
    nested_runs_root: &Path,
    root_run_id: &str,
    run: &NestedRunSummary,
) -> Option<PathBuf> {
    let async_dir = run.async_dir.as_ref()?;
    let resolved = std::path::absolute(async_dir).unwrap_or_else(|_| PathBuf::from(async_dir));
    let nested_root = nested_runs_root.join(root_run_id).join(&run.id);
    if resolved == nested_root || resolved.starts_with(&nested_root) {
        Some(resolved)
    } else {
        None
    }
}

/// pi `nestedArtifactEnv`.
#[must_use]
pub fn nested_artifact_env(root_run_id: &str, parent_run_id: &str) -> HashMap<String, String> {
    HashMap::from([
        ("CYRUP_SUBAGENT_NESTED_ROOT_RUN_ID".to_string(), root_run_id.to_string()),
        ("CYRUP_SUBAGENT_NESTED_PARENT_RUN_ID".to_string(), parent_run_id.to_string()),
    ])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

    use super::*;
    use std::process::Command as StdCommand;

    /// Unique root id per test so parallel tests never share a route directory (routes are also
    /// keyed by a random capability token, so this is belt-and-suspenders).
    fn unique_root() -> String {
        format!("root-{}", uuid::Uuid::new_v4().simple())
    }

    fn cleanup(route: &NestedRoute) {
        if let Some(root) = route.event_sink.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }

    fn child_summary(id: &str, state: &str, ts: i64, parent: &str) -> NestedRunSummary {
        NestedRunSummary {
            id: id.to_string(),
            parent_run_id: parent.to_string(),
            parent_step_index: Some(1),
            parent_agent: None,
            depth: 1,
            path: vec![NestedPathEntry { run_id: parent.to_string(), step_index: Some(1), agent: None }],
            async_dir: None,
            pid: None,
            session_id: None,
            session_file: None,
            intercom_target: None,
            owner_intercom_target: None,
            leaf_intercom_target: None,
            owner_state: None,
            control_inbox: None,
            capability_token: None,
            mode: Some("single".to_string()),
            state: state.to_string(),
            agent: Some("reviewer".to_string()),
            agents: Some(vec!["reviewer".to_string()]),
            current_step: None,
            chain_step_count: None,
            activity_state: None,
            last_activity_at: None,
            current_tool: None,
            current_tool_started_at: None,
            current_path: None,
            turn_count: None,
            tool_count: None,
            total_tokens: None,
            total_cost: None,
            started_at: Some(10),
            ended_at: None,
            last_update: Some(ts),
            error: None,
            steps: Some(vec![NestedStepSummary {
                agent: "leaf".to_string(),
                status: if state == "running" { "running" } else { "complete" }.to_string(),
                session_file: None,
                activity_state: None,
                last_activity_at: None,
                current_tool: None,
                current_tool_started_at: None,
                current_path: None,
                turn_count: None,
                tool_count: None,
                started_at: None,
                ended_at: None,
                error: None,
                children: None,
            }]),
            children: None,
        }
    }

    // ---- MANDATED: a nested run's events reach the grandparent route (via a REAL subprocess) ----

    #[test]
    fn nested_run_events_reach_the_grandparent_route_via_real_subprocess() {
        if cfg!(windows) {
            return;
        }
        let root = unique_root();
        let route = create_nested_route(&root).expect("mint route");

        // The grandparent hands ONLY the route env down (as it would to a fanout-authorized child).
        let env = nested_route_env(&route);
        assert_eq!(env.get(PARENT_ROOT_RUN_ID_ENV), Some(&root));

        // A real, separate OS process (the "nested descendant") writes a status event into the
        // sink the grandparent handed it through the environment — no in-process shortcut.
        let script = r#"
sink="$CYRUP_SUBAGENT_PARENT_EVENT_SINK"
root="$CYRUP_SUBAGENT_PARENT_ROOT_RUN_ID"
token="$CYRUP_SUBAGENT_PARENT_CAPABILITY_TOKEN"
printf '{"type":"subagent.nested.started","ts":100,"rootRunId":"%s","parentRunId":"%s","parentStepIndex":1,"capabilityToken":"%s","child":{"id":"nested-a","parentRunId":"%s","parentStepIndex":1,"depth":1,"path":[{"runId":"%s","stepIndex":1}],"mode":"single","state":"running","agent":"reviewer","steps":[{"agent":"leaf","status":"running"}]}}\n' \
  "$root" "$root" "$token" "$root" "$root" > "$sink/0000000000100-evt.json"
"#;
        let status = StdCommand::new("sh")
            .arg("-c")
            .arg(script)
            .envs(env.iter())
            .status()
            .expect("child sh spawns");
        assert!(status.success(), "the nested descendant subprocess must succeed");

        // The grandparent projects the sink and sees the descendant it never spawned directly.
        let registry = project_nested_events(&route).expect("project");
        assert_eq!(registry.children.len(), 1, "grandparent must see exactly one nested run");
        assert_eq!(registry.children[0].id, "nested-a");
        assert_eq!(registry.children[0].state, "running");
        assert_eq!(registry.children[0].steps.as_ref().unwrap()[0].agent, "leaf");
        // The route stamped the child with its control coordinates (so the grandparent can steer it).
        assert_eq!(
            registry.children[0].control_inbox.as_deref(),
            Some(route.control_inbox.to_string_lossy().as_ref())
        );
        assert_eq!(registry.children[0].capability_token.as_deref(), Some(route.capability_token.as_str()));

        cleanup(&route);
    }

    #[test]
    fn projects_started_updated_completed_into_registry() {
        let root = unique_root();
        let route = create_nested_route(&root).expect("route");
        for (ty, ts, state) in [
            ("subagent.nested.started", 100, "running"),
            ("subagent.nested.updated", 200, "running"),
            ("subagent.nested.completed", 300, "complete"),
        ] {
            write_nested_event(
                &route,
                &NestedEventInput {
                    event_type: ty.to_string(),
                    ts,
                    parent_run_id: root.clone(),
                    parent_step_index: Some(1),
                    child: child_summary("nested-a", state, ts, &root),
                },
            )
            .expect("write");
        }
        let registry = project_nested_events(&route).expect("project");
        assert_eq!(registry.children.len(), 1);
        assert_eq!(registry.children[0].id, "nested-a");
        assert_eq!(registry.children[0].state, "complete");
        cleanup(&route);
    }

    #[test]
    fn ignores_wrong_token_and_preserves_terminal_state() {
        let root = unique_root();
        let route = create_nested_route(&root).expect("route");
        write_nested_event(
            &route,
            &NestedEventInput {
                event_type: "subagent.nested.completed".to_string(),
                ts: 300,
                parent_run_id: root.clone(),
                parent_step_index: Some(1),
                child: child_summary("nested-terminal", "complete", 300, &root),
            },
        )
        .expect("write terminal");

        // A stale update (older lastUpdate) must NOT regress the terminal state.
        let stale = serde_json::json!({
            "type": "subagent.nested.updated", "ts": 400, "rootRunId": root,
            "parentRunId": root, "parentStepIndex": 1, "capabilityToken": route.capability_token,
            "child": child_summary("nested-terminal", "running", 100, &root),
        });
        std::fs::write(route.event_sink.join("0000000000400-stale.json"), format!("{stale}\n")).unwrap();
        // A wrong-token record must be dropped entirely.
        let wrong = serde_json::json!({
            "type": "subagent.nested.started", "ts": 500, "rootRunId": root,
            "parentRunId": root, "parentStepIndex": 1, "capabilityToken": "wrong",
            "child": child_summary("wrong-token", "running", 500, &root),
        });
        std::fs::write(route.event_sink.join("0000000000500-wrong.json"), format!("{wrong}\n")).unwrap();

        let registry = project_nested_events(&route).expect("project");
        assert_eq!(
            registry.children.iter().find(|c| c.id == "nested-terminal").map(|c| c.state.as_str()),
            Some("complete")
        );
        assert!(!registry.children.iter().any(|c| c.id == "wrong-token"));
        cleanup(&route);
    }

    #[test]
    fn env_route_round_trips_and_rejects_wrong_token() {
        let root = unique_root();
        let route = create_nested_route(&root).expect("route");
        let base = nested_route_env(&route);
        let good = base.clone();
        let resolved = resolve_nested_route_from_env(|k| good.get(k).cloned())
            .expect("resolve")
            .expect("some");
        assert_eq!(resolved, route);

        let mut bad = base;
        bad.insert(PARENT_CAPABILITY_TOKEN_ENV.to_string(), "wrong-token".to_string());
        assert!(resolve_nested_route_from_env(|k| bad.get(k).cloned()).is_err());
        cleanup(&route);
    }

    #[test]
    fn resolves_nested_parent_address_with_full_inherited_path() {
        let path = serde_json::json!([
            { "runId": "root-run", "stepIndex": 0, "agent": "root-agent" },
            { "runId": "../unsafe", "stepIndex": 1, "agent": "bad" },
            { "runId": "nested-parent", "stepIndex": 2, "agent": "nested-agent" },
        ])
        .to_string();
        let env = HashMap::from([
            (PARENT_RUN_ID_ENV.to_string(), "nested-parent".to_string()),
            (PARENT_CHILD_INDEX_ENV.to_string(), "2".to_string()),
            (PARENT_DEPTH_ENV.to_string(), "3".to_string()),
            (PARENT_PATH_ENV.to_string(), path),
        ]);
        let address = resolve_nested_parent_address_from_env(|k| env.get(k).cloned()).expect("address");
        assert_eq!(address.parent_run_id, "nested-parent");
        assert_eq!(address.parent_step_index, Some(2));
        assert_eq!(address.depth, 3);
        assert_eq!(address.path.len(), 2, "the unsafe hop is dropped");
        assert_eq!(address.path[0].run_id, "root-run");
        assert_eq!(address.path[1].run_id, "nested-parent");
    }

    #[test]
    fn ignores_unsafe_nested_parent_ids() {
        let env = HashMap::from([
            (PARENT_RUN_ID_ENV.to_string(), "../unsafe".to_string()),
            (PARENT_CHILD_INDEX_ENV.to_string(), "2".to_string()),
        ]);
        assert!(resolve_nested_parent_address_from_env(|k| env.get(k).cloned()).is_none());
    }

    #[test]
    fn fanout_child_env_gates_route_behind_authorization() {
        let root = unique_root();
        let route = create_nested_route(&root).expect("route");
        let address = NestedParentAddress {
            parent_run_id: root.clone(),
            parent_step_index: Some(1),
            depth: 1,
            path: vec![NestedPathEntry { run_id: root.clone(), step_index: Some(1), agent: None }],
        };

        let authed = nested_child_auth_env(true, Some(&route), Some(&address));
        assert_eq!(authed.get(FANOUT_CHILD_ENV), Some(&"1".to_string()));
        assert_eq!(authed.get(PARENT_ROOT_RUN_ID_ENV), Some(&root));
        assert_eq!(authed.get(PARENT_CAPABILITY_TOKEN_ENV), Some(&route.capability_token));

        let denied = nested_child_auth_env(false, Some(&route), Some(&address));
        assert_eq!(denied.get(FANOUT_CHILD_ENV), Some(&"0".to_string()));
        assert_eq!(denied.get(PARENT_ROOT_RUN_ID_ENV), Some(&String::new()));
        assert_eq!(denied.get(PARENT_CAPABILITY_TOKEN_ENV), Some(&String::new()));
        cleanup(&route);
    }

    #[test]
    fn control_request_and_result_round_trip() {
        let root = unique_root();
        let route = create_nested_route(&root).expect("route");
        write_nested_control_request(
            &route,
            &NestedControlRequestInput {
                ts: 100,
                request_id: "req1".to_string(),
                target_run_id: "nested-a".to_string(),
                action: "interrupt".to_string(),
                message: Some("stop".to_string()),
            },
        )
        .expect("write request");
        let requests = read_nested_control_requests(&route).expect("read");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0.action, "interrupt");
        assert_eq!(requests[0].0.target_run_id, "nested-a");

        write_nested_control_result(
            &route,
            &NestedControlResultInput {
                ts: 150,
                request_id: "req1".to_string(),
                target_run_id: "nested-a".to_string(),
                ok: true,
                message: "interrupted".to_string(),
            },
        )
        .expect("write result");
        let results = read_nested_control_results(&route).expect("read results");
        assert_eq!(results.len(), 1);
        assert!(results[0].ok);
        cleanup(&route);
    }

    #[test]
    fn parse_records_drops_trailing_partial_jsonl_line() {
        let root = unique_root();
        let route = create_nested_route(&root).expect("route");
        let good = serde_json::json!({
            "type": "subagent.nested.started", "ts": 100, "rootRunId": root,
            "parentRunId": root, "parentStepIndex": 1, "capabilityToken": route.capability_token,
            "child": child_summary("jsonl-good", "running", 100, &root),
        });
        let content = format!("{good}\n{{\"type\":\"subagent.nested.started\"");
        let records = parse_nested_event_records(&content, &route);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].child.id, "jsonl-good");
        cleanup(&route);
    }
}
