//! The watchdog vocabulary — a 1:1 port of `pi-subagents/src/watchdog/types.ts` (198 lines
//! @v0.43.0).
//!
//! Upstream is a pure type/const module: eight string-literal unions exported both as a
//! `readonly [...]` const tuple and as the derived TS union type, plus the warning, config and
//! settings-result record shapes every other `watchdog/` file consumes. This port keeps the same
//! split — each union becomes a fieldless enum whose [`as_str`](WatchdogSeverity::as_str) reproduces
//! the exact wire string and whose `ALL` const reproduces the upstream tuple **in upstream order**
//! (the order is load-bearing: `settings.ts`'s `parseEnum` renders it into its error text, and
//! `child-status.ts`'s `CHILD_WATCHDOG_PHASES.includes` validates against it).
//!
//! Two unions carry a non-string variant and get bespoke serde:
//!
//! * `thinking?: string | false` (`WatchdogEndpointConfig`) -> [`ThinkingSetting`], which serializes
//!   as the bare JSON `false` for `Off` and as the level string otherwise. A JSON `false` and the
//!   *string* `"false"` are NOT the same input upstream — `model-selection.ts`'s
//!   `parseWatchdogThinkingInput` maps both to `false`, but `settings.ts`'s `parseThinking` accepts
//!   only the boolean — so the two parsers stay distinct here too.
//! * `WatchdogSyncBacklog = "off" | number` -> [`WatchdogSyncBacklog`].
//!
//! TypeScript's structural `WatchdogWarningDetails extends WatchdogWarning` (which makes `category`
//! and `source` REQUIRED where the base leaves them optional) becomes two separate structs here;
//! [`crate::watchdog::warning_format::normalize_watchdog_warning_details`] is the one widening
//! conversion between them, exactly as upstream's `normalizeWatchdogWarningDetails` is.

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

/// `SUBAGENT_WATCHDOG_WARNING_TYPE` (`types.ts:1`) — the `customType` discriminator every watchdog
/// warning message carries, and the key the main-session renderer registers against.
pub const SUBAGENT_WATCHDOG_WARNING_TYPE: &str = "subagent_watchdog_warning";

/// Declare a fieldless enum over an upstream string-literal union: `ALL` in upstream order,
/// `as_str`, and a `parse` that is exactly `ALL.includes(value)`.
macro_rules! watchdog_str_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $( $variant:ident => $wire:literal ),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub enum $name {
            $(
                #[serde(rename = $wire)]
                $variant
            ),+
        }

        impl $name {
            /// Every variant, in the upstream tuple's declaration order.
            pub const ALL: &'static [$name] = &[$( $name::$variant ),+];

            /// The exact upstream wire string for this variant.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( $name::$variant => $wire ),+
                }
            }

            /// The upstream `(<TUPLE> as readonly string[]).includes(value)` membership test,
            /// returning the variant it matched.
            #[must_use]
            pub fn parse(value: &str) -> Option<$name> {
                match value {
                    $( $wire => Some($name::$variant), )+
                    _ => None,
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

watchdog_str_enum! {
    /// `WATCHDOG_WARNING_SEVERITIES` (`types.ts:3-4`).
    WatchdogSeverity { Concern => "concern", Blocker => "blocker" }
}

watchdog_str_enum! {
    /// `WATCHDOG_WARNING_CATEGORIES` (`types.ts:6-16`).
    WatchdogCategory {
        Correctness => "correctness",
        MissedConstraint => "missed-constraint",
        TestGap => "test-gap",
        UnsafeChange => "unsafe-change",
        ScopeDrift => "scope-drift",
        StaleFact => "stale-fact",
        LoopRisk => "loop-risk",
        Other => "other",
    }
}

watchdog_str_enum! {
    /// `WATCHDOG_WARNING_CONFIDENCES` (`types.ts:18-19`). Upstream deliberately omits `low` — the
    /// review prompt forbids low-confidence warnings outright.
    WatchdogConfidence { Medium => "medium", High => "high" }
}

watchdog_str_enum! {
    /// `WATCHDOG_WARNING_SOURCES` (`types.ts:21-22`).
    WatchdogWarningSource {
        Main => "main",
        Child => "child",
        AsyncCompletion => "async-completion",
        Lsp => "lsp",
    }
}

watchdog_str_enum! {
    /// `WATCHDOG_LSP_DIAGNOSTIC_SEVERITIES` (`types.ts:24-25`).
    WatchdogLspDiagnosticSeverity {
        Error => "error",
        Warning => "warning",
        Info => "info",
        Hint => "hint",
    }
}

watchdog_str_enum! {
    /// `WATCHDOG_LSP_STATUSES` (`types.ts:27-28`).
    WatchdogLspStatus {
        Disabled => "disabled",
        Ok => "ok",
        Skipped => "skipped",
        Unavailable => "unavailable",
        Timeout => "timeout",
        Failed => "failed",
    }
}

watchdog_str_enum! {
    /// `WATCHDOG_RUNTIME_STATUSES` (`types.ts:30-31`).
    WatchdogRuntimeStatus {
        Idle => "idle",
        Queued => "queued",
        Reviewing => "reviewing",
        WaitingAtAgentEnd => "waiting-at-agent-end",
        Stale => "stale",
        Failed => "failed",
    }
}

watchdog_str_enum! {
    /// `WATCHDOG_WARNING_STATES` (`types.ts:33-43`).
    WatchdogWarningState {
        Candidate => "candidate",
        Confirmed => "confirmed",
        Displayed => "displayed",
        Stale => "stale",
        Failed => "failed",
        Resolved => "resolved",
        Stalemate => "stalemate",
        Suppressed => "suppressed",
    }
}

watchdog_str_enum! {
    /// `WATCHDOG_LATE_WARNING_POLICIES` (`types.ts:45-46`) — a one-member union upstream, kept as an
    /// enum so a future second policy lands as a variant rather than a stringly-typed compare.
    WatchdogLateWarningPolicy { ShowStaleNoAutofollow => "show-stale-no-autofollow" }
}

watchdog_str_enum! {
    /// `WATCHDOG_DELIVERY_MODES` (`types.ts:48-49`) — likewise one member upstream.
    WatchdogDeliveryMode { Held => "held" }
}

watchdog_str_enum! {
    /// The settings-layer discriminator shared by `WatchdogSettingsError.scope` and
    /// `WatchdogSettingsSource.scope` (`types.ts:180,186`).
    WatchdogSettingsScope { User => "user", Project => "project", Session => "session" }
}

/// `WatchdogSyncBacklog = "off" | number` (`types.ts:51`) — the literal string `"off"` or a positive
/// integer. Serializes back to exactly those two JSON shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogSyncBacklog {
    /// The `"off"` literal.
    Off,
    /// A positive backlog depth.
    Count(u64),
}

impl Serialize for WatchdogSyncBacklog {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            WatchdogSyncBacklog::Off => serializer.serialize_str("off"),
            WatchdogSyncBacklog::Count(n) => serializer.serialize_u64(*n),
        }
    }
}

impl<'de> Deserialize<'de> for WatchdogSyncBacklog {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SyncBacklogVisitor;
        impl Visitor<'_> for SyncBacklogVisitor {
            type Value = WatchdogSyncBacklog;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("the string \"off\" or a positive integer")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<WatchdogSyncBacklog, E> {
                if value == "off" {
                    return Ok(WatchdogSyncBacklog::Off);
                }
                Err(E::custom("syncBacklog must be 'off' or a positive integer"))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<WatchdogSyncBacklog, E> {
                Ok(WatchdogSyncBacklog::Count(value))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<WatchdogSyncBacklog, E> {
                u64::try_from(value)
                    .map(WatchdogSyncBacklog::Count)
                    .map_err(|_| E::custom("syncBacklog must be 'off' or a positive integer"))
            }
        }
        deserializer.deserialize_any(SyncBacklogVisitor)
    }
}

/// `thinking?: string | false` — the reasoning level an endpoint pins, or the JSON literal `false`
/// meaning "reasoning off" (`WatchdogEndpointConfig.thinking`, `types.ts:105`).
///
/// Kept OPEN over the level string (rather than closed over cyrup's `ThinkingLevel`) for the same
/// reason [`crate::exec::apply_thinking_suffix`] is: `off` is a level upstream carries as a plain
/// string in this position and cyrup's on-only enum cannot represent, and a settings file may name
/// a level a future cyrup release adds. Validation against the recognized set is
/// `model_selection`'s job, exactly as it is upstream's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkingSetting {
    /// The JSON literal `false`.
    Off,
    /// A level string (`"off"`, `"minimal"`, ... `"max"` — see `crate::exec::THINKING_LEVELS`).
    Level(String),
}

impl ThinkingSetting {
    /// The `formatThinking`-style label upstream renders for a settings value: `off` for the
    /// literal `false`, else the level verbatim (`register-main.ts:190-193`).
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            ThinkingSetting::Off => "off",
            ThinkingSetting::Level(level) => level.as_str(),
        }
    }
}

impl Serialize for ThinkingSetting {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ThinkingSetting::Off => serializer.serialize_bool(false),
            ThinkingSetting::Level(level) => serializer.serialize_str(level),
        }
    }
}

impl<'de> Deserialize<'de> for ThinkingSetting {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ThinkingVisitor;
        impl Visitor<'_> for ThinkingVisitor {
            type Value = ThinkingSetting;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a thinking-level string or the boolean false")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<ThinkingSetting, E> {
                if value {
                    return Err(E::custom("thinking must be a string or false"));
                }
                Ok(ThinkingSetting::Off)
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<ThinkingSetting, E> {
                Ok(ThinkingSetting::Level(value.to_string()))
            }
        }
        deserializer.deserialize_any(ThinkingVisitor)
    }
}

/// `WatchdogWarning` (`types.ts:53-66`) — the shape the review model's `watchdog_warn` tool emits and
/// the emission guard evaluates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogWarning {
    /// `concern` for actionable risk, `blocker` for a likely wrong or unsafe outcome.
    pub severity: WatchdogSeverity,
    /// One concise sentence naming the issue.
    pub summary: String,
    /// Specific evidence from the turn delta or inspected files.
    pub evidence: String,
    /// Specific action the parent should take before accepting or continuing.
    pub recommended_action: String,
    /// Optional classification; normalizes to [`WatchdogCategory::Other`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<WatchdogCategory>,
    /// Optional confidence; the review tool defaults it to `medium`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<WatchdogConfidence>,
    /// Which watchdog produced it; normalizes to [`WatchdogWarningSource::Main`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<WatchdogWarningSource>,
    /// The child agent name, when this came from a child watchdog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The run id, when this came from a child watchdog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Arrived after the agent-end catch-up timeout: display, never auto-follow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale: Option<bool>,
    /// Which auto-follow attempt this warning belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_follow_attempt: Option<u32>,
    /// Lifecycle state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<WatchdogWarningState>,
}

impl WatchdogWarning {
    /// A minimal warning with only the four required fields set — the shape a `watchdog_warn` tool
    /// call carries before normalization.
    #[must_use]
    pub fn new(
        severity: WatchdogSeverity,
        summary: impl Into<String>,
        evidence: impl Into<String>,
        recommended_action: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            summary: summary.into(),
            evidence: evidence.into(),
            recommended_action: recommended_action.into(),
            category: None,
            confidence: None,
            source: None,
            agent: None,
            run_id: None,
            stale: None,
            auto_follow_attempt: None,
            state: None,
        }
    }
}

/// `WatchdogWarningDetails extends WatchdogWarning` (`types.ts:68-75`): the same record with
/// `category`/`source` resolved to concrete values plus the four runtime-only fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogWarningDetails {
    /// See [`WatchdogWarning::severity`].
    pub severity: WatchdogSeverity,
    /// See [`WatchdogWarning::summary`].
    pub summary: String,
    /// See [`WatchdogWarning::evidence`].
    pub evidence: String,
    /// See [`WatchdogWarning::recommended_action`].
    pub recommended_action: String,
    /// Resolved (never absent, unlike the base record).
    pub category: WatchdogCategory,
    /// Resolved (never absent, unlike the base record).
    pub source: WatchdogWarningSource,
    /// See [`WatchdogWarning::confidence`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<WatchdogConfidence>,
    /// See [`WatchdogWarning::agent`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// See [`WatchdogWarning::run_id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// See [`WatchdogWarning::stale`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale: Option<bool>,
    /// See [`WatchdogWarning::auto_follow_attempt`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_follow_attempt: Option<u32>,
    /// See [`WatchdogWarning::state`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<WatchdogWarningState>,
    /// The emission guard's dedup identity for this warning (`emission-guard.ts`'s
    /// `watchdogWarningIdentity`), stamped when the runtime accepted it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// ISO-8601 instant the warning was rendered into the transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub displayed_at: Option<String>,
    /// The review failure text, for `state: "failed"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// How many consecutive identical blockers stopped auto-follow, for `state: "stalemate"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stalemate_repeats: Option<u32>,
}

/// `WatchdogWarningMessage` (`types.ts:77-82`) — the custom transcript message
/// `createWatchdogWarningMessage` builds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogWarningMessage {
    /// Always [`SUBAGENT_WATCHDOG_WARNING_TYPE`].
    pub custom_type: String,
    /// The `<subagent_watchdog>` XML block the model sees.
    pub content: String,
    /// Whether the TUI renders it.
    pub display: bool,
    /// The structured payload the registered renderer reads.
    pub details: WatchdogWarningDetails,
}

/// `WatchdogAutoFollowConfig` (`types.ts:84-88`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogAutoFollowConfig {
    /// Auto-follow blocker warnings with a synthetic user message.
    pub blockers: bool,
    /// Attempt ceiling; `None` is upstream's `null` (unbounded).
    pub max_attempts: Option<u32>,
    /// Consecutive identical blockers that declare a stalemate and stop auto-follow.
    pub stalemate_repeats: u32,
}

/// `WatchdogGuidanceConfig` (`types.ts:90-93`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogGuidanceConfig {
    /// Fold a discovered `WATCHDOG.md` into the review's guidance.
    pub watchdog_md: bool,
    /// An explicit system-prompt file for the review model.
    pub system_prompt_path: Option<String>,
}

/// `WatchdogScopeConfig` (`types.ts:95-97`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogScopeConfig {
    /// Include the rolling user-prompt scope artifact in the review input.
    pub enabled: bool,
}

/// `WatchdogCadenceConfig` (`types.ts:99-101`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogCadenceConfig {
    /// Run a mid-run review every N tool results; `None` is upstream's `null` (boundary only).
    pub every_n_tools: Option<u32>,
}

/// `WatchdogEndpointConfig` (`types.ts:103-107`) — the `main` endpoint's own knobs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogEndpointConfig {
    /// Whether this endpoint reviews at all.
    pub enabled: bool,
    /// An explicit review model; absent means "inherit the session model".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// An explicit reasoning level for the review model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingSetting>,
}

/// `WatchdogChildOverrideConfig` (`types.ts:109-113`) — a per-agent override of the children block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogChildOverrideConfig {
    /// Override `children.enabled` for this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Override `children.model` for this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Override `children.thinking` for this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingSetting>,
}

/// `WatchdogChildrenConfig extends WatchdogEndpointConfig` (`types.ts:115-119`).
///
/// The `extends` is flattened into explicit fields (Rust has no structural inheritance); the
/// `overrides` map is a [`BTreeMap`] so `register-main.ts`'s `Object.entries(children.overrides)`
/// status line renders deterministically rather than in hash order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogChildrenConfig {
    /// Whether child (subagent-side) watchdogs run at all.
    pub enabled: bool,
    /// An explicit review model for children.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// An explicit reasoning level for children.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingSetting>,
    /// How long the parent holds a settled child open waiting for its watchdog tail.
    pub watchdog_tail_timeout_ms: u64,
    /// The children's auto-follow policy.
    pub auto_follow: WatchdogAutoFollowConfig,
    /// Per-agent overrides.
    pub overrides: BTreeMap<String, WatchdogChildOverrideConfig>,
}

/// `WatchdogAsyncCompletionConfig` (`types.ts:121-124`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogAsyncCompletionConfig {
    /// Review a background run's completion payload.
    pub enabled: bool,
    /// Auto-follow blockers found in that review.
    pub auto_follow_blockers: bool,
}

/// `WatchdogLspConfig` (`types.ts:126-131`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogLspConfig {
    /// Collect language-server diagnostics for changed files at the agent-end boundary.
    pub enabled: bool,
    /// Whole-collection budget.
    pub timeout_ms: u64,
    /// Cap on files opened in one collection.
    pub max_files: u32,
    /// Cap on diagnostics carried out of one collection.
    pub max_diagnostics: u32,
}

/// `WatchdogLspDiagnostic` (`types.ts:133-141`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogLspDiagnostic {
    /// Repo-relative path.
    pub path: String,
    /// 1-based line.
    pub line: u32,
    /// 1-based column.
    pub column: u32,
    /// Mapped LSP severity.
    pub severity: WatchdogLspDiagnosticSeverity,
    /// The diagnostic's `source` field, defaulted to the provider name.
    pub source: String,
    /// The diagnostic code, stringified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Whitespace-collapsed, length-capped message.
    pub message: String,
}

/// `WatchdogLspResult` (`types.ts:143-150`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogLspResult {
    /// Outcome of the collection.
    pub status: WatchdogLspStatus,
    /// Human label of the language server that ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Paths actually opened.
    pub checked_paths: Vec<String>,
    /// Paths skipped (wrong language, outside the root, missing, over the file cap).
    pub skipped_paths: Vec<String>,
    /// The diagnostics themselves.
    pub diagnostics: Vec<WatchdogLspDiagnostic>,
    /// Failure/timeout detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `WatchdogLspRuntimeSnapshot extends WatchdogLspResult` (`types.ts:152-157`) — the result plus the
/// three runtime-only counters the status renderer reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogLspRuntimeSnapshot {
    /// The collection result (flattened upstream via `extends`).
    #[serde(flatten)]
    pub result: WatchdogLspResult,
    /// Whether LSP collection is configured on.
    pub enabled: bool,
    /// Diagnostics the server reported before the freshness ledger reduced them.
    pub diagnostic_count: usize,
    /// Diagnostics that survived the ledger (i.e. not seen on a previous pass).
    pub fresh_diagnostic_count: usize,
    /// ISO-8601 instant of the last collection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// `ResolvedWatchdogConfig` (`types.ts:159-178`) — the fully-defaulted config every runtime path
/// reads. Built only by `settings.rs` (`resolveWatchdogConfig`/`DEFAULT_WATCHDOG_CONFIG`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedWatchdogConfig {
    /// Master switch.
    pub enabled: bool,
    /// How warnings reach the transcript.
    pub delivery: WatchdogDeliveryMode,
    /// Render warnings while a run is still in flight.
    pub show_during_run: bool,
    /// Synchronous-backlog depth.
    pub sync_backlog: WatchdogSyncBacklog,
    /// How long the agent-end boundary waits for the review to land.
    pub agent_end_timeout_ms: u64,
    /// What to do with a warning that arrives after that timeout.
    pub late_warning_policy: WatchdogLateWarningPolicy,
    /// Minimum severity that is displayed at all.
    pub severity_threshold: WatchdogSeverity,
    /// Emission-guard ceiling; `None` is upstream's `null` (unbounded).
    pub max_warnings: Option<u32>,
    /// Review-guidance sources.
    pub guidance: WatchdogGuidanceConfig,
    /// The main endpoint's auto-follow policy.
    pub auto_follow: WatchdogAutoFollowConfig,
    /// The scope-artifact toggle.
    pub scope: WatchdogScopeConfig,
    /// Mid-run review cadence.
    pub cadence: WatchdogCadenceConfig,
    /// The main endpoint.
    pub main: WatchdogEndpointConfig,
    /// The children endpoint.
    pub children: WatchdogChildrenConfig,
    /// Async-completion review policy.
    pub async_completion: WatchdogAsyncCompletionConfig,
    /// LSP diagnostics policy.
    pub lsp: WatchdogLspConfig,
    /// Context-window percentage at which the runtime compacts its own review history.
    pub compact_at_percent: u32,
    /// Backoff before retrying a failed review.
    pub review_retry_delay_ms: u64,
    /// Consecutive review failures tolerated.
    pub max_review_failures: u32,
}

/// `WatchdogSettingsError` (`types.ts:180-184`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogSettingsError {
    /// Which layer failed to parse.
    pub scope: WatchdogSettingsScope,
    /// The file, when the layer has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The parse failure text.
    pub message: String,
}

/// `WatchdogSettingsSource` (`types.ts:186-190`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogSettingsSource {
    /// Which layer this is.
    pub scope: WatchdogSettingsScope,
    /// The file, when the layer has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether that file exists on disk.
    pub exists: bool,
}

/// `WatchdogSettingsResult` (`types.ts:192-197`) — what `resolveWatchdogConfig` hands the runtime:
/// the config, whether every layer parsed, and the per-layer diagnostics the status renderer prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogSettingsResult {
    /// True iff `errors` is empty.
    pub ok: bool,
    /// The resolved config — the pristine defaults when `ok` is false.
    pub config: ResolvedWatchdogConfig,
    /// Per-layer parse failures.
    pub errors: Vec<WatchdogSettingsError>,
    /// Every layer consulted, in precedence order.
    pub sources: Vec<WatchdogSettingsSource>,
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn union_tuples_keep_upstream_order_and_wire_strings() {
        assert_eq!(
            WatchdogCategory::ALL
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>(),
            vec![
                "correctness",
                "missed-constraint",
                "test-gap",
                "unsafe-change",
                "scope-drift",
                "stale-fact",
                "loop-risk",
                "other"
            ]
        );
        assert_eq!(
            WatchdogRuntimeStatus::ALL
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            vec![
                "idle",
                "queued",
                "reviewing",
                "waiting-at-agent-end",
                "stale",
                "failed"
            ]
        );
        assert_eq!(
            WatchdogWarningState::ALL
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            vec![
                "candidate",
                "confirmed",
                "displayed",
                "stale",
                "failed",
                "resolved",
                "stalemate",
                "suppressed"
            ]
        );
        assert_eq!(
            WatchdogSeverity::parse("blocker"),
            Some(WatchdogSeverity::Blocker)
        );
        assert_eq!(WatchdogSeverity::parse("Blocker"), None);
    }

    #[test]
    fn thinking_setting_serializes_false_as_json_bool() {
        assert_eq!(
            serde_json::to_string(&ThinkingSetting::Off).unwrap(),
            "false"
        );
        assert_eq!(
            serde_json::to_string(&ThinkingSetting::Level("high".into())).unwrap(),
            "\"high\""
        );
        assert_eq!(
            serde_json::from_str::<ThinkingSetting>("false").unwrap(),
            ThinkingSetting::Off
        );
        assert_eq!(
            serde_json::from_str::<ThinkingSetting>("\"high\"").unwrap(),
            ThinkingSetting::Level("high".into())
        );
        // `true` is not a member of `string | false`.
        assert!(serde_json::from_str::<ThinkingSetting>("true").is_err());
    }

    #[test]
    fn sync_backlog_serializes_as_off_or_number() {
        assert_eq!(
            serde_json::to_string(&WatchdogSyncBacklog::Off).unwrap(),
            "\"off\""
        );
        assert_eq!(
            serde_json::to_string(&WatchdogSyncBacklog::Count(4)).unwrap(),
            "4"
        );
    }

    #[test]
    fn warning_omits_absent_optionals_the_way_upstream_spreads_do() {
        let warning = WatchdogWarning::new(
            WatchdogSeverity::Concern,
            "summary",
            "evidence",
            "recommendedAction",
        );
        let json = serde_json::to_value(&warning).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "severity": "concern",
                "summary": "summary",
                "evidence": "evidence",
                "recommendedAction": "recommendedAction",
            })
        );
    }
}
