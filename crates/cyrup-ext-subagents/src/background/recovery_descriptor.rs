//! The async recovery descriptor — pi `SteeringRecoveryDescriptor` (`src/shared/types.ts:805-864`
//! @v0.68.0): the RESOLVED launch contract of one async single run, persisted at launch as
//! `<run_dir>/recovery-descriptor.json` and read back by `action: "resume"` so a revived child is
//! still the run that was launched — same model, tools, budgets, skills, prompt, output contract
//! and capability ceiling — rather than a bare agent name re-discovered from whatever the persona
//! file says today.
//!
//! # Where it is written and read
//!
//! * **Written** by [`crate::extension::SubagentExecutor::spawn_background_steps`] for a
//!   `RunMode::Single` run with exactly one `SingleStep`, BEFORE `runner-config.json` and before
//!   any process is spawned (pi `async-execution.ts:1993-2055`, gated `if (!externalRunner)`). A
//!   write failure FAILS THE LAUNCH (`:2053`): a run that cannot be recovered is refused up front,
//!   not discovered later. Chain/parallel/graph launches write none — pi writes one only from
//!   `executeAsyncSingle`.
//! * **Read** by the terminal-revival arm of `action: "resume"`
//!   (`extension/executor/control.rs::revive_from_transcript`; pi `async-resume.ts:310-440` +
//!   `subagent-executor.ts:1882-2188`): a missing descriptor REFUSES the revive with pi's
//!   sentence, a descriptor for another agent or another source run refuses, and otherwise the
//!   descriptor OVERLAYS the discovered persona field-for-field (pi
//!   `applySteeringRecoveryAgentConfig`, `async-resume.ts:604-632`) — the file on disk does not
//!   win.
//! * **Read** by the retention scan ([`crate::background::async_retention`], pi
//!   `hasResumableContract`), which inspects only `sourceRunId` and `sessionFile` — the two keys
//!   that are the hard on-disk compatibility floor.
//!
//! # On-disk shape
//!
//! pi's key names, camelCase, `version: 1`, `deny_unknown_fields` (pi's reader keeps an allowlist
//! and throws on anything else, `async-resume.ts:322-333`). Every `Option` is written only when
//! `Some`, matching pi's `...(x ? { x } : {})` spreads; the fields pi writes unconditionally are
//! non-`Option` here.
//!
//! # Fields cyrup cannot carry (omitted from the struct, deliberately)
//!
//! `mcpDirectTools`, `skillPath`, `intercomBridge`, `maxOutput`, `launchResolvedExtensions`,
//! `modelResponseAliases`, `extensionBindings`, `requiredExtensions`, `agentContract`, `baseRef`:
//! none of these has a cyrup concept on the async single path (see the `[AUG — descriptor]` field
//! map for the per-field grep). `deny_unknown_fields` is safe because the writer and every reader
//! of this file are in this crate. (`fast`, `mutationTools` and `inheritGlobalContext` left this
//! list with SUBA-101/102/103 and are carried below.)
//!
//! # `[CYRUP-DELTA]` — four cyrup-only run-level keys
//!
//! `turnBudget`, `usageBudget`, `permissionRules` and `includeProgress` are additive keys pi's
//! descriptor does not have (pi removed `initialTurnBudget` and never had the other three).
//! Without them a revive degrades to `None` on exactly the run-level fields cyrup's runner
//! enforces (SUBA-008/021/073/N06). Premise (true): no pi process ever reads cyrup's async root —
//! the writer and every reader of this file are this crate — so pi's allowlist rejecting the keys
//! is immaterial.
//!
//! # `[CYRUP-DELTA]` — `lane` and `runFanoutBudget` are typed but always `None` today
//!
//! `lane`: cyrup attaches lane metadata to `ParallelGroupSpec` only (`spawn/chain_graph.rs`) and
//! this descriptor is written only for SINGLE runs, which carry no lane; the slot is kept so a
//! future single-lane lands without a format change. `runFanoutBudget`: pi writes it
//! unconditionally, but `create_run_fanout_budget` (`exec/run_fanout_budget.rs`) has zero
//! production callers — no cyrup async launch allocates a fan-out ledger — so writing a ledger
//! handle here would be a lie on disk. Wiring the ledger into the launch is a separate feature.

use std::path::{Path, PathBuf};

use cyrup_core::{ModelId, ProviderId};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::runner_main::RunnerConfig;
use super::{RunId, RunMode, RunStatus};
use crate::artifacts::ArtifactConfig;
use crate::discovery::types::{
    AgentMemoryConfig, OutputMode, ResolvedToolBudget, SystemPromptMode, ToolRef,
};
use crate::exec::ResolvedAgentPersona;
use crate::exec::capability_ceiling::{CAPABILITY_CEILING_VERSION, ResolvedCapabilityCeiling};
use crate::exec::control::ResolvedControlConfig;
use crate::exec::run_fanout_budget::RunFanoutBudgetDescriptor;
use crate::exec::turn_budget::ResolvedTurnBudget;
use crate::exec::usage_budget::UsageBudgetConfig;
use crate::fork_context::ContextMode;
use crate::spawn::chain_graph::{RunnerStep, SingleStepSpec};
use crate::watchdog::permission_arbiter::PermissionRules;
use crate::workflows::WorkflowLaneMetadata;

// =================================================================================================
// Errors
// =================================================================================================

/// Every way persisting, reading, validating or matching a recovery descriptor can fail. One
/// variant per upstream `throw`, each carrying pi's wording verbatim (the crate's convention,
/// recorded on [`crate::error::SubagentError::Management`]) plus the structured data a caller
/// can branch on.
#[derive(thiserror::Error, Debug)]
pub enum RecoveryDescriptorError {
    /// The launch-time write failed — pi `async-execution.ts:2053`, which FAILS THE LAUNCH.
    #[error("Failed to persist async recovery descriptor for '{run_id}': {source}")]
    Persist {
        /// The run whose launch is refused.
        run_id: RunId,
        /// The underlying filesystem error.
        #[source]
        source: std::io::Error,
    },

    /// The file exists but is not JSON — pi `async-resume.ts:318`.
    #[error("Failed to parse async recovery descriptor '{path}': {detail}")]
    Parse {
        /// The descriptor's path.
        path: PathBuf,
        /// The parser's own message.
        detail: String,
    },

    /// The file is JSON but not a descriptor this build accepts (unknown field, wrong version,
    /// wrong shape) — pi `async-resume.ts:320-437`.
    #[error("Invalid async recovery descriptor '{path}': {detail}")]
    Invalid {
        /// The descriptor's path.
        path: PathBuf,
        /// Which check failed.
        detail: String,
    },

    /// `launchContractDigest` is not a 64-hex SHA-256 digest (the same shape rule
    /// `handoff::ManifestDigest` enforces).
    #[error("launchContractDigest must be a full 64-character SHA-256 digest.")]
    NotADigest,

    /// The descriptor names a different agent than the step being revived — pi
    /// `async-resume.ts:566`, verbatim.
    #[error(
        "Async run '{run_id}' has a recovery descriptor for '{descriptor_agent}', not '{step_agent}'."
    )]
    AgentMismatch {
        /// The run being revived.
        run_id: RunId,
        /// What the descriptor says.
        descriptor_agent: String,
        /// What the run's own status says.
        step_agent: String,
    },

    /// The descriptor's `sourceRunId` is not the run it sits under — pi
    /// `subagent-executor.ts:1493`, verbatim.
    #[error("Nested run '{run_id}' has a recovery descriptor for a different source run.")]
    SourceRunMismatch {
        /// The run being revived.
        run_id: RunId,
        /// The `sourceRunId` the descriptor carries instead.
        found: RunId,
    },

    /// No descriptor at all — pi `subagent-executor.ts:2060`, verbatim. Runs launched before the
    /// descriptor existed have none and are refused exactly as upstream refuses them; the
    /// alternative is the bare-agent revive this file exists to remove.
    #[error(
        "Async child '{run_id}' is missing its required run fan-out recovery identity. Start a new run instead."
    )]
    Missing {
        /// The run being revived.
        run_id: RunId,
    },
}

// =================================================================================================
// Newtypes and enums
// =================================================================================================

/// The literal `version: 1` — refuses anything else on read, the way [`crate::handoff`]'s
/// `ManifestVersion` does (pi `async-resume.ts:338`: `version !== 1` throws).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DescriptorVersion;

impl DescriptorVersion {
    /// The only value this type represents.
    pub const VALUE: u32 = 1;
}

impl Serialize for DescriptorVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(Self::VALUE)
    }
}

impl<'de> Deserialize<'de> for DescriptorVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == Self::VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported async recovery descriptor version {raw} (this build reads version {})",
                Self::VALUE
            )))
        }
    }
}

/// pi `modelOrigin` (`types.ts:823`): how the launch's primary model was selected. Decides where
/// the model lands on a revive — an `Explicit` model goes back on the step as the per-call
/// override; a `Configured` or `Inherited` one is pinned onto the persona so the revived ladder
/// starts from the SAME model, never the reviving session's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelOrigin {
    /// A per-call `model:` override selected it.
    Explicit,
    /// The launching session's own model was inherited.
    Inherited,
    /// The persona's own `model:` (or nothing) selected it.
    Configured,
}

/// A full 64-hex-digit SHA-256 digest of the launch binding — evidence, never a gate (nothing on
/// the async revive path reads a step's launch digest; see the module doc).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct LaunchContractDigest(String);

impl LaunchContractDigest {
    /// The only fallible constructor: 64 hex digits, stored lower-cased.
    ///
    /// # Errors
    ///
    /// [`RecoveryDescriptorError::NotADigest`] when the shape fails.
    pub fn parse(value: &str) -> Result<Self, RecoveryDescriptorError> {
        let normalized = value.trim().to_ascii_lowercase();
        if normalized.len() != 64 || !normalized.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(RecoveryDescriptorError::NotADigest);
        }
        Ok(Self(normalized))
    }

    /// Borrows the validated digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LaunchContractDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for LaunchContractDigest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// What [`RecoveryDescriptor::for_single_launch`] needs that `RunnerConfig` does not carry: the
/// two ceilings the LAUNCHING process resolved from its own environment and session registry
/// (the runner derives its own from env, so neither lives on the config), and — on a revive —
/// the source descriptor's own `modelOrigin`, pi's `storedOrigin`
/// (`runs/shared/model-resolution.ts:382` @v0.68.0: `if (input.storedOrigin) return
/// input.storedOrigin;`), which wins over re-derivation so a chain of revives keeps the
/// provenance the run was launched with.
#[derive(Clone, Copy, Debug, Default)]
pub struct LaunchInputs<'a> {
    /// The effective thinking ceiling (pi `thinkingCeiling`), already intersected.
    pub thinking_ceiling: Option<&'a str>,
    /// The effective capability ceiling (pi `capabilityCeiling`), already intersected.
    pub capability_ceiling: Option<&'a ResolvedCapabilityCeiling>,
    /// The source descriptor's `modelOrigin` on a revive (pi `modelOrigin:
    /// recoveryDescriptor?.modelOrigin`, `subagent-executor.ts:2149`, consumed as `storedOrigin`
    /// at `async-execution.ts:1859`); `None` on an ordinary launch, which derives it from the
    /// launch itself.
    pub stored_model_origin: Option<ModelOrigin>,
}

// =================================================================================================
// The descriptor
// =================================================================================================

/// pi `SteeringRecoveryDescriptor` — see the module doc for what is carried, what lands where on
/// a revive, and what cyrup cannot carry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryDescriptor {
    /// Always `1`.
    pub version: DescriptorVersion,
    /// Evidence: sha256 of the stable JSON of the launch binding (pi
    /// `launch-contract.ts:111-135`).
    pub launch_contract_digest: LaunchContractDigest,
    /// The run this descriptor belongs to; must equal the status's own `run_id` on read.
    pub source_run_id: RunId,
    /// The launched agent; must equal the revived step's agent on read.
    pub agent: String,
    /// The run's working directory — the LAST rung of the revive's cwd ladder (pi `:579`).
    pub cwd: PathBuf,
    /// The fork-context branch this run was seeded from, when its context was `fork`. Evidence
    /// here (the revive seeds from the status's own step transcript); the retention scan reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_file: Option<PathBuf>,
    /// The selected primary model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelId>,
    /// How [`Self::model`] was selected.
    pub model_origin: ModelOrigin,
    /// `true` iff [`Self::model_origin`] is [`ModelOrigin::Inherited`] (pi writes it only then).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_override_from_parent: Option<bool>,
    /// The persona's `modelProvider`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<ProviderId>,
    /// The effective thinking level (caller's rung already folded onto the persona at launch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// The thinking ceiling in force at launch; re-applied on the revive spawn's env, intersected
    /// with the reviver's own — a revive never widens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_ceiling: Option<String>,
    /// The persona's tool allowlist, written in pi's `string[]` form (`"read"`, `"mcp:x"`) and
    /// read back through [`ToolRef`]'s own string-accepting visitor.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "tool_refs_as_pi_strings"
    )]
    pub tools: Option<Vec<ToolRef>>,
    /// The persona's `excludeTools`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_tools: Vec<String>,
    /// The persona's `allowNestedSubagents`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_nested_subagents: Option<bool>,
    /// The persona's extension allowlist tri-state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    /// The persona's child-only extension paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagent_only_extensions: Vec<String>,
    /// The persona's system-prompt body (written iff non-empty) — the reason this file is `0600`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// The persona's prompt mode.
    pub system_prompt_mode: SystemPromptMode,
    /// The persona's project-context inheritance flag.
    pub inherit_project_context: bool,
    /// The persona's skills inheritance flag.
    pub inherit_skills: bool,
    /// SUBA-101 — pi `inheritGlobalContext` (`async-resume.ts:368-369` @v0.68.0). A descriptor
    /// written before this field existed reads as `None`; upstream then takes
    /// `inheritProjectContext` (`:368`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherit_global_context: Option<bool>,
    /// SUBA-102 — pi `mutationTools` (`async-resume.ts:323,372` @v0.68.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_tools: Option<Vec<String>>,
    /// SUBA-103 — pi `fast` (`async-resume.ts:323`, validated `:362` @v0.68.0), written by
    /// `async-execution.ts:2008`. The revive passes it as `params.fast` (`subagent-executor.ts:2147`)
    /// and relaunches with `params.fast ?? agentConfig.fast` (`async-execution.ts:1967`), so an
    /// ABSENT value falls back to the agent's current `fast` (`extension/executor/control.rs`'s
    /// revive does the same). Present whenever some launch rung set it — including `false`, which
    /// pi drops (see [`Self::for_single_launch`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<bool>,
    /// The EFFECTIVE skill list (per-call override, else the persona's), written iff non-empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// The persona's definition file, consumed only by [`Self::synthesised_persona`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_file_path: Option<PathBuf>,
    /// The persona's completion-guard setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_guard: Option<bool>,
    /// The persona's `memory:` block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<AgentMemoryConfig>,
    /// The resolved absolute output file path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    /// `inline` or `file-only` (the only two values the async path produces; pi's reader admits
    /// no other, and neither does [`Self::validate`]).
    pub output_mode: OutputMode,
    /// The step's structured-output schema (an object).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output_schema: Option<serde_json::Value>,
    /// The step's RAW acceptance input, re-validated on read exactly as the tool boundary does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<serde_json::Value>,
    /// The resolved live-control config the run was authorized with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_config: Option<ResolvedControlConfig>,
    /// The step's resolved context mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextMode>,
    /// Always `None` today — see the module doc's delta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<WorkflowLaneMetadata>,
    /// The absolute epoch-ms deadline the run was armed with. Evidence: pi's plain `resume` does
    /// not re-arm it (`subagent-executor.ts:2180-2181`); only the unported steering-recovery
    /// path does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_deadline_at: Option<u64>,
    /// The tool budget in force at launch (caller's rung already folded onto the persona).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_tool_budget: Option<ResolvedToolBudget>,
    /// The EFFECTIVE child depth ceiling (pi `resolveChildMaxSubagentDepth`, `:2041`) — the
    /// second capability-widening fix: a revive re-applies it as a tightening-only override.
    pub max_subagent_depth: u32,
    /// The capability ceiling in force at launch; re-applied on the revive spawn's env,
    /// intersected with the reviver's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_ceiling: Option<ResolvedCapabilityCeiling>,
    /// pi `share`.
    pub share: bool,
    /// The step's resolved session directory leaf.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_dir: Option<PathBuf>,
    /// pi `artifactsDir` — absent when artifacts were disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts_dir: Option<PathBuf>,
    /// pi `artifactConfig`.
    pub artifact_config: ArtifactConfig,
    /// Always `None` today — see the module doc's delta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_fanout_budget: Option<RunFanoutBudgetDescriptor>,
    // ---- cyrup-only run-level keys (additive `[CYRUP-DELTA]`, module doc) ----
    /// SUBA-008 — the run-level turn budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_budget: Option<ResolvedTurnBudget>,
    /// SUBA-021 — the run-level usage budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_budget: Option<UsageBudgetConfig>,
    /// SUBA-073 — the fully-merged permission policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_rules: Option<PermissionRules>,
    /// SUBA-N06 — the caller's `includeProgress`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_progress: Option<bool>,
}

/// pi writes `tools` as a `string[]` (`"read"`, `"mcp:server.tool"`); [`ToolRef`]'s own derive
/// would write the tagged `{kind, content}` map instead. Its hand-written `Deserialize` accepts
/// BOTH forms, so the string form round-trips. An `ExtensionPath` has no pi string spelling and is
/// written in the tagged form the same visitor accepts.
fn tool_refs_as_pi_strings<S: serde::Serializer>(
    tools: &Option<Vec<ToolRef>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq as _;
    let Some(tools) = tools else {
        return serializer.serialize_none();
    };
    let mut seq = serializer.serialize_seq(Some(tools.len()))?;
    for tool in tools {
        match tool {
            ToolRef::Builtin(name) => seq.serialize_element(name)?,
            ToolRef::Mcp(name) => seq.serialize_element(&format!("mcp:{name}"))?,
            ToolRef::ExtensionPath(_) => seq.serialize_element(tool)?,
        }
    }
    seq.end()
}

/// pi's own `tools` spelling of one [`ToolRef`], for the launch-binding digest.
fn tool_ref_as_pi_json(tool: &ToolRef) -> serde_json::Value {
    match tool {
        ToolRef::Builtin(name) => serde_json::Value::String(name.clone()),
        ToolRef::Mcp(name) => serde_json::Value::String(format!("mcp:{name}")),
        ToolRef::ExtensionPath(_) => serde_json::to_value(tool).unwrap_or(serde_json::Value::Null),
    }
}

/// `sha256(bytes)`, lower hex — the same fold `workflows::stable_json::stable_json_digest` uses.
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// pi `resolveModelOrigin` (`runs/shared/model-resolution.ts:375-388`) minus its first two rungs
/// (`storedOrigin`/`fromParent` are a revive's, and arrive through
/// [`LaunchInputs::stored_model_origin`]) + the model it selects, with cyrup's session rung
/// standing in for `ctx.currentModel`.
fn resolve_model_and_origin(
    step: &SingleStepSpec,
    persona: &ResolvedAgentPersona,
    inherited_session_model: Option<&ModelId>,
) -> (Option<ModelId>, ModelOrigin) {
    let is_real = |model: &ModelId| {
        let trimmed = model.as_str().trim();
        !trimmed.is_empty() && trimmed != crate::exec::fallback::INHERIT_MODEL_SENTINEL
    };
    // pi `inheritsParentModel(explicit, agent, parent)`: `requested = explicit ?? agent`, and the
    // parent's model is inherited when there is a parent and the request is blank or `inherit`.
    let requested = step.model.as_ref().or(persona.model.as_ref());
    if let Some(parent) = inherited_session_model
        && requested.is_none_or(|model| !is_real(model))
    {
        return (Some(parent.clone()), ModelOrigin::Inherited);
    }
    match &step.model {
        Some(explicit) if is_real(explicit) => (Some(explicit.clone()), ModelOrigin::Explicit),
        _ => (
            persona.model.as_ref().filter(|m| is_real(m)).cloned(),
            ModelOrigin::Configured,
        ),
    }
}

impl RecoveryDescriptor {
    /// Build the descriptor for one async SINGLE launch from the one-shot config the launch is
    /// about to hand to hop 2 — `RunnerConfig` already holds the resolved step, persona, cwd,
    /// session file, deadline, share/artifacts, control, the four run-level values and the
    /// session-inheritance rungs, so it IS the resolved launch (pi builds from the same locals,
    /// `async-execution.ts:1993-2048`, with `recoveryAgentConfig = params.recoveryAgentConfig ??
    /// agentConfig` — on a revive the OVERLAID persona is what `resolved_agents` carries, so a
    /// chain of revives never drifts back to the file).
    ///
    /// `None` when this launch is not a single-step `RunMode::Single` run (pi writes a descriptor
    /// only from `executeAsyncSingle`), or when the step's persona is absent from
    /// `resolved_agents` — a run the runner itself will refuse as `Unknown agent` has no contract
    /// to record.
    #[must_use]
    pub fn for_single_launch(config: &RunnerConfig, inputs: LaunchInputs<'_>) -> Option<Self> {
        if config.mode != RunMode::Single {
            return None;
        }
        let [RunnerStep::SingleStep(step)] = config.steps.as_slice() else {
            return None;
        };
        let persona = config.resolved_agents.get(&step.agent)?;
        // SUBA-100 [CYRUP-DELTA] — upstream writes a descriptor for a placed native run too
        // (`async-execution.ts:2049-2051` @v0.68.0), and it carries no machine, so a revive would
        // quietly run the child LOCALLY. A placed run's session lives on the machine; it is not
        // revivable here, and no descriptor claims otherwise.
        if step.machine.is_some() || persona.machine.is_some() {
            return None;
        }

        let (model, derived_origin) =
            resolve_model_and_origin(step, persona, config.inherited_session_model.as_ref());
        // pi `resolveModelOrigin` (`model-resolution.ts:382`): `if (input.storedOrigin) return
        // input.storedOrigin;` — a revive keeps the origin the run was LAUNCHED with; only a fresh
        // launch derives one. The model is the same either way (on a revive the overlay pinned it
        // on the persona, so re-derivation finds it there and would call it `configured`); what
        // the stored origin preserves is the provenance the descriptor records, and with it the
        // slot the model lands on at the NEXT revive.
        let model_origin = inputs.stored_model_origin.unwrap_or(derived_origin);
        let thinking = persona
            .thinking
            .clone()
            .or_else(|| config.inherited_session_thinking.clone());
        // pi `params.skills ?? agentConfig.skills` (`async-execution.ts:876`), written iff the
        // EFFECTIVE list is non-empty (`resolvedSkills.length`, `:2026`).
        let skills = step
            .skills
            .clone()
            .unwrap_or_else(|| persona.skills.clone());
        let output_mode = step.output_mode.unwrap_or(OutputMode::Inline);
        // pi `resolveChildMaxSubagentDepth(maxSubagentDepth, recoveryAgentConfig.maxSubagentDepth)`
        // (`:2041`): the EFFECTIVE child ceiling — the tightest of the config's, the persona's
        // own and any per-step override — never the raw agent value.
        let agent_cap = [persona.max_subagent_depth, step.max_depth_override]
            .into_iter()
            .flatten()
            .min();
        let max_subagent_depth = agent_cap.map_or(config.max_subagent_depth, |cap| {
            cap.min(config.max_subagent_depth)
        });
        let launch_contract_digest = launch_binding_digest(LaunchBinding {
            step,
            persona,
            model: model.as_ref(),
            thinking: thinking.as_deref(),
            skills: &skills,
            output_mode,
        });

        Some(Self {
            // SUBA-103 — pi `...(params.fast ?? recoveryAgentConfig.fast ? { fast } : {})`
            // (`async-execution.ts:2008` @v0.68.0). The step already carries the EFFECTIVE value
            // (`s.fast ?? params.fast ?? a.fast`, folded at plan time by `spawn_background` /
            // `apply_agent_launch_defaults`), which is `None` when NO rung set it — and then, as in
            // pi, nothing is recorded and the revive falls back to the agent's current `fast`.
            // `[CYRUP-DELTA]`: when the effective value is an explicit `false` (a call or step that
            // said `fast: false`, or an agent file that did), it is recorded — pi drops it, so its
            // revive falls back to whatever the agent file says NOW (`params.fast ??
            // agentConfig.fast` with `params.fast` undefined, `async-execution.ts:1967`) and a run
            // launched deliberately standard can revive fast; cyrup's revive replays the launch's
            // own decision there.
            fast: step.fast,
            // SUBA-101 — pi writes `inheritGlobalContext: recoveryAgentConfig.inheritGlobalContext`
            // unconditionally (`:2024`); `Option` only so a pre-field descriptor still reads.
            inherit_global_context: Some(persona.inherit_global_context),
            // SUBA-102 — pi `...(recoveryAgentConfig.mutationTools ? { mutationTools } : {})`
            // (`:2020`).
            mutation_tools: persona.mutation_tools.clone(),
            version: DescriptorVersion,
            launch_contract_digest,
            source_run_id: config.run_id.clone(),
            agent: step.agent.clone(),
            cwd: config.cwd.clone(),
            session_file: config.session_file.clone(),
            model,
            model_override_from_parent: (model_origin == ModelOrigin::Inherited).then_some(true),
            model_origin,
            model_provider: persona.model_provider.clone(),
            thinking,
            thinking_ceiling: inputs.thinking_ceiling.map(str::to_string),
            tools: persona.tools.clone(),
            exclude_tools: persona.exclude_tools.clone(),
            allow_nested_subagents: persona.allow_nested_subagents,
            extensions: persona.extensions.clone(),
            subagent_only_extensions: persona.subagent_only_extensions.clone(),
            system_prompt: (!persona.system_prompt_body.is_empty())
                .then(|| persona.system_prompt_body.clone()),
            system_prompt_mode: persona.system_prompt_mode,
            inherit_project_context: persona.inherit_project_context,
            inherit_skills: persona.inherit_skills,
            skills,
            agent_file_path: persona.file_path.clone(),
            completion_guard: persona.completion_guard,
            memory: persona.memory.clone(),
            output_path: step.output_path.clone(),
            output_mode,
            structured_output_schema: step.structured_output_schema.clone(),
            acceptance: step.acceptance.clone(),
            control_config: config.control.clone(),
            context: step.context,
            lane: None,
            absolute_deadline_at: config.deadline_at_ms,
            initial_tool_budget: persona.tool_budget.clone(),
            max_subagent_depth,
            capability_ceiling: inputs.capability_ceiling.cloned(),
            share: config.share.unwrap_or(false),
            session_dir: step.session_dir.clone(),
            artifacts_dir: config.artifacts_dir.clone(),
            artifact_config: config.artifact_config,
            run_fanout_budget: None,
            turn_budget: config.turn_budget,
            usage_budget: config.usage_budget,
            permission_rules: config.permission_rules.clone(),
            include_progress: config.include_progress,
        })
    }

    /// Persist at `path` — pi `writePrivateAtomicJson` (`async-execution.ts:2051`): atomic, and
    /// `0600` before the rename, because the file carries the system prompt and every path the
    /// run touches.
    ///
    /// # Errors
    ///
    /// [`RecoveryDescriptorError::Persist`] — the caller FAILS THE LAUNCH on it (pi `:2053`).
    pub async fn write(&self, path: &Path) -> Result<(), RecoveryDescriptorError> {
        super::atomic::write_private_atomic_json(path, self)
            .await
            .map_err(|source| RecoveryDescriptorError::Persist {
                run_id: self.source_run_id.clone(),
                source,
            })
    }

    /// Read and validate the descriptor at `path` — pi `readAsyncRecoveryDescriptor`
    /// (`async-resume.ts:310-440`). `Ok(None)` iff the file is absent (`:313`).
    ///
    /// # Errors
    ///
    /// [`RecoveryDescriptorError::Parse`] for unreadable or non-JSON bytes,
    /// [`RecoveryDescriptorError::Invalid`] for a JSON document that is not a descriptor this
    /// build accepts (unknown key, other version, wrong shape) or fails [`Self::validate`].
    pub async fn read(path: &Path) -> Result<Option<Self>, RecoveryDescriptorError> {
        let bytes = match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(RecoveryDescriptorError::Parse {
                    path: path.to_path_buf(),
                    detail: error.to_string(),
                });
            }
        };
        let descriptor: Self = serde_json::from_slice(&bytes).map_err(|error| {
            let detail = error.to_string();
            match error.classify() {
                serde_json::error::Category::Data => RecoveryDescriptorError::Invalid {
                    path: path.to_path_buf(),
                    detail,
                },
                _ => RecoveryDescriptorError::Parse {
                    path: path.to_path_buf(),
                    detail,
                },
            }
        })?;
        descriptor.validate(path)?;
        Ok(Some(descriptor))
    }

    /// The shape checks serde cannot express — pi `async-resume.ts:334-438`, minus the ones the
    /// field types already make structural (`version`, booleans, integers, enums, the ceiling's
    /// own parser).
    ///
    /// # Errors
    ///
    /// [`RecoveryDescriptorError::Invalid`] naming the failed check.
    pub fn validate(&self, path: &Path) -> Result<(), RecoveryDescriptorError> {
        let invalid = |detail: String| RecoveryDescriptorError::Invalid {
            path: path.to_path_buf(),
            detail,
        };
        if self.agent.trim().is_empty() {
            return Err(invalid("agent must be a non-empty string.".to_string()));
        }
        if self.cwd.as_os_str().is_empty() {
            return Err(invalid("cwd must be a non-empty string.".to_string()));
        }
        // pi `:377`: `outputMode !== "inline" && outputMode !== "file-only"` throws — cyrup's enum
        // has a third value the async path never produces, so it is refused here rather than
        // silently carried.
        if self.output_mode == OutputMode::FileAndInline {
            return Err(invalid("outputMode is invalid.".to_string()));
        }
        if let Some(level) = &self.thinking_ceiling {
            crate::exec::thinking_ceiling::parse_thinking_level(Some(level), "thinkingCeiling")
                .map_err(invalid)?;
        }
        if let Some(ceiling) = &self.capability_ceiling
            && ceiling.version != CAPABILITY_CEILING_VERSION
        {
            return Err(invalid(format!(
                "capabilityCeiling version must be {CAPABILITY_CEILING_VERSION}."
            )));
        }
        if let Some(schema) = &self.structured_output_schema
            && !schema.is_object()
        {
            return Err(invalid(
                "structuredOutputSchema must be an object.".to_string(),
            ));
        }
        // pi `normalizeRecoveryAcceptance` (`:293-297`): the SAME validator the tool boundary
        // uses, so a descriptor can never smuggle in an acceptance shape the tool would refuse.
        if let Some(acceptance) = &self.acceptance {
            crate::exec::acceptance::lower_acceptance_input(acceptance).map_err(invalid)?;
        }
        match (self.model_origin, self.model_override_from_parent) {
            (ModelOrigin::Inherited, Some(true)) => {}
            (ModelOrigin::Inherited, _) => {
                return Err(invalid(
                    "modelOverrideFromParent must be true when modelOrigin is 'inherited'."
                        .to_string(),
                ));
            }
            (_, Some(true)) => {
                return Err(invalid(
                    "modelOverrideFromParent is only valid when modelOrigin is 'inherited'."
                        .to_string(),
                ));
            }
            _ => {}
        }
        if self.absolute_deadline_at == Some(0) {
            return Err(invalid(
                "absoluteDeadlineAt must be a positive timestamp.".to_string(),
            ));
        }
        let lists: [(&str, &[String]); 5] = [
            ("excludeTools", &self.exclude_tools),
            ("extensions", self.extensions.as_deref().unwrap_or_default()),
            ("subagentOnlyExtensions", &self.subagent_only_extensions),
            // SUBA-102 — pi `:372-375` checks `mutationTools` with the other lists.
            (
                "mutationTools",
                self.mutation_tools.as_deref().unwrap_or_default(),
            ),
            ("skills", &self.skills),
        ];
        for (field, entries) in lists {
            if entries.iter().any(|entry| entry.trim().is_empty()) {
                return Err(invalid(format!("{field} must contain non-empty strings.")));
            }
        }
        let strings: [(&str, Option<&str>); 4] = [
            ("model", self.model.as_ref().map(ModelId::as_str)),
            (
                "modelProvider",
                self.model_provider.as_ref().map(ProviderId::as_str),
            ),
            ("thinking", self.thinking.as_deref()),
            ("outputPath", self.output_path.as_deref()),
        ];
        for (field, value) in strings {
            if value.is_some_and(|value| value.trim().is_empty()) {
                return Err(invalid(format!("{field} must be a non-empty string.")));
            }
        }
        let paths: [(&str, Option<&Path>); 4] = [
            ("sessionFile", self.session_file.as_deref()),
            ("agentFilePath", self.agent_file_path.as_deref()),
            ("sessionDir", self.session_dir.as_deref()),
            ("artifactsDir", self.artifacts_dir.as_deref()),
        ];
        for (field, value) in paths {
            if value.is_some_and(|value| value.as_os_str().is_empty()) {
                return Err(invalid(format!("{field} must be a non-empty string.")));
            }
        }
        Ok(())
    }

    /// The two cross-checks a reader performs against the run it found the descriptor under —
    /// the source-run check (pi `subagent-executor.ts:1493`, `retained-children.ts:61`) and the
    /// agent-mismatch refusal (pi `async-resume.ts:566`).
    ///
    /// # Errors
    ///
    /// [`RecoveryDescriptorError::SourceRunMismatch`] / [`RecoveryDescriptorError::AgentMismatch`].
    pub fn assert_belongs_to(
        &self,
        status: &RunStatus,
        step_agent: &str,
    ) -> Result<(), RecoveryDescriptorError> {
        if self.source_run_id != status.run_id {
            return Err(RecoveryDescriptorError::SourceRunMismatch {
                run_id: status.run_id.clone(),
                found: self.source_run_id.clone(),
            });
        }
        if self.agent != step_agent {
            return Err(RecoveryDescriptorError::AgentMismatch {
                run_id: status.run_id.clone(),
                descriptor_agent: self.agent.clone(),
                step_agent: step_agent.to_string(),
            });
        }
        Ok(())
    }

    /// pi `applySteeringRecoveryAgentConfig` (`async-resume.ts:604-632`): the descriptor OVERLAYS
    /// the discovered persona field-for-field. The file on disk does not win — that is the whole
    /// point: a child launched with a narrowed tool set or a tighter depth resumes with exactly
    /// that set, not with whatever the persona file has been widened to since.
    ///
    /// Fields the descriptor does not carry (`fallback_models`, `default_context`, `runner`,
    /// `acceptance_role`, `default_acceptance`) keep the discovered value, exactly as pi's
    /// `...agentConfig` spread keeps them. `file_path` keeps the discovered path only when the
    /// descriptor recorded none.
    ///
    /// `output` also keeps the discovered value, and there this overlay departs from pi's, which
    /// writes `output: descriptor.outputPath` (`async-resume.ts:628`). pi's `agentConfig.output`
    /// is the string its launch folds into the resolved output path (`async-execution.ts:1833`),
    /// so overlaying the resolved path there is how pi hands it to the revived launch, which also
    /// receives it as its `output` param (`subagent-executor.ts:2173`).
    /// cyrup's launch-time fold in `spawn_background` (`normalize_single_output_override` →
    /// `resolve_single_output_path`, `background.rs` ~`:184-188`) reads `output` off the
    /// `AgentDefinition` that `resolve_agent` returns — NOT off the persona: the persona's copy
    /// (`ResolvedAgentPersona::output`, made at `agent_config.rs::from_agent_definition`) is
    /// written back by `to_agent_config` and read by nothing. A revive does not re-run the fold: the launch's already-resolved `outputPath` rides `SingleStepSpec::output_path`
    /// (`control.rs::revive_from_transcript`), the one slot the runner reads
    /// (`runner_main/executor.rs`, the file-output handoff). That is the equivalent place and the
    /// whole of pi's `:628` effect; writing an absolute path into the spec slot would change
    /// nothing the runner reads.
    pub fn apply_to_persona(&self, persona: &mut ResolvedAgentPersona) {
        persona.model = self.model.clone();
        persona.model_provider = self.model_provider.clone();
        persona.thinking = self.thinking.clone();
        persona.tools = self.tools.clone();
        persona.exclude_tools = self.exclude_tools.clone();
        persona.allow_nested_subagents = self.allow_nested_subagents;
        persona.extensions = self.extensions.clone();
        persona.subagent_only_extensions = self.subagent_only_extensions.clone();
        // pi `systemPrompt: descriptor.systemPrompt ?? agentConfig.systemPrompt` (`:618`).
        if let Some(prompt) = &self.system_prompt {
            persona.system_prompt_body = prompt.clone();
        }
        persona.system_prompt_mode = self.system_prompt_mode;
        persona.inherit_project_context = self.inherit_project_context;
        // SUBA-101 — pi `inheritGlobalContext: descriptor.inheritGlobalContext` (`:621`), where
        // the reader already defaulted a missing value to `inheritProjectContext` (`:368`).
        persona.inherit_global_context = self.effective_inherit_global_context();
        // SUBA-102 — pi `mutationTools: descriptor.mutationTools ? [...] : undefined` (`:617`).
        persona.mutation_tools = self.mutation_tools.clone();
        persona.inherit_skills = self.inherit_skills;
        persona.skills = self.skills.clone();
        persona.file_path = self
            .agent_file_path
            .clone()
            .or_else(|| persona.file_path.take());
        persona.completion_guard = self.completion_guard;
        persona.memory = self.memory.clone();
        persona.tool_budget = self.initial_tool_budget.clone();
        persona.max_subagent_depth = Some(self.max_subagent_depth);
    }

    /// SUBA-101 — pi `readAsyncRecoveryDescriptor`'s legacy default (`async-resume.ts:368`
    /// @v0.68.0): `if (parsed.inheritGlobalContext === undefined) parsed.inheritGlobalContext =
    /// parsed.inheritProjectContext;` — a descriptor written before the field existed resumes with
    /// global context exactly when it resumed with project context.
    #[must_use]
    pub fn effective_inherit_global_context(&self) -> bool {
        self.inherit_global_context
            .unwrap_or(self.inherit_project_context)
    }

    /// pi `subagent-executor.ts:1894-1905`: when discovery no longer finds the agent, a base
    /// persona is synthesised from the descriptor (`description: "Persisted async recovery
    /// contract"`, empty prompt, the prompt-mode/inherit flags, `filePath: descriptor.agentFilePath
    /// ?? <project agents dir>/recovery-agent`) — and then [`Self::apply_to_persona`] overlays the
    /// rest, exactly as it would over a discovered one.
    #[must_use]
    pub fn synthesised_persona(&self) -> ResolvedAgentPersona {
        let file_path = self.agent_file_path.clone().unwrap_or_else(|| {
            crate::discovery::resolve_project_agent_read_dirs(&self.cwd)
                .pop()
                .unwrap_or_else(|| self.cwd.join(".cyrup").join("agents"))
                .join("recovery-agent")
        });
        let mut persona = ResolvedAgentPersona {
            // pi `:1900`: `inheritGlobalContext: recoveryDescriptor.inheritGlobalContext`.
            inherit_global_context: self.effective_inherit_global_context(),
            machine: None,
            mutation_tools: None,
            name: self.agent.clone(),
            model: None,
            model_provider: None,
            fallback_models: Vec::new(),
            thinking: None,
            system_prompt_mode: self.system_prompt_mode,
            system_prompt_body: String::new(),
            tools: None,
            exclude_tools: Vec::new(),
            allow_nested_subagents: None,
            extensions: None,
            subagent_only_extensions: Vec::new(),
            output: None,
            inherit_project_context: self.inherit_project_context,
            inherit_skills: self.inherit_skills,
            skills: Vec::new(),
            completion_guard: None,
            max_subagent_depth: None,
            default_context: None,
            memory: None,
            tool_budget: None,
            runner: None,
            acceptance_role: None,
            default_acceptance: None,
            file_path: Some(file_path),
        };
        self.apply_to_persona(&mut persona);
        persona
    }
}

// =================================================================================================
// Launch-binding digest (evidence)
// =================================================================================================

/// The resolved inputs pi's `projectLaunchBinding` digests (`launch-contract.ts:111-135`),
/// restricted to the fields cyrup has.
struct LaunchBinding<'a> {
    step: &'a SingleStepSpec,
    persona: &'a ResolvedAgentPersona,
    model: Option<&'a ModelId>,
    thinking: Option<&'a str>,
    skills: &'a [String],
    output_mode: OutputMode,
}

/// pi `projectAgentDefinition` (`launch-contract.ts:37-84`): the parsed definition plus its file
/// path and a digest of the file's bytes when readable.
fn definition_digest(persona: &ResolvedAgentPersona) -> String {
    let file_content_digest = persona
        .file_path
        .as_deref()
        .and_then(|path| std::fs::read(path).ok())
        .map(|bytes| sha256_hex(&bytes));
    let mut projection = serde_json::Map::new();
    projection.insert("version".into(), serde_json::json!(1));
    projection.insert(
        "definition".into(),
        serde_json::to_value(persona).unwrap_or(serde_json::Value::Null),
    );
    if let Some(path) = &persona.file_path {
        projection.insert(
            "filePath".into(),
            serde_json::Value::String(path.display().to_string()),
        );
    }
    if let Some(digest) = file_content_digest {
        projection.insert(
            "fileContentDigest".into(),
            serde_json::Value::String(digest),
        );
    }
    crate::workflows::stable_json_digest(&serde_json::Value::Object(projection))
}

/// pi `launchBindingDigest(projectLaunchBinding(input))` (`launch-contract.ts:137-139`), with
/// `undefined` keys omitted exactly as `stableJson` omits them.
fn launch_binding_digest(binding: LaunchBinding<'_>) -> LaunchContractDigest {
    let digest = crate::workflows::stable_json_digest(&serde_json::Value::Object(
        launch_binding_projection(binding),
    ));
    // `stable_json_digest` is 64 lower-hex by construction; the fallback below is unreachable
    // and exists only so this function is total without an `expect`.
    LaunchContractDigest::parse(&digest)
        .unwrap_or_else(|_| LaunchContractDigest(sha256_hex(digest.as_bytes())))
}

/// pi `projectLaunchBinding` (`launch-contract.ts:111-135` @v0.68.0): the canonical projection
/// [`launch_binding_digest`] hashes, split out so what it BINDS is assertable key by key — a
/// digest-changes test alone cannot tell a key bound here from one that only moves the
/// `definitionDigest` (the persona is serialized into that too).
fn launch_binding_projection(
    binding: LaunchBinding<'_>,
) -> serde_json::Map<String, serde_json::Value> {
    use serde_json::Value;
    let LaunchBinding {
        step,
        persona,
        model,
        thinking,
        skills,
        output_mode,
    } = binding;
    let mut projection = serde_json::Map::new();
    let mut put = |key: &str, value: Value| {
        projection.insert(key.to_string(), value);
    };
    put("version", serde_json::json!(2));
    put(
        "definitionDigest",
        Value::String(definition_digest(persona)),
    );
    put(
        "taskDigest",
        Value::String(crate::workflows::stable_json_digest(&Value::String(
            step.task.clone(),
        ))),
    );
    if let Some(model) = model {
        put("model", Value::String(model.as_str().to_string()));
    }
    if let Some(thinking) = thinking {
        put("thinking", Value::String(thinking.to_string()));
    }
    put(
        "systemPromptDigest",
        Value::String(crate::workflows::stable_json_digest(&Value::String(
            persona.system_prompt_body.clone(),
        ))),
    );
    put(
        "systemPromptMode",
        serde_json::to_value(persona.system_prompt_mode).unwrap_or(Value::Null),
    );
    put(
        "inheritProjectContext",
        Value::Bool(persona.inherit_project_context),
    );
    // SUBA-101 — pi binds `inheritGlobalContext` beside the project flag (`launch-contract.ts:122`).
    put(
        "inheritGlobalContext",
        Value::Bool(persona.inherit_global_context),
    );
    put("inheritSkills", Value::Bool(persona.inherit_skills));
    // SUBA-103 — pi binds the launch's `fast` (`launch-contract.ts:117`), omitted when undefined.
    if let Some(fast) = step.fast {
        put("fast", Value::Bool(fast));
    }
    if !skills.is_empty() {
        put("skills", serde_json::json!(skills));
    }
    if let Some(tools) = &persona.tools {
        put(
            "tools",
            Value::Array(tools.iter().map(tool_ref_as_pi_json).collect()),
        );
    }
    if !persona.exclude_tools.is_empty() {
        put("excludeTools", serde_json::json!(persona.exclude_tools));
    }
    if let Some(extensions) = &persona.extensions {
        put("extensions", serde_json::json!(extensions));
    }
    if !persona.subagent_only_extensions.is_empty() {
        put(
            "subagentOnlyExtensions",
            serde_json::json!(persona.subagent_only_extensions),
        );
    }
    if let Some(output_path) = &step.output_path {
        put("outputPath", Value::String(output_path.clone()));
    }
    put(
        "outputMode",
        serde_json::to_value(output_mode).unwrap_or(Value::Null),
    );
    if let Some(schema) = &step.structured_output_schema {
        put("structuredOutputSchema", schema.clone());
    }
    projection
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use std::collections::BTreeMap;

    use super::*;
    use crate::discovery::types::{MemoryScope, ToolBudgetBlock};
    use crate::exec::usage_budget::UsageBudgetLimit;
    use crate::watchdog::permission_arbiter::PermissionRuleDecision;

    /// A persona with every descriptor-relevant field set to a distinctive value, so a dropped
    /// projection in [`RecoveryDescriptor::for_single_launch`] shows up as a wrong value rather
    /// than a matching default.
    fn distinctive_persona() -> ResolvedAgentPersona {
        ResolvedAgentPersona {
            // SUBA-101: `true` against `inherit_project_context: false`, so the legacy default
            // (`None` -> the project flag) is distinguishable from the recorded value.
            inherit_global_context: true,
            machine: None,
            mutation_tools: Some(vec!["apply_patch".to_string()]),
            name: "worker".to_string(),
            model: Some(ModelId::from("fixture/persona-model")),
            model_provider: Some(ProviderId::from("fixture")),
            fallback_models: vec![ModelId::from("fixture/fallback")],
            thinking: Some("low".to_string()),
            system_prompt_mode: SystemPromptMode::Replace,
            system_prompt_body: "Launch body.".to_string(),
            tools: Some(vec![
                ToolRef::Builtin("read".to_string()),
                ToolRef::Mcp("fs.list".to_string()),
                ToolRef::ExtensionPath("./ext-tool".to_string()),
            ]),
            exclude_tools: vec!["bash".to_string()],
            allow_nested_subagents: Some(true),
            extensions: Some(vec!["ext-a".to_string()]),
            subagent_only_extensions: vec!["./child.ts".to_string()],
            output: None,
            inherit_project_context: false,
            inherit_skills: false,
            skills: vec!["alpha".to_string()],
            completion_guard: Some(false),
            max_subagent_depth: Some(2),
            default_context: None,
            memory: Some(AgentMemoryConfig {
                scope: MemoryScope::Project,
                path: "notes.md".to_string(),
            }),
            tool_budget: Some(ResolvedToolBudget {
                hard: 5,
                soft: None,
                block: ToolBudgetBlock::Names(vec!["read".to_string()]),
            }),
            runner: None,
            acceptance_role: None,
            default_acceptance: None,
            file_path: Some(PathBuf::from("/agents/worker.md")),
        }
    }

    fn distinctive_step() -> SingleStepSpec {
        SingleStepSpec {
            machine: None,
            agent: "worker".to_string(),
            task: "do the thing".to_string(),
            cwd: None,
            model: Some(ModelId::from("anthropic/override")),
            tools: None,
            extensions: None,
            session_file: Some(PathBuf::from("/sessions/fork.jsonl")),
            max_depth_override: None,
            structured_output_schema: Some(serde_json::json!({"type": "object"})),
            output: None,
            output_path: Some("/out/out.md".to_string()),
            output_mode: Some(OutputMode::FileOnly),
            fast: Some(true),
            reads: None,
            acceptance: Some(serde_json::json!({"level": "checked"})),
            skills: Some(vec!["beta".to_string()]),
            session_dir: Some(PathBuf::from("/sessions/run-0")),
            context: Some(ContextMode::Fresh),
            agent_scope: None,
        }
    }

    fn runner_config(step: SingleStepSpec, persona: ResolvedAgentPersona) -> RunnerConfig {
        RunnerConfig {
            runner_process_instance_id: None,
            revival_lease: None,
            run_id: RunId::from_token("run-launch".to_string()),
            mode: RunMode::Single,
            steps: vec![RunnerStep::SingleStep(step)],
            cwd: PathBuf::from("/work"),
            session_file: Some(PathBuf::from("/sessions/fork.jsonl")),
            session_id: Some("session-1".to_string()),
            completion_owner_id: None,
            global_concurrency_limit: 4,
            worktree_base_dir: None,
            max_subagent_depth: 4,
            async_root: PathBuf::from("/async"),
            results_dir: PathBuf::from("/results"),
            resolved_agents: BTreeMap::from([("worker".to_string(), persona)]),
            original_task: "do the thing".to_string(),
            chain_dir: None,
            orchestrator_intercom_target: None,
            inherited_session_model: Some(ModelId::from("anthropic/session-model")),
            inherited_session_thinking: Some("medium".to_string()),
            host_available_builtins: None,
            turn_budget: Some(ResolvedTurnBudget {
                max_turns: 9,
                grace_turns: 1,
            }),
            permission_rules: Some(BTreeMap::from([(
                "write".to_string(),
                PermissionRuleDecision::Deny,
            )])),
            usage_budget: Some(UsageBudgetConfig {
                tokens: Some(UsageBudgetLimit {
                    soft: None,
                    hard: 1000.0,
                }),
                cost_usd: None,
            }),
            model_scope: None,
            nested_route: None,
            nested_self: None,
            dynamic_fanout_max_items: None,
            control: Some(ResolvedControlConfig {
                needs_attention_after_ms: 1234,
                ..ResolvedControlConfig::default()
            }),
            include_progress: Some(true),
            timeout_ms: Some(60_000),
            deadline_at_ms: Some(1_900_000_060_000),
            share: Some(true),
            artifacts_dir: Some(PathBuf::from("/artifacts")),
            artifact_config: ArtifactConfig {
                enabled: false,
                ..ArtifactConfig::default()
            },
        }
    }

    fn ceiling() -> ResolvedCapabilityCeiling {
        ResolvedCapabilityCeiling {
            version: CAPABILITY_CEILING_VERSION,
            allowed_tools: None,
            allowed_agents: Some(vec!["worker".to_string()]),
            deny_extensions: true,
            sources: vec!["policy".to_string()],
        }
    }

    /// Every field the writer projects, checked one by one against a persona/step/config whose
    /// values all differ from the defaults — dropping any one projection fails its own line.
    #[test]
    fn for_single_launch_projects_every_field_of_the_resolved_launch() {
        let config = runner_config(distinctive_step(), distinctive_persona());
        let ceiling = ceiling();
        let d = RecoveryDescriptor::for_single_launch(
            &config,
            LaunchInputs {
                thinking_ceiling: Some("low"),
                capability_ceiling: Some(&ceiling),
                stored_model_origin: None,
            },
        )
        .expect("a single-step Single run writes a descriptor");

        assert_eq!(d.version, DescriptorVersion);
        assert_eq!(d.launch_contract_digest.as_str().len(), 64);
        assert_eq!(d.source_run_id.as_str(), "run-launch");
        assert_eq!(d.agent, "worker");
        assert_eq!(d.cwd, PathBuf::from("/work"));
        assert_eq!(d.session_file, Some(PathBuf::from("/sessions/fork.jsonl")));
        assert_eq!(
            d.model.as_ref().map(ModelId::as_str),
            Some("anthropic/override")
        );
        assert_eq!(d.model_origin, ModelOrigin::Explicit);
        assert_eq!(d.model_override_from_parent, None);
        assert_eq!(
            d.model_provider.as_ref().map(ProviderId::as_str),
            Some("fixture")
        );
        assert_eq!(d.thinking.as_deref(), Some("low"));
        assert_eq!(d.thinking_ceiling.as_deref(), Some("low"));
        assert_eq!(d.tools, distinctive_persona().tools);
        assert_eq!(d.exclude_tools, vec!["bash".to_string()]);
        assert_eq!(d.allow_nested_subagents, Some(true));
        assert_eq!(d.extensions, Some(vec!["ext-a".to_string()]));
        assert_eq!(d.subagent_only_extensions, vec!["./child.ts".to_string()]);
        assert_eq!(d.system_prompt.as_deref(), Some("Launch body."));
        assert_eq!(d.system_prompt_mode, SystemPromptMode::Replace);
        assert!(!d.inherit_project_context);
        assert!(!d.inherit_skills);
        // SUBA-101/102/103.
        assert_eq!(d.inherit_global_context, Some(true));
        assert_eq!(d.mutation_tools, Some(vec!["apply_patch".to_string()]));
        assert_eq!(d.fast, Some(true), "the step's EFFECTIVE fast");
        assert_eq!(
            d.skills,
            vec!["beta".to_string()],
            "the per-call override wins"
        );
        assert_eq!(d.agent_file_path, Some(PathBuf::from("/agents/worker.md")));
        assert_eq!(d.completion_guard, Some(false));
        assert_eq!(d.memory, distinctive_persona().memory);
        assert_eq!(d.output_path.as_deref(), Some("/out/out.md"));
        assert_eq!(d.output_mode, OutputMode::FileOnly);
        assert_eq!(
            d.structured_output_schema,
            Some(serde_json::json!({"type": "object"}))
        );
        assert_eq!(d.acceptance, Some(serde_json::json!({"level": "checked"})));
        assert_eq!(
            d.control_config
                .as_ref()
                .map(|c| c.needs_attention_after_ms),
            Some(1234)
        );
        assert_eq!(d.context, Some(ContextMode::Fresh));
        assert_eq!(d.lane, None);
        assert_eq!(d.absolute_deadline_at, Some(1_900_000_060_000));
        assert_eq!(d.initial_tool_budget, distinctive_persona().tool_budget);
        assert_eq!(d.max_subagent_depth, 2, "min(config 4, persona 2)");
        assert_eq!(d.capability_ceiling, Some(ceiling));
        assert!(d.share);
        assert_eq!(d.session_dir, Some(PathBuf::from("/sessions/run-0")));
        assert_eq!(d.artifacts_dir, Some(PathBuf::from("/artifacts")));
        assert!(!d.artifact_config.enabled);
        assert_eq!(d.run_fanout_budget, None);
        assert_eq!(
            d.turn_budget,
            Some(ResolvedTurnBudget {
                max_turns: 9,
                grace_turns: 1
            })
        );
        assert_eq!(
            d.usage_budget.and_then(|b| b.tokens).map(|t| t.hard),
            Some(1000.0)
        );
        assert_eq!(
            d.permission_rules.as_ref().and_then(|r| r.get("write")),
            Some(&PermissionRuleDecision::Deny)
        );
        assert_eq!(d.include_progress, Some(true));
    }

    /// pi `resolveModelOrigin`: the three origins, each with the model it selects.
    #[test]
    fn model_origin_follows_pi_s_three_rungs() {
        let mut step = distinctive_step();
        let mut persona = distinctive_persona();
        let config = runner_config(step.clone(), persona.clone());
        let none = LaunchInputs::default();

        let explicit = RecoveryDescriptor::for_single_launch(&config, none).unwrap();
        assert_eq!(explicit.model_origin, ModelOrigin::Explicit);

        step.model = None;
        let config = runner_config(step.clone(), persona.clone());
        let configured = RecoveryDescriptor::for_single_launch(&config, none).unwrap();
        assert_eq!(configured.model_origin, ModelOrigin::Configured);
        assert_eq!(
            configured.model.as_ref().map(ModelId::as_str),
            Some("fixture/persona-model")
        );
        assert_eq!(configured.model_override_from_parent, None);

        persona.model = None;
        let config = runner_config(step.clone(), persona.clone());
        let inherited = RecoveryDescriptor::for_single_launch(&config, none).unwrap();
        assert_eq!(inherited.model_origin, ModelOrigin::Inherited);
        assert_eq!(
            inherited.model.as_ref().map(ModelId::as_str),
            Some("anthropic/session-model")
        );
        assert_eq!(inherited.model_override_from_parent, Some(true));

        let mut headless = runner_config(step, persona);
        headless.inherited_session_model = None;
        headless.inherited_session_thinking = None;
        let bare = RecoveryDescriptor::for_single_launch(&headless, none).unwrap();
        assert_eq!(bare.model_origin, ModelOrigin::Configured);
        assert_eq!(bare.model, None);
        assert_eq!(
            bare.thinking.as_deref(),
            Some("low"),
            "the persona's own level"
        );
    }

    /// pi `resolveModelOrigin`'s first rung (`model-resolution.ts:382`): a stored origin wins over
    /// re-derivation. On a revive the overlay has pinned an inherited model onto the persona, so
    /// re-derivation over that persona says `configured`; the stored `inherited` (and the
    /// `modelOverrideFromParent` that goes with it) is what the revived descriptor records, the
    /// model being identical either way.
    #[test]
    fn a_stored_origin_wins_over_re_derivation() {
        let mut step = distinctive_step();
        step.model = None;
        let mut persona = distinctive_persona();
        persona.model = Some(ModelId::from("anthropic/session-model"));
        let config = runner_config(step, persona);

        let derived =
            RecoveryDescriptor::for_single_launch(&config, LaunchInputs::default()).unwrap();
        assert_eq!(
            derived.model_origin,
            ModelOrigin::Configured,
            "over the pinned persona a fresh derivation cannot tell an inherited model apart"
        );
        assert_eq!(derived.model_override_from_parent, None);

        let stored = RecoveryDescriptor::for_single_launch(
            &config,
            LaunchInputs {
                stored_model_origin: Some(ModelOrigin::Inherited),
                ..LaunchInputs::default()
            },
        )
        .unwrap();
        assert_eq!(stored.model_origin, ModelOrigin::Inherited);
        assert_eq!(stored.model_override_from_parent, Some(true));
        assert_eq!(
            stored.model, derived.model,
            "the model itself is the same either way"
        );
        stored
            .validate(Path::new("/stored"))
            .expect("origin and modelOverrideFromParent agree");
    }

    /// The session rungs fill in where the persona says nothing — model AND thinking.
    #[test]
    fn session_rungs_fill_an_undeclared_persona() {
        let mut persona = distinctive_persona();
        persona.thinking = None;
        persona.skills = vec!["gamma".to_string()];
        let mut step = distinctive_step();
        step.skills = None;
        let config = runner_config(step, persona);
        let d = RecoveryDescriptor::for_single_launch(&config, LaunchInputs::default()).unwrap();
        assert_eq!(d.thinking.as_deref(), Some("medium"));
        assert_eq!(
            d.skills,
            vec!["gamma".to_string()],
            "no override: the persona's list"
        );
    }

    /// pi writes a descriptor only from `executeAsyncSingle`.
    #[test]
    fn only_a_single_step_single_run_gets_a_descriptor() {
        let mut chain = runner_config(distinctive_step(), distinctive_persona());
        chain.mode = RunMode::Chain;
        assert!(RecoveryDescriptor::for_single_launch(&chain, LaunchInputs::default()).is_none());

        let mut two = runner_config(distinctive_step(), distinctive_persona());
        two.steps.push(RunnerStep::SingleStep(distinctive_step()));
        assert!(RecoveryDescriptor::for_single_launch(&two, LaunchInputs::default()).is_none());

        let mut orphan = runner_config(distinctive_step(), distinctive_persona());
        orphan.resolved_agents.clear();
        assert!(RecoveryDescriptor::for_single_launch(&orphan, LaunchInputs::default()).is_none());
    }

    /// The on-disk form is pi's: camelCase keys, `version: 1`, tools as strings, optionals
    /// omitted — and it reads back equal.
    #[tokio::test]
    async fn the_on_disk_form_is_pi_s_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery-descriptor.json");
        let ceiling = ceiling();
        let config = runner_config(distinctive_step(), distinctive_persona());
        let written = RecoveryDescriptor::for_single_launch(
            &config,
            LaunchInputs {
                thinking_ceiling: Some("low"),
                capability_ceiling: Some(&ceiling),
                stored_model_origin: None,
            },
        )
        .unwrap();
        written.write(&path).await.unwrap();

        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["version"], 1);
        assert_eq!(raw["sourceRunId"], "run-launch");
        assert_eq!(
            raw["tools"],
            serde_json::json!([
                "read",
                "mcp:fs.list",
                {"kind": "extensionPath", "content": "./ext-tool"}
            ]),
            "pi's string spelling for builtin/mcp; the tagged form only where pi has none"
        );
        assert_eq!(raw["modelOrigin"], "explicit");
        assert_eq!(raw["outputMode"], "file-only");
        assert_eq!(raw["systemPromptMode"], "replace");
        assert_eq!(
            raw["memory"],
            serde_json::json!({"scope": "project", "path": "notes.md"})
        );
        assert_eq!(raw["maxSubagentDepth"], 2);
        assert_eq!(raw["share"], true);
        assert!(
            raw.get("modelOverrideFromParent").is_none(),
            "written only when inherited"
        );
        assert!(raw.get("lane").is_none());
        assert!(raw.get("runFanoutBudget").is_none());
        assert_eq!(raw["thinkingCeiling"], "low");
        assert_eq!(
            raw["capabilityCeiling"]["allowedAgents"],
            serde_json::json!(["worker"])
        );
        assert_eq!(
            raw["turnBudget"],
            serde_json::json!({"maxTurns": 9, "graceTurns": 1})
        );
        assert_eq!(raw["permissionRules"], serde_json::json!({"write": "deny"}));
        // SUBA-101/102/103 — pi's key spellings (`async-resume.ts:323-324` @v0.68.0).
        assert_eq!(raw["inheritGlobalContext"], true);
        assert_eq!(raw["mutationTools"], serde_json::json!(["apply_patch"]));
        assert_eq!(raw["fast"], true);

        let read = RecoveryDescriptor::read(&path).await.unwrap().unwrap();
        assert_eq!(read, written);
    }

    /// SUBA-101 — pi's legacy default (`async-resume.ts:368` @v0.68.0): a descriptor written
    /// before `inheritGlobalContext` existed reads back with the PROJECT flag in its place, on
    /// both reader paths (the overlay and the synthesised persona); a recorded value wins over it.
    /// SUBA-102/103 — `mutationTools` entries must be non-empty strings (`:372-375`) and `fast`
    /// a boolean (`:362`). Mutation killed: `effective_inherit_global_context` returning a
    /// constant, dropping the `mutationTools` row from `validate`, or `apply_to_persona` not
    /// copying `mutation_tools`.
    #[tokio::test]
    async fn a_pre_field_descriptor_takes_the_project_flag_and_the_new_fields_are_validated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery-descriptor.json");
        let config = runner_config(distinctive_step(), distinctive_persona());
        let written =
            RecoveryDescriptor::for_single_launch(&config, LaunchInputs::default()).unwrap();
        let mut legacy = serde_json::to_value(&written).unwrap();
        let object = legacy.as_object_mut().unwrap();
        object.remove("inheritGlobalContext");
        object.remove("mutationTools");
        object.remove("fast");
        for project in [true, false] {
            object.insert("inheritProjectContext".into(), serde_json::json!(project));
            std::fs::write(&path, serde_json::to_vec(&*object).unwrap()).unwrap();
            let read = RecoveryDescriptor::read(&path).await.unwrap().unwrap();
            assert_eq!(read.inherit_global_context, None);
            assert_eq!(read.fast, None);
            assert_eq!(read.effective_inherit_global_context(), project);
            let mut persona = distinctive_persona();
            persona.inherit_global_context = !project;
            read.apply_to_persona(&mut persona);
            assert_eq!(persona.inherit_global_context, project, "overlay, legacy");
            assert_eq!(
                persona.mutation_tools, None,
                "the descriptor's absence wins"
            );
            assert_eq!(
                read.synthesised_persona().inherit_global_context,
                project,
                "synthesised, legacy"
            );
        }

        // A recorded value wins over the project flag, on both reader paths.
        written.write(&path).await.unwrap();
        let read = RecoveryDescriptor::read(&path).await.unwrap().unwrap();
        assert!(!read.inherit_project_context);
        assert!(read.effective_inherit_global_context());
        let mut persona = distinctive_persona();
        persona.inherit_global_context = false;
        persona.mutation_tools = None;
        read.apply_to_persona(&mut persona);
        assert!(persona.inherit_global_context);
        assert_eq!(
            persona.mutation_tools,
            Some(vec!["apply_patch".to_string()])
        );
        let synthesised = read.synthesised_persona();
        assert!(synthesised.inherit_global_context);
        assert_eq!(
            synthesised.mutation_tools,
            Some(vec!["apply_patch".to_string()])
        );

        let base = serde_json::to_value(&written).unwrap();
        for (key, bad, needle) in [
            (
                "mutationTools",
                serde_json::json!(["apply_patch", "  "]),
                "mutationTools must contain non-empty strings.",
            ),
            // The two booleans are refused structurally by the field type (this reader's
            // documented split with serde), so the detail is serde's, not pi's field sentence.
            ("fast", serde_json::json!("yes"), "expected a boolean"),
            (
                "inheritGlobalContext",
                serde_json::json!(1),
                "expected a boolean",
            ),
        ] {
            let mut doc = base.clone();
            doc[key] = bad;
            std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
            let err = RecoveryDescriptor::read(&path)
                .await
                .expect_err("refused")
                .to_string();
            assert!(
                err.starts_with("Invalid async recovery descriptor") && err.contains(needle),
                "{key}: {err}"
            );
        }
    }

    #[tokio::test]
    async fn an_absent_file_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let read = RecoveryDescriptor::read(&dir.path().join("recovery-descriptor.json"))
            .await
            .unwrap();
        assert!(read.is_none());
    }

    /// pi's reader refusals, each through the real `read`.
    #[tokio::test]
    async fn the_reader_refuses_what_pi_s_reader_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery-descriptor.json");
        let config = runner_config(distinctive_step(), distinctive_persona());
        let good = RecoveryDescriptor::for_single_launch(&config, LaunchInputs::default()).unwrap();
        let base = serde_json::to_value(&good).unwrap();

        let refuse = |mutate: &dyn Fn(&mut serde_json::Value)| {
            let mut doc = base.clone();
            mutate(&mut doc);
            let path = path.clone();
            async move {
                std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
                RecoveryDescriptor::read(&path)
                    .await
                    .expect_err("must refuse")
                    .to_string()
            }
        };

        let version = refuse(&|doc| doc["version"] = serde_json::json!(2)).await;
        assert!(version.contains("version 2"), "{version}");
        assert!(
            version.starts_with("Invalid async recovery descriptor"),
            "{version}"
        );

        let unknown = refuse(&|doc| doc["bogus"] = serde_json::json!(1)).await;
        assert!(unknown.contains("bogus"), "{unknown}");

        let mode = refuse(&|doc| doc["outputMode"] = serde_json::json!("file-and-inline")).await;
        assert!(mode.contains("outputMode is invalid."), "{mode}");

        let schema = refuse(&|doc| doc["structuredOutputSchema"] = serde_json::json!([1])).await;
        assert!(
            schema.contains("structuredOutputSchema must be an object."),
            "{schema}"
        );

        let ceiling = refuse(&|doc| doc["thinkingCeiling"] = serde_json::json!("ultra")).await;
        assert!(ceiling.contains("thinkingCeiling"), "{ceiling}");

        let acceptance =
            refuse(&|doc| doc["acceptance"] = serde_json::json!({"level": "bogus"})).await;
        assert!(
            acceptance.starts_with("Invalid async recovery descriptor"),
            "{acceptance}"
        );

        let origin = refuse(&|doc| doc["modelOverrideFromParent"] = serde_json::json!(true)).await;
        assert!(origin.contains("modelOverrideFromParent"), "{origin}");

        let empty_agent = refuse(&|doc| doc["agent"] = serde_json::json!("  ")).await;
        assert!(
            empty_agent.contains("agent must be a non-empty string."),
            "{empty_agent}"
        );

        std::fs::write(&path, b"{ not json").unwrap();
        let parse = RecoveryDescriptor::read(&path).await.expect_err("not JSON");
        assert!(
            matches!(parse, RecoveryDescriptorError::Parse { .. }),
            "{parse}"
        );
        assert!(
            parse
                .to_string()
                .starts_with("Failed to parse async recovery descriptor")
        );
    }

    #[test]
    fn the_digest_newtype_accepts_only_64_hex() {
        assert!(LaunchContractDigest::parse("abc").is_err());
        let hex = "A".repeat(64);
        let parsed = LaunchContractDigest::parse(&hex).unwrap();
        assert_eq!(parsed.as_str(), "a".repeat(64), "stored lower-cased");
        assert_eq!(
            serde_json::from_str::<LaunchContractDigest>("\"zz\"")
                .unwrap_err()
                .to_string(),
            "launchContractDigest must be a full 64-character SHA-256 digest."
        );
    }

    /// The digest is deterministic for the same launch and differs when only the task differs.
    #[test]
    fn the_launch_digest_binds_the_task() {
        let a = runner_config(distinctive_step(), distinctive_persona());
        let mut other_task = distinctive_step();
        other_task.task = "do a different thing".to_string();
        let b = runner_config(other_task, distinctive_persona());
        let none = LaunchInputs::default();
        let da = RecoveryDescriptor::for_single_launch(&a, none).unwrap();
        let da2 = RecoveryDescriptor::for_single_launch(&a, none).unwrap();
        let db = RecoveryDescriptor::for_single_launch(&b, none).unwrap();
        assert_eq!(da.launch_contract_digest, da2.launch_contract_digest);
        assert_ne!(da.launch_contract_digest, db.launch_contract_digest);
    }

    /// SUBA-101/102/103 — the launch binding binds all three keys, as pi's does:
    /// `inheritGlobalContext` and `fast` directly (`projectLaunchBinding`, `launch-contract.ts:117,
    /// 122` @v0.68.0; `fast` omitted when unset, as `stableJson` drops `undefined`), and
    /// `mutationTools` through the definition digest (`projectAgentDefinition`, `:62`). Asserted
    /// on the PROJECTION key by key and on the digest end to end: the persona is serialized into
    /// `definitionDigest`, so a digest-only test would stay green with the direct
    /// `inheritGlobalContext` key gutted.
    ///
    /// *Gutted by*, each alone: dropping the `put("inheritGlobalContext", …)` (the key is missing);
    /// dropping the `if let Some(fast) = step.fast { put("fast", …) }` (the key is missing and the
    /// fast digest equals the base); `#[serde(skip)]` on `ResolvedAgentPersona::mutation_tools`
    /// (the definition digest no longer moves).
    #[test]
    fn the_launch_digest_binds_global_context_mutation_tools_and_fast() {
        let project = |step: &SingleStepSpec, persona: &ResolvedAgentPersona| {
            launch_binding_projection(LaunchBinding {
                step,
                persona,
                model: None,
                thinking: None,
                skills: &[],
                output_mode: OutputMode::Inline,
            })
        };
        let digest = |step: &SingleStepSpec, persona: &ResolvedAgentPersona| {
            launch_binding_digest(LaunchBinding {
                step,
                persona,
                model: None,
                thinking: None,
                skills: &[],
                output_mode: OutputMode::Inline,
            })
        };
        let mut step = distinctive_step();
        step.fast = None;
        let mut persona = distinctive_persona();
        persona.inherit_global_context = false;
        persona.mutation_tools = None;
        let base = project(&step, &persona);
        assert_eq!(
            base.get("inheritGlobalContext"),
            Some(&serde_json::json!(false))
        );
        assert_eq!(base.get("fast"), None, "an unset fast is omitted");
        let base_digest = digest(&step, &persona);

        let mut global = persona.clone();
        global.inherit_global_context = true;
        assert_eq!(
            project(&step, &global).get("inheritGlobalContext"),
            Some(&serde_json::json!(true))
        );
        assert_ne!(digest(&step, &global), base_digest);

        let mut fast = step.clone();
        fast.fast = Some(true);
        assert_eq!(
            project(&fast, &persona).get("fast"),
            Some(&serde_json::json!(true))
        );
        assert_ne!(digest(&fast, &persona), base_digest);
        fast.fast = Some(false);
        assert_eq!(
            project(&fast, &persona).get("fast"),
            Some(&serde_json::json!(false)),
            "a stated `false` is bound too"
        );

        let mut mutating = persona.clone();
        mutating.mutation_tools = Some(vec!["apply_patch".to_string()]);
        assert_ne!(
            project(&step, &mutating).get("definitionDigest"),
            base.get("definitionDigest"),
            "mutationTools moves the definition digest"
        );
        assert_ne!(digest(&step, &mutating), base_digest);
    }

    /// The two cross-checks, with pi's sentences.
    #[test]
    fn belongs_to_refuses_another_run_or_another_agent() {
        let config = runner_config(distinctive_step(), distinctive_persona());
        let d = RecoveryDescriptor::for_single_launch(&config, LaunchInputs::default()).unwrap();
        let mut status = RunStatus::queued(
            RunId::from_token("run-launch".to_string()),
            RunMode::Single,
            None,
        );
        d.assert_belongs_to(&status, "worker").unwrap();
        assert_eq!(
            d.assert_belongs_to(&status, "other")
                .unwrap_err()
                .to_string(),
            "Async run 'run-launch' has a recovery descriptor for 'worker', not 'other'."
        );
        status.run_id = RunId::from_token("run-other".to_string());
        assert_eq!(
            d.assert_belongs_to(&status, "worker")
                .unwrap_err()
                .to_string(),
            "Nested run 'run-other' has a recovery descriptor for a different source run."
        );
    }

    /// pi `applySteeringRecoveryAgentConfig`: every overlaid field takes the descriptor's value
    /// over a persona that disagrees on all of them; the fields the descriptor does not carry
    /// keep the persona's.
    #[test]
    fn apply_to_persona_overlays_the_descriptor_over_the_file() {
        let config = runner_config(distinctive_step(), distinctive_persona());
        let d = RecoveryDescriptor::for_single_launch(&config, LaunchInputs::default()).unwrap();
        let mut widened = ResolvedAgentPersona {
            inherit_global_context: false,
            machine: None,
            mutation_tools: None,
            name: "worker".to_string(),
            model: Some(ModelId::from("fixture/other-model")),
            model_provider: None,
            fallback_models: vec![ModelId::from("fixture/kept-fallback")],
            thinking: Some("high".to_string()),
            system_prompt_mode: SystemPromptMode::Append,
            system_prompt_body: "Rewritten body.".to_string(),
            tools: Some(vec![
                ToolRef::Builtin("read".to_string()),
                ToolRef::Builtin("bash".to_string()),
            ]),
            exclude_tools: Vec::new(),
            allow_nested_subagents: Some(false),
            extensions: Some(vec!["ext-z".to_string()]),
            subagent_only_extensions: Vec::new(),
            output: None,
            inherit_project_context: true,
            inherit_skills: true,
            skills: vec!["zeta".to_string()],
            completion_guard: Some(true),
            max_subagent_depth: Some(9),
            default_context: Some(ContextMode::Fork),
            memory: None,
            tool_budget: None,
            runner: None,
            acceptance_role: None,
            default_acceptance: None,
            file_path: Some(PathBuf::from("/agents/worker-v2.md")),
        };
        d.apply_to_persona(&mut widened);
        let expected = distinctive_persona();
        assert_eq!(
            widened.model.as_ref().map(ModelId::as_str),
            Some("anthropic/override")
        );
        assert_eq!(widened.model_provider, expected.model_provider);
        assert_eq!(widened.thinking, expected.thinking);
        assert_eq!(widened.system_prompt_mode, expected.system_prompt_mode);
        assert_eq!(widened.system_prompt_body, expected.system_prompt_body);
        assert_eq!(widened.tools, expected.tools);
        assert_eq!(widened.exclude_tools, expected.exclude_tools);
        assert_eq!(
            widened.allow_nested_subagents,
            expected.allow_nested_subagents
        );
        assert_eq!(widened.extensions, expected.extensions);
        assert_eq!(
            widened.subagent_only_extensions,
            expected.subagent_only_extensions
        );
        assert_eq!(
            widened.inherit_project_context,
            expected.inherit_project_context
        );
        assert_eq!(widened.inherit_skills, expected.inherit_skills);
        // SUBA-101/102 — the file says `false`/`None`; the launch said `true`/`[apply_patch]`.
        assert_eq!(
            widened.inherit_global_context,
            expected.inherit_global_context
        );
        assert_eq!(widened.mutation_tools, expected.mutation_tools);
        assert_eq!(
            widened.skills,
            vec!["beta".to_string()],
            "the EFFECTIVE launch list"
        );
        assert_eq!(widened.file_path, expected.file_path);
        assert_eq!(widened.completion_guard, expected.completion_guard);
        assert_eq!(widened.memory, expected.memory);
        assert_eq!(widened.tool_budget, expected.tool_budget);
        assert_eq!(widened.max_subagent_depth, Some(2));
        // Not carried by the descriptor: the discovered values survive (pi's `...agentConfig`).
        assert_eq!(
            widened.fallback_models,
            vec![ModelId::from("fixture/kept-fallback")]
        );
        assert_eq!(widened.default_context, Some(ContextMode::Fork));
    }

    /// pi `subagent-executor.ts:1894-1905`: an agent discovery no longer finds is synthesised
    /// from the descriptor — and then overlaid like any discovered one.
    #[test]
    fn synthesised_persona_is_the_descriptor_s_contract() {
        let config = runner_config(distinctive_step(), distinctive_persona());
        let mut d =
            RecoveryDescriptor::for_single_launch(&config, LaunchInputs::default()).unwrap();
        let persona = d.synthesised_persona();
        assert_eq!(persona.name, "worker");
        assert_eq!(persona.tools, distinctive_persona().tools);
        assert_eq!(persona.file_path, Some(PathBuf::from("/agents/worker.md")));
        assert_eq!(persona.system_prompt_body, "Launch body.");
        assert_eq!(persona.max_subagent_depth, Some(2));
        assert!(persona.fallback_models.is_empty());

        d.agent_file_path = None;
        d.system_prompt = None;
        let fallback = d.synthesised_persona();
        assert_eq!(
            fallback.file_path,
            Some(PathBuf::from("/work/.cyrup/agents/recovery-agent")),
            "pi's `<project agents dir>/recovery-agent` fallback"
        );
        assert_eq!(fallback.system_prompt_body, "", "pi's `systemPrompt: \"\"`");
    }
}
