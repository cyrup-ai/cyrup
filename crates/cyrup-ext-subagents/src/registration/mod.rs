//! Extension registration (native `NativeExtension`, tool + 12 slash commands + doctor
//! diagnostics), config-schema layering (`config.json` + `cyrup-config` settings), and durable
//! persistence (func-SA §5.6; arch-SA §3.8/§4.6/§6.8).
//!
//! This file owns the **shared config types and settings-precedence resolution** consumed by
//! every other subsystem in this crate (R-SA-126..143's data-model surface): [`SubagentExtensionConfig`]
//! (the `config.json`-backed extension-level knobs, func-SA §4.7), [`SubagentsSettingsView`] (the
//! namespaced `subagents` slice of `cyrup-config::Settings`, func-SA §4.7), [`HookSpec`] (the
//! worktree-setup-hook external-command descriptor, func-SA §4.7), and [`resolve_effective_config`]
//! (the five-tier precedence walk fixed by R-SA-133).
//!
//! Slash-command descriptors ([`registration::slash_commands`](super)) are a separate, later phase
//! of this crate's build-out and are **not** implemented in this file — they consume the types
//! defined here but live in their own sibling module (`registration/slash_commands.rs`), which does
//! not exist yet as of this phase; this file does not declare a `pub mod` item for it. The crate's
//! `extension.rs` façade (also a later phase) is the eventual sole caller of
//! [`resolve_effective_config`] at spawn/dispatch time.
//!
//! [`profiles`] (named model-tier profiles, R-SA-141/142), [`doctor`] (`/subagents-doctor`'s
//! concurrent check runner, R-SA-131), and [`cost`] (`/subagent-cost`'s recursive dual-shape usage
//! accounting, R-SA-140) **are** implemented and declared below — each is its own phase's
//! deliverable, not deferred.
//!
//! # R-SA-133: layered config resolution precedence
//!
//! Config resolution layers, in this **exact** precedence (highest wins):
//!
//! 1. **Inline per-call tool/slash-command overrides** — a caller-supplied value for this one
//!    invocation (e.g. `model=...` on `/run`, or a `subagent` tool-call parameter).
//! 2. **`subagents.agentOverrides.<name>`/`subagents.defaultModel`** from `cyrup-config`'s
//!    effective (CLI ▷ project ▷ global) settings view — [`SubagentsSettingsView`].
//! 3. **`config.json`'s** `maxSubagentSpawnsPerSession`/`globalConcurrencyLimit`/etc. —
//!    [`SubagentExtensionConfig`].
//! 4. **Agent-frontmatter defaults** — `AgentDefinition`'s own parsed fields
//!    (`crate::discovery::types::AgentDefinition`).
//! 5. **Hardcoded extension defaults** — [`SubagentExtensionConfig::default`]'s constant values.
//!
//! [`resolve_effective_config`] and its field-level sibling [`resolve_field`] implement this walk
//! as a single, explicit, testable precedence chain — never five separately-scattered `.or_else`
//! call sites that could silently drift out of order. Every field this crate resolves through the
//! five tiers (model, max-depth, concurrency limits, etc.) is expected to route through
//! [`resolve_field`] (or the whole-config convenience wrapper [`resolve_effective_config`]) rather
//! than re-deriving its own precedence chain.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::discovery::types::AgentOverrideConfig;

/// `/subagents-doctor`'s concurrent check runner (R-SA-131): [`doctor::DoctorRunner`] executes
/// the six mandated checks — binary resolution, temp-dir writability, `config.json` validity,
/// agent-discovery count, chain-discovery count, and provider-catalog freshness — concurrently via
/// `tokio::join!`, each catching its own failure independently rather than aborting the whole
/// report. See [`doctor`] for the full subsystem doc.
pub mod doctor;

pub mod profiles;

/// The 12 slash-command descriptors and their pure argument parsers (R-SA-129):
/// [`slash_commands::SLASH_COMMANDS`] is the static registration table `extension.rs` iterates at
/// `init()` time, and `slash_commands::parse_*` functions turn each command's raw trailing
/// argument string into a strongly-typed parsed-command value, including `/chain`'s inline
/// parallel-group `(a | b)[opts]` chain-expression grammar. See [`slash_commands`] for the full
/// subsystem doc, including what is explicitly deferred to `extension.rs`'s later-phase wiring
/// (agent-name existence validation, actual `InitApi` registration, and the single shared
/// dispatch path itself, R-SA-130).
pub mod slash_commands;

/// `/subagent-cost`'s recursive dual-shape token/cost usage accounting (R-SA-140):
/// [`cost::compute_recursive_cost`] sums usage recursively through nested subagent-of-subagent
/// trees across BOTH a run's `_meta.json` `children` array shape and any per-step nested children
/// (`StepStatus::nested_run_ids`) within async chain jobs — additively combining both, never just
/// one (a flat single-level or single-shape sum is explicitly non-conformant per func-SA §5.6's own
/// warning text). See [`cost`] for the full subsystem doc, including the dual-recursion rationale.
pub mod cost;

/// Bundled packaged resources (R-SA-132/134): the 7 `prompts/*.md` recipe templates and the
/// `skills/pi-subagents/SKILL.md` operational skill this extension ships, discovered through the
/// SAME `cyrup-resources` manifest plumbing the builtin agent personas use. See [`resources`] for
/// the full subsystem doc, including why the manifest's directory entries are expanded to concrete
/// files here.
pub mod resources;

/// Prompt-template workflows (R-SA-132/134): discovery of the `prompts/*.md` recipes across the
/// package/user/project tiers plus the argument grammar and recipe→run lowering behind the
/// `/prompt-workflow` and `/chain-prompts` slash commands. A 1:1 port of
/// `pi-subagents/src/slash/prompt-workflows.ts` @v0.34.0. See [`prompt_workflows`] for the full
/// subsystem doc, including which fields of a recipe cyrup's dispatch surface can carry today.
pub mod prompt_workflows;

// -------------------------------------------------------------------------------------------
// SubagentExtensionConfig (func-SA §4.7; arch-SA §3.8) — tier 3 of R-SA-133
// -------------------------------------------------------------------------------------------

/// The `config.json`-backed extension-level configuration (func-SA §4.7 `SubagentExtensionConfig`;
/// arch-SA §3.8). This is tier **3** of R-SA-133's five-tier precedence — below inline call
/// overrides and `subagents.*` settings, above agent-frontmatter defaults and this struct's own
/// hardcoded [`Default`] values.
///
/// Persisted via `background::atomic::write_atomic_json` (R-SA-135, same shared atomic-write
/// primitive `status.json`/`meta.json` use — this type only defines the shape; the read/write I/O
/// itself is owned by a later phase of this crate's build-out, `registration`'s own persistence
/// helpers, not this file).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubagentExtensionConfig {
    /// When `true`, a `subagent` tool call / slash command with no explicit `--bg`/foreground
    /// choice defaults to background (async) execution rather than foreground.
    pub async_by_default: bool,
    /// When `true`, forces every **top-level** (directly orchestrator-invoked, not nested) run to
    /// async execution regardless of `async_by_default` or an inline per-call override — a
    /// stricter knob than `async_by_default` alone.
    pub force_top_level_async: bool,
    /// Process-wide cap on concurrently-running subagent child processes across all tracked runs
    /// (foreground + background combined). Default 20 (func-SA §4.7).
    pub global_concurrency_limit: u32,
    /// Cap on the total number of subagent spawns permitted within one orchestrator session,
    /// across every run mode. Default 40 (func-SA §4.7).
    pub max_subagent_spawns_per_session: u32,
    /// Top-level parallel fan-out limits, as a NESTED object matching pi's
    /// `ExtensionConfig.parallel?: { maxTasks?, concurrency? }` (types.ts:829-832/874) — NOT two
    /// flat `parallelMaxTasks`/`parallelConcurrency` keys. Read via the [`Self::parallel_max_tasks`]
    /// / [`Self::parallel_concurrency`] accessors, which fall back to pi's defaults (8 / 4) when the
    /// object, or a field within it, is omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel: Option<TopLevelParallelConfig>,
    /// Live-control notice thresholds/channels — pi `ExtensionConfig.control?: ControlConfig`
    /// (types.ts:101-110/873). Feeds the control-notice state machine (`tui/notices.rs`); a resolved
    /// view is produced by pi's `resolveControlConfig`. `None` = every threshold defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control: Option<ControlConfig>,
    /// Chain-specific extension config — pi `ExtensionConfig.chain?: { dynamicFanout?: { maxItems? } }`
    /// (types.ts:834-838/875): the per-run cap on how many items a dynamic fan-out may expand to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain: Option<ExtensionChainConfig>,
    /// Proactive skill-subagent suggestion config — pi
    /// `ExtensionConfig.proactiveSkillSubagents?: ProactiveSkillSubagentsConfig | false`
    /// (types.ts:840-845/880): an object of tuning knobs, or the literal `false` to disable the
    /// feature entirely.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proactive_skill_subagents: Option<ProactiveSkillSubagents>,
    /// Default directory new subagent session files are written under, when neither an inline
    /// call override nor an agent-frontmatter default supplies one. `None` defers to this crate's
    /// own computed default (owned by `exec`/`background`, not this type).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_session_dir: Option<PathBuf>,
    /// Base directory single-run (non-chain, non-parallel) output artifacts are written under,
    /// when no more specific output path is resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub single_run_output_base_dir: Option<PathBuf>,
    /// The effective recursion-depth ceiling new top-level runs start from (R-SA-055/056's
    /// tightening-only algorithm may only lower this per nested spawn, never raise it). Default 2
    /// (func-SA §4.7).
    pub max_subagent_depth: u32,
    /// Base directory new git worktrees (R-SA-060..064's `worktree: true` fan-out isolation) are
    /// created under. `None` defers to a per-repository computed default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_base_dir: Option<PathBuf>,
    /// An optional external setup script invoked once per `worktree: true` group after worktree
    /// creation, before any child process is spawned into it (R-SA-063). Matches pi's
    /// `ExtensionConfig.worktreeSetupHook?: string` (types.ts:876): a bare **script-path string**
    /// (e.g. `"./scripts/setup-worktree.mjs"`), NOT a `{ command, args }` object — pi resolves it
    /// into a runnable `{ hookPath, timeoutMs }` at spawn time (`subagent-runner.ts:1975`). The
    /// crate-internal runnable shape (`spawn::worktree`'s `WorktreeSetupHookConfig`/[`HookSpec`]) is
    /// derived from this path plus [`Self::worktree_setup_hook_timeout_ms`] downstream, not stored
    /// here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_setup_hook: Option<PathBuf>,
    /// Timeout, in milliseconds, for the worktree setup hook (R-SA-063: "target 30000ms, if
    /// unset"). `None` here means "use the hard-coded 30000ms default" — the concrete default
    /// constant itself lives in `spawn::worktree::DEFAULT_HOOK_TIMEOUT`, not duplicated here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_setup_hook_timeout_ms: Option<u64>,
    /// The `wait` tool's config gate — pi `ExtensionConfig.waitTool?: WaitToolConfig`
    /// (`extension/index.ts:260` `resolveWaitToolConfig(config.waitTool)`), accepting either a bare
    /// boolean or `{ enabled?: boolean }`. `None` (the field omitted) = enabled, pi's default.
    /// [`crate::background::wait::WAIT_TOOL_ENABLED_ENV`] overrides whatever this says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_tool: Option<crate::background::wait::WaitToolSetting>,
}

/// pi's hardcoded default for `parallel.maxTasks` (func-SA §4.7) — the cap applied when the nested
/// `parallel` object, or its `maxTasks` field, is omitted from `config.json`.
pub const DEFAULT_PARALLEL_MAX_TASKS: u32 = 8;
/// pi's hardcoded default for `parallel.concurrency` (func-SA §4.7).
pub const DEFAULT_PARALLEL_CONCURRENCY: u32 = 4;

impl Default for SubagentExtensionConfig {
    /// Tier 5 of R-SA-133: the hardcoded extension defaults every other tier layers on top of.
    fn default() -> Self {
        Self {
            async_by_default: false,
            force_top_level_async: false,
            global_concurrency_limit: 20,
            max_subagent_spawns_per_session: 40,
            parallel: None,
            control: None,
            chain: None,
            proactive_skill_subagents: None,
            default_session_dir: None,
            single_run_output_base_dir: None,
            max_subagent_depth: 2,
            worktree_base_dir: None,
            worktree_setup_hook: None,
            worktree_setup_hook_timeout_ms: None,
            wait_tool: None,
        }
    }
}

impl SubagentExtensionConfig {
    /// The effective `parallel.maxTasks` (pi `ExtensionConfig.parallel?.maxTasks`), falling back to
    /// [`DEFAULT_PARALLEL_MAX_TASKS`] (8) when the nested `parallel` object — or its `maxTasks`
    /// field — is omitted.
    #[must_use]
    pub fn parallel_max_tasks(&self) -> u32 {
        self.parallel
            .as_ref()
            .and_then(|p| p.max_tasks)
            .unwrap_or(DEFAULT_PARALLEL_MAX_TASKS)
    }

    /// The effective `parallel.concurrency` (pi `ExtensionConfig.parallel?.concurrency`), falling
    /// back to [`DEFAULT_PARALLEL_CONCURRENCY`] (4) when the nested `parallel` object — or its
    /// `concurrency` field — is omitted.
    #[must_use]
    pub fn parallel_concurrency(&self) -> u32 {
        self.parallel
            .as_ref()
            .and_then(|p| p.concurrency)
            .unwrap_or(DEFAULT_PARALLEL_CONCURRENCY)
    }

    /// The per-run dynamic-fanout item cap (pi `ExtensionConfig.chain?.dynamicFanout?.maxItems`),
    /// or `None` when unconfigured (the fan-out subsystem then applies its own hard default).
    #[must_use]
    pub fn dynamic_fanout_max_items(&self) -> Option<u32> {
        self.chain
            .as_ref()
            .and_then(|c| c.dynamic_fanout.as_ref())
            .and_then(|d| d.max_items)
    }
}

// -------------------------------------------------------------------------------------------
// HookSpec (func-SA §4.7; arch-SA §3.8) — the canonical definition
// -------------------------------------------------------------------------------------------

/// A configured external hook command: invoked with a JSON payload on stdin, expecting a JSON
/// response on stdout (func-SA §4.7 `HookSpec`; §5.3 R-SA-034/R-SA-063).
///
/// This is the **canonical** definition arch-SA §2.2 designates for `registration/mod.rs`.
/// `spawn::worktree::HookSpec` currently carries its own textually-identical, independently
/// defined copy of this exact shape (`command: PathBuf, args: Vec<String>`), because — per that
/// module's own doc comment — `registration/mod.rs` was still a doc-comment-only stub at the time
/// `spawn::worktree.rs` was written and had no `HookSpec` to import. Now that this type exists
/// here, `spawn::worktree`'s copy is expected to become a type alias (`pub type HookSpec =
/// crate::registration::HookSpec;`) or be removed in favor of importing this one directly — that
/// migration is left to whichever later phase next touches `spawn/worktree.rs`, so as not to
/// perturb that already-complete, already-tested module's file outside this task's declared
/// ownership boundary. The two shapes are guaranteed identical field-for-field so that migration
/// is a pure rename with no behavior change.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookSpec {
    /// The executable to invoke.
    pub command: PathBuf,
    /// Arguments passed to `command`, before the JSON-on-stdin payload.
    pub args: Vec<String>,
}

// -------------------------------------------------------------------------------------------
// Nested config objects (pi types.ts:829-882) — the shapes pi's ExtensionConfig nests
// -------------------------------------------------------------------------------------------

/// pi `TopLevelParallelConfig` (types.ts:829-832): the nested `parallel: { maxTasks?, concurrency? }`
/// object of [`SubagentExtensionConfig`]. Both fields are optional; an omitted field defers to the
/// hardcoded pi default via [`SubagentExtensionConfig::parallel_max_tasks`] /
/// [`SubagentExtensionConfig::parallel_concurrency`].
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TopLevelParallelConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tasks: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
}

/// pi `ExtensionChainConfig` (types.ts:834-838): the nested `chain: { dynamicFanout?: { maxItems? } }`
/// object of [`SubagentExtensionConfig`].
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExtensionChainConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_fanout: Option<DynamicFanoutConfig>,
}

/// The `chain.dynamicFanout` object (pi types.ts:835-837): the per-run cap on how many items a
/// dynamic fan-out step may expand to.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DynamicFanoutConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,
}

/// One control-notice event class (pi `ControlEventType`, types.ts:98): the two activity-state
/// transitions a run may raise a control notice for. Serializes as `active_long_running` /
/// `needs_attention` (matching pi's string union and [`crate::background::ActivityState`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlEventType {
    ActiveLongRunning,
    NeedsAttention,
}

/// One control-notice delivery channel (pi `ControlNotificationChannel`, types.ts:99). Serializes
/// as `event` / `async` / `intercom`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ControlNotificationChannel {
    Event,
    Async,
    Intercom,
}

/// pi `ControlConfig` (types.ts:101-110): the live-control notice thresholds/channels nested under
/// [`SubagentExtensionConfig::control`]. Every field is optional; pi's `resolveControlConfig`
/// derives a fully-defaulted `ResolvedControlConfig` from this plus per-call overrides. This crate
/// carries the raw config shape faithfully so the resolved view (owned by the control-notice
/// subsystem, `tui/notices.rs`) can be produced from it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ControlConfig {
    /// Master enable/disable for control notices (pi `ControlConfig.enabled`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Idle time (ms) after which a run is flagged `needs_attention` (pi `needsAttentionAfterMs`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_attention_after_ms: Option<u64>,
    /// Elapsed time (ms) after which a still-running run raises an `active_long_running` notice (pi
    /// `activeNoticeAfterMs`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_notice_after_ms: Option<u64>,
    /// Turn count after which an `active_long_running` notice is raised (pi `activeNoticeAfterTurns`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_notice_after_turns: Option<u64>,
    /// Token count after which an `active_long_running` notice is raised (pi `activeNoticeAfterTokens`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_notice_after_tokens: Option<u64>,
    /// Consecutive failed tool attempts that escalate a run to `needs_attention` (pi
    /// `failedToolAttemptsBeforeAttention`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_tool_attempts_before_attention: Option<u32>,
    /// Which event classes to actually notify on (pi `notifyOn`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify_on: Option<Vec<ControlEventType>>,
    /// Which channels to deliver notices through (pi `notifyChannels`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify_channels: Option<Vec<ControlNotificationChannel>>,
}

/// pi `ProactiveSkillSubagentsConfig` (types.ts:840-845): the tuning knobs for proactive
/// skill-subagent suggestions.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProactiveSkillSubagentsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_references: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_recommendations: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_agent: Option<String>,
}

/// pi `proactiveSkillSubagents?: ProactiveSkillSubagentsConfig | false` (types.ts:880): either a
/// tuning-knob object, or the literal `false` to disable the feature entirely. Deserialized
/// untagged so both a JSON object and a bare `false` parse.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ProactiveSkillSubagents {
    /// The literal `false` (or `true`) form — `false` disables the feature.
    Toggle(bool),
    /// The full config-object form.
    Config(ProactiveSkillSubagentsConfig),
}

impl ProactiveSkillSubagents {
    /// Whether proactive skill-subagent suggestions are enabled: `false` when set to the literal
    /// `false`, or when the config object's own `enabled` field is `Some(false)`; otherwise enabled
    /// (pi treats a bare object or an omitted `enabled` as on).
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        match self {
            ProactiveSkillSubagents::Toggle(on) => *on,
            ProactiveSkillSubagents::Config(cfg) => cfg.enabled.unwrap_or(true),
        }
    }
}

// -------------------------------------------------------------------------------------------
// SubagentsSettingsView (func-SA §4.7; arch-SA §3.8) — tier 2 of R-SA-133
// -------------------------------------------------------------------------------------------

/// The namespaced `subagents` slice of `cyrup-config::Settings` (func-SA §4.7
/// `SubagentsSettingsView`; arch-SA §3.8), as read via `SettingsManager::effective().get(
/// "subagents")` and written back (for the narrow cases this extension mutates settings at all,
/// e.g. named-profile load, R-SA-141) via `SettingsManager::set_nested(scope, &["subagents",
/// ...], value)`.
///
/// This is tier **2** of R-SA-133's five-tier precedence — below inline per-call overrides, above
/// `config.json` (tier 3). It is deliberately a **distinct on-disk store** from
/// [`SubagentExtensionConfig`] (arch-SA §4.6): `SubagentExtensionConfig` lives in a
/// crate-owned `config.json` file under the extension's own config directory, while this view is
/// backed by `cyrup-config`'s already-layered (global ◁ project ◁ CLI) `Settings` document under
/// the `"subagents"` top-level key. No new schema registration is required inside `cyrup-config`
/// itself for this — the key is read/written through that crate's already-untyped,
/// extension-namespaced design (`Settings::get`/`SettingsManager::set_nested`), exactly as arch-SA
/// §4.6 specifies.
///
/// Distinct in shape from [`crate::discovery::types::SubagentSettings`] (which `discovery/`'s own
/// `parse_subagent_settings` produces for the discovery/merge pipeline's consumption,
/// `overrides: BTreeMap<String, AgentOverrideConfig>`): this view flattens `disable_builtins`/
/// `disable_thinking` to concrete, already-defaulted `bool`s (rather than `Option<bool>`) and
/// renames `overrides` to `agent_overrides: HashMap<...>` per arch-SA §3.8's own field list —
/// [`SubagentsSettingsView::from_subagent_settings`] performs that one conversion in one place so
/// the two shapes never silently drift out of sync with each other.
#[derive(Clone, Debug, Default)]
pub struct SubagentsSettingsView {
    /// `subagents.defaultModel` — the fallback model name applied when neither an inline call
    /// override nor a per-agent override nor the agent's own frontmatter supplies one.
    pub default_model: Option<String>,
    /// `subagents.overrides.<name>` — per-agent settings-layer override deltas (R-SA-010/011/012),
    /// keyed by the agent's runtime name.
    pub agent_overrides: HashMap<String, AgentOverrideConfig>,
    /// `subagents.disableBuiltins` — when `true`, the builtin-scope tier of agent discovery
    /// (R-SA-001) is excluded entirely. Defaults to `false` when unset in settings.
    pub disable_builtins: bool,
    /// `subagents.disableThinking` — when `true`, extended-thinking is force-disabled for every
    /// resolved agent unless a same-scope override explicitly re-sets `thinking` (R-SA-012).
    /// Defaults to `false` when unset in settings.
    pub disable_thinking: bool,
}

impl SubagentsSettingsView {
    /// Convert `discovery`'s already-parsed [`crate::discovery::types::SubagentSettings`] (the
    /// shape `discovery::parse_subagent_settings` produces from raw settings JSON) into this
    /// module's `arch-SA §3.8` view shape — the one place the two representations' field-name/
    /// optionality differences are reconciled, so `registration/`'s precedence resolution and
    /// `discovery/`'s override-application pipeline are guaranteed to observe an identical
    /// underlying `subagents.*` settings value, never two independently-parsed copies that could
    /// disagree.
    #[must_use]
    pub fn from_subagent_settings(
        settings: &crate::discovery::types::SubagentSettings,
    ) -> Self {
        Self {
            default_model: settings.default_model.clone(),
            agent_overrides: settings
                .overrides
                .iter()
                .map(|(name, cfg)| (name.clone(), cfg.clone()))
                .collect(),
            disable_builtins: settings.disable_builtins.unwrap_or(false),
            disable_thinking: settings.disable_thinking.unwrap_or(false),
        }
    }

    /// The settings-layer override delta for one agent's runtime name, if any (R-SA-010's
    /// custom-agent-vs-builtin branch is applied by `discovery::merge`, not here — this accessor
    /// only looks the delta up).
    pub fn override_for(&self, agent_name: &str) -> Option<&AgentOverrideConfig> {
        self.agent_overrides.get(agent_name)
    }
}

// -------------------------------------------------------------------------------------------
// Five-tier config-precedence resolution (R-SA-133)
// -------------------------------------------------------------------------------------------

/// One resolved field's value, tagged with **which tier of R-SA-133's five-tier precedence chain
/// actually supplied it** — mirrors [`crate::discovery::types::AgentModelSourceInfo`]'s pattern of
/// carrying provenance alongside a resolved value, applied here to the config layer generally
/// rather than only to `model`, so `/subagents-doctor` (R-SA-131) and other diagnostic surfaces
/// can report *why* a given effective value was chosen without re-deriving the precedence walk a
/// second time.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigTier {
    /// Tier 1: an inline per-call tool/slash-command override.
    InlineCallOverride,
    /// Tier 2: `subagents.agentOverrides.<name>` / `subagents.defaultModel` from the layered
    /// `cyrup-config` settings view.
    Settings,
    /// Tier 3: this extension's own `config.json`.
    ExtensionConfig,
    /// Tier 4: the agent's own on-disk frontmatter default.
    AgentFrontmatter,
    /// Tier 5: this crate's hardcoded extension default.
    HardcodedDefault,
}

/// One field's resolved value plus the tier that supplied it (R-SA-133 provenance).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedField<T> {
    pub value: T,
    pub tier: ConfigTier,
}

/// The five-tier field-level precedence resolver at the exact heart of R-SA-133: given up to four
/// candidate `Option<T>` values — one per tier, from highest to lowest precedence — plus a
/// mandatory tier-5 hardcoded fallback, return the first `Some` candidate encountered (highest
/// precedence wins) tagged with the [`ConfigTier`] it came from.
///
/// This is the single, explicit precedence chain every field this crate resolves through the five
/// tiers is expected to route through, rather than each call site re-deriving its own `.or_else`
/// chain (which could silently reorder tiers or drop one). `None` at any tier means "this tier had
/// nothing to say about this field," matching the general shape of tiers 1/2/3/4 (an inline call
/// may omit the field, settings may have no override for this agent, `config.json` may not
/// mention it, and the agent's own frontmatter field is itself an `Option<T>`) — the fifth
/// argument is deliberately a plain `T`, not `Option<T>`, since tier 5 (this crate's own hardcoded
/// default) MUST always have a concrete value to fall back to; a caller with no reasonable
/// hardcoded default should not be calling this resolver for that field at all.
///
/// # Examples
///
/// ```
/// use cyrup_ext_subagents::registration::{resolve_field, ConfigTier};
///
/// // Tier 1 (inline) wins over everything else when present.
/// let resolved = resolve_field(Some("inline-model"), Some("settings-model"), None, None, "default-model");
/// assert_eq!(resolved.value, "inline-model");
/// assert_eq!(resolved.tier, ConfigTier::InlineCallOverride);
///
/// // No inline/settings/config.json/frontmatter value: falls all the way to the hardcoded default.
/// let resolved = resolve_field::<&str>(None, None, None, None, "default-model");
/// assert_eq!(resolved.value, "default-model");
/// assert_eq!(resolved.tier, ConfigTier::HardcodedDefault);
/// ```
pub fn resolve_field<T>(
    inline_call_override: Option<T>,
    settings: Option<T>,
    extension_config: Option<T>,
    agent_frontmatter: Option<T>,
    hardcoded_default: T,
) -> ResolvedField<T> {
    if let Some(value) = inline_call_override {
        return ResolvedField {
            value,
            tier: ConfigTier::InlineCallOverride,
        };
    }
    if let Some(value) = settings {
        return ResolvedField {
            value,
            tier: ConfigTier::Settings,
        };
    }
    if let Some(value) = extension_config {
        return ResolvedField {
            value,
            tier: ConfigTier::ExtensionConfig,
        };
    }
    if let Some(value) = agent_frontmatter {
        return ResolvedField {
            value,
            tier: ConfigTier::AgentFrontmatter,
        };
    }
    ResolvedField {
        value: hardcoded_default,
        tier: ConfigTier::HardcodedDefault,
    }
}

/// The four candidate inputs a single [`resolve_field`] call needs for one field, named per
/// R-SA-133's own tier vocabulary rather than positionally — used by
/// [`resolve_effective_config`]'s per-field call sites so each is self-documenting about which
/// tier's data it is threading through, and by callers in other subsystems (`exec/`,
/// `spawn/`) that need the same five-tier walk for a field this struct does not itself cover
/// (e.g. per-call `model_override`), without re-deriving [`resolve_field`]'s own precedence order.
#[derive(Clone, Debug, Default)]
pub struct FieldCandidates<T> {
    pub inline_call_override: Option<T>,
    pub settings: Option<T>,
    pub extension_config: Option<T>,
    pub agent_frontmatter: Option<T>,
}

impl<T> FieldCandidates<T> {
    pub fn resolve(self, hardcoded_default: T) -> ResolvedField<T> {
        resolve_field(
            self.inline_call_override,
            self.settings,
            self.extension_config,
            self.agent_frontmatter,
            hardcoded_default,
        )
    }
}

/// Per-call inline overrides relevant to the whole-config resolution
/// [`resolve_effective_config`] performs (R-SA-133 tier 1). Every field is `Option` because an
/// inline call/slash-command invocation may specify none, some, or all of them; an absent field
/// falls through to tier 2 (settings) and below, exactly as [`resolve_field`] implements.
#[derive(Clone, Debug, Default)]
pub struct InlineConfigOverrides {
    pub model: Option<String>,
    pub max_subagent_depth: Option<u32>,
    pub global_concurrency_limit: Option<u32>,
    pub parallel_concurrency: Option<u32>,
    pub parallel_max_tasks: Option<u32>,
}

/// The subset of one [`crate::discovery::types::AgentDefinition`]'s own frontmatter fields that
/// participate in [`resolve_effective_config`]'s tier-4 candidates (func-SA §4.6: "agent
/// frontmatter defaults"). Kept as a small, explicit projection (rather than threading a whole
/// `&AgentDefinition` through) so this function's signature stays legible about exactly which
/// frontmatter fields feed the five-tier walk, and so a caller resolving config with no agent in
/// scope at all (e.g. a bare `/subagents-doctor` check) can pass [`AgentFrontmatterDefaults::default`]
/// without needing to fabricate a whole [`crate::discovery::types::AgentDefinition`].
#[derive(Clone, Debug, Default)]
pub struct AgentFrontmatterDefaults {
    pub model: Option<String>,
    pub max_subagent_depth: Option<u32>,
}

/// The fully-resolved effective configuration for one call/run, produced by
/// [`resolve_effective_config`]'s whole-struct walk over R-SA-133's five tiers. Each field carries
/// its resolved value plus [`ConfigTier`] provenance (mirroring
/// [`crate::discovery::types::AgentModelSourceInfo`]'s per-field provenance pattern, generalized
/// here across the whole config surface) so diagnostic/doctor tooling can report *why* a value was
/// chosen without re-running the walk.
#[derive(Clone, Debug)]
pub struct EffectiveConfig {
    pub model: ResolvedField<Option<String>>,
    pub max_subagent_depth: ResolvedField<u32>,
    pub global_concurrency_limit: ResolvedField<u32>,
    pub parallel_concurrency: ResolvedField<u32>,
    pub parallel_max_tasks: ResolvedField<u32>,
}

/// Resolve one call/run's effective configuration across all five R-SA-133 tiers in one pass,
/// given: the caller's inline overrides (tier 1), the layered `subagents.*` settings view (tier
/// 2, already narrowed to the specific agent being resolved for via
/// [`SubagentsSettingsView::override_for`] where applicable), this extension's own `config.json`
/// (tier 3), the target agent's frontmatter defaults (tier 4, `None` when resolving with no
/// specific agent in scope), and this module's hardcoded [`SubagentExtensionConfig::default`]
/// (tier 5, implicit — supplied by this function itself, never passed in, so a caller cannot
/// accidentally supply a *different* tier-5 default than every other call site uses).
///
/// `agent_name` selects which per-agent settings-override delta (if any) participates in tier 2
/// for `model`; `None` (no specific agent in scope, e.g. a doctor check or a global default
/// lookup) skips straight to `settings.default_model` for that field, matching R-SA-133's own
/// text ("`subagents.agentOverrides.<name>`/`subagents.defaultModel`" — the per-name override is
/// consulted first when a name is in scope, the flat default otherwise).
#[must_use]
pub fn resolve_effective_config(
    inline: &InlineConfigOverrides,
    settings: &SubagentsSettingsView,
    agent_name: Option<&str>,
    extension_config: &SubagentExtensionConfig,
    agent_frontmatter: &AgentFrontmatterDefaults,
) -> EffectiveConfig {
    let settings_model = agent_name
        .and_then(|name| settings.override_for(name))
        .and_then(|ov| match &ov.model {
            crate::discovery::types::OverrideField::Value(v) => Some(v.clone()),
            _ => None,
        })
        .or_else(|| settings.default_model.clone());

    // Tier 7: pi has NO per-agent `maxSubagentDepth` settings override (`BuiltinAgentOverrideConfig`,
    // agents.ts:65-79, carries none) — an earlier port invented one and consulted it here. The
    // settings tier therefore never supplies a max depth; it resolves from `config.json` (tier 3),
    // agent frontmatter (tier 4), or the hardcoded default (tier 5) instead.
    let settings_max_depth: Option<u32> = None;

    // `model`'s hardcoded (tier 5) default is "no model resolved" — `Option<String>::None` — so
    // this field's `T` for `resolve_field` is itself `Option<String>`, with every candidate
    // wrapped one level deeper (`Some(Some(v))` for "this tier supplies value `v`",
    // `None` for "this tier is silent"). This keeps the resolved-value type honest
    // (`ResolvedField<Option<String>>`, since even the hardcoded tier may legitimately produce
    // "no model") while every OTHER field below has a genuinely concrete, always-present
    // hardcoded default and stays a plain `T`.
    let model = FieldCandidates {
        inline_call_override: inline.model.clone().map(Some),
        settings: settings_model.map(Some),
        // `config.json` (`SubagentExtensionConfig`) does not itself carry a global default model
        // field (func-SA §4.7's `SubagentExtensionConfig` has no such field) — tier 3 is `None`
        // for `model` by construction, not an oversight.
        extension_config: None,
        agent_frontmatter: agent_frontmatter.model.clone().map(Some),
    }
    .resolve(None);

    let max_subagent_depth = FieldCandidates {
        inline_call_override: inline.max_subagent_depth,
        settings: settings_max_depth,
        extension_config: Some(extension_config.max_subagent_depth),
        agent_frontmatter: agent_frontmatter.max_subagent_depth,
    }
    .resolve(SubagentExtensionConfig::default().max_subagent_depth);

    let global_concurrency_limit = FieldCandidates {
        inline_call_override: inline.global_concurrency_limit,
        settings: None,
        extension_config: Some(extension_config.global_concurrency_limit),
        agent_frontmatter: None,
    }
    .resolve(SubagentExtensionConfig::default().global_concurrency_limit);

    let parallel_concurrency = FieldCandidates {
        inline_call_override: inline.parallel_concurrency,
        settings: None,
        extension_config: Some(extension_config.parallel_concurrency()),
        agent_frontmatter: None,
    }
    .resolve(SubagentExtensionConfig::default().parallel_concurrency());

    let parallel_max_tasks = FieldCandidates {
        inline_call_override: inline.parallel_max_tasks,
        settings: None,
        extension_config: Some(extension_config.parallel_max_tasks()),
        agent_frontmatter: None,
    }
    .resolve(SubagentExtensionConfig::default().parallel_max_tasks());

    EffectiveConfig {
        model,
        max_subagent_depth,
        global_concurrency_limit,
        parallel_concurrency,
        parallel_max_tasks,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::discovery::types::{AgentOverrideConfig, OverrideField, SubagentSettings};

    // -----------------------------------------------------------------------------------------
    // SubagentExtensionConfig defaults (tier 5)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn subagent_extension_config_default_matches_func_sa_4_7_constants() {
        let cfg = SubagentExtensionConfig::default();
        assert!(!cfg.async_by_default);
        assert!(!cfg.force_top_level_async);
        assert_eq!(cfg.global_concurrency_limit, 20);
        assert_eq!(cfg.max_subagent_spawns_per_session, 40);
        assert_eq!(cfg.parallel_max_tasks(), 8);
        assert_eq!(cfg.parallel_concurrency(), 4);
        assert_eq!(cfg.max_subagent_depth, 2);
        assert!(cfg.parallel.is_none());
        assert!(cfg.control.is_none());
        assert!(cfg.chain.is_none());
        assert!(cfg.proactive_skill_subagents.is_none());
        assert!(cfg.default_session_dir.is_none());
        assert!(cfg.single_run_output_base_dir.is_none());
        assert!(cfg.worktree_base_dir.is_none());
        assert!(cfg.worktree_setup_hook.is_none());
        assert!(cfg.worktree_setup_hook_timeout_ms.is_none());
    }

    #[test]
    fn subagent_extension_config_round_trips_through_json() {
        let cfg = SubagentExtensionConfig {
            worktree_setup_hook: Some(PathBuf::from("./scripts/setup-worktree.mjs")),
            worktree_setup_hook_timeout_ms: Some(15_000),
            parallel: Some(TopLevelParallelConfig {
                max_tasks: Some(12),
                concurrency: Some(3),
            }),
            control: Some(ControlConfig {
                needs_attention_after_ms: Some(5_000),
                notify_on: Some(vec![ControlEventType::NeedsAttention]),
                notify_channels: Some(vec![ControlNotificationChannel::Async]),
                ..ControlConfig::default()
            }),
            chain: Some(ExtensionChainConfig {
                dynamic_fanout: Some(DynamicFanoutConfig {
                    max_items: Some(100),
                }),
            }),
            proactive_skill_subagents: Some(ProactiveSkillSubagents::Config(
                ProactiveSkillSubagentsConfig {
                    enabled: Some(true),
                    min_references: Some(2),
                    ..ProactiveSkillSubagentsConfig::default()
                },
            )),
            ..SubagentExtensionConfig::default()
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        let round_tripped: SubagentExtensionConfig =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cfg, round_tripped);
    }

    #[test]
    fn subagent_extension_config_deserializes_partial_json_with_defaults_for_rest() {
        // `#[serde(default)]` at the struct level: a partial JSON document (as a hand-edited
        // config.json realistically would be) fills every unspecified field from `Default`,
        // rather than failing to parse.
        let cfg: SubagentExtensionConfig =
            serde_json::from_str(r#"{"maxSubagentDepth": 5}"#).expect("deserialize partial");
        assert_eq!(cfg.max_subagent_depth, 5);
        assert_eq!(cfg.global_concurrency_limit, 20);
        assert_eq!(cfg.parallel_max_tasks(), 8);
    }

    #[test]
    fn subagent_extension_config_parses_pi_control_and_nested_parallel_shapes() {
        // The exact pi ExtensionConfig shape (types.ts:864-882): nested `parallel {}`, `control {}`
        // with all eight keys, `chain.dynamicFanout.maxItems`, `proactiveSkillSubagents` as an
        // object, and `worktreeSetupHook` as a bare script-path string.
        let cfg: SubagentExtensionConfig = serde_json::from_str(
            r#"{
                "parallel": { "maxTasks": 16, "concurrency": 6 },
                "control": {
                    "enabled": true,
                    "needsAttentionAfterMs": 45000,
                    "activeNoticeAfterMs": 120000,
                    "activeNoticeAfterTurns": 8,
                    "activeNoticeAfterTokens": 50000,
                    "failedToolAttemptsBeforeAttention": 3,
                    "notifyOn": ["active_long_running", "needs_attention"],
                    "notifyChannels": ["event", "async", "intercom"]
                },
                "chain": { "dynamicFanout": { "maxItems": 250 } },
                "proactiveSkillSubagents": { "enabled": true, "minReferences": 2, "preferredAgent": "scout" },
                "worktreeSetupHook": "./scripts/setup-worktree.mjs"
            }"#,
        )
        .expect("deserialize pi-shaped config");

        // Nested parallel.
        assert_eq!(cfg.parallel_max_tasks(), 16);
        assert_eq!(cfg.parallel_concurrency(), 6);

        // Control block, all keys.
        let control = cfg.control.as_ref().expect("control present");
        assert_eq!(control.enabled, Some(true));
        assert_eq!(control.needs_attention_after_ms, Some(45_000));
        assert_eq!(control.active_notice_after_ms, Some(120_000));
        assert_eq!(control.active_notice_after_turns, Some(8));
        assert_eq!(control.active_notice_after_tokens, Some(50_000));
        assert_eq!(control.failed_tool_attempts_before_attention, Some(3));
        assert_eq!(
            control.notify_on.as_deref(),
            Some(
                [
                    ControlEventType::ActiveLongRunning,
                    ControlEventType::NeedsAttention
                ]
                .as_slice()
            )
        );
        assert_eq!(
            control.notify_channels.as_deref(),
            Some(
                [
                    ControlNotificationChannel::Event,
                    ControlNotificationChannel::Async,
                    ControlNotificationChannel::Intercom
                ]
                .as_slice()
            )
        );

        // chain.dynamicFanout.maxItems.
        assert_eq!(cfg.dynamic_fanout_max_items(), Some(250));

        // proactiveSkillSubagents object form.
        let proactive = cfg.proactive_skill_subagents.as_ref().expect("present");
        assert!(matches!(proactive, ProactiveSkillSubagents::Config(_)));
        if let ProactiveSkillSubagents::Config(c) = proactive {
            assert_eq!(c.enabled, Some(true));
            assert_eq!(c.min_references, Some(2));
            assert_eq!(c.preferred_agent.as_deref(), Some("scout"));
        }
        assert!(proactive.is_enabled());

        // worktreeSetupHook script-path string form.
        assert_eq!(
            cfg.worktree_setup_hook.as_deref(),
            Some(std::path::Path::new("./scripts/setup-worktree.mjs"))
        );
    }

    #[test]
    fn proactive_skill_subagents_parses_false_toggle_and_reports_disabled() {
        let cfg: SubagentExtensionConfig =
            serde_json::from_str(r#"{ "proactiveSkillSubagents": false }"#)
                .expect("deserialize false toggle");
        let proactive = cfg.proactive_skill_subagents.as_ref().expect("present");
        assert!(matches!(proactive, ProactiveSkillSubagents::Toggle(false)));
        assert!(!proactive.is_enabled());
    }

    #[test]
    fn nested_parallel_field_omission_falls_back_to_pi_defaults() {
        // An empty `parallel: {}` object still defers each omitted field to the pi default.
        let cfg: SubagentExtensionConfig =
            serde_json::from_str(r#"{ "parallel": { "concurrency": 9 } }"#).expect("deserialize");
        assert_eq!(cfg.parallel_concurrency(), 9);
        assert_eq!(cfg.parallel_max_tasks(), DEFAULT_PARALLEL_MAX_TASKS);
    }

    // -----------------------------------------------------------------------------------------
    // HookSpec shape parity with spawn::worktree::HookSpec
    // -----------------------------------------------------------------------------------------

    #[test]
    fn hook_spec_round_trips_through_json() {
        let hook = HookSpec {
            command: PathBuf::from("/bin/true"),
            args: vec!["a".to_string(), "b".to_string()],
        };
        let json = serde_json::to_string(&hook).expect("serialize");
        let round_tripped: HookSpec = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(hook, round_tripped);
    }

    // -----------------------------------------------------------------------------------------
    // SubagentsSettingsView conversion
    // -----------------------------------------------------------------------------------------

    #[test]
    fn settings_view_from_subagent_settings_flattens_optional_bools_to_false_when_unset() {
        let settings = SubagentSettings::default();
        let view = SubagentsSettingsView::from_subagent_settings(&settings);
        assert!(!view.disable_builtins);
        assert!(!view.disable_thinking);
        assert!(view.default_model.is_none());
        assert!(view.agent_overrides.is_empty());
    }

    #[test]
    fn settings_view_from_subagent_settings_preserves_explicit_true_flags() {
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert(
            "reviewer".to_string(),
            AgentOverrideConfig {
                model: OverrideField::Value("claude-opus".to_string()),
                ..Default::default()
            },
        );
        let settings = SubagentSettings {
            model_scope: None,
            overrides,
            default_model: Some("claude-sonnet".to_string()),
            disable_builtins: Some(true),
            disable_thinking: Some(true),
        };
        let view = SubagentsSettingsView::from_subagent_settings(&settings);
        assert!(view.disable_builtins);
        assert!(view.disable_thinking);
        assert_eq!(view.default_model.as_deref(), Some("claude-sonnet"));
        assert_eq!(
            view.override_for("reviewer").map(|o| &o.model),
            Some(&OverrideField::Value("claude-opus".to_string()))
        );
        assert!(view.override_for("nonexistent").is_none());
    }

    // -----------------------------------------------------------------------------------------
    // resolve_field: the primitive precedence chain
    // -----------------------------------------------------------------------------------------

    #[test]
    fn resolve_field_tier1_inline_wins_over_all_lower_tiers() {
        let resolved = resolve_field(
            Some("inline"),
            Some("settings"),
            Some("config"),
            Some("frontmatter"),
            "hardcoded",
        );
        assert_eq!(resolved.value, "inline");
        assert_eq!(resolved.tier, ConfigTier::InlineCallOverride);
    }

    #[test]
    fn resolve_field_tier2_settings_wins_when_inline_absent() {
        let resolved = resolve_field(
            None,
            Some("settings"),
            Some("config"),
            Some("frontmatter"),
            "hardcoded",
        );
        assert_eq!(resolved.value, "settings");
        assert_eq!(resolved.tier, ConfigTier::Settings);
    }

    #[test]
    fn resolve_field_tier3_extension_config_wins_when_inline_and_settings_absent() {
        let resolved = resolve_field(None, None, Some("config"), Some("frontmatter"), "hardcoded");
        assert_eq!(resolved.value, "config");
        assert_eq!(resolved.tier, ConfigTier::ExtensionConfig);
    }

    #[test]
    fn resolve_field_tier4_agent_frontmatter_wins_when_only_it_and_hardcoded_remain() {
        let resolved = resolve_field(None, None, None, Some("frontmatter"), "hardcoded");
        assert_eq!(resolved.value, "frontmatter");
        assert_eq!(resolved.tier, ConfigTier::AgentFrontmatter);
    }

    #[test]
    fn resolve_field_tier5_hardcoded_default_wins_when_all_higher_tiers_absent() {
        let resolved = resolve_field::<&str>(None, None, None, None, "hardcoded");
        assert_eq!(resolved.value, "hardcoded");
        assert_eq!(resolved.tier, ConfigTier::HardcodedDefault);
    }

    // -----------------------------------------------------------------------------------------
    // Fixtures at each of the five tiers, overriding (and NOT overriding) correctly.
    //
    // Each fixture below builds a full `resolve_effective_config` call with exactly one tier
    // populated for the field under test (all higher tiers empty, matching what a real caller at
    // that tier alone would supply) and asserts both the resolved value AND its `ConfigTier`
    // provenance tag — proving the tier that "should" win did, not merely that some plausible
    // value came out.
    // -----------------------------------------------------------------------------------------

    fn extension_config_fixture() -> SubagentExtensionConfig {
        SubagentExtensionConfig {
            max_subagent_depth: 7,
            global_concurrency_limit: 99,
            parallel: Some(TopLevelParallelConfig {
                concurrency: Some(11),
                max_tasks: Some(33),
            }),
            ..SubagentExtensionConfig::default()
        }
    }

    fn frontmatter_fixture() -> AgentFrontmatterDefaults {
        AgentFrontmatterDefaults {
            model: Some("frontmatter-model".to_string()),
            max_subagent_depth: Some(3),
        }
    }

    fn settings_fixture_with_override(agent_name: &str) -> SubagentsSettingsView {
        let mut agent_overrides = HashMap::new();
        agent_overrides.insert(
            agent_name.to_string(),
            AgentOverrideConfig {
                model: OverrideField::Value("settings-override-model".to_string()),
                ..Default::default()
            },
        );
        SubagentsSettingsView {
            default_model: Some("settings-default-model".to_string()),
            agent_overrides,
            disable_builtins: false,
            disable_thinking: false,
        }
    }

    /// Tier 1 (inline call override) overrides every lower tier for `model`.
    #[test]
    fn fixture_tier1_inline_overrides_all_lower_tiers_for_model() {
        let inline = InlineConfigOverrides {
            model: Some("inline-model".to_string()),
            ..Default::default()
        };
        let settings = settings_fixture_with_override("reviewer");
        let ext_cfg = extension_config_fixture();
        let frontmatter = frontmatter_fixture();

        let resolved = resolve_effective_config(
            &inline,
            &settings,
            Some("reviewer"),
            &ext_cfg,
            &frontmatter,
        );

        assert_eq!(resolved.model.value.as_deref(), Some("inline-model"));
        assert_eq!(resolved.model.tier, ConfigTier::InlineCallOverride);
    }

    /// Tier 2 (per-agent settings override) wins over config.json/frontmatter/hardcoded when tier
    /// 1 is absent, for `model`. `max_subagent_depth` has NO settings-override tier (Tier 7: pi has
    /// no per-agent `maxSubagentDepth` override), so it falls through tier 2 to config.json (tier 3).
    #[test]
    fn fixture_tier2_settings_agent_override_wins_when_inline_absent() {
        let inline = InlineConfigOverrides::default();
        let settings = settings_fixture_with_override("reviewer");
        let ext_cfg = extension_config_fixture();
        let frontmatter = frontmatter_fixture();

        let resolved = resolve_effective_config(
            &inline,
            &settings,
            Some("reviewer"),
            &ext_cfg,
            &frontmatter,
        );

        assert_eq!(
            resolved.model.value.as_deref(),
            Some("settings-override-model")
        );
        assert_eq!(resolved.model.tier, ConfigTier::Settings);

        // The settings tier no longer supplies a per-agent max depth; it resolves from config.json
        // (`extension_config_fixture` sets `max_subagent_depth: 7`), NOT the settings tier.
        assert_eq!(resolved.max_subagent_depth.value, 7);
        assert_eq!(resolved.max_subagent_depth.tier, ConfigTier::ExtensionConfig);
    }

    /// Tier 2 falls back to the FLAT `subagents.defaultModel` (not the per-agent override) when
    /// resolving for an agent with no override entry of its own.
    #[test]
    fn fixture_tier2_settings_flat_default_used_when_agent_has_no_override_entry() {
        let inline = InlineConfigOverrides::default();
        let settings = settings_fixture_with_override("reviewer"); // override keyed to "reviewer" only
        let ext_cfg = extension_config_fixture();
        let frontmatter = frontmatter_fixture();

        let resolved = resolve_effective_config(
            &inline,
            &settings,
            Some("someone-else"), // no override entry for this name
            &ext_cfg,
            &frontmatter,
        );

        assert_eq!(
            resolved.model.value.as_deref(),
            Some("settings-default-model")
        );
        assert_eq!(resolved.model.tier, ConfigTier::Settings);
    }

    /// Tier 2 is skipped entirely (falls through to tier 3/4/5) when NO agent name is in scope
    /// and settings carries no flat default either.
    #[test]
    fn fixture_tier2_settings_skipped_when_no_agent_name_and_no_flat_default() {
        let inline = InlineConfigOverrides::default();
        let settings = SubagentsSettingsView::default(); // no default_model, no overrides at all
        let ext_cfg = extension_config_fixture();
        let frontmatter = frontmatter_fixture();

        let resolved = resolve_effective_config(&inline, &settings, None, &ext_cfg, &frontmatter);

        // Falls through past tier1/tier2 (both empty), tier3 (config.json has no model field at
        // all by construction), straight to tier4 (agent frontmatter).
        assert_eq!(resolved.model.value.as_deref(), Some("frontmatter-model"));
        assert_eq!(resolved.model.tier, ConfigTier::AgentFrontmatter);
    }

    /// Tier 3 (`config.json`) wins over tier 4 (frontmatter) and tier 5 (hardcoded) for
    /// `max_subagent_depth` when tiers 1/2 are absent.
    #[test]
    fn fixture_tier3_extension_config_wins_over_frontmatter_and_hardcoded() {
        let inline = InlineConfigOverrides::default();
        let settings = SubagentsSettingsView::default();
        let ext_cfg = extension_config_fixture(); // max_subagent_depth: 7
        let frontmatter = frontmatter_fixture(); // max_subagent_depth: Some(3)

        let resolved = resolve_effective_config(&inline, &settings, None, &ext_cfg, &frontmatter);

        assert_eq!(resolved.max_subagent_depth.value, 7);
        assert_eq!(
            resolved.max_subagent_depth.tier,
            ConfigTier::ExtensionConfig
        );

        // Fields with no settings/frontmatter concept at all (concurrency knobs) resolve straight
        // from config.json too, at the same tier.
        assert_eq!(resolved.global_concurrency_limit.value, 99);
        assert_eq!(
            resolved.global_concurrency_limit.tier,
            ConfigTier::ExtensionConfig
        );
        assert_eq!(resolved.parallel_concurrency.value, 11);
        assert_eq!(resolved.parallel_max_tasks.value, 33);
    }

    /// Tier 4 (agent frontmatter) wins over tier 5 (hardcoded default) for `max_subagent_depth`
    /// when tiers 1/2/3 are all absent (a caller that deliberately omits its own config.json
    /// value, e.g. by constructing `SubagentExtensionConfig::default()` -- which for this field
    /// specifically also has a value, so this fixture uses `model`, which config.json never
    /// supplies, to isolate tier 4 cleanly).
    #[test]
    fn fixture_tier4_agent_frontmatter_wins_over_hardcoded_for_model() {
        let inline = InlineConfigOverrides::default();
        let settings = SubagentsSettingsView::default();
        let ext_cfg = SubagentExtensionConfig::default(); // no model field to contribute at tier 3
        let frontmatter = frontmatter_fixture(); // model: Some("frontmatter-model")

        let resolved = resolve_effective_config(&inline, &settings, None, &ext_cfg, &frontmatter);

        assert_eq!(resolved.model.value.as_deref(), Some("frontmatter-model"));
        assert_eq!(resolved.model.tier, ConfigTier::AgentFrontmatter);
    }

    /// Tier 5 (hardcoded default) is reached ONLY when every higher tier is genuinely silent —
    /// the ultimate fallback, never accidentally shadowed by an empty-but-`Some` value at a
    /// higher tier.
    #[test]
    fn fixture_tier5_hardcoded_default_reached_when_every_higher_tier_absent() {
        let inline = InlineConfigOverrides::default();
        let settings = SubagentsSettingsView::default();
        let ext_cfg = SubagentExtensionConfig::default();
        let frontmatter = AgentFrontmatterDefaults::default(); // nothing set

        let resolved = resolve_effective_config(&inline, &settings, None, &ext_cfg, &frontmatter);

        assert!(resolved.model.value.is_none());
        assert_eq!(resolved.model.tier, ConfigTier::HardcodedDefault);

        assert_eq!(
            resolved.max_subagent_depth.value,
            SubagentExtensionConfig::default().max_subagent_depth
        );
        assert_eq!(
            resolved.max_subagent_depth.tier,
            ConfigTier::ExtensionConfig,
            "config.json's own Default still supplies a concrete tier-3 value ahead of tier 5"
        );
    }

    /// End-to-end: a single resolution call where EACH field independently lands on a different
    /// tier, proving the five-tier walk composes correctly across an entire config rather than
    /// only in isolated single-field tests.
    #[test]
    fn fixture_mixed_fields_each_resolve_from_their_own_correct_tier() {
        let inline = InlineConfigOverrides {
            // tier 1 only for this one field
            global_concurrency_limit: Some(500),
            ..Default::default()
        };
        // tier 2 only supplies a flat default model (no per-agent override, no agent name passed)
        let settings = SubagentsSettingsView {
            default_model: Some("settings-model".to_string()),
            ..Default::default()
        };
        // tier 3 supplies max_subagent_depth and parallel_concurrency
        let ext_cfg = SubagentExtensionConfig {
            max_subagent_depth: 9,
            parallel: Some(TopLevelParallelConfig {
                concurrency: Some(6),
                max_tasks: None,
            }),
            ..SubagentExtensionConfig::default()
        };
        // tier 4 has nothing relevant left to contribute uniquely (model already won by tier 2)
        let frontmatter = AgentFrontmatterDefaults::default();

        let resolved = resolve_effective_config(&inline, &settings, None, &ext_cfg, &frontmatter);

        assert_eq!(resolved.global_concurrency_limit.value, 500);
        assert_eq!(
            resolved.global_concurrency_limit.tier,
            ConfigTier::InlineCallOverride
        );

        assert_eq!(resolved.model.value.as_deref(), Some("settings-model"));
        assert_eq!(resolved.model.tier, ConfigTier::Settings);

        assert_eq!(resolved.max_subagent_depth.value, 9);
        assert_eq!(
            resolved.max_subagent_depth.tier,
            ConfigTier::ExtensionConfig
        );

        assert_eq!(resolved.parallel_concurrency.value, 6);
        assert_eq!(
            resolved.parallel_concurrency.tier,
            ConfigTier::ExtensionConfig
        );

        // parallel_max_tasks: nothing supplied at tiers 1-3 beyond config.json's own struct
        // Default (8), tier 4 has no concept of this field at all -> lands at tier 3 anyway,
        // since `SubagentExtensionConfig::default()` always supplies a concrete value.
        assert_eq!(resolved.parallel_max_tasks.value, 8);
        assert_eq!(
            resolved.parallel_max_tasks.tier,
            ConfigTier::ExtensionConfig
        );
    }
}
