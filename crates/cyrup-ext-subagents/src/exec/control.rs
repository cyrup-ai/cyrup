//! Subagent live-control: config resolution + the control-event/notice pipeline.
//!
//! Port of `pi-subagents/src/runs/shared/subagent-control.ts` (the whole file, v0.34.0 baseline:
//! `resolveControlConfig`/`DEFAULT_CONTROL_CONFIG` `:10-71`, `deriveActivityState` `:73-85`,
//! `buildControlEvent` `:87-135`, `shouldNotifyControlEvent`/`controlNotificationKey`/
//! `claimControlNotification` `:137-152`, `formatLongRunningFacts` `:154-163`,
//! `formatControlNoticeMessage` `:165-212`, `formatControlIntercomMessage` `:214-231`), plus the
//! control-relevant half of `pi-subagents/src/runs/shared/long-running-guard.ts`
//! (`resolveCurrentPath` `:54-67`, `isMutatingTool` `:99-110`, `didMutatingToolFail` `:112-115`,
//! `nextLongRunningTrigger` `:117-126`, the `MutatingFailureState` machinery `:128-172`), and the
//! per-attempt live state machine `runSingleAttempt` builds out of them
//! (`pi-subagents/src/runs/foreground/execution.ts:344-354,578-722,775-890,896-905,1234-1247`) —
//! here reified as [`ControlMonitor`] so `exec::drive_attempt` can drive it from one place instead
//! of scattering a dozen closures through the NDJSON loop.
//!
//! `isMutatingBashCommand` is deliberately NOT re-ported here: [`crate::exec::completion_guard`]
//! already owns the verbatim port of it (same upstream file), and this module reuses that one
//! canonical implementation.
//!
//! # Where the notice half lands
//!
//! Upstream splits the pipeline in two: `execution.ts` RAISES `ControlEvent`s from the child's
//! stdout and hands each to `options.onControlEvent`; `subagent-executor.ts:801-831` @v0.43.0
//! (`emitControlNotification`) then decides which CHANNELS a raised event travels
//! (`notifyChannels`), and `extension/control-notices.ts` debounces/re-validates/dedups the
//! resulting transcript notice. This module owns the first half (raise + the two message
//! formatters the second half renders); the second half is
//! [`crate::tui::notices::ControlNoticeState`], which this crate already carried fully ported and
//! fully tested but with no producer — the bridge that finally feeds it is
//! `extension::SubagentExecutor::foreground_control_notifier`.

use std::collections::HashSet;

use crate::background::ActivityState;
use crate::exec::completion_guard::is_mutating_bash_command;
use crate::exec::ndjson::SubagentEvent;
use crate::registration::{ControlConfig, ControlEventType, ControlNotificationChannel};

// =================================================================================================
// Defaults + resolution (subagent-control.ts:10-71)
// =================================================================================================

/// `DEFAULT_CONTROL_CONFIG.needsAttentionAfterMs` (`subagent-control.ts:16`).
pub const DEFAULT_NEEDS_ATTENTION_AFTER_MS: i64 = 60_000;
/// `DEFAULT_CONTROL_CONFIG.activeNoticeAfterMs` (`subagent-control.ts:17`).
pub const DEFAULT_ACTIVE_NOTICE_AFTER_MS: i64 = 240_000;
/// `DEFAULT_CONTROL_CONFIG.failedToolAttemptsBeforeAttention` (`subagent-control.ts:18`).
pub const DEFAULT_FAILED_TOOL_ATTEMPTS_BEFORE_ATTENTION: u32 = 3;

/// The activity-timer period the per-attempt drive loop re-evaluates the idle/long-running
/// heuristics on (pi `setInterval(..., 1000)`, `execution.ts:897-905`).
pub const ACTIVITY_TICK_MS: u64 = 1_000;

/// The rolling window a mutating-tool failure streak is counted over (pi
/// `mutatingFailureWindowMs = 5 * 60_000`, `execution.ts:588`).
pub const MUTATING_FAILURE_WINDOW_MS: i64 = 5 * 60_000;

/// pi `ResolvedControlConfig` (`shared/types.ts:169-178`): the fully-defaulted view
/// [`resolve_control_config`] derives from the extension-level [`ControlConfig`] plus one call's
/// own override. Every consumer in this crate reads THIS type, never the sparse wire shape.
///
/// Serializable because the ASYNC path resolves it ORCHESTRATOR-side and carries the resolved value
/// to the detached hop-2 runner in `runner-config.json`
/// ([`crate::background::runner_main::RunnerConfig::control`]) — exactly as upstream does, where
/// `runSinglePath` computes `resolveControlConfig(deps.config.control, params.control)` and passes
/// the RESOLVED object into `executeAsyncSingle` (`subagent-executor.ts:2845,2868` @v0.34.0), which the
/// runner then reads back as `config.controlConfig ?? DEFAULT_CONTROL_CONFIG`
/// (`subagent-runner.ts:1802`, all @v0.34.0). Resolving parent-side is load-bearing, not stylistic:
/// the detached runner has no settings access by design, so re-resolving inside it could apply a
/// *different* `subagents.control` block than the one in force when the run was authorized.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedControlConfig {
    /// Master enable. `false` disables every raise path in [`ControlMonitor`] outright.
    pub enabled: bool,
    /// Idle window (ms) past which a run with no in-flight tool is `needs_attention`.
    pub needs_attention_after_ms: i64,
    /// Elapsed window (ms) past which a still-active run raises `active_long_running`.
    pub active_notice_after_ms: i64,
    /// Optional assistant-turn threshold for `active_long_running` (disabled by default).
    pub active_notice_after_turns: Option<u64>,
    /// Optional total-token threshold for `active_long_running` (disabled by default).
    pub active_notice_after_tokens: Option<u64>,
    /// Consecutive (or same-path) mutating-tool failures that escalate to `needs_attention`.
    pub failed_tool_attempts_before_attention: u32,
    /// Which event classes actually notify (`shouldNotifyControlEvent`).
    pub notify_on: Vec<ControlEventType>,
    /// Which channels a notified event travels (`emitControlNotification`).
    pub notify_channels: Vec<ControlNotificationChannel>,
}

impl Default for ResolvedControlConfig {
    /// `DEFAULT_CONTROL_CONFIG` (`subagent-control.ts:14-21`) verbatim: enabled, 60s attention,
    /// 240s long-running, 3 failed mutating attempts, both event types, all three channels.
    fn default() -> Self {
        Self {
            enabled: true,
            needs_attention_after_ms: DEFAULT_NEEDS_ATTENTION_AFTER_MS,
            active_notice_after_ms: DEFAULT_ACTIVE_NOTICE_AFTER_MS,
            active_notice_after_turns: None,
            active_notice_after_tokens: None,
            failed_tool_attempts_before_attention:
                DEFAULT_FAILED_TOOL_ATTEMPTS_BEFORE_ATTENTION,
            notify_on: vec![
                ControlEventType::ActiveLongRunning,
                ControlEventType::NeedsAttention,
            ],
            notify_channels: vec![
                ControlNotificationChannel::Event,
                ControlNotificationChannel::Async,
                ControlNotificationChannel::Intercom,
            ],
        }
    }
}

/// pi `parsePositiveInt` (`subagent-control.ts:23-27`): only a finite integer `>= 1` survives.
/// Rust's `Option<u64>` already excludes the non-finite/non-integer/negative cases the source has
/// to test for, so this reduces to rejecting `0` (and anything that cannot be an `i64` ms count).
fn parse_positive_ms(value: Option<u64>) -> Option<i64> {
    let value = value?;
    if value < 1 {
        return None;
    }
    i64::try_from(value).ok()
}

/// The turns/tokens flavour of [`parse_positive_ms`] — same `>= 1` rule, no ms conversion.
fn parse_positive_count(value: Option<u64>) -> Option<u64> {
    value.filter(|n| *n >= 1)
}

/// pi `parseControlList` (`subagent-control.ts:29-35`): a non-array is `undefined`; an EXPLICIT
/// empty array is a meaningful `[]` (which therefore wins over the default, disabling the list);
/// otherwise the allowed entries are de-duplicated preserving first-seen order, and a list whose
/// entries were ALL rejected is `undefined`.
///
/// The "reject unknown entries" step has no work to do against an already-typed `Vec<T>` — that
/// filtering happens one layer out, in [`parse_control_overrides`], which is where an untyped wire
/// value is first seen.
fn parse_control_list<T: Copy + PartialEq>(value: Option<&Vec<T>>) -> Option<Vec<T>> {
    let value = value?;
    if value.is_empty() {
        return Some(Vec::new());
    }
    let mut out: Vec<T> = Vec::with_capacity(value.len());
    for entry in value {
        if !out.contains(entry) {
            out.push(*entry);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// pi `resolveControlConfig` (`subagent-control.ts:37-71`): per-call override wins, then the
/// extension-level config, then [`ResolvedControlConfig::default`] — evaluated FIELD BY FIELD, so
/// an override that sets only `needsAttentionAfterMs` inherits every other field from the global
/// config rather than replacing it wholesale.
#[must_use]
pub fn resolve_control_config(
    global: Option<&ControlConfig>,
    call_override: Option<&ControlConfig>,
) -> ResolvedControlConfig {
    let defaults = ResolvedControlConfig::default();
    let enabled = call_override
        .and_then(|c| c.enabled)
        .or_else(|| global.and_then(|c| c.enabled))
        .unwrap_or(defaults.enabled);
    let needs_attention_after_ms =
        parse_positive_ms(call_override.and_then(|c| c.needs_attention_after_ms))
            .or_else(|| parse_positive_ms(global.and_then(|c| c.needs_attention_after_ms)))
            .unwrap_or(defaults.needs_attention_after_ms);
    let active_notice_after_ms =
        parse_positive_ms(call_override.and_then(|c| c.active_notice_after_ms))
            .or_else(|| parse_positive_ms(global.and_then(|c| c.active_notice_after_ms)))
            .unwrap_or(defaults.active_notice_after_ms);
    let active_notice_after_turns =
        parse_positive_count(call_override.and_then(|c| c.active_notice_after_turns))
            .or_else(|| parse_positive_count(global.and_then(|c| c.active_notice_after_turns)));
    let active_notice_after_tokens =
        parse_positive_count(call_override.and_then(|c| c.active_notice_after_tokens))
            .or_else(|| parse_positive_count(global.and_then(|c| c.active_notice_after_tokens)));
    let failed_tool_attempts_before_attention = parse_positive_count(
        call_override.and_then(|c| c.failed_tool_attempts_before_attention).map(u64::from),
    )
    .or_else(|| {
        parse_positive_count(
            global.and_then(|c| c.failed_tool_attempts_before_attention).map(u64::from),
        )
    })
    .and_then(|n| u32::try_from(n).ok())
    .unwrap_or(defaults.failed_tool_attempts_before_attention);
    let notify_on = parse_control_list(call_override.and_then(|c| c.notify_on.as_ref()))
        .or_else(|| parse_control_list(global.and_then(|c| c.notify_on.as_ref())))
        .unwrap_or(defaults.notify_on);
    let notify_channels =
        parse_control_list(call_override.and_then(|c| c.notify_channels.as_ref()))
            .or_else(|| parse_control_list(global.and_then(|c| c.notify_channels.as_ref())))
            .unwrap_or(defaults.notify_channels);
    ResolvedControlConfig {
        enabled,
        needs_attention_after_ms,
        active_notice_after_ms,
        active_notice_after_turns,
        active_notice_after_tokens,
        failed_tool_attempts_before_attention,
        notify_on,
        notify_channels,
    }
}

/// Tolerant lowering of the tool's raw `control` object into [`ControlConfig`].
///
/// `resolveControlConfig` is itself defensive about wire shapes (`parsePositiveInt` returns
/// `undefined` for a non-number; `parseControlList` filters out entries outside the allowed union),
/// so a `control` object carrying a wrong-typed field or an unknown `notifyOn` string must degrade
/// to "that field was not supplied" rather than failing the whole tool call. A plain
/// `serde_json::from_value::<ControlConfig>` would instead hard-error, which is a behavioural
/// divergence a caller would experience as a refused run — so the wire lowering is done by hand
/// here, one field at a time, exactly matching the source's per-field tolerance.
#[must_use]
pub fn parse_control_overrides(raw: &serde_json::Value) -> ControlConfig {
    fn positive_u64(raw: &serde_json::Value, key: &str) -> Option<u64> {
        let value = raw.get(key)?;
        // `parsePositiveInt` demands `typeof value === "number" && Number.isInteger(value)`, so a
        // float (2.5) and a numeric string ("2") are both rejected, not coerced.
        if !value.is_i64() && !value.is_u64() {
            return None;
        }
        value.as_u64().filter(|n| *n >= 1)
    }
    fn string_list<T: Copy + PartialEq>(
        raw: &serde_json::Value,
        key: &str,
        allowed: &[(&str, T)],
    ) -> Option<Vec<T>> {
        let entries = raw.get(key)?.as_array()?;
        // An explicit `[]` is preserved as `[]` (source: `if (value.length === 0) return []`).
        let mut out: Vec<T> = Vec::with_capacity(entries.len());
        for entry in entries {
            let Some(text) = entry.as_str() else { continue };
            let Some((_, mapped)) = allowed.iter().find(|(name, _)| *name == text) else {
                continue;
            };
            if !out.contains(mapped) {
                out.push(*mapped);
            }
        }
        if out.is_empty() && !entries.is_empty() {
            // Every entry was rejected — the source's `parsed.length > 0 ? … : undefined`.
            return None;
        }
        Some(out)
    }

    ControlConfig {
        enabled: raw.get("enabled").and_then(serde_json::Value::as_bool),
        needs_attention_after_ms: positive_u64(raw, "needsAttentionAfterMs"),
        active_notice_after_ms: positive_u64(raw, "activeNoticeAfterMs"),
        active_notice_after_turns: positive_u64(raw, "activeNoticeAfterTurns"),
        active_notice_after_tokens: positive_u64(raw, "activeNoticeAfterTokens"),
        failed_tool_attempts_before_attention: positive_u64(
            raw,
            "failedToolAttemptsBeforeAttention",
        )
        .and_then(|n| u32::try_from(n).ok()),
        notify_on: string_list(
            raw,
            "notifyOn",
            &[
                ("active_long_running", ControlEventType::ActiveLongRunning),
                ("needs_attention", ControlEventType::NeedsAttention),
            ],
        ),
        notify_channels: string_list(
            raw,
            "notifyChannels",
            &[
                ("event", ControlNotificationChannel::Event),
                ("async", ControlNotificationChannel::Async),
                ("intercom", ControlNotificationChannel::Intercom),
            ],
        ),
    }
}

// =================================================================================================
// ControlEvent (shared/types.ts:205-225)
// =================================================================================================

/// pi `ControlEvent["reason"]` (`shared/types.ts:216`) — the eight discriminants a raised control
/// event may carry. Serializes in pi's own snake_case wire spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlEventReason {
    /// No observed activity for longer than `needsAttentionAfterMs` (the default reason).
    Idle,
    /// The completion-mutation guard tripped after the run settled.
    CompletionGuard,
    /// The elapsed/turn/token long-running threshold tripped.
    ActiveLongRunning,
    /// Repeated mutating-tool failures escalated the run.
    ToolFailures,
    /// A pending supervisor request is waiting on the parent.
    SupervisorRequest,
    /// `activeNoticeAfterMs` tripped.
    TimeThreshold,
    /// `activeNoticeAfterTurns` tripped.
    TurnThreshold,
    /// `activeNoticeAfterTokens` tripped.
    TokenThreshold,
}

/// pi `ControlEvent` (`shared/types.ts:205-225`). Field ORDER matches the source's object literal
/// (`buildControlEvent`'s return, `subagent-control.ts:116-134`) so a serialized event reads
/// identically to pi's; the `nestedRunId`/`nestingPath` pair is omitted because this crate has no
/// nested-run addressing on the foreground single path that raises these.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlEvent {
    /// Which of the two notice classes this is.
    #[serde(rename = "type")]
    pub event_type: ControlEventType,
    /// The activity state the run was in before this transition, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<ActivityState>,
    /// The activity state this event transitions the run INTO.
    pub to: ActivityState,
    /// Wall-clock epoch millis the event was raised at (pi `Date.now()`).
    pub ts: i64,
    /// The run this event concerns.
    pub run_id: String,
    /// The agent persona active when it was raised.
    pub agent: String,
    /// The zero-based child index within the run, when the run has more than one child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    /// The human-facing one-line signal.
    pub message: String,
    /// Why the event fired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<ControlEventReason>,
    /// Assistant turns observed so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<u64>,
    /// Input+output tokens observed so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    /// Tool calls started so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<u32>,
    /// The tool in flight when the event fired, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    /// How long that tool had been running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool_duration_ms: Option<i64>,
    /// The path that tool was operating on, if it named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_path: Option<String>,
    /// Elapsed millis this event measures (idle age, or run elapsed for long-running).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<i64>,
    /// A rendered summary of the recent mutating-tool failures, when that is why it fired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_failure_summary: Option<String>,
}

/// The optional-argument bag `buildControlEvent` takes (`subagent-control.ts:87-106`), reified so
/// the Rust call sites read like the source's object-literal calls.
#[derive(Clone, Debug, Default)]
pub struct ControlEventInput {
    /// Explicit event class; defaults from `to` when omitted.
    pub event_type: Option<ControlEventType>,
    /// Previous activity state.
    pub from: Option<ActivityState>,
    /// Explicit timestamp; the caller always supplies one here (there is no ambient clock in this
    /// module, deliberately — every threshold test is driven off a caller-supplied `now`).
    pub ts: i64,
    /// The run id.
    pub run_id: String,
    /// The agent persona.
    pub agent: String,
    /// Child index within the run.
    pub index: Option<u32>,
    /// Last-observed-activity timestamp, from which `elapsedMs` is derived when not explicit.
    pub last_activity_at: Option<i64>,
    /// Explicit message; a default is derived from `type`/`elapsedMs` when omitted.
    pub message: Option<String>,
    /// Explicit reason; defaults from `type` when omitted.
    pub reason: Option<ControlEventReason>,
    /// Assistant turns so far.
    pub turns: Option<u64>,
    /// Tokens so far.
    pub tokens: Option<u64>,
    /// Tool calls so far.
    pub tool_count: Option<u32>,
    /// Tool in flight.
    pub current_tool: Option<String>,
    /// How long that tool has been in flight.
    pub current_tool_duration_ms: Option<i64>,
    /// Path that tool names.
    pub current_path: Option<String>,
    /// Explicit elapsed millis (overrides the `ts - last_activity_at` derivation).
    pub elapsed_ms: Option<i64>,
    /// Recent mutating-failure summary.
    pub recent_failure_summary: Option<String>,
}

/// pi `buildControlEvent` (`subagent-control.ts:87-135`): fills `type`/`reason`/`elapsedMs`/
/// `message` defaults, then emits the event object.
#[must_use]
pub fn build_control_event(to: ActivityState, input: ControlEventInput) -> ControlEvent {
    let ts = input.ts;
    let event_type = input.event_type.unwrap_or(match to {
        ActivityState::ActiveLongRunning => ControlEventType::ActiveLongRunning,
        ActivityState::NeedsAttention => ControlEventType::NeedsAttention,
    });
    let elapsed_ms = input
        .elapsed_ms
        .or_else(|| input.last_activity_at.map(|at| (ts - at).max(0)));
    let elapsed_seconds = elapsed_ms.map(|ms| ms / 1000);
    let message = input.message.unwrap_or_else(|| match event_type {
        ControlEventType::ActiveLongRunning => {
            format!("{} is still active but long-running", input.agent)
        }
        ControlEventType::NeedsAttention => match elapsed_seconds {
            Some(seconds) => format!(
                "{} needs attention (no observed activity for {seconds}s)",
                input.agent
            ),
            None => format!("{} needs attention", input.agent),
        },
    });
    let reason = input.reason.unwrap_or(match event_type {
        ControlEventType::ActiveLongRunning => ControlEventReason::ActiveLongRunning,
        ControlEventType::NeedsAttention => ControlEventReason::Idle,
    });
    ControlEvent {
        event_type,
        from: input.from,
        to,
        ts,
        run_id: input.run_id,
        agent: input.agent,
        index: input.index,
        message,
        reason: Some(reason),
        turns: input.turns,
        tokens: input.tokens,
        tool_count: input.tool_count,
        current_tool: input.current_tool.filter(|s| !s.is_empty()),
        current_tool_duration_ms: input.current_tool_duration_ms,
        current_path: input.current_path.filter(|s| !s.is_empty()),
        elapsed_ms,
        recent_failure_summary: input.recent_failure_summary.filter(|s| !s.is_empty()),
    }
}

/// pi `deriveActivityState` (`subagent-control.ts:73-85`): a run with a tool IN FLIGHT is never
/// idle, and a run whose last observed activity is older than `needsAttentionAfterMs` is
/// `needs_attention`. Anything else is "neither".
#[must_use]
pub fn derive_activity_state(
    config: &ResolvedControlConfig,
    started_at: i64,
    last_activity_at: Option<i64>,
    current_tool: Option<&str>,
    now: i64,
) -> Option<ActivityState> {
    if !config.enabled || current_tool.is_some_and(|t| !t.is_empty()) {
        return None;
    }
    let last_activity = last_activity_at.unwrap_or(started_at);
    let age_ms = (now - last_activity).max(0);
    (age_ms > config.needs_attention_after_ms).then_some(ActivityState::NeedsAttention)
}

/// pi `shouldNotifyControlEvent` (`subagent-control.ts:137-139`).
#[must_use]
pub fn should_notify_control_event(config: &ResolvedControlConfig, event: &ControlEvent) -> bool {
    config.enabled && config.notify_on.contains(&event.event_type)
}

/// pi `controlNotificationKey` (`subagent-control.ts:142-145`): the dedup identity of one notice —
/// `<child>:<type>:<reason>`, where `<child>` is the child's intercom target when one exists, else
/// `runId:index` (or the bare `runId` for a single-child run).
#[must_use]
pub fn control_notification_key(
    event: &ControlEvent,
    child_intercom_target: Option<&str>,
) -> String {
    let child_key = match child_intercom_target {
        Some(target) => target.to_string(),
        None => match event.index {
            Some(index) => format!("{}:{index}", event.run_id),
            None => event.run_id.clone(),
        },
    };
    let reason = match event.reason {
        Some(reason) => control_event_reason_wire(reason),
        None => "idle",
    };
    let event_type = control_event_type_wire(event.event_type);
    format!("{child_key}:{event_type}:{reason}")
}

/// The wire spelling of a [`ControlEventType`] — the key builder above interpolates the string
/// union member, not a Rust `Debug` rendering.
#[must_use]
pub fn control_event_type_wire(event_type: ControlEventType) -> &'static str {
    match event_type {
        ControlEventType::ActiveLongRunning => "active_long_running",
        ControlEventType::NeedsAttention => "needs_attention",
    }
}

/// The wire spelling of a [`ControlEventReason`], for the same reason as
/// [`control_event_type_wire`].
#[must_use]
pub fn control_event_reason_wire(reason: ControlEventReason) -> &'static str {
    match reason {
        ControlEventReason::Idle => "idle",
        ControlEventReason::CompletionGuard => "completion_guard",
        ControlEventReason::ActiveLongRunning => "active_long_running",
        ControlEventReason::ToolFailures => "tool_failures",
        ControlEventReason::SupervisorRequest => "supervisor_request",
        ControlEventReason::TimeThreshold => "time_threshold",
        ControlEventReason::TurnThreshold => "turn_threshold",
        ControlEventReason::TokenThreshold => "token_threshold",
    }
}

/// pi `claimControlNotification` (`subagent-control.ts:146-152`): notify-gate, then at-most-once
/// per `(child, type, reason)` key for the lifetime of `seen_keys`.
pub fn claim_control_notification(
    config: &ResolvedControlConfig,
    event: &ControlEvent,
    seen_keys: &mut HashSet<String>,
    child_intercom_target: Option<&str>,
) -> bool {
    if !should_notify_control_event(config, event) {
        return false;
    }
    seen_keys.insert(control_notification_key(event, child_intercom_target))
}

// =================================================================================================
// Notice rendering (subagent-control.ts:154-231)
// =================================================================================================

/// pi `formatLongRunningFacts` (`subagent-control.ts:154-163`).
#[must_use]
pub fn format_long_running_facts(event: &ControlEvent) -> Option<String> {
    let mut facts: Vec<String> = Vec::new();
    if let Some(elapsed) = event.elapsed_ms {
        facts.push(format!("elapsed {}s", elapsed.max(0) / 1000));
    }
    if let Some(turns) = event.turns {
        facts.push(format!("{turns} turns"));
    }
    if let Some(tokens) = event.tokens {
        facts.push(format!("{tokens} tokens"));
    }
    if let Some(tool_count) = event.tool_count {
        facts.push(format!("{tool_count} tools"));
    }
    if let Some(tool) = &event.current_tool {
        match event.current_tool_duration_ms {
            Some(duration) => facts.push(format!("tool {tool} {}s", duration.max(0) / 1000)),
            None => facts.push(format!("tool {tool}")),
        }
    }
    if let Some(path) = &event.current_path {
        facts.push(format!("path {path}"));
    }
    if facts.is_empty() { None } else { Some(facts.join(" | ")) }
}

/// pi `formatControlNoticeMessage` (`subagent-control.ts:165-212`) — the three notice bodies
/// (completion-guard failure, active-but-long-running, needs-attention), verbatim including the
/// command hints, which are rendered in pi's `subagent({ … })` tool-call spelling because that is
/// what the reading model is expected to type back.
#[must_use]
pub fn format_control_notice_message(
    event: &ControlEvent,
    child_intercom_target: Option<&str>,
) -> String {
    let run_target = &event.run_id;
    let step_suffix = match event.index {
        Some(index) => format!(" step {}", index.saturating_add(1)),
        None => String::new(),
    };

    if event.reason == Some(ControlEventReason::CompletionGuard) {
        let mut lines = vec![
            format!("Subagent failed: {}", event.agent),
            format!("Run: {run_target}{step_suffix}"),
            format!("Signal: {}", event.message),
            "Next: read the output artifact or session from the subagent result, then retry with \
             a more explicit implementation prompt or handle the fix directly."
                .to_string(),
        ];
        if let Some(target) = child_intercom_target {
            lines.push(format!("Run intercom target (may be inactive): {target}"));
        }
        return lines.join("\n");
    }

    let nudge_message =
        "What are you blocked on? Reply with the smallest next step or ask for a decision.";
    let index_arg = match event.index {
        Some(index) => format!("index: {index}, "),
        None => String::new(),
    };
    let steer_command = format!(
        "subagent({{ action: \"steer\", id: \"{run_target}\", {index_arg}message: \
         \"{nudge_message}\" }})"
    );
    let nested_resume_command = format!(
        "subagent({{ action: \"resume\", id: \"{run_target}\", message: \"{nudge_message}\" }})"
    );

    if event.event_type == ControlEventType::ActiveLongRunning {
        let mut lines = vec![
            format!("Subagent active but long-running: {}", event.agent),
            format!("Run: {run_target}{step_suffix}"),
            format!("Signal: {}", event.message),
        ];
        if let Some(facts) = format_long_running_facts(event) {
            lines.push(format!("Facts: {facts}"));
        }
        lines.push(
            "Hint: Inspect status first. Use steer for a top-level live async child, routed \
             resume for a live nested child, or resume to revive a paused/completed/failed child."
                .to_string(),
        );
        lines.push(format!("Top-level live async nudge: {steer_command}"));
        lines.push(format!("Routed live nested nudge: {nested_resume_command}"));
        if let Some(target) = child_intercom_target {
            lines.push(format!("Direct intercom target: {target}"));
        }
        lines.push(format!(
            "Status: subagent({{ action: \"status\", id: \"{run_target}\" }})"
        ));
        lines.push(format!(
            "Interrupt: subagent({{ action: \"interrupt\", id: \"{run_target}\" }})"
        ));
        return lines.join("\n");
    }

    let mut lines = vec![
        format!("Subagent needs attention: {}", event.agent),
        format!("Run: {run_target}{step_suffix}"),
        format!("Signal: {}", event.message),
    ];
    if let Some(summary) = &event.recent_failure_summary {
        lines.push(format!("Recent failures: {summary}"));
    }
    if event.reason == Some(ControlEventReason::SupervisorRequest) {
        lines.push(
            "Supervisor request: reply to the pending request. If subagent_supervisor pending is \
             empty, check intercom pending because an external intercom tool may own the request."
                .to_string(),
        );
    }
    lines.push(
        "Hint: Inspect status first unless the run is clearly blocked. Use steer for a top-level \
         live async child, routed resume for a live nested child, or resume to revive a \
         paused/completed/failed child."
            .to_string(),
    );
    lines.push(format!("Top-level live async nudge: {steer_command}"));
    lines.push(format!("Routed live nested nudge: {nested_resume_command}"));
    if let Some(target) = child_intercom_target {
        lines.push(format!("Direct intercom target: {target}"));
    }
    lines.push(format!(
        "Status: subagent({{ action: \"status\", id: \"{run_target}\" }})"
    ));
    lines.push(format!(
        "Interrupt: subagent({{ action: \"interrupt\", id: \"{run_target}\" }})"
    ));
    lines.join("\n")
}

/// pi `formatControlIntercomMessage` (`subagent-control.ts:214-231`): the same notice body, with a
/// short status headline + one-line restatement prepended, for the intercom channel.
#[must_use]
pub fn format_control_intercom_message(
    event: &ControlEvent,
    child_intercom_target: Option<&str>,
) -> String {
    let completion_guard = event.reason == Some(ControlEventReason::CompletionGuard);
    let long_running = event.event_type == ControlEventType::ActiveLongRunning;
    let status_label = if completion_guard {
        "subagent failed"
    } else if long_running {
        "subagent active but long-running"
    } else {
        "subagent needs attention"
    };
    let restatement = if completion_guard {
        format!("{} failed in run {}.", event.agent, event.run_id)
    } else if long_running {
        format!(
            "{} is still active but long-running in run {}.",
            event.agent, event.run_id
        )
    } else {
        format!("{} needs attention in run {}.", event.agent, event.run_id)
    };
    [
        status_label.to_string(),
        String::new(),
        restatement,
        String::new(),
        format_control_notice_message(event, child_intercom_target),
    ]
    .join("\n")
}

// =================================================================================================
// long-running-guard.ts — the control-relevant half
// =================================================================================================

/// pi `LongRunningTriggerReason` (`long-running-guard.ts:11`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LongRunningTrigger {
    /// `activeNoticeAfterMs` elapsed.
    TimeThreshold,
    /// `activeNoticeAfterTurns` reached.
    TurnThreshold,
    /// `activeNoticeAfterTokens` reached.
    TokenThreshold,
}

impl LongRunningTrigger {
    /// The [`ControlEventReason`] a trigger is carried as on the raised event.
    #[must_use]
    pub fn reason(self) -> ControlEventReason {
        match self {
            Self::TimeThreshold => ControlEventReason::TimeThreshold,
            Self::TurnThreshold => ControlEventReason::TurnThreshold,
            Self::TokenThreshold => ControlEventReason::TokenThreshold,
        }
    }
}

/// pi `nextLongRunningTrigger` (`long-running-guard.ts:162-171`): elapsed-time first, then turns,
/// then tokens — first match wins.
#[must_use]
pub fn next_long_running_trigger(
    config: &ResolvedControlConfig,
    started_at: i64,
    now: i64,
    turns: u64,
    tokens: u64,
) -> Option<LongRunningTrigger> {
    if now - started_at >= config.active_notice_after_ms {
        return Some(LongRunningTrigger::TimeThreshold);
    }
    if config.active_notice_after_turns.is_some_and(|t| turns >= t) {
        return Some(LongRunningTrigger::TurnThreshold);
    }
    if config.active_notice_after_tokens.is_some_and(|t| tokens >= t) {
        return Some(LongRunningTrigger::TokenThreshold);
    }
    None
}

/// pi `resolveCurrentPath` (`long-running-guard.ts:54-67`): the first non-empty
/// `path`/`file`/`filename`/`target`/`cwd` argument, or — for `bash` — the first redirect/`tee`
/// destination in the command.
#[must_use]
pub fn resolve_current_path(tool_name: &str, args: &serde_json::Value) -> Option<String> {
    if tool_name.is_empty() {
        return None;
    }
    let args = args.as_object()?;
    for key in ["path", "file", "filename", "target", "cwd"] {
        if let Some(value) = args.get(key).and_then(serde_json::Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    if tool_name != "bash" {
        return None;
    }
    let command = args.get("command").and_then(serde_json::Value::as_str)?;
    first_redirect_target(command)
}

/// The `/(?:>|>>|tee\s+)(\S+)/` capture from `resolveCurrentPath`'s bash branch, hand-rolled so the
/// crate stays regex-free at this seam (the same choice `completion_guard` already made for the
/// mutating-bash patterns). Scans left to right for the first `>` or `tee` + whitespace, then takes
/// the immediately following run of non-whitespace characters.
fn first_redirect_target(command: &str) -> Option<String> {
    let bytes = command.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let matched_end = if bytes.get(i) == Some(&b'>') {
            // `>` and `>>` are both matched by the source's alternation; `>` alone matches first
            // and captures whatever non-whitespace immediately follows, so `>>out` captures `>out`
            // exactly as the JavaScript regex does.
            Some(i + 1)
        } else if command.get(i..).is_some_and(|rest| rest.starts_with("tee"))
            && command
                .get(i + 3..)
                .and_then(|rest| rest.chars().next())
                .is_some_and(char::is_whitespace)
        {
            // `tee\s+` — consume the whole whitespace run, matching `\s+`'s greediness.
            let mut cursor = i + 3;
            while command
                .get(cursor..)
                .and_then(|rest| rest.chars().next())
                .is_some_and(char::is_whitespace)
            {
                cursor += 1;
            }
            Some(cursor)
        } else {
            None
        };
        if let Some(start) = matched_end {
            let rest = command.get(start..).unwrap_or_default();
            let target: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
            if !target.is_empty() {
                return Some(target);
            }
        }
        i += 1;
    }
    None
}

/// pi `isMutatingTool` (`long-running-guard.ts:138-155`): `edit`/`write` always; `cursor` when its
/// `activityTitle` starts with `Cursor edit`/`Cursor write` (case-insensitively); `bash` when its
/// command is classified mutating by [`is_mutating_bash_command`]; nothing else.
#[must_use]
pub fn is_mutating_tool(tool_name: &str, args: &serde_json::Value) -> bool {
    match tool_name {
        "" => false,
        "edit" | "write" => true,
        "cursor" => {
            let title = args
                .get("activityTitle")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            // `/^Cursor (?:edit|write)\b/i` — the `\b` after the verb rejects `Cursor editor`.
            for verb in ["cursor edit", "cursor write"] {
                if let Some(rest) = title.strip_prefix(verb)
                    && rest
                        .chars()
                        .next()
                        .is_none_or(|c| !c.is_alphanumeric() && c != '_')
                {
                    return true;
                }
            }
            false
        }
        "bash" => args
            .get("command")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|command| !command.trim().is_empty() && is_mutating_bash_command(command)),
        _ => false,
    }
}

/// pi `MUTATING_FAILURE_HINTS` (`long-running-guard.ts:42-52`), verbatim and in source order.
const MUTATING_FAILURE_HINTS: [&str; 9] = [
    "failed",
    "error",
    "no exact match",
    "did not match",
    "malformed",
    "rejected",
    "unable",
    "cannot",
    "could not",
];

/// pi `didMutatingToolFail` (`long-running-guard.ts:157-160`): a case-insensitive substring test
/// against the failure hints.
#[must_use]
pub fn did_mutating_tool_fail(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    MUTATING_FAILURE_HINTS
        .iter()
        .any(|hint| lowered.contains(hint))
}

/// pi `FailedMutatingAttempt` (`long-running-guard.ts:13-18`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedMutatingAttempt {
    /// The tool that failed.
    pub tool: String,
    /// The path it named, when it named one.
    pub path: Option<String>,
    /// The first non-blank line of its result, capped at 180 characters.
    pub error: String,
    /// When it failed (epoch millis).
    pub ts: i64,
}

/// pi `MutatingFailureState` (`long-running-guard.ts:20-26`) + its four operations
/// (`createMutatingFailureState`/`recordMutatingFailure`/`shouldEscalateMutatingFailures`/
/// `summarizeRecentMutatingFailures`/`resetMutatingFailureState`, `:128-172`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MutatingFailureState {
    consecutive_failures: u32,
    last_failure_at: Option<i64>,
    recent_failures: Vec<FailedMutatingAttempt>,
    last_mutating_path: Option<String>,
    repeated_path_failures: u32,
}

impl MutatingFailureState {
    /// pi `resetMutatingFailureState` (`:128-134`) — a SUCCESSFUL mutating call clears the streak.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// pi `recordMutatingFailure` (`:144-165`): a failure older than `window_ms` since the last one
    /// restarts the streak; otherwise the consecutive (and, for a repeated path, the same-path)
    /// counters advance. `recent_failures` is a bounded 3-entry tail.
    pub fn record(&mut self, input: FailedMutatingAttempt, window_ms: i64) {
        if self
            .last_failure_at
            .is_none_or(|at| input.ts - at > window_ms)
        {
            self.consecutive_failures = 0;
            self.recent_failures.clear();
            self.repeated_path_failures = 0;
            self.last_mutating_path = None;
        }
        self.last_failure_at = Some(input.ts);
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        match &input.path {
            Some(path) if self.last_mutating_path.as_deref() == Some(path.as_str()) => {
                self.repeated_path_failures = self.repeated_path_failures.saturating_add(1);
            }
            Some(path) => {
                self.last_mutating_path = Some(path.clone());
                self.repeated_path_failures = 1;
            }
            None => {}
        }
        self.recent_failures.push(input);
        if self.recent_failures.len() > 3 {
            self.recent_failures.remove(0);
        }
    }

    /// pi `shouldEscalateMutatingFailures` (`:167-169`).
    #[must_use]
    pub fn should_escalate(&self, threshold: u32) -> bool {
        self.consecutive_failures >= threshold || self.repeated_path_failures >= threshold
    }

    /// pi `summarizeRecentMutatingFailures` (`:171-176`).
    #[must_use]
    pub fn summarize(&self) -> Option<String> {
        if self.recent_failures.is_empty() {
            return None;
        }
        Some(
            self.recent_failures
                .iter()
                .map(|entry| match &entry.path {
                    Some(path) => format!("{}({path}): {}", entry.tool, entry.error),
                    None => format!("{}: {}", entry.tool, entry.error),
                })
                .collect::<Vec<_>>()
                .join(" | "),
        )
    }
}

// =================================================================================================
// ControlEventSink + ControlMonitor — the live per-attempt state machine
// =================================================================================================

/// The `options.onControlEvent` callback (`execution.ts:354`), as a cheaply-cloneable handle — the
/// same shape [`crate::exec::LiveEventSink`] already uses for the raw-NDJSON tee, and for the same
/// reason (a runtime callback with no serializable content, so it never rides along with the rest
/// of [`crate::exec::RunOptions`]).
#[derive(Clone)]
pub struct ControlEventSink(std::sync::Arc<dyn Fn(&ControlEvent) + Send + Sync>);

impl std::fmt::Debug for ControlEventSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ControlEventSink(..)")
    }
}

impl ControlEventSink {
    /// Wrap a callback as a sink.
    #[must_use]
    pub fn new(sink: impl Fn(&ControlEvent) + Send + Sync + 'static) -> Self {
        Self(std::sync::Arc::new(sink))
    }

    /// Deliver one raised control event to the installed callback.
    pub fn emit(&self, event: &ControlEvent) {
        (self.0)(event);
    }
}

/// One in-flight mutating tool call, held between its start and its result so the result's text can
/// be attributed back to the tool/path that produced it (pi `pendingToolResult`,
/// `execution.ts:678`).
#[derive(Clone, Debug)]
struct PendingToolResult {
    tool: String,
    path: Option<String>,
    mutates: bool,
    started_at: i64,
}

/// The per-attempt live control state machine — the Rust home for the closure soup
/// `runSingleAttempt` builds inline (`execution.ts:344-354` the emit gate, `:578-722` the
/// raise/derive closures, `:775-890` the per-event fold, `:896-905` the 1s activity timer,
/// `:1234-1247` the completion-guard raise).
///
/// Scope is deliberately PER ATTEMPT, exactly like the source: `allControlEvents`,
/// `emittedControlEventKeys` and `activeLongRunningNotified` are all locals of `runSingleAttempt`,
/// so a model-fallback retry starts from a clean slate and may legitimately re-raise the same
/// notice for the fresh child. [`crate::exec::run_sync`] carries the WINNING attempt's monitor out
/// of the ladder so the post-settlement completion-guard raise (`:1234`) shares that attempt's
/// dedup set, again matching the source's own scoping.
#[derive(Debug)]
pub struct ControlMonitor {
    config: ResolvedControlConfig,
    run_id: String,
    agent: String,
    index: Option<u32>,
    sink: Option<ControlEventSink>,
    started_at: i64,
    last_activity_at: Option<i64>,
    activity_state: Option<ActivityState>,
    active_long_running_notified: bool,
    emitted_keys: HashSet<String>,
    events: Vec<ControlEvent>,
    turns: u64,
    tokens: u64,
    tool_count: u32,
    current_tool: Option<String>,
    current_tool_started_at: Option<i64>,
    current_path: Option<String>,
    pending_tool_result: Option<PendingToolResult>,
    mutating_failures: MutatingFailureState,
}

impl ControlMonitor {
    /// Build a monitor for one attempt. `started_at` is the attempt's own start (pi `startTime`,
    /// `execution.ts:404-411`), from which the long-running elapsed threshold is measured.
    #[must_use]
    pub fn new(
        config: ResolvedControlConfig,
        run_id: String,
        agent: String,
        index: Option<u32>,
        sink: Option<ControlEventSink>,
        started_at: i64,
    ) -> Self {
        Self {
            config,
            run_id,
            agent,
            index,
            sink,
            started_at,
            last_activity_at: None,
            activity_state: None,
            active_long_running_notified: false,
            emitted_keys: HashSet::new(),
            events: Vec::new(),
            turns: 0,
            tokens: 0,
            tool_count: 0,
            current_tool: None,
            current_tool_started_at: None,
            current_path: None,
            pending_tool_result: None,
            mutating_failures: MutatingFailureState::default(),
        }
    }

    /// A disabled monitor for callers that raise nothing (the `controlConfig.enabled === false`
    /// path, and every construction site that has no run identity to attribute events to).
    #[must_use]
    pub fn disabled() -> Self {
        Self::new(
            ResolvedControlConfig {
                enabled: false,
                ..ResolvedControlConfig::default()
            },
            String::new(),
            String::new(),
            None,
            None,
            0,
        )
    }

    /// Whether the resolved config has control tracking on at all.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    /// The resolved config this monitor is driving.
    #[must_use]
    pub fn config(&self) -> &ResolvedControlConfig {
        &self.config
    }

    /// The events raised so far, in raise order (pi `allControlEvents`).
    #[must_use]
    pub fn events(&self) -> &[ControlEvent] {
        &self.events
    }

    /// Consume the monitor, yielding its raised events — what `run_sync` folds onto
    /// [`crate::exec::SingleResult::control_events`] (pi `result.controlEvents`,
    /// `execution.ts:1260`).
    #[must_use]
    pub fn into_events(self) -> Vec<ControlEvent> {
        self.events
    }

    /// The live activity state, for the parent-side actionability re-check.
    #[must_use]
    pub fn activity_state(&self) -> Option<ActivityState> {
        self.activity_state
    }

    /// pi `progress.activityState = undefined` on a soft interrupt (`execution.ts:1090`, and again
    /// at `:1113` once the interrupt settles): an intentionally paused run is NOT "needing
    /// attention", so a still-debouncing notice must stop being actionable the moment the pause
    /// lands. Deliberately does NOT retract already-raised events from
    /// [`Self::events`] — pi keeps `allControlEvents` intact too (`:1112` assigns the full list on
    /// the interrupted path); what changes is the live state the notice re-check reads.
    pub fn clear_activity_state(&mut self) {
        self.activity_state = None;
    }

    /// pi `emitControlEvent` (`execution.ts:417-423`): notify-gate + at-most-once claim, then
    /// record and forward. Returns whether the event actually passed the gate.
    fn emit_control_event(&mut self, event: ControlEvent) -> bool {
        if !should_notify_control_event(&self.config, &event) {
            return false;
        }
        if !claim_control_notification(
            &self.config,
            &event,
            &mut self.emitted_keys,
            // The foreground path has no child intercom target at THIS layer (pi's own
            // `emitControlEvent` likewise calls `claimControlNotification` with no target;
            // the target only enters one layer out, in `emitControlNotification`).
            None,
        ) {
            return false;
        }
        if let Some(sink) = &self.sink {
            sink.emit(&event);
        }
        self.events.push(event);
        true
    }

    fn current_tool_duration_ms(&self, now: i64) -> Option<i64> {
        self.current_tool_started_at
            .map(|started| (now - started).max(0))
    }

    /// pi `emitNeedsAttention` (`execution.ts:682-707`). Returns `true` when this was a genuine
    /// state TRANSITION into `needs_attention` (the source's `previous !== "needs_attention"`).
    pub fn emit_needs_attention(&mut self, now: i64, input: NeedsAttentionInput) -> bool {
        if !self.config.enabled {
            return false;
        }
        let previous = self.activity_state;
        self.activity_state = Some(ActivityState::NeedsAttention);
        let event = build_control_event(
            ActivityState::NeedsAttention,
            ControlEventInput {
                event_type: Some(ControlEventType::NeedsAttention),
                from: previous,
                ts: now,
                run_id: self.run_id.clone(),
                agent: self.agent.clone(),
                index: self.index,
                last_activity_at: self.last_activity_at,
                message: input.message,
                reason: Some(input.reason.unwrap_or(ControlEventReason::Idle)),
                turns: Some(self.turns),
                tokens: Some(self.tokens),
                tool_count: Some(self.tool_count),
                current_tool: input.current_tool.or_else(|| self.current_tool.clone()),
                current_tool_duration_ms: input
                    .current_tool_duration_ms
                    .or_else(|| self.current_tool_duration_ms(now)),
                current_path: input.current_path.or_else(|| self.current_path.clone()),
                recent_failure_summary: input.recent_failure_summary,
                elapsed_ms: None,
            },
        );
        self.emit_control_event(event);
        previous != Some(ActivityState::NeedsAttention)
    }

    /// pi `emitActiveLongRunning` (`execution.ts:708-732`): at most once per attempt, and never
    /// while the run is already flagged `needs_attention`.
    pub fn emit_active_long_running(&mut self, now: i64, trigger: LongRunningTrigger) -> bool {
        if !self.config.enabled
            || self.active_long_running_notified
            || self.activity_state == Some(ActivityState::NeedsAttention)
        {
            return false;
        }
        self.active_long_running_notified = true;
        let previous = self.activity_state;
        self.activity_state = Some(ActivityState::ActiveLongRunning);
        let event = build_control_event(
            ActivityState::ActiveLongRunning,
            ControlEventInput {
                event_type: Some(ControlEventType::ActiveLongRunning),
                from: previous,
                ts: now,
                run_id: self.run_id.clone(),
                agent: self.agent.clone(),
                index: self.index,
                message: Some(format!("{} is still active but long-running", self.agent)),
                reason: Some(trigger.reason()),
                turns: Some(self.turns),
                tokens: Some(self.tokens),
                tool_count: Some(self.tool_count),
                current_tool: self.current_tool.clone(),
                current_tool_duration_ms: self.current_tool_duration_ms(now),
                current_path: self.current_path.clone(),
                elapsed_ms: Some(now - self.started_at),
                last_activity_at: None,
                recent_failure_summary: None,
            },
        );
        self.emit_control_event(event);
        true
    }

    /// pi `updateActivityState` (`execution.ts:784-803`): the idle heuristic first, then the
    /// long-running trigger. Returns `true` when a fresh notice was raised (which is what the
    /// source's 1s timer uses to decide whether to also fire a progress update).
    pub fn update_activity_state(&mut self, now: i64) -> bool {
        if !self.config.enabled {
            return false;
        }
        let idle = derive_activity_state(
            &self.config,
            self.started_at,
            self.last_activity_at,
            self.current_tool.as_deref(),
            now,
        );
        if idle == Some(ActivityState::NeedsAttention) {
            if self.activity_state == Some(ActivityState::NeedsAttention) {
                return false;
            }
            return self.emit_needs_attention(now, NeedsAttentionInput::default());
        }
        match next_long_running_trigger(
            &self.config,
            self.started_at,
            now,
            self.turns,
            self.tokens,
        ) {
            Some(trigger) => self.emit_active_long_running(now, trigger),
            None => false,
        }
    }

    /// pi `execution.ts:775-778` — every parsed child event is fresh activity, and re-derives the
    /// activity state before the per-type fold below runs.
    pub fn note_activity(&mut self, now: i64) {
        self.last_activity_at = Some(now);
        self.update_activity_state(now);
    }

    /// The per-event fold (`execution.ts:775-890`), restricted to the fields control actually
    /// consumes. Call once per parsed NDJSON event, with `now` the observation time.
    ///
    /// Divergence note, deliberate and load-bearing: pi folds `tool_result_end` (a separate wire
    /// event carrying the tool-result MESSAGE). cyrup's wire has no such event — the terminal
    /// tool-call event is `ToolExecutionEnd`, which carries the identical `result`/`is_error`
    /// payload (see [`crate::exec::ndjson::SubagentEvent::ToolExecutionEnd`]'s own note), so BOTH
    /// of pi's tool branches are folded off that one variant here: the `tool_execution_end` half
    /// (clear `currentTool`) and the `tool_result_end` half (mutating-failure accounting).
    pub fn observe_event(&mut self, event: &SubagentEvent, now: i64) {
        self.note_activity(now);
        match event {
            SubagentEvent::ToolExecutionStart {
                tool_name, args, ..
            } => {
                self.tool_count = self.tool_count.saturating_add(1);
                self.current_tool = Some(tool_name.clone());
                self.current_tool_started_at = Some(now);
                self.current_path = resolve_current_path(tool_name, args);
                let mutates = is_mutating_tool(tool_name, args);
                self.pending_tool_result = Some(PendingToolResult {
                    tool: if tool_name.is_empty() {
                        "tool".to_string()
                    } else {
                        tool_name.clone()
                    },
                    path: self.current_path.clone(),
                    mutates,
                    started_at: now,
                });
            }
            SubagentEvent::ToolExecutionEnd {
                result, is_error, ..
            } => {
                // pi `tool_execution_end` half (`execution.ts:803-816`).
                self.current_tool = None;
                self.current_tool_started_at = None;
                self.current_path = None;
                // pi `tool_result_end` half (`execution.ts:861-889`).
                let Some(snapshot) = self.pending_tool_result.take() else {
                    return;
                };
                if !snapshot.mutates {
                    return;
                }
                let text = crate::exec::output::extract_tool_result_text(result)
                    .unwrap_or_default();
                // A wire-level `is_error` is an unambiguous failure; the source has no such flag
                // on `tool_result_end` and can only sniff the text, so the text test is kept as
                // the primary and `is_error` widens it rather than replacing it.
                if *is_error || did_mutating_tool_fail(&text) {
                    let error = text
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .map(|line| {
                            let trimmed = line.trim();
                            trimmed.chars().take(180).collect::<String>()
                        })
                        .unwrap_or_else(|| "mutating tool failed".to_string());
                    self.mutating_failures.record(
                        FailedMutatingAttempt {
                            tool: snapshot.tool.clone(),
                            path: snapshot.path.clone(),
                            error,
                            ts: now,
                        },
                        MUTATING_FAILURE_WINDOW_MS,
                    );
                    if self
                        .mutating_failures
                        .should_escalate(self.config.failed_tool_attempts_before_attention)
                    {
                        let summary = self.mutating_failures.summarize();
                        let agent = self.agent.clone();
                        self.emit_needs_attention(
                            now,
                            NeedsAttentionInput {
                                message: Some(format!(
                                    "{agent} needs attention after repeated mutating tool failures"
                                )),
                                reason: Some(ControlEventReason::ToolFailures),
                                current_tool: Some(snapshot.tool),
                                current_path: snapshot.path,
                                current_tool_duration_ms: Some((now - snapshot.started_at).max(0)),
                                recent_failure_summary: summary,
                            },
                        );
                    }
                } else {
                    self.mutating_failures.reset();
                }
            }
            SubagentEvent::MessageEnd { message } => {
                if message.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
                    return;
                }
                self.turns = self.turns.saturating_add(1);
                if let Some(usage) = event.assistant_usage() {
                    self.tokens = self.tokens.saturating_add(usage.input + usage.output);
                }
                // pi re-derives the activity state after folding an assistant turn
                // (`execution.ts:855`), because the fresh turn/token counts can themselves trip the
                // long-running thresholds.
                self.update_activity_state(now);
            }
            _ => {}
        }
    }

    /// pi `execution.ts:1234-1247`: the completion-mutation guard's own `needs_attention` raise,
    /// fired AFTER the attempt settles (and therefore after the drive loop is gone), sharing the
    /// attempt's dedup set.
    pub fn emit_completion_guard_notice(&mut self, now: i64, message: String) {
        if !self.config.enabled {
            return;
        }
        let event = build_control_event(
            ActivityState::NeedsAttention,
            ControlEventInput {
                from: self.activity_state,
                ts: now,
                run_id: if self.run_id.is_empty() {
                    self.agent.clone()
                } else {
                    self.run_id.clone()
                },
                agent: self.agent.clone(),
                index: self.index,
                message: Some(message),
                reason: Some(ControlEventReason::CompletionGuard),
                ..ControlEventInput::default()
            },
        );
        self.emit_control_event(event);
    }
}

/// The optional arguments `emitNeedsAttention` takes (`execution.ts:682-707`).
#[derive(Clone, Debug, Default)]
pub struct NeedsAttentionInput {
    /// Explicit message; the default is derived from the idle age.
    pub message: Option<String>,
    /// Explicit reason; defaults to `idle`.
    pub reason: Option<ControlEventReason>,
    /// A summary of the recent mutating-tool failures, for the `tool_failures` reason.
    pub recent_failure_summary: Option<String>,
    /// Explicit in-flight tool name.
    pub current_tool: Option<String>,
    /// Explicit in-flight tool path.
    pub current_path: Option<String>,
    /// Explicit in-flight tool duration.
    pub current_tool_duration_ms: Option<i64>,
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    fn cfg(json: serde_json::Value) -> ControlConfig {
        parse_control_overrides(&json)
    }

    // ---- resolveControlConfig ----

    #[test]
    fn resolve_control_config_defaults_match_pi_default_control_config() {
        let resolved = resolve_control_config(None, None);
        assert!(resolved.enabled);
        assert_eq!(resolved.needs_attention_after_ms, 60_000);
        assert_eq!(resolved.active_notice_after_ms, 240_000);
        assert_eq!(resolved.active_notice_after_turns, None);
        assert_eq!(resolved.active_notice_after_tokens, None);
        assert_eq!(resolved.failed_tool_attempts_before_attention, 3);
        assert_eq!(
            resolved.notify_on,
            vec![
                ControlEventType::ActiveLongRunning,
                ControlEventType::NeedsAttention
            ]
        );
        assert_eq!(
            resolved.notify_channels,
            vec![
                ControlNotificationChannel::Event,
                ControlNotificationChannel::Async,
                ControlNotificationChannel::Intercom
            ]
        );
    }

    #[test]
    fn per_call_override_beats_global_field_by_field_not_wholesale() {
        let global = cfg(serde_json::json!({
            "needsAttentionAfterMs": 30_000,
            "activeNoticeAfterMs": 90_000,
            "notifyOn": ["active_long_running"],
        }));
        let call = cfg(serde_json::json!({ "needsAttentionAfterMs": 5_000 }));
        let resolved = resolve_control_config(Some(&global), Some(&call));
        assert_eq!(resolved.needs_attention_after_ms, 5_000, "override wins");
        assert_eq!(resolved.active_notice_after_ms, 90_000, "global survives");
        assert_eq!(
            resolved.notify_on,
            vec![ControlEventType::ActiveLongRunning],
            "an untouched field is NOT reset to the default by an override of a sibling"
        );
    }

    #[test]
    fn zero_and_non_integer_thresholds_are_rejected_not_honoured() {
        // pi `parsePositiveInt`: `< 1` and non-integers are `undefined`, so the next rung wins.
        let call = cfg(serde_json::json!({
            "needsAttentionAfterMs": 0,
            "activeNoticeAfterMs": 2.5,
            "activeNoticeAfterTurns": "12",
        }));
        assert_eq!(call.needs_attention_after_ms, None);
        assert_eq!(call.active_notice_after_ms, None);
        assert_eq!(call.active_notice_after_turns, None);
        let resolved = resolve_control_config(None, Some(&call));
        assert_eq!(resolved.needs_attention_after_ms, 60_000);
        assert_eq!(resolved.active_notice_after_ms, 240_000);
    }

    #[test]
    fn an_explicit_empty_notify_list_disables_notification_entirely() {
        // pi `parseControlList`: `[]` returns `[]` (truthy for `??`), so it WINS over the default.
        let call = cfg(serde_json::json!({ "notifyOn": [] }));
        let resolved = resolve_control_config(None, Some(&call));
        assert!(resolved.notify_on.is_empty());
        let event = sample_event(ControlEventType::NeedsAttention);
        assert!(!should_notify_control_event(&resolved, &event));
    }

    #[test]
    fn an_all_unknown_notify_list_falls_through_rather_than_disabling() {
        // pi: `parsed.length > 0 ? … : undefined` — every entry filtered out means "not supplied".
        let call = cfg(serde_json::json!({ "notifyOn": ["nope", "also-nope"] }));
        assert_eq!(call.notify_on, None);
        let resolved = resolve_control_config(None, Some(&call));
        assert_eq!(resolved.notify_on.len(), 2, "the default list survives");
    }

    #[test]
    fn notify_lists_are_deduplicated_preserving_first_seen_order() {
        let call = cfg(serde_json::json!({
            "notifyOn": ["needs_attention", "active_long_running", "needs_attention"]
        }));
        let resolved = resolve_control_config(None, Some(&call));
        assert_eq!(
            resolved.notify_on,
            vec![
                ControlEventType::NeedsAttention,
                ControlEventType::ActiveLongRunning
            ]
        );
    }

    fn sample_event(event_type: ControlEventType) -> ControlEvent {
        build_control_event(
            match event_type {
                ControlEventType::ActiveLongRunning => ActivityState::ActiveLongRunning,
                ControlEventType::NeedsAttention => ActivityState::NeedsAttention,
            },
            ControlEventInput {
                ts: 1_000,
                run_id: "run1".to_string(),
                agent: "scout".to_string(),
                ..ControlEventInput::default()
            },
        )
    }

    // ---- buildControlEvent / keys / formatting ----

    #[test]
    fn build_control_event_derives_type_reason_elapsed_and_message() {
        let event = build_control_event(
            ActivityState::NeedsAttention,
            ControlEventInput {
                ts: 100_000,
                run_id: "abc".to_string(),
                agent: "scout".to_string(),
                last_activity_at: Some(35_000),
                ..ControlEventInput::default()
            },
        );
        assert_eq!(event.event_type, ControlEventType::NeedsAttention);
        assert_eq!(event.reason, Some(ControlEventReason::Idle));
        assert_eq!(event.elapsed_ms, Some(65_000));
        assert_eq!(
            event.message,
            "scout needs attention (no observed activity for 65s)"
        );
    }

    #[test]
    fn control_notification_key_matches_pi_shape() {
        let mut event = sample_event(ControlEventType::NeedsAttention);
        assert_eq!(
            control_notification_key(&event, None),
            "run1:needs_attention:idle"
        );
        event.index = Some(2);
        assert_eq!(
            control_notification_key(&event, None),
            "run1:2:needs_attention:idle"
        );
        assert_eq!(
            control_notification_key(&event, Some("child-target")),
            "child-target:needs_attention:idle"
        );
    }

    #[test]
    fn claim_is_at_most_once_per_key() {
        let config = ResolvedControlConfig::default();
        let event = sample_event(ControlEventType::NeedsAttention);
        let mut seen = HashSet::new();
        assert!(claim_control_notification(&config, &event, &mut seen, None));
        assert!(!claim_control_notification(&config, &event, &mut seen, None));
    }

    #[test]
    fn notice_message_bodies_carry_pis_headlines_and_command_hints() {
        let mut event = sample_event(ControlEventType::NeedsAttention);
        event.index = Some(0);
        let text = format_control_notice_message(&event, Some("child-1"));
        assert!(text.starts_with("Subagent needs attention: scout\n"), "{text}");
        assert!(text.contains("Run: run1 step 1"), "{text}");
        assert!(text.contains("subagent({ action: \"steer\", id: \"run1\", index: 0, message:"));
        assert!(text.contains("Direct intercom target: child-1"));
        assert!(text.contains("Interrupt: subagent({ action: \"interrupt\", id: \"run1\" })"));

        let long = sample_event(ControlEventType::ActiveLongRunning);
        let long_text = format_control_notice_message(&long, None);
        assert!(long_text.starts_with("Subagent active but long-running: scout\n"));
        assert!(!long_text.contains("Direct intercom target"));

        let mut guard = sample_event(ControlEventType::NeedsAttention);
        guard.reason = Some(ControlEventReason::CompletionGuard);
        let guard_text = format_control_notice_message(&guard, None);
        assert!(guard_text.starts_with("Subagent failed: scout\n"), "{guard_text}");
        assert!(guard_text.contains("Next: read the output artifact"));
    }

    #[test]
    fn intercom_message_prepends_the_status_headline_and_restatement() {
        let event = sample_event(ControlEventType::NeedsAttention);
        let text = format_control_intercom_message(&event, None);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "subagent needs attention");
        assert_eq!(lines[1], "");
        assert_eq!(lines[2], "scout needs attention in run run1.");
        assert_eq!(lines[3], "");
        assert_eq!(lines[4], "Subagent needs attention: scout");
    }

    // ---- long-running-guard ----

    #[test]
    fn long_running_trigger_prefers_time_then_turns_then_tokens() {
        let config = ResolvedControlConfig {
            active_notice_after_ms: 1_000,
            active_notice_after_turns: Some(5),
            active_notice_after_tokens: Some(100),
            ..ResolvedControlConfig::default()
        };
        assert_eq!(
            next_long_running_trigger(&config, 0, 1_000, 0, 0),
            Some(LongRunningTrigger::TimeThreshold)
        );
        assert_eq!(
            next_long_running_trigger(&config, 0, 100, 5, 0),
            Some(LongRunningTrigger::TurnThreshold)
        );
        assert_eq!(
            next_long_running_trigger(&config, 0, 100, 0, 100),
            Some(LongRunningTrigger::TokenThreshold)
        );
        assert_eq!(next_long_running_trigger(&config, 0, 100, 0, 0), None);
    }

    /// SUBA-N05, REWRITTEN — the previous revision of this test asserted
    /// `resolve_current_path("bash", {"command": "echo hi > out.txt"}) == Some("out.txt")`, which is
    /// NOT what pi does, and "fixing" [`first_redirect_target`] to satisfy it would have
    /// manufactured a divergence from upstream rather than removing one.
    ///
    /// pi's pattern is `/(?:>|>>|tee\s+)(\S+)/` (`long-running-guard.ts:65` @v0.34.0). `\S+` must
    /// match IMMEDIATELY after the `>`, so a space between the redirect operator and its target
    /// kills the match at that position — and, with no further `>`/`tee` in the string, kills it
    /// outright. Verified empirically against a PCRE-family backtracking engine with identical
    /// leftmost-first alternation semantics (`python3 -c "import re;
    /// re.compile(r'(?:>|>>|tee\s+)(\S+)').search(cmd)"` — no JS runtime is installed on this box):
    ///
    /// ```text
    /// 'echo hi > out.txt'    -> None       <- the OLD assertion demanded Some("out.txt")
    /// 'echo hi >out.txt'     -> 'out.txt'
    /// 'echo hi >> out.txt'   -> '>'        <- `>` matches first, `\S+` then captures the second `>`
    /// 'echo a >>b'           -> '>b'
    /// 'cat x | tee  log.txt' -> 'log.txt'
    /// 'ls -la'               -> None
    /// ```
    ///
    /// The space-sensitivity is not even engine-dependent: `\S` cannot match the space that
    /// immediately follows `>` under any regex flavour. Every case below is pinned so a future
    /// "obvious" whitespace-skipping tweak to `first_redirect_target` fails loudly instead of
    /// silently diverging. (`currentPath` is a diagnostic string interpolated into a control notice
    /// — `Facts: … | path <p>` — so upstream's imprecision here is cosmetic, and reproducing it is
    /// strictly better than inventing a "better" answer pi never produces.)
    #[test]
    fn resolve_current_path_reads_direct_args_then_bash_redirects() {
        assert_eq!(
            resolve_current_path("edit", &serde_json::json!({ "path": " src/a.rs " })),
            Some("src/a.rs".to_string())
        );
        // The capturing case: `\S+` starts at the very next byte after `>`.
        assert_eq!(
            resolve_current_path("bash", &serde_json::json!({ "command": "echo hi >out.txt" })),
            Some("out.txt".to_string())
        );
        // pi's real answer for a SPACED redirect is `undefined`, not the path.
        assert_eq!(
            resolve_current_path("bash", &serde_json::json!({ "command": "echo hi > out.txt" })),
            None,
            "`\\S+` cannot match the space after `>`, and no later position matches either"
        );
        // `>` wins the alternation at the first `>` of a `>>`, so the capture is the SECOND `>`.
        assert_eq!(
            resolve_current_path("bash", &serde_json::json!({ "command": "echo hi >> out.txt" })),
            Some(">".to_string())
        );
        assert_eq!(
            resolve_current_path("bash", &serde_json::json!({ "command": "echo a >>b" })),
            Some(">b".to_string())
        );
        // `tee\s+` consumes the whole whitespace run before the capture (greedy `\s+`).
        assert_eq!(
            resolve_current_path("bash", &serde_json::json!({ "command": "cat x | tee  log.txt" })),
            Some("log.txt".to_string())
        );
        assert_eq!(
            resolve_current_path("bash", &serde_json::json!({ "command": "ls -la" })),
            None
        );
        assert_eq!(resolve_current_path("", &serde_json::json!({})), None);
    }

    #[test]
    fn is_mutating_tool_covers_edit_write_cursor_and_bash() {
        assert!(is_mutating_tool("edit", &serde_json::json!({})));
        assert!(is_mutating_tool("write", &serde_json::json!({})));
        assert!(is_mutating_tool(
            "cursor",
            &serde_json::json!({ "activityTitle": "Cursor edit main.rs" })
        ));
        assert!(!is_mutating_tool(
            "cursor",
            &serde_json::json!({ "activityTitle": "Cursor editor opened" })
        ));
        assert!(is_mutating_tool(
            "bash",
            &serde_json::json!({ "command": "rm -rf build" })
        ));
        assert!(!is_mutating_tool(
            "bash",
            &serde_json::json!({ "command": "ls" })
        ));
        assert!(!is_mutating_tool("read", &serde_json::json!({})));
    }

    #[test]
    fn mutating_failure_streak_escalates_and_resets() {
        let mut state = MutatingFailureState::default();
        for i in 0..3 {
            state.record(
                FailedMutatingAttempt {
                    tool: "edit".to_string(),
                    path: Some("a.rs".to_string()),
                    error: format!("failed {i}"),
                    ts: i64::from(i) * 10,
                },
                MUTATING_FAILURE_WINDOW_MS,
            );
        }
        assert!(state.should_escalate(3));
        assert_eq!(
            state.summarize().as_deref(),
            Some("edit(a.rs): failed 0 | edit(a.rs): failed 1 | edit(a.rs): failed 2")
        );
        state.reset();
        assert!(!state.should_escalate(3));
        assert_eq!(state.summarize(), None);
    }

    #[test]
    fn a_failure_outside_the_window_restarts_the_streak() {
        let mut state = MutatingFailureState::default();
        state.record(
            FailedMutatingAttempt {
                tool: "edit".to_string(),
                path: None,
                error: "failed".to_string(),
                ts: 0,
            },
            MUTATING_FAILURE_WINDOW_MS,
        );
        state.record(
            FailedMutatingAttempt {
                tool: "edit".to_string(),
                path: None,
                error: "failed".to_string(),
                ts: MUTATING_FAILURE_WINDOW_MS + 1,
            },
            MUTATING_FAILURE_WINDOW_MS,
        );
        assert!(!state.should_escalate(2), "the streak restarted");
    }

    // ---- ControlMonitor ----

    fn monitor(config: ResolvedControlConfig) -> ControlMonitor {
        ControlMonitor::new(
            config,
            "run1".to_string(),
            "scout".to_string(),
            Some(0),
            None,
            0,
        )
    }

    #[test]
    fn idle_past_the_attention_window_raises_exactly_one_needs_attention_event() {
        let mut m = monitor(ResolvedControlConfig {
            needs_attention_after_ms: 1_000,
            ..ResolvedControlConfig::default()
        });
        assert!(!m.update_activity_state(500));
        assert!(m.update_activity_state(2_000), "transition raises");
        assert!(!m.update_activity_state(3_000), "already needs_attention");
        assert_eq!(m.events().len(), 1);
        assert_eq!(m.events()[0].event_type, ControlEventType::NeedsAttention);
        assert_eq!(m.activity_state(), Some(ActivityState::NeedsAttention));
    }

    #[test]
    fn a_tool_in_flight_suppresses_the_idle_heuristic() {
        let mut m = monitor(ResolvedControlConfig {
            needs_attention_after_ms: 1_000,
            ..ResolvedControlConfig::default()
        });
        m.observe_event(
            &SubagentEvent::ToolExecutionStart {
                tool_call_id: cyrup_core::ToolCallId::from("t1"),
                tool_name: "bash".to_string(),
                args: serde_json::json!({ "command": "sleep 600" }),
            },
            0,
        );
        assert!(!m.update_activity_state(60_000), "a live tool is not idle");
        assert!(m.events().is_empty());
    }

    #[test]
    fn the_long_running_notice_fires_at_most_once_and_yields_to_needs_attention() {
        let mut m = monitor(ResolvedControlConfig {
            active_notice_after_ms: 1_000,
            needs_attention_after_ms: 10_000,
            ..ResolvedControlConfig::default()
        });
        assert!(m.update_activity_state(1_000));
        assert!(!m.update_activity_state(2_000), "at most once");
        assert_eq!(m.events().len(), 1);
        assert_eq!(
            m.events()[0].event_type,
            ControlEventType::ActiveLongRunning
        );
        assert_eq!(m.events()[0].reason, Some(ControlEventReason::TimeThreshold));
    }

    #[test]
    fn repeated_failing_mutating_tools_escalate_to_needs_attention() {
        let mut m = monitor(ResolvedControlConfig {
            failed_tool_attempts_before_attention: 2,
            needs_attention_after_ms: 10_000_000,
            active_notice_after_ms: 10_000_000,
            ..ResolvedControlConfig::default()
        });
        for i in 0..2 {
            let ts = i64::from(i) * 10;
            m.observe_event(
                &SubagentEvent::ToolExecutionStart {
                    tool_call_id: cyrup_core::ToolCallId::from("t"),
                    tool_name: "edit".to_string(),
                    args: serde_json::json!({ "path": "a.rs" }),
                },
                ts,
            );
            m.observe_event(
                &SubagentEvent::ToolExecutionEnd {
                    tool_call_id: cyrup_core::ToolCallId::from("t"),
                    tool_name: "edit".to_string(),
                    result: serde_json::json!("Error: no exact match for the old string"),
                    is_error: false,
                },
                ts + 1,
            );
        }
        assert_eq!(m.events().len(), 1, "{:?}", m.events());
        let event = &m.events()[0];
        assert_eq!(event.reason, Some(ControlEventReason::ToolFailures));
        assert_eq!(
            event.message,
            "scout needs attention after repeated mutating tool failures"
        );
        assert!(
            event
                .recent_failure_summary
                .as_deref()
                .is_some_and(|s| s.contains("edit(a.rs)")),
            "{event:?}"
        );
    }

    #[test]
    fn a_successful_mutating_tool_clears_the_failure_streak() {
        let mut m = monitor(ResolvedControlConfig {
            failed_tool_attempts_before_attention: 2,
            needs_attention_after_ms: 10_000_000,
            active_notice_after_ms: 10_000_000,
            ..ResolvedControlConfig::default()
        });
        let start = SubagentEvent::ToolExecutionStart {
            tool_call_id: cyrup_core::ToolCallId::from("t"),
            tool_name: "edit".to_string(),
            args: serde_json::json!({ "path": "a.rs" }),
        };
        m.observe_event(&start, 0);
        m.observe_event(
            &SubagentEvent::ToolExecutionEnd {
                tool_call_id: cyrup_core::ToolCallId::from("t"),
                tool_name: "edit".to_string(),
                result: serde_json::json!("failed to apply"),
                is_error: false,
            },
            1,
        );
        m.observe_event(&start, 2);
        m.observe_event(
            &SubagentEvent::ToolExecutionEnd {
                tool_call_id: cyrup_core::ToolCallId::from("t"),
                tool_name: "edit".to_string(),
                result: serde_json::json!("wrote 12 lines"),
                is_error: false,
            },
            3,
        );
        m.observe_event(&start, 4);
        m.observe_event(
            &SubagentEvent::ToolExecutionEnd {
                tool_call_id: cyrup_core::ToolCallId::from("t"),
                tool_name: "edit".to_string(),
                result: serde_json::json!("failed to apply"),
                is_error: false,
            },
            5,
        );
        assert!(
            m.events().is_empty(),
            "the success reset the streak, so one later failure must not escalate: {:?}",
            m.events()
        );
    }

    #[test]
    fn disabled_control_raises_nothing_at_all() {
        let mut m = monitor(ResolvedControlConfig {
            enabled: false,
            needs_attention_after_ms: 1,
            active_notice_after_ms: 1,
            ..ResolvedControlConfig::default()
        });
        assert!(!m.update_activity_state(1_000_000));
        m.emit_completion_guard_notice(1, "guard".to_string());
        assert!(m.events().is_empty());
    }

    #[test]
    fn events_reach_the_sink_in_raise_order() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sink_seen = std::sync::Arc::clone(&seen);
        let mut m = ControlMonitor::new(
            ResolvedControlConfig {
                needs_attention_after_ms: 1_000,
                active_notice_after_ms: 500,
                ..ResolvedControlConfig::default()
            },
            "run1".to_string(),
            "scout".to_string(),
            Some(0),
            Some(ControlEventSink::new(move |event| {
                sink_seen
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(control_event_type_wire(event.event_type).to_string());
            })),
            0,
        );
        m.update_activity_state(600); // long-running first
        m.update_activity_state(2_000); // then idle -> needs_attention
        m.emit_completion_guard_notice(3_000, "guard".to_string());
        let order = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            order,
            vec!["active_long_running", "needs_attention", "needs_attention"]
        );
        assert_eq!(m.events().len(), 3, "the sink and the record agree");
    }

    #[test]
    fn notify_on_gates_which_classes_are_raised_at_all() {
        let mut m = monitor(ResolvedControlConfig {
            notify_on: vec![ControlEventType::NeedsAttention],
            active_notice_after_ms: 500,
            needs_attention_after_ms: 1_000,
            ..ResolvedControlConfig::default()
        });
        m.update_activity_state(600);
        assert!(
            m.events().is_empty(),
            "active_long_running is not in notifyOn"
        );
        m.update_activity_state(2_000);
        assert_eq!(m.events().len(), 1);
        assert_eq!(m.events()[0].event_type, ControlEventType::NeedsAttention);
    }
}
