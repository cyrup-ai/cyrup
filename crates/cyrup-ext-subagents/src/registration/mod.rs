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
//! 2. **`subagents.agentOverrides.<name>`/`subagents.defaultModel`** from the layered
//!    agent-`settings.json` pair — project (`<project_root>/.cyrup/agents/settings.json`) beats
//!    user (`~/.cyrup/agents/settings.json`), pi `agents/agents.ts:924-931` @v0.43.0 — as
//!    resolved by [`crate::discovery::load_layered_subagent_settings`] and viewed through
//!    [`SubagentsSettingsView`]. SUBA-071: this is the crate's ONLY settings store; it is not
//!    `cyrup-config`'s `~/.cyrup/agent/settings.json`, which this crate never reads.
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

pub mod authority;
pub mod doctor;
pub mod guide;
pub mod profiles;
pub mod slash_commands;
pub mod tool_description;
pub mod cost;
pub mod resources;
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
    ///
    /// **An OMITTED `asyncByDefault` key backgrounds** — pi `resolveAsyncByDefault`
    /// (`config.ts:222-224`) is `config.asyncByDefault !== false`, so only the literal `false`
    /// opts out. See this field's seed in [`SubagentExtensionConfig::default`]; a plain `bool`
    /// reproduces pi's tri-state exactly because absent and `true` both mean background.
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
    /// `ExtensionConfig.parallel?: { maxTasks?, concurrency? }` (shared/types.ts:1715-1718/1771) — NOT two
    /// flat `parallelMaxTasks`/`parallelConcurrency` keys. Read via the [`Self::parallel_max_tasks`]
    /// / [`Self::parallel_concurrency`] accessors, which fall back to pi's defaults (8 / 4) when the
    /// object, or a field within it, is omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel: Option<TopLevelParallelConfig>,
    /// Live-control notice thresholds/channels — pi `ExtensionConfig.control?: ControlConfig`
    /// (shared/types.ts:160-169/1764). Feeds the control-notice state machine (`tui/notices.rs`); a resolved
    /// view is produced by pi's `resolveControlConfig`. `None` = every threshold defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control: Option<ControlConfig>,
    /// Chain-specific extension config — pi `ExtensionConfig.chain?: { dynamicFanout?: { maxItems? } }`
    /// (shared/types.ts:1720-1724/1772): the per-run cap on how many items a dynamic fan-out may expand to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain: Option<ExtensionChainConfig>,
    /// Proactive skill-subagent suggestion config — pi
    /// `ExtensionConfig.proactiveSkillSubagents?: ProactiveSkillSubagentsConfig | false`
    /// (shared/types.ts:1726-1731 interface, :1779 field): an object of tuning knobs, or the literal `false` to disable the
    /// feature entirely.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proactive_skill_subagents: Option<ProactiveSkillSubagents>,
    /// Default directory new subagent session files are written under, when neither an inline
    /// call override nor an agent-frontmatter default supplies one. `None` defers to this crate's
    /// own computed default (owned by `exec`/`background`, not this type).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_session_dir: Option<PathBuf>,
    /// In-process override for the binary this extension's runs re-exec, mirroring
    /// [`crate::exec::agent_config::RunOptions::spawn_command`]. `None` — the only value a
    /// deserialized config can ever hold — means "resolve from the environment", so every
    /// installed configuration behaves exactly as it did before this field existed.
    ///
    /// `#[serde(skip)]` is load-bearing twice over. [`crate::spawn::SpawnCommand`] derives no
    /// serde impls, and more importantly a value read out of `config.json` that could repoint
    /// WHICH EXECUTABLE a subagent spawns would be a genuine hazard: this override exists for a
    /// caller that already holds the process (an integration test wiring a scripted fixture, say),
    /// never for a file the process merely reads.
    ///
    /// # What this reaches, and what it does not
    ///
    /// Two paths honour it, both in THIS process:
    ///
    /// - the foreground single run, carried into its `RunOptions` by
    ///   [`crate::extension::SubagentExecutor`]'s prologue alongside the
    ///   `turn_budget`/`permission_rules` it already resolves from the same config snapshot; and
    /// - foreground chain and parallel steps, whose walk takes its OWN snapshot of this config
    ///   and hands the value to `ExecSingleStepExecutor::foreground`. The two paths resolve it
    ///   independently; neither feeds the other.
    ///
    /// **No detached or background spawn honours it** — `spawn_background_steps` and
    /// `spawn_detached_runner` alike. Those children are separate processes that resolve their own
    /// command through [`crate::spawn::resolve_spawn_command`] against the environment they
    /// inherited, and the `RunnerConfig` driving them crosses that boundary as JSON, which carries
    /// no [`crate::spawn::SpawnCommand`]. Setting this field for such a run is inert: to redirect a
    /// detached child, set [`crate::spawn::SUBAGENT_BINARY_ENV_VAR`] (and, for leading argv,
    /// [`crate::spawn::SUBAGENT_BINARY_ARGS_ENV_VAR`]) in the environment it will inherit.
    ///
    /// # Zero production callers, and why it keeps its place
    ///
    /// Production redirects a child through [`crate::spawn::SUBAGENT_BINARY_ENV_VAR`]. This exists
    /// so an in-process caller need not move that variable on a process every concurrent test
    /// shares — which is the mutation this crate cannot make at all
    /// (`#![forbid(unsafe_code)]` plus edition 2024's `unsafe set_var`). It costs production no
    /// parameter, defaults to `None`, and is `#[serde(skip)]`.
    #[serde(skip)]
    pub spawn_command: Option<crate::spawn::SpawnCommand>,

    /// Where this extension's runs resolve paths: home, agent dir, and the two independent
    /// scratch roots ([`crate::paths::Roots`]).
    ///
    /// # Not an `Option`, and that is the point
    ///
    /// This replaced `home_root: Option<PathBuf>`, which was not a resolved root but a DEFERRED
    /// one: `None` meant "go read the environment", in the callee, four separate times. That is how
    /// this crate came to hold ladders that disagreed while `paths`' own module doc claimed there
    /// was one, and it is why the deferred form could not be tested — no test ever exercised the
    /// `None` arm, because every test supplied a root. A resolved value cannot drift from itself.
    ///
    /// # What it reaches, and what it does not
    ///
    /// Reaches every path this crate derives: the run-artifact roots
    /// (`background::run_artifact_roots_in` and the `default_async_root_in` /
    /// `default_results_dir_in` / `resolve_background_storage_roots` helpers every executor call
    /// site goes through), the user-scope discovery roots, the wait tool's own resolution, and the
    /// nested-events tree.
    ///
    /// **A detached run IS governed by it — the roots cross, the resolver does not.**
    /// `spawn_background_steps` resolves both roots in THIS process and hands them to the runner as
    /// absolute paths in `RunnerConfig::async_root`/`results_dir`, so the child writes where this
    /// value says without re-deriving them. Anything the child derives for ITSELF still comes from
    /// its own environment, which is why `detached_runner_env_overlay_with` puts `CYRUP_HOME` on
    /// the child's `Command` — the tier-2 mechanism, set on the child, never on this process.
    ///
    /// It does NOT reach the `~`-expansion sites that anchor a USER-supplied path against the real
    /// home on purpose, nor any other crate's resolver.
    ///
    /// [`Default`] is [`crate::paths::Roots::from_env`], so a deserialized `config.json` behaves
    /// exactly as it did before this field existed. `crates/cyrup/src/subagent_config.rs` sets it
    /// explicitly at startup from the layout the binary already resolved, which is the production
    /// path.
    ///
    /// `#[serde(skip)]` for the reason [`Self::spawn_command`] carries it: a `config.json` able to
    /// relocate where a run writes its artifacts is a hazard, and this value exists for a caller
    /// that already holds the process.
    #[serde(skip)]
    pub roots: crate::paths::Roots,

    /// Pinned answers for the environment lookups this extension routes through an injectable
    /// resolver — the `&dyn Fn(&str) -> Option<String>` convention 26 functions in this crate
    /// already take.
    ///
    /// `Some(value)` answers that value; **`None` answers "unset"**, which is the case that makes
    /// this field earn its place: a caller that must prove a gate's behaviour has to be able to
    /// SCRUB an ambient variable, not merely set its own. `CYRUP_INTERCOM` is the live example —
    /// it is a documented product opt-in exported on developer machines and CI runners, and
    /// `intercom_supervisor_channel_available` reads it at `init` time, so a value inherited from
    /// the surrounding shell silently changes which tools get registered.
    ///
    /// This is the in-process twin of the `env_overlay` handed to a DETACHED child
    /// (`detached_runner_env_overlay_with`): both are "a map applied over the inherited
    /// environment", one for a child's `Command` and one for this process's own resolvers. Neither
    /// mutates anything global, which is why no `unsafe` is involved on either side.
    ///
    /// It shadows lookups ONLY where a resolver takes the injected closure. A direct
    /// `std::env::var` elsewhere is unaffected — deliberately: this is an injection point, not a
    /// process-wide environment shim.
    ///
    /// `#[serde(skip)]` for the reason [`Self::spawn_command`] and [`Self::roots`] carry it: a
    /// `config.json` able to rewrite what the process believes its environment says is a hazard.
    ///
    /// # Zero production callers, and why it keeps its place
    ///
    /// Nothing in the shipped binary sets this. It stays because it is the only mechanism that can
    /// answer **unset**, and proving a gate's behaviour against an ambient product opt-in requires
    /// exactly that: `CYRUP_INTERCOM` is exported on developer machines and CI runners, so a value
    /// inherited from the surrounding shell silently changes which tools get registered. No
    /// `Command::env` reaches an in-process resolver, and no set-only helper can scrub.
    ///
    /// The cost this crate's constraint actually names is what production code has to READ. This
    /// field adds no parameter to any production signature, defaults to empty, and is
    /// `#[serde(skip)]`, so a reader who never sets it never encounters it. That is the distinction
    /// from the `home_root` this struct used to carry, which put an `Option<&Path>` into 19
    /// production signatures and left four environment reads alive behind it.
    #[serde(skip)]
    pub env_overrides: std::collections::BTreeMap<String, Option<String>>,

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
    /// `ExtensionConfig.worktreeSetupHook?: string` (shared/types.ts:876): a bare **script-path string**
    /// (e.g. `"./scripts/setup-worktree.mjs"`), NOT a `{ command, args }` object — pi resolves it
    /// into a runnable `{ hookPath, timeoutMs }` at spawn time (`subagent-runner.ts:1975`). The
    /// crate-internal runnable shape (`spawn::worktree`'s `WorktreeSetupHookConfig`/[`HookSpec`]) is
    /// derived from this path plus [`Self::worktree_setup_hook_timeout_ms`] downstream, not stored
    /// here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_setup_hook: Option<PathBuf>,
    /// Timeout, in milliseconds, for the worktree setup hook (R-SA-063: "target 30000ms, if
    /// unset"). `None` here means "use the hard-coded 30000ms default" — the concrete default
    /// constant itself lives in `spawn::worktree::DEFAULT_WORKTREE_SETUP_HOOK_TIMEOUT_MS`, not
    /// duplicated here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_setup_hook_timeout_ms: Option<u64>,
    /// pi `ExtensionConfig.fleetView?: boolean` (`shared/types.ts:1750-1751`, its own comment:
    /// "Show the Claude Code-style navigable fleet. Defaults to true."). Read by upstream as
    /// `config.fleetView !== false` (`extension/index.ts:333`), so ONLY an explicit `false`
    /// disables it — which is exactly what a `bool` defaulting to `true` expresses here.
    /// Consumed by `extension.rs`'s `refresh_fleet_status_widget`, which publishes nothing when it
    /// is off (upstream's `fleetStatus` is `undefined` in that case, `extension/index.ts:378-383`).
    pub fleet_view: bool,
    /// pi `ExtensionConfig.fleetViewPlacement?: FleetViewPlacement` (`shared/types.ts:1752-1753`,
    /// its own comment: "Place the persistent FleetView above or below the editor. Defaults to
    /// belowEditor."). Resolved through
    /// [`crate::tui::fleet_status::resolve_fleet_view_placement`] — upstream's own
    /// `resolveFleetViewPlacement(config.fleetViewPlacement)` (`extension/index.ts:334`) — which
    /// accepts ONLY the exact string `"aboveEditor"` and treats everything else as below.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fleet_view_placement: Option<String>,
    /// The `wait` tool's config gate — pi `ExtensionConfig.waitTool?: WaitToolConfig`
    /// (`extension/index.ts:332` `resolveWaitToolConfig(config.waitTool)`), accepting either a bare
    /// boolean or `{ enabled?: boolean }`. `None` (the field omitted) = enabled, pi's default.
    /// [`crate::background::wait::WAIT_TOOL_ENABLED_ENV`] overrides whatever this says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_tool: Option<crate::background::wait::WaitToolSetting>,
    /// The durable-mission store config — pi `ExtensionConfig.missions?: MissionStoreConfig`
    /// (`pi-subagents/src/missions/types.ts:102-108`, validated on every config read by
    /// `extension/config.ts:25`'s `validateMissionStoreConfig`).
    ///
    /// `None` (the field omitted) is the default and means "missions enabled, default directories,
    /// global pointer index on": every field inside is itself optional and
    /// [`crate::missions::resolve_mission_store_location`] supplies the defaults. Setting
    /// `{"enabled": false}` disables only the AUTOMATIC per-launch mission creation — an explicit
    /// `mission`/`missionId` parameter and the six `mission.*` actions still work
    /// (`missions/lifecycle.ts:65-66`).
    ///
    /// Validated through [`crate::missions::validate_mission_store_config`] by
    /// [`Self::validate_missions`], which the config loader calls: serde alone would silently
    /// accept an unknown key inside the block, and upstream refuses one loudly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missions: Option<crate::missions::MissionStoreConfig>,
    /// SUBA-059 — pi `ExtensionConfig.artifactConfig?: Pick<ArtifactConfig, "cleanupDays">`
    /// (`shared/types.ts:1859` @v0.47.1, *"Artifact cleanup retention. Set cleanupDays to 0 to
    /// disable cleanup."*), read at `extension/index.ts:369-370` as
    /// `config.artifactConfig?.cleanupDays ?? DEFAULT_ARTIFACT_CONFIG.cleanupDays` and handed
    /// straight to `cleanupAllArtifactDirs`. Landed in `b69aafb` ("fix: honor artifact cleanup
    /// retention config", #1013), released v0.47.1.
    ///
    /// Note the upstream TYPE: it is a `Pick` of ONE field, not the whole `ArtifactConfig`. The
    /// per-run `enabled`/`include*` switches are not configurable through `config.json` upstream,
    /// so advertising them here would recreate the accepted-and-ignored defect this item exists to
    /// close. [`ArtifactRetentionConfig`] mirrors the `Pick` exactly.
    ///
    /// `None` (the key omitted) means pi's `DEFAULT_ARTIFACT_CONFIG.cleanupDays` = 7.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_config: Option<ArtifactRetentionConfig>,
    /// SUBA-048 — pi `ExtensionConfig.artifactDir?: ArtifactDirPreference`
    /// (`shared/types.ts:1857` @v0.47.1: *"Where to store subagent artifact files. Defaults to
    /// 'project' (cwd/.pi/subagents). Set to 'session' for pi session dir, or 'temp' for OS
    /// temp."*), seeded onto the live state at `extension/index.ts:375` and consulted by
    /// `getArtifactsDir`/`getChainRunsDir`.
    ///
    /// `None` (the key omitted) is pi's `project` default. An INVALID value must be refused at
    /// config load — see [`Self::validate_artifact_dir`], which is upstream's own `throw`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_dir: Option<crate::artifacts::ArtifactDirPreference>,
    /// SUBA-064 — pi `ExtensionConfig.authorityPolicy?: AuthorityPolicyConfig`, consulted by
    /// `resolveAuthorityDecision` (`policy/authority.ts:23`) and — the live-reachable half — by the
    /// `stop`/`steer` gate at `runs/foreground/subagent-executor.ts:4412-4423` @v0.43.0.
    ///
    /// `None` (the key omitted) means every action takes its
    /// [`authority::AuthorityAction::default_decision`]. An INVALID block must fail config load —
    /// see [`Self::validate_authority_policy`], which is upstream's own `throw`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority_policy: Option<authority::AuthorityPolicyConfig>,
    /// SUBA-008 — pi `ExtensionConfig.turnBudget?: TurnBudgetConfig` (`shared/types.ts:1766`
    /// @v0.43.0), the LAST rung of the assistant-turn-budget chain
    /// (`effectiveParams.turnBudget ?? deps.config.turnBudget`,
    /// `runs/foreground/subagent-executor.ts:4928`), where `effectiveParams.turnBudget` has already
    /// absorbed the agent's own `turnBudget:` frontmatter.
    ///
    /// Carried RAW rather than pre-resolved because upstream validates it at USE time, through the
    /// same `resolveTurnBudgetConfig` call that validates the tool param — so a malformed
    /// `subagents.turnBudget` produces the tool call's own error text with upstream's label, and
    /// does not take the whole extension down at load. `None` (the key omitted) is unbudgeted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_budget: Option<serde_json::Value>,
    /// SUBA-077 — pi `config.timeoutMs`, read through `resolveConfigDefaultTimeoutMs`
    /// (`runs/foreground/subagent-executor.ts:2684` @v0.57.0): the global default wall-clock
    /// deadline, replacing the built-in backstop wherever a concrete default is applied. It is the
    /// only way to raise a long fan-out's ceiling without passing `timeoutMs` on every call.
    ///
    /// Carried RAW, exactly like [`Self::turn_budget`] and for the SAME reason — with one
    /// difference worth stating: upstream's validator returns `undefined` for ANY invalid value and
    /// never errors, so a malformed `subagents.timeoutMs` must degrade to the built-in default
    /// rather than fail a run. A typed `Option<u64>` here would be worse still: it would fail
    /// deserialization of this WHOLE struct on `"timeoutMs": -5` and take every other setting down
    /// with it. Validated at use by `extension::tool::params::resolve_config_default_timeout_ms`
    /// — named rather than linked, because `extension::tool` is a private module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<serde_json::Value>,
    /// SUBA-079 — pi `config.defaultSubagentContext` (`extension/config.ts:140-142` @v0.57.0): the
    /// global fork/fresh preference for every subagent launch that does not name one explicitly.
    ///
    /// **It OUTRANKS each agent's own `defaultContext`**, unlike every other settings rung here —
    /// `"fresh"` exists precisely to overrule agents that declare `defaultContext: fork`. See
    /// [`crate::fork_context::resolve_effective_context`] for the full ladder.
    ///
    /// Carried RAW like [`Self::turn_budget`], and validated at use by
    /// `fork_context::resolve_default_subagent_context`. Unlike [`Self::timeout_ms`], upstream
    /// THROWS for an invalid value rather than ignoring it, so validation here is loud — but doing
    /// it at USE keeps a malformed value from failing this whole struct's deserialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_subagent_context: Option<serde_json::Value>,
    /// SUBA-025 — pi `ExtensionConfig.toolDescriptionMode?: ToolDescriptionMode`, read at
    /// `extension/index.ts:458` (@v0.34.0) / `:540` (@v0.43.0) as
    /// `description: buildSubagentToolDescription(config)`.
    ///
    /// Carried RAW, exactly like [`Self::turn_budget`] and for the SAME upstream reason: pi's
    /// `resolveToolDescriptionMode` (`tool-description.ts:104`) does not throw on a bad value — it
    /// `console.warn`s and degrades to `"full"`. A parsed enum here would either reject the whole
    /// `config.json` at load (which upstream does not) or silently drop the key without the
    /// warning (which is the accepted-and-ignored defect this item exists to close).
    ///
    /// `None` (the key omitted) is pi's `"full"` default. Resolved through
    /// [`tool_description::build_subagent_tool_description`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_description_mode: Option<serde_json::Value>,
    /// SUBA-073 — pi `ExtensionConfig.permissions?: PermissionConfig` (`shared/types.ts:2268`
    /// @v0.57.0, *"Opt-in native tool permissions. Bash remains outside this policy."*). Carried
    /// RAW, exactly like [`Self::turn_budget`] and for the same reason: validated at the point of
    /// use ([`crate::exec::permissions::validate_permission_config`]) rather than at config load,
    /// so a malformed block degrades that one resolution rather than discarding the whole config
    /// file.
    ///
    /// `None` (the key omitted) means no global policy rung — the effective policy is then
    /// whatever the agent's own frontmatter declares, or no policy at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<serde_json::Value>,
}

/// SUBA-059 — pi's `Pick<ArtifactConfig, "cleanupDays">` (`shared/types.ts:1859` @v0.47.1): the
/// only artifact field `config.json` may set. A separate type from
/// [`crate::artifacts::ArtifactConfig`] precisely because that struct carries five more fields that
/// upstream does NOT read from config.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRetentionConfig {
    /// Delete artifacts older than this many days on every extension load. `0` DISABLES the sweep
    /// (`shared/types.ts:1858`), which is why [`crate::artifacts::cleanup_old_artifacts`] carries an
    /// explicit `<= 0` short-circuit (`shared/artifacts.ts:231`) — a literal `0` under the
    /// `now - days * ONE_DAY` arithmetic would otherwise mean "delete everything".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_days: Option<u64>,
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
            // pi `resolveAsyncByDefault` (`config.ts:222-224`): `config.asyncByDefault !== false`
            // — an ABSENT key means TRUE, so a stock install with no `config.json` backgrounds.
            // A plain `bool` seeded `true` reproduces that tri-state exactly: absent -> this
            // default, `false` -> false, `true` -> true. Upstream publishes the opt-out in its own
            // module header (`index.ts:9`) and in the `async` param description (`schemas.ts:324`);
            // seeding this `false` made that documented opt-out a no-op, because the port already
            // behaved as if it had been set.
            async_by_default: true,
            force_top_level_async: false,
            global_concurrency_limit: 20,
            max_subagent_spawns_per_session: 40,
            parallel: None,
            control: None,
            chain: None,
            proactive_skill_subagents: None,
            default_session_dir: None,
            spawn_command: None,
            roots: crate::paths::Roots::from_env(),
            env_overrides: std::collections::BTreeMap::new(),
            single_run_output_base_dir: None,
            max_subagent_depth: 2,
            worktree_base_dir: None,
            worktree_setup_hook: None,
            worktree_setup_hook_timeout_ms: None,
            // pi `config.fleetView !== false` (`extension/index.ts:333`) — on unless explicitly off.
            fleet_view: true,
            fleet_view_placement: None,
            wait_tool: None,
            missions: None,
            artifact_config: None,
            artifact_dir: None,
            authority_policy: None,
            turn_budget: None,
            timeout_ms: None,
            default_subagent_context: None,
            // SUBA-025 — pi's `mode === undefined => "full"` (`tool-description.ts:106`).
            tool_description_mode: None,
            permissions: None,
        }
    }
}

impl SubagentExtensionConfig {
    /// pi `validateMissionStoreConfig(config.missions)` (`extension/config.ts:25`) — the
    /// unknown-key/wrong-type check upstream runs on every config read, applied to the RAW config
    /// JSON so an unknown key inside the `missions` block is refused rather than silently dropped
    /// by serde's field matching.
    ///
    /// # Errors
    ///
    /// The upstream refusal text (`config.missions.<key> is unknown`, `… must be boolean`, `…
    /// must be a positive integer`).
    /// SUBA-064 — pi `validateAuthorityPolicy(config.authorityPolicy)`
    /// (`policy/authority.ts:30-45`), applied to the RAW config JSON beside
    /// [`Self::validate_missions`] for the same reason: serde drops an unknown action key and a bad
    /// decision string silently, where upstream throws.
    ///
    /// # Errors
    ///
    /// Upstream's typed refusals — see [`authority::validate_authority_policy`].
    pub fn validate_authority_policy(raw: &serde_json::Value) -> Result<(), String> {
        authority::validate_authority_policy(raw.get("authorityPolicy"), "config.authorityPolicy")
    }

    /// SUBA-048 — the artifact-directory preference this process resolves runs against: pi
    /// `config.artifactDir ?? DEFAULT_ARTIFACT_CONFIG.dir` (`extension/index.ts:375` @v0.47.1).
    #[must_use]
    pub fn artifact_dir_preference(&self) -> crate::artifacts::ArtifactDirPreference {
        self.artifact_dir.unwrap_or_default()
    }

    /// SUBA-048 — pi `validateConfig`'s first clause (`extension/config.ts:51-53` @v0.47.1):
    /// `if (config.artifactDir !== undefined && !ARTIFACT_DIR_PREFERENCES.has(config.artifactDir))
    /// throw ...`. Applied to the RAW config JSON because serde silently drops an unknown enum
    /// string where upstream THROWS — the same reason [`Self::validate_missions`] exists.
    ///
    /// # Errors
    ///
    /// `config.artifactDir must be "project", "session", or "temp"`.
    pub fn validate_artifact_dir(raw: &serde_json::Value) -> Result<(), String> {
        let Some(value) = raw.get("artifactDir") else {
            return Ok(());
        };
        let invalid = r#"config.artifactDir must be "project", "session", or "temp""#.to_string();
        let Some(text) = value.as_str() else {
            return Err(invalid);
        };
        crate::artifacts::ArtifactDirPreference::parse(text).map(|_| ())
    }

    /// SUBA-059 — the retention horizon this load will sweep with: pi
    /// `config.artifactConfig?.cleanupDays ?? DEFAULT_ARTIFACT_CONFIG.cleanupDays`
    /// (`extension/index.ts:369` @v0.47.1).
    #[must_use]
    pub fn artifact_cleanup_days(&self) -> u64 {
        self.artifact_config
            .and_then(|c| c.cleanup_days)
            .unwrap_or(crate::artifacts::DEFAULT_CLEANUP_DAYS)
    }

    /// SUBA-059 — pi `validateArtifactConfig` (`extension/config.ts:40-47` @v0.47.1), applied to
    /// the RAW config JSON for the same reason [`Self::validate_missions`] is: serde alone accepts
    /// a wrong-typed `cleanupDays` and silently drops it, where upstream THROWS.
    ///
    /// Both messages are upstream's, verbatim.
    ///
    /// # Errors
    ///
    /// `config.artifactConfig must be a JSON object` for a non-object value, and
    /// `config.artifactConfig.cleanupDays must be a non-negative integer` for anything that is not
    /// a non-negative integer.
    pub fn validate_artifact_config(raw: &serde_json::Value) -> Result<(), String> {
        let Some(value) = raw.get("artifactConfig") else {
            return Ok(());
        };
        // pi's guard is `!value || typeof value !== "object" || Array.isArray(value)`, so `null`
        // is refused too — JS `!null` is true.
        let Some(obj) = value.as_object().filter(|_| !value.is_null()) else {
            return Err("config.artifactConfig must be a JSON object".to_string());
        };
        let Some(days) = obj.get("cleanupDays") else {
            return Ok(());
        };
        if days.is_null() {
            // `cleanupDays !== undefined` is TRUE for an explicit JSON `null`, which then fails
            // `typeof === "number"`.
            return Err(
                "config.artifactConfig.cleanupDays must be a non-negative integer".to_string()
            );
        }
        if days.as_u64().is_none() {
            return Err(
                "config.artifactConfig.cleanupDays must be a non-negative integer".to_string()
            );
        }
        Ok(())
    }

    pub fn validate_missions(raw: &serde_json::Value) -> Result<(), String> {
        crate::missions::validate_mission_store_config(raw.get("missions"), "config.missions")
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

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
/// This is the **canonical** definition arch-SA §2.2 designates for `registration/mod.rs`, and the
/// only one: [`crate::spawn::worktree::HookSpec`] is a type alias to it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookSpec {
    /// The executable to invoke.
    pub command: PathBuf,
    /// Arguments passed to `command`, before the JSON-on-stdin payload.
    pub args: Vec<String>,
}

// -------------------------------------------------------------------------------------------
// Nested config objects (pi shared/types.ts:829-882) — the shapes pi's ExtensionConfig nests
// -------------------------------------------------------------------------------------------

/// pi `TopLevelParallelConfig` (shared/types.ts:1715-1718): the nested `parallel: { maxTasks?, concurrency? }`
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

/// pi `ExtensionChainConfig` (shared/types.ts:1720-1724): the nested `chain: { dynamicFanout?: { maxItems? } }`
/// object of [`SubagentExtensionConfig`].
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExtensionChainConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_fanout: Option<DynamicFanoutConfig>,
}

/// The `chain.dynamicFanout` object (pi shared/types.ts:835-837): the per-run cap on how many items a
/// dynamic fan-out step may expand to.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DynamicFanoutConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,
}

/// One control-notice event class (pi `ControlEventType`, shared/types.ts:157): the two activity-state
/// transitions a run may raise a control notice for. Serializes as `active_long_running` /
/// `needs_attention` (matching pi's string union and [`crate::background::ActivityState`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlEventType {
    ActiveLongRunning,
    NeedsAttention,
}

/// One control-notice delivery channel (pi `ControlNotificationChannel`, shared/types.ts:158). Serializes
/// as `event` / `async` / `intercom`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ControlNotificationChannel {
    Event,
    Async,
    Intercom,
}

/// pi `ControlConfig` (shared/types.ts:160-169): the live-control notice thresholds/channels nested under
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

/// pi `ProactiveSkillSubagentsConfig` (shared/types.ts:1726-1731): the tuning knobs for proactive
/// skill-subagent suggestions.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProactiveSkillSubagentsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    // These are `i64`, not `u32`, so that an out-of-range value REACHES the guard instead of
    // failing deserialization. `positive_integer` (discovery/skills.rs:412) already filters
    // `>= 1` and is written for `i64`, matching upstream's `positiveInteger`
    // (proactive-skills.ts:32-36), which returns `undefined` for a non-positive value and lets
    // the caller fall back to the default while KEEPING the rest of the file.
    //
    // With `u32`, serde rejected `-1` before the guard ever ran, and
    // `load_subagent_extension_config` (crates/cyrup/src/subagent_config.rs) discards the WHOLE
    // config.json on any deserialization error — so one bad value silently dropped every other
    // setting in the file (parallel.maxTasks, control.*, chain.dynamicFanout.maxItems,
    // worktreeSetupHook, maxSubagentDepth, globalConcurrencyLimit).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_references: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_recommendations: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_agent: Option<String>,
}

/// pi `proactiveSkillSubagents?: ProactiveSkillSubagentsConfig | false` (shared/types.ts:1779, interface at :1726-1731): either a
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

/// The namespaced `subagents` settings slice — tier **2** of R-SA-133's five-tier precedence,
/// below inline per-call overrides and above this extension's own `config.json` (tier 3).
///
/// # There is exactly ONE on-disk source for this, and its precedence is upstream's (SUBA-071)
///
/// This doc comment used to say the view was "read via `SettingsManager::effective().get(
/// \"subagents\")` and written back via `SettingsManager::set_nested`". **No such read or write
/// has ever existed in this crate** — `rg 'SettingsManager|effective\(\)' crates/cyrup-ext-subagents/src`
/// matches only prose — and the claim caused **SUBA-071** to be filed against a two-store
/// divergence that is not there. The stale sentence is deleted rather than softened, because a
/// doc comment describing a read path that does not exist is indistinguishable, to a reader, from
/// one that does.
///
/// The single real source is the pair of `subagents`-blocked `settings.json` files the discovery
/// path reads — `~/.cyrup/agents/settings.json` (user) and `<project_root>/.cyrup/agents/settings.json`
/// (project) — layered by [`crate::discovery::load_layered_subagent_settings`] with **project
/// beating user on every scalar and every per-agent override name**. That is upstream's own rule
/// and upstream's own pair of files: pi resolves `subagents.defaultModel` at
/// `agents/agents.ts:924-931` @v0.43.0 (`projectSettings.defaultModel !== undefined` → project
/// scope, else user), `defaultThinking` at `:949-951` and `defaultExtensions` at `:969-971`, from
/// `getUserAgentSettingsPath()` (`:674-676`) and `getProjectAgentSettingsPath(cwd)` (`:678-681`).
/// So there is nothing to merge and no precedence to invent: [`Self::from_subagent_settings`] is
/// the ONLY constructor that carries real data, and its input is that already-layered result.
///
/// `~/.cyrup/agent/settings.json` — `cyrup_config::Dirs::settings_path()`, the binary's own
/// layered settings document — is a **different file that this crate never reads**. See
/// [`profiles`]'s R-SA-141 note for the one place a store-based writer was deleted for aiming at
/// it.
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
    // agents.ts:82-100, carries none) — an earlier port invented one and consulted it here. The
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

    /// SUBA-059 — pi `config.artifactConfig?.cleanupDays ?? DEFAULT_ARTIFACT_CONFIG.cleanupDays`
    /// (`extension/index.ts:369` @v0.47.1) plus `validateArtifactConfig`
    /// (`extension/config.ts:40-47`).
    ///
    /// THE USER ACTION: a user wants subagent transcripts kept for audit, or purged sooner, or not
    /// swept at all. Before the fix both sweeps passed the hardcoded 7-day constant and the config
    /// key did not exist on `SubagentExtensionConfig`, so `"artifactConfig": {"cleanupDays": 0}`
    /// was accepted into `config.json`, dropped by serde, and every extension load still deleted
    /// run inputs, outputs and JSONL older than a week — including the record of what a fan-out
    /// actually did.
    #[test]
    fn artifact_cleanup_days_round_trips_and_validates_like_upstream() {
        assert_eq!(
            SubagentExtensionConfig::default().artifact_cleanup_days(),
            crate::artifacts::DEFAULT_CLEANUP_DAYS,
            "an omitted key must fall back to pi's DEFAULT_ARTIFACT_CONFIG.cleanupDays"
        );

        let parsed: SubagentExtensionConfig =
            serde_json::from_value(serde_json::json!({ "artifactConfig": { "cleanupDays": 30 } }))
                .expect("config parses");
        assert_eq!(parsed.artifact_cleanup_days(), 30);

        // `0` is upstream's documented opt-out, not "delete everything".
        let disabled: SubagentExtensionConfig =
            serde_json::from_value(serde_json::json!({ "artifactConfig": { "cleanupDays": 0 } }))
                .expect("config parses");
        assert_eq!(disabled.artifact_cleanup_days(), 0);

        // Validation — upstream's exact texts.
        assert!(SubagentExtensionConfig::validate_artifact_config(&serde_json::json!({})).is_ok());
        assert_eq!(
            SubagentExtensionConfig::validate_artifact_config(&serde_json::json!({
                "artifactConfig": []
            }))
            .expect_err("an array is not an object"),
            "config.artifactConfig must be a JSON object"
        );
        for bad in [
            serde_json::json!("7"),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!(null),
        ] {
            assert_eq!(
                SubagentExtensionConfig::validate_artifact_config(&serde_json::json!({
                    "artifactConfig": { "cleanupDays": bad }
                }))
                .expect_err("cleanupDays must be a non-negative integer"),
                "config.artifactConfig.cleanupDays must be a non-negative integer",
                "value was {bad}"
            );
        }
    }

    #[test]
    fn subagent_extension_config_default_matches_func_sa_4_7_constants() {
        let cfg = SubagentExtensionConfig::default();
        // NOT a func-SA §4.7 constant like the numbers below it — this one is pi parity:
        // `resolveAsyncByDefault` (`config.ts:222-224`) is `!== false`, so an absent key is TRUE.
        assert!(
            cfg.async_by_default,
            "an absent `asyncByDefault` must background (pi `config.ts:222-224`)"
        );
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
        // The exact pi ExtensionConfig shape (shared/types.ts:864-882): nested `parallel {}`, `control {}`
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
    // HookSpec (aliased as spawn::worktree::HookSpec)
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
            default_thinking: None,
            default_extensions: None,
            disable_builtins: Some(true),
            disable_thinking: Some(true),
                    max_thinking: None,
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
