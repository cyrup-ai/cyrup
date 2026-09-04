//! SUBA-084 — runtime agent registration: a direct port of pi
//! `src/agents/runtime-agent-registry.ts` (429 lines @v0.64.0; 424 lines @v0.57.0).
//!
//! An embedder defines an agent IN-PROCESS — no `.md` file, no settings write — and it takes
//! part in every discovery exactly like an on-disk agent: [`RuntimeAgentRegistry::register`] is
//! pi's `registerRuntimeAgent` (`:371-398`), re-exported by `src/api/agents.ts:2` as the public
//! `registerAgent`; [`merge_runtime_agents`] is `mergeRuntimeAgents` (`:423-429`), the single
//! seam every discovery consumer funnels through (`extension/index.ts:528-546`,
//! `slash/slash-commands.ts:120-130`, `agents/agent-management.ts:132-141`).
//!
//! # Ownership — the `WeakMap<ExtensionAPI, …>` partition
//!
//! Upstream keys its registry on `globalThis[Symbol.for(RUNTIME_AGENT_REGISTRY_KEY)]` and
//! partitions records by the OWNING `ExtensionAPI` object (`:71-74`, `:90-107`), so two
//! pi-subagents instances in one process never see each other's agents and `clearRuntimeAgentsForPi`
//! (`:400-402`) drops exactly one owner's records. cyrup has no `globalThis`; the same "per owning
//! runtime" scope is one [`RuntimeAgentRegistry`] value owned by each
//! [`crate::extension::SubagentExecutor`] (`[CYRUP-DELTA]` in mechanism only —
//! the partition, the caps and every check are upstream's). [`RUNTIME_AGENT_REGISTRY_KEY`] is
//! kept as a documented constant for traceability; nothing is keyed on it.
//!
//! # What is NOT here (recorded, not silently dropped)
//!
//! - The v0.64.0-only cross-extension EVENT bridge (`src/agents/runtime-agent-events.ts`,
//!   `registerAgentViaEvents` — synchronous `emit` with the handler mutating `request.result` in
//!   place). cyrup's `SharedBus` queues emits and passes payloads by value, so that bridge needs a
//!   request/response design of its own; it is a separate row.
//! - Five `RuntimeAgentDefinition` fields have no [`AgentDefinition`] landing at HEAD
//!   (`mcpDirectTools`, `inheritGlobalContext`, `mutationTools`, `skillPath`,
//!   `defaultToolTimeoutMs`). They are validated EXACTLY as upstream validates them (so a
//!   malformed value produces upstream's message) and are then REFUSED by name
//!   ([`unrepresentable_field_error`]) rather than accepted and dropped — the same
//!   refuse-don't-downgrade stance `crate::runner::AgentRunnerConfig::refusal_reason` takes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cyrup_core::ModelId;
use serde_json::Value;

use super::frontmatter::normalize_agent_aliases;
use super::management::BUILTIN_AGENT_NAMES;
use super::types::{
    AgentDefinition, AgentSource, OutputMode, OutputSpec, ResolvedToolBudget, SystemPromptMode,
    ToolRef,
};
use crate::error::SubagentError;
use crate::exec::acceptance::model::AcceptanceRole;
use crate::fork_context::ContextMode;
use crate::runner::{AgentRunnerConfig, ExternalCliRunner, ExternalJobRunner, contract};
use crate::watchdog::permission_arbiter::PermissionRules;

/// `RUNTIME_AGENT_REGISTRY_KEY` (`runtime-agent-registry.ts:10`). Documentation-only in cyrup —
/// see the module doc's ownership note.
pub const RUNTIME_AGENT_REGISTRY_KEY: &str = "pi-subagents.runtime-agents.v1";

/// `MAX_RUNTIME_AGENTS_PER_PI` (`:12`).
pub const MAX_RUNTIME_AGENTS_PER_OWNER: usize = 200;
/// `MAX_AGENT_NAME_LENGTH` (`:13`).
pub const MAX_AGENT_NAME_LENGTH: usize = 128;
/// `MAX_DESCRIPTION_LENGTH` (`:14`).
pub const MAX_DESCRIPTION_LENGTH: usize = 4_096;
/// `MAX_SYSTEM_PROMPT_LENGTH` (`:15`).
pub const MAX_SYSTEM_PROMPT_LENGTH: usize = 1024 * 1024;
/// `MAX_FIELD_STRING_LENGTH` (`:16`).
pub const MAX_FIELD_STRING_LENGTH: usize = 8_192;

/// The 35 keys `validateDefinition` accepts (`:201-207`). Anything else is
/// `Runtime agent definition has unknown fields: …` (`:208-209`).
const SUPPORTED_FIELDS: [&str; 35] = [
    "description",
    "systemPrompt",
    "aliases",
    "tools",
    "excludeTools",
    "allowNestedSubagents",
    "mcpDirectTools",
    "model",
    "fallbackModels",
    "thinking",
    "systemPromptMode",
    "inheritProjectContext",
    "inheritGlobalContext",
    "inheritSkills",
    "defaultContext",
    "defaultAsync",
    "defaultTimeoutMs",
    "defaultToolTimeoutMs",
    "defaultAcceptance",
    "acceptanceRole",
    "runner",
    "skills",
    "skillPath",
    "extensions",
    "subagentOnlyExtensions",
    "mutationTools",
    "output",
    "outputMode",
    "defaultReads",
    "defaultProgress",
    "interactive",
    "maxSubagentDepth",
    "completionGuard",
    "toolBudget",
    "permissions",
];

/// `[CYRUP-DELTA]` — the supported-upstream fields this port cannot carry onto an
/// [`AgentDefinition`] yet: `(camelCase key, the landing it is waiting on)`. See the module doc.
const UNREPRESENTABLE_FIELDS: [(&str, &str); 5] = [
    (
        "mcpDirectTools",
        "a standalone `mcp_direct_tools` list (cyrup only derives MCP direct tools from `mcp:`-prefixed `tools` entries)",
    ),
    ("inheritGlobalContext", "an `inherit_global_context` flag"),
    ("mutationTools", "a `mutation_tools` list"),
    ("skillPath", "a `skill_path` list"),
    (
        "defaultToolTimeoutMs",
        "a `default_tool_timeout_ms` launch default",
    ),
];

// -------------------------------------------------------------------------------------------
// Public input shape
// -------------------------------------------------------------------------------------------

/// `RuntimeAgentDefinition.thinking?: string | false` (`:28`): an open reasoning-level string,
/// or the literal `false` — which upstream renders and applies as `off`
/// (`agent-management.ts:744,1014` @v0.64.0), the same value [`AgentDefinition::thinking`]
/// carries as `Some("off")`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeThinking {
    /// A reasoning level string (`"high"`, `"off"`, …), preserved verbatim.
    Level(String),
    /// The literal `false`.
    Off,
}

impl RuntimeThinking {
    fn to_value(&self) -> Value {
        match self {
            RuntimeThinking::Level(level) => Value::String(level.clone()),
            RuntimeThinking::Off => Value::Bool(false),
        }
    }
}

/// `RuntimeAgentDefinition` (`runtime-agent-registry.ts:18-54` @v0.64.0) — the typed input an
/// embedder hands to [`RuntimeAgentRegistry::register`]. Every optional field is upstream's
/// `undefined` when `None`. `default_acceptance`, `tool_budget` and `permissions` are held as the
/// raw config-INPUT shapes upstream types them as (`AcceptanceInput`, `ToolBudgetConfig`,
/// `PermissionRules`) and are validated by the crate's existing validators at registration, so
/// the same value an agent file's frontmatter would carry is accepted here.
///
/// Field-count history: v0.57.0 declared 32 fields; v0.64.0 dropped `defaultTurnBudget` and added
/// `excludeTools`, `allowNestedSubagents`, `inheritGlobalContext` and `mutationTools` (35). This
/// is the v0.64.0 shape (ADR-0006).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuntimeAgentDefinition {
    pub description: String,
    pub system_prompt: String,
    pub aliases: Option<Vec<String>>,
    pub tools: Option<Vec<String>>,
    pub exclude_tools: Option<Vec<String>>,
    pub allow_nested_subagents: Option<bool>,
    pub mcp_direct_tools: Option<Vec<String>>,
    pub model: Option<String>,
    pub fallback_models: Option<Vec<String>>,
    pub thinking: Option<RuntimeThinking>,
    pub system_prompt_mode: Option<SystemPromptMode>,
    pub inherit_project_context: Option<bool>,
    pub inherit_global_context: Option<bool>,
    pub inherit_skills: Option<bool>,
    pub default_context: Option<ContextMode>,
    pub default_async: Option<bool>,
    pub default_timeout_ms: Option<u64>,
    pub default_tool_timeout_ms: Option<u64>,
    pub default_acceptance: Option<Value>,
    pub acceptance_role: Option<AcceptanceRole>,
    pub runner: Option<AgentRunnerConfig>,
    pub skills: Option<Vec<String>>,
    pub skill_path: Option<Vec<String>>,
    pub extensions: Option<Vec<String>>,
    pub subagent_only_extensions: Option<Vec<String>>,
    pub mutation_tools: Option<Vec<String>>,
    pub output: Option<String>,
    pub output_mode: Option<OutputMode>,
    pub default_reads: Option<Vec<String>>,
    pub default_progress: Option<bool>,
    pub interactive: Option<bool>,
    pub max_subagent_depth: Option<u64>,
    pub completion_guard: Option<bool>,
    pub tool_budget: Option<Value>,
    pub permissions: Option<Value>,
}

impl RuntimeAgentDefinition {
    /// The two required fields (`:19-20`); everything else stays `None`.
    #[must_use]
    pub fn new(description: impl Into<String>, system_prompt: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            system_prompt: system_prompt.into(),
            ..Self::default()
        }
    }

    /// The untyped object upstream's `validateDefinition` (`:198`) sees — `None` fields are
    /// OMITTED (upstream's `undefined`, which every validator treats as absent), `runner` is
    /// emitted through [`crate::runner::runner_to_json_string`] so it has exactly upstream's key
    /// shape. Registering a typed definition routes through this so there is ONE validator and
    /// one set of messages for typed and untyped ([`RuntimeAgentRegistry::register_value`]) input.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut object = serde_json::Map::new();
        let string_list = |list: &Option<Vec<String>>| {
            list.as_ref()
                .map(|items| Value::Array(items.iter().cloned().map(Value::String).collect()))
        };
        let mut put = |key: &str, value: Option<Value>| {
            if let Some(value) = value {
                object.insert(key.to_string(), value);
            }
        };
        put("description", Some(Value::String(self.description.clone())));
        put(
            "systemPrompt",
            Some(Value::String(self.system_prompt.clone())),
        );
        put("aliases", string_list(&self.aliases));
        put("tools", string_list(&self.tools));
        put("excludeTools", string_list(&self.exclude_tools));
        put(
            "allowNestedSubagents",
            self.allow_nested_subagents.map(Value::Bool),
        );
        put("mcpDirectTools", string_list(&self.mcp_direct_tools));
        put("model", self.model.clone().map(Value::String));
        put("fallbackModels", string_list(&self.fallback_models));
        put(
            "thinking",
            self.thinking.as_ref().map(RuntimeThinking::to_value),
        );
        put(
            "systemPromptMode",
            self.system_prompt_mode
                .and_then(|mode| serde_json::to_value(mode).ok()),
        );
        put(
            "inheritProjectContext",
            self.inherit_project_context.map(Value::Bool),
        );
        put(
            "inheritGlobalContext",
            self.inherit_global_context.map(Value::Bool),
        );
        put("inheritSkills", self.inherit_skills.map(Value::Bool));
        put(
            "defaultContext",
            self.default_context
                .and_then(|mode| serde_json::to_value(mode).ok()),
        );
        put("defaultAsync", self.default_async.map(Value::Bool));
        put("defaultTimeoutMs", self.default_timeout_ms.map(Value::from));
        put(
            "defaultToolTimeoutMs",
            self.default_tool_timeout_ms.map(Value::from),
        );
        put("defaultAcceptance", self.default_acceptance.clone());
        put(
            "acceptanceRole",
            self.acceptance_role
                .and_then(|role| serde_json::to_value(role).ok()),
        );
        put(
            "runner",
            self.runner.as_ref().and_then(|runner| {
                serde_json::from_str::<Value>(&crate::runner::runner_to_json_string(runner)).ok()
            }),
        );
        put("skills", string_list(&self.skills));
        put("skillPath", string_list(&self.skill_path));
        put("extensions", string_list(&self.extensions));
        put(
            "subagentOnlyExtensions",
            string_list(&self.subagent_only_extensions),
        );
        put("mutationTools", string_list(&self.mutation_tools));
        put("output", self.output.clone().map(Value::String));
        put(
            "outputMode",
            self.output_mode
                .and_then(|mode| serde_json::to_value(mode).ok()),
        );
        put("defaultReads", string_list(&self.default_reads));
        put("defaultProgress", self.default_progress.map(Value::Bool));
        put("interactive", self.interactive.map(Value::Bool));
        put("maxSubagentDepth", self.max_subagent_depth.map(Value::from));
        put("completionGuard", self.completion_guard.map(Value::Bool));
        put("toolBudget", self.tool_budget.clone());
        put("permissions", self.permissions.clone());
        Value::Object(object)
    }
}

// -------------------------------------------------------------------------------------------
// Field validators (`:116-146`)
// -------------------------------------------------------------------------------------------

fn management_error(message: String) -> SubagentError {
    SubagentError::Management(message)
}

/// JS `String.prototype.length` counts UTF-16 code units, so the caps are measured the same way.
fn js_length(value: &str) -> usize {
    value.encode_utf16().count()
}

/// `validateString(value, field, maxLength)` (`:116-123`).
fn validate_string(
    value: Option<&Value>,
    field: &str,
    max_length: usize,
) -> Result<String, SubagentError> {
    let text = match value {
        Some(Value::String(text)) if !text.is_empty() && text.trim() == text => text,
        _ => {
            return Err(management_error(format!(
                "{field} must be a non-empty string without leading or trailing whitespace."
            )));
        }
    };
    if js_length(text) > max_length {
        return Err(management_error(format!(
            "{field} must be at most {max_length} characters."
        )));
    }
    if text.contains('\0') {
        return Err(management_error(format!(
            "{field} must not contain NUL characters."
        )));
    }
    Ok(text.clone())
}

/// `validateOptionalString` (`:125-128`).
fn validate_optional_string(
    value: Option<&Value>,
    field: &str,
) -> Result<Option<String>, SubagentError> {
    match value {
        None => Ok(None),
        Some(_) => validate_string(value, field, MAX_FIELD_STRING_LENGTH).map(Some),
    }
}

/// `validateStringList` (`:130-134`): every entry is a full `validateString` at the per-field cap,
/// then de-duplicated in first-seen order (`[...new Set(...)]`).
fn validate_string_list(
    value: Option<&Value>,
    field: &str,
) -> Result<Option<Vec<String>>, SubagentError> {
    let Some(value) = value else { return Ok(None) };
    let Value::Array(entries) = value else {
        return Err(management_error(format!(
            "{field} must be an array of strings when provided."
        )));
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let text = validate_string(
            Some(entry),
            &format!("{field}[{index}]"),
            MAX_FIELD_STRING_LENGTH,
        )?;
        if seen.insert(text.clone()) {
            out.push(text);
        }
    }
    Ok(Some(out))
}

/// `validatePositiveInteger` (`:136-140`): `typeof value === "number" && Number.isInteger(value)
/// && value > 0`.
fn validate_positive_integer(
    value: Option<&Value>,
    field: &str,
) -> Result<Option<u64>, SubagentError> {
    match value {
        None => Ok(None),
        Some(Value::Number(number)) => match number.as_u64() {
            Some(n) if n > 0 => Ok(Some(n)),
            _ => Err(management_error(format!(
                "{field} must be a positive integer when provided."
            ))),
        },
        Some(_) => Err(management_error(format!(
            "{field} must be a positive integer when provided."
        ))),
    }
}

/// `validateBoolean` (`:142-146`).
fn validate_boolean(value: Option<&Value>, field: &str) -> Result<Option<bool>, SubagentError> {
    match value {
        None => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(_) => Err(management_error(format!(
            "{field} must be a boolean when provided."
        ))),
    }
}

/// `validateRunner(value)` (`:156-184`) — upstream's fourteen refusals, verbatim, in upstream's
/// order. Shares the adapter-id set, the label and the capability-narrowing parser with the
/// frontmatter runner parser (`crate::runner`), but NOT its messages: those are prefixed
/// `Agent '<name>'`, these `Runtime agent definition`.
fn validate_runner(value: Option<&Value>) -> Result<Option<AgentRunnerConfig>, SubagentError> {
    let Some(value) = value else { return Ok(None) };
    let Some(runner) = value.as_object() else {
        return Err(management_error(
            "Runtime agent definition runner must be an object when provided.".to_string(),
        ));
    };
    match runner.get("type").and_then(Value::as_str) {
        Some("pi") => {
            if runner.keys().any(|key| key != "type") {
                return Err(management_error(
                    "Runtime agent definition Pi runner supports only 'type'.".to_string(),
                ));
            }
            Ok(Some(AgentRunnerConfig::Pi))
        }
        Some("external-job") => {
            let provider = runner
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if provider.trim().is_empty() || provider.trim() != provider {
                return Err(management_error(
                    "Runtime agent definition external-job runner requires a non-empty trimmed provider string."
                        .to_string(),
                ));
            }
            let options = match runner.get("options") {
                None => None,
                // A `serde_json::Value` is JSON-serializable by construction, so upstream's
                // `isJsonSerializable` reduces to the object-shape test.
                Some(options) if options.is_object() => Some(options.clone()),
                Some(_) => {
                    return Err(management_error(
                        "Runtime agent definition external-job runner options must be a JSON-serializable object."
                            .to_string(),
                    ));
                }
            };
            let unknown: Vec<&str> = runner
                .keys()
                .map(String::as_str)
                .filter(|key| !matches!(*key, "type" | "provider" | "options"))
                .collect();
            if !unknown.is_empty() {
                return Err(management_error(format!(
                    "Runtime agent definition external-job runner has unsupported fields: {}.",
                    unknown.join(", ")
                )));
            }
            Ok(Some(AgentRunnerConfig::ExternalJob(ExternalJobRunner {
                provider: provider.to_string(),
                options,
            })))
        }
        Some("external-cli") => {
            let command = runner
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if command.trim().is_empty() {
                return Err(management_error(
                    "Runtime agent definition external-cli runner requires a non-empty command string."
                        .to_string(),
                ));
            }
            let args: Vec<String> = match runner.get("args") {
                None => Vec::new(),
                Some(Value::Array(items)) if items.iter().all(Value::is_string) => items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect(),
                Some(_) => {
                    return Err(management_error(
                        "Runtime agent definition external-cli runner args must be an array of strings."
                            .to_string(),
                    ));
                }
            };
            let adapter = match runner.get("adapter") {
                None => None,
                Some(adapter) => {
                    let id = adapter.as_str().unwrap_or_default();
                    if !contract::is_code_owned_adapter_id(id) {
                        return Err(management_error(format!(
                            "Runtime agent definition external-cli runner adapter must be {}.",
                            contract::CODE_OWNED_ADAPTER_LABEL
                        )));
                    }
                    if !args.is_empty() {
                        return Err(management_error(format!(
                            "Runtime agent definition {id} adapter owns its argv; runner args are not supported."
                        )));
                    }
                    Some(id.to_string())
                }
            };
            let prompt_delivery_stdin = match runner.get("promptDelivery") {
                None => false,
                Some(Value::String(mode)) if mode == "stdin" => true,
                Some(_) => {
                    return Err(management_error(
                        "Runtime agent definition external-cli runner promptDelivery must be 'stdin'."
                            .to_string(),
                    ));
                }
            };
            let capabilities = contract::parse_capability_narrowing(
                runner.get("capabilities"),
                "Runtime agent definition external-cli runner capabilities",
            )
            .map_err(management_error)?;
            let unknown: Vec<&str> = runner
                .keys()
                .map(String::as_str)
                .filter(|key| {
                    !matches!(
                        *key,
                        "type" | "adapter" | "command" | "args" | "promptDelivery" | "capabilities"
                    )
                })
                .collect();
            if !unknown.is_empty() {
                return Err(management_error(format!(
                    "Runtime agent definition external-cli runner has unsupported fields: {}.",
                    unknown.join(", ")
                )));
            }
            Ok(Some(AgentRunnerConfig::ExternalCli(ExternalCliRunner {
                adapter,
                command: command.trim().to_string(),
                args,
                prompt_delivery_stdin,
                capabilities,
            })))
        }
        _ => Err(management_error(
            "Runtime agent definition runner.type must be 'pi', 'external-cli', or 'external-job'."
                .to_string(),
        )),
    }
}

/// `validateAcceptance` (`:186-190`): `validateAcceptanceInput(value, "Runtime agent definition
/// defaultAcceptance")`, errors space-joined. A JSON `null` is treated as absent, as
/// `crate::discovery::frontmatter::parse_agent_acceptance_frontmatter` does for the same reason.
fn validate_acceptance(value: Option<&Value>) -> Result<Option<Value>, SubagentError> {
    let Some(value) = value else { return Ok(None) };
    let errors = crate::exec::acceptance::model::validate_acceptance_input(
        value,
        "Runtime agent definition defaultAcceptance",
    );
    if !errors.is_empty() {
        return Err(management_error(errors.join(" ")));
    }
    Ok(if value.is_null() {
        None
    } else {
        Some(value.clone())
    })
}

/// `validateToolBudget` (`:192-196`).
fn validate_tool_budget(
    value: Option<&Value>,
) -> Result<Option<ResolvedToolBudget>, SubagentError> {
    crate::exec::tool_budget::validate_tool_budget_config(
        value,
        "Runtime agent definition toolBudget",
    )
    .map_err(management_error)
}

/// `validatePermissionRules(definition.permissions, "Runtime agent definition permissions")`
/// (`:247`).
fn validate_permissions(value: Option<&Value>) -> Result<Option<PermissionRules>, SubagentError> {
    crate::watchdog::permission_arbiter::validate_permission_rules(
        value,
        "Runtime agent definition permissions",
    )
    .map_err(management_error)
}

/// `[CYRUP-DELTA]` — the refusal for a field this port validates but cannot carry (module doc).
fn unrepresentable_field_error(field: &str, landing: &str) -> SubagentError {
    management_error(format!(
        "Runtime agent definition {field} is not supported by this cyrup build: AgentDefinition has no landing for it ({landing}). [CYRUP-DELTA] SUBA-084 — the value was validated but is refused rather than silently dropped."
    ))
}

/// The output of [`validate_definition`]: the normalized typed definition plus the two fields
/// whose validators hand back RESOLVED (not raw) shapes, and the set of keys the caller supplied
/// (for [`AgentDefinition::present_fields`]).
struct ValidatedDefinition {
    definition: RuntimeAgentDefinition,
    tool_budget: Option<ResolvedToolBudget>,
    permission_rules: Option<PermissionRules>,
    present_fields: HashSet<String>,
}

fn enum_string<'a>(
    value: Option<&'a Value>,
    allowed: &[&str],
    error: &str,
) -> Result<Option<&'a str>, SubagentError> {
    match value {
        None => Ok(None),
        Some(Value::String(text)) if allowed.contains(&text.as_str()) => Ok(Some(text.as_str())),
        Some(_) => Err(management_error(error.to_string())),
    }
}

/// `validateDefinition(value)` (`:198-285`), in upstream's exact check order: object shape,
/// unknown keys, the five enum-valued scalars (`:210-219`), then every optional field in
/// declaration order (`:220-247`), and — last, because upstream evaluates them inside the returned
/// object literal (`:249-250`) — the two required strings.
///
/// `[CYRUP-DELTA]` (message text only): upstream lists unknown keys in object insertion order;
/// `serde_json` without `preserve_order` yields them sorted.
fn validate_definition(value: &Value) -> Result<ValidatedDefinition, SubagentError> {
    let Some(definition) = value.as_object() else {
        return Err(management_error(
            "Runtime agent definition must be an object.".to_string(),
        ));
    };
    let unknown: Vec<&str> = definition
        .keys()
        .map(String::as_str)
        .filter(|key| !SUPPORTED_FIELDS.contains(key))
        .collect();
    if !unknown.is_empty() {
        return Err(management_error(format!(
            "Runtime agent definition has unknown fields: {}.",
            unknown.join(", ")
        )));
    }
    let system_prompt_mode = enum_string(
        definition.get("systemPromptMode"),
        &["append", "replace"],
        "Runtime agent definition systemPromptMode must be 'append' or 'replace'.",
    )?
    .map(|mode| {
        if mode == "append" {
            SystemPromptMode::Append
        } else {
            SystemPromptMode::Replace
        }
    });
    let default_context = enum_string(
        definition.get("defaultContext"),
        &["fresh", "fork"],
        "Runtime agent definition defaultContext must be 'fresh' or 'fork'.",
    )?
    .map(|mode| {
        if mode == "fork" {
            ContextMode::Fork
        } else {
            ContextMode::Fresh
        }
    });
    let thinking = match definition.get("thinking") {
        None => None,
        Some(Value::Bool(false)) => Some(RuntimeThinking::Off),
        Some(Value::String(level)) => Some(RuntimeThinking::Level(level.clone())),
        Some(_) => {
            return Err(management_error(
                "Runtime agent definition thinking must be a string or false when provided."
                    .to_string(),
            ));
        }
    };
    let acceptance_role = enum_string(
        definition.get("acceptanceRole"),
        &["read-only", "writer"],
        "Runtime agent definition acceptanceRole must be 'read-only' or 'writer'.",
    )?
    .and_then(AcceptanceRole::parse_exact);
    let output_mode = enum_string(
        definition.get("outputMode"),
        &["inline", "file-only"],
        "Runtime agent definition outputMode must be 'inline' or 'file-only'.",
    )?
    .map(|mode| {
        if mode == "inline" {
            OutputMode::Inline
        } else {
            OutputMode::FileOnly
        }
    });

    let field = |key: &str| format!("Runtime agent definition {key}");
    let aliases = validate_string_list(definition.get("aliases"), &field("aliases"))?;
    let tools = validate_string_list(definition.get("tools"), &field("tools"))?;
    let exclude_tools =
        validate_string_list(definition.get("excludeTools"), &field("excludeTools"))?;
    let allow_nested_subagents = validate_boolean(
        definition.get("allowNestedSubagents"),
        &field("allowNestedSubagents"),
    )?;
    let mcp_direct_tools =
        validate_string_list(definition.get("mcpDirectTools"), &field("mcpDirectTools"))?;
    let model = validate_optional_string(definition.get("model"), &field("model"))?;
    let fallback_models =
        validate_string_list(definition.get("fallbackModels"), &field("fallbackModels"))?;
    let inherit_project_context = validate_boolean(
        definition.get("inheritProjectContext"),
        &field("inheritProjectContext"),
    )?;
    let inherit_global_context = validate_boolean(
        definition.get("inheritGlobalContext"),
        &field("inheritGlobalContext"),
    )?;
    let inherit_skills =
        validate_boolean(definition.get("inheritSkills"), &field("inheritSkills"))?;
    let default_async = validate_boolean(definition.get("defaultAsync"), &field("defaultAsync"))?;
    let default_timeout_ms = validate_positive_integer(
        definition.get("defaultTimeoutMs"),
        &field("defaultTimeoutMs"),
    )?;
    let default_tool_timeout_ms = validate_positive_integer(
        definition.get("defaultToolTimeoutMs"),
        &field("defaultToolTimeoutMs"),
    )?;
    let default_acceptance = validate_acceptance(definition.get("defaultAcceptance"))?;
    let runner = validate_runner(definition.get("runner"))?;
    let skills = validate_string_list(definition.get("skills"), &field("skills"))?;
    let skill_path = validate_string_list(definition.get("skillPath"), &field("skillPath"))?;
    let extensions = validate_string_list(definition.get("extensions"), &field("extensions"))?;
    let subagent_only_extensions = validate_string_list(
        definition.get("subagentOnlyExtensions"),
        &field("subagentOnlyExtensions"),
    )?;
    let mutation_tools =
        validate_string_list(definition.get("mutationTools"), &field("mutationTools"))?;
    let output = validate_optional_string(definition.get("output"), &field("output"))?;
    let default_reads =
        validate_string_list(definition.get("defaultReads"), &field("defaultReads"))?;
    let default_progress =
        validate_boolean(definition.get("defaultProgress"), &field("defaultProgress"))?;
    let interactive = validate_boolean(definition.get("interactive"), &field("interactive"))?;
    let max_subagent_depth = validate_positive_integer(
        definition.get("maxSubagentDepth"),
        &field("maxSubagentDepth"),
    )?;
    let completion_guard =
        validate_boolean(definition.get("completionGuard"), &field("completionGuard"))?;
    let tool_budget = validate_tool_budget(definition.get("toolBudget"))?;
    let permission_rules = validate_permissions(definition.get("permissions"))?;
    let description = validate_string(
        definition.get("description"),
        &field("description"),
        MAX_DESCRIPTION_LENGTH,
    )?;
    let system_prompt = validate_string(
        definition.get("systemPrompt"),
        &field("systemPrompt"),
        MAX_SYSTEM_PROMPT_LENGTH,
    )?;

    // `[CYRUP-DELTA]` — after every upstream check has passed (so a malformed value reports
    // upstream's message first), refuse the fields this port cannot carry.
    for (key, landing) in UNREPRESENTABLE_FIELDS {
        if definition.contains_key(key) {
            return Err(unrepresentable_field_error(key, landing));
        }
    }

    Ok(ValidatedDefinition {
        definition: RuntimeAgentDefinition {
            description,
            system_prompt,
            aliases,
            tools,
            exclude_tools,
            allow_nested_subagents,
            mcp_direct_tools,
            model,
            fallback_models,
            thinking,
            system_prompt_mode,
            inherit_project_context,
            inherit_global_context,
            inherit_skills,
            default_context,
            default_async,
            default_timeout_ms,
            default_tool_timeout_ms,
            default_acceptance,
            acceptance_role,
            runner,
            skills,
            skill_path,
            extensions,
            subagent_only_extensions,
            mutation_tools,
            output,
            output_mode,
            default_reads,
            default_progress,
            interactive,
            max_subagent_depth,
            completion_guard,
            tool_budget: definition.get("toolBudget").cloned(),
            permissions: definition.get("permissions").cloned(),
        },
        tool_budget,
        permission_rules,
        present_fields: definition.keys().cloned().collect(),
    })
}

// -------------------------------------------------------------------------------------------
// Identity helpers and collision checks (`:287-323`, `:408-421`)
// -------------------------------------------------------------------------------------------

/// pi's name-sensitive defaults (`:78-88`): `delegate` -> `append` / inherit-project-context,
/// else `replace` / no-inherit; `inheritSkills` always defaults `false`. Replicated locally per
/// this crate's "each module keeps its own small helper" convention (`management/handlers.rs`
/// does the same).
fn default_system_prompt_mode(name: &str) -> SystemPromptMode {
    if name == "delegate" {
        SystemPromptMode::Append
    } else {
        SystemPromptMode::Replace
    }
}

fn default_inherit_project_context(name: &str) -> bool {
    name == "delegate"
}

/// `identityKeys(agent)` (`:293-295`): `[name, localName?, ...aliases]`. Upstream never sets
/// `localName` on a runtime agent; cyrup's `local_name` is non-optional and equals `name` for one,
/// so it contributes a key only when it differs (an on-disk packaged agent's local name).
fn identity_keys(agent: &AgentDefinition) -> Vec<&str> {
    let mut keys: Vec<&str> = vec![agent.name.as_str()];
    if agent.local_name != agent.name {
        keys.push(agent.local_name.as_str());
    }
    keys.extend(agent.aliases.iter().map(String::as_str));
    keys
}

/// `assertNoIdentityCollisions(agents, context)` (`:297-306`).
fn assert_no_identity_collisions(
    agents: &[AgentDefinition],
    context: &str,
) -> Result<(), SubagentError> {
    let mut seen: HashMap<&str, &str> = HashMap::new();
    for agent in agents {
        for key in identity_keys(agent) {
            if let Some(previous) = seen.get(key) {
                return Err(management_error(format!(
                    "{context} collision for '{key}' between '{previous}' and '{}'.",
                    agent.name
                )));
            }
            seen.insert(key, agent.name.as_str());
        }
    }
    Ok(())
}

/// `assertNoRuntimeCollision(agent, existing)` (`:308-317`).
fn assert_no_runtime_collision(
    agent: &AgentDefinition,
    existing: &[AgentDefinition],
) -> Result<(), SubagentError> {
    let mut existing_keys: HashMap<&str, &str> = HashMap::new();
    for registered in existing {
        for key in identity_keys(registered) {
            existing_keys.insert(key, registered.name.as_str());
        }
    }
    for key in identity_keys(agent) {
        if let Some(previous) = existing_keys.get(key) {
            return Err(management_error(format!(
                "Runtime agent '{}' collides with runtime agent '{previous}' on name or alias '{key}'.",
                agent.name
            )));
        }
    }
    Ok(())
}

/// `assertNoBuiltinCollision(agent)` (`:319-323`), against the builtin roster this crate ships
/// ([`BUILTIN_AGENT_NAMES`]). Upstream's v0.64.0 roster also carries the six code-owned external
/// CLI adapter names; cyrup does not ship those as builtins (SUBA-074 stage 2), and their
/// selection names are still guarded by `validate_code_owned_profile_runner`.
fn assert_no_builtin_collision(agent: &AgentDefinition) -> Result<(), SubagentError> {
    for key in identity_keys(agent) {
        if BUILTIN_AGENT_NAMES.contains(&key) {
            return Err(management_error(format!(
                "Runtime agent '{}' collides with builtin agent '{key}'.",
                agent.name
            )));
        }
    }
    Ok(())
}

/// `assertNoConfiguredCollision(configuredAgents, runtimeAgents)` (`:408-421`).
fn assert_no_configured_collision(
    configured_agents: &[AgentDefinition],
    runtime_agents: &[AgentDefinition],
) -> Result<(), SubagentError> {
    let mut configured: HashMap<&str, &str> = HashMap::new();
    for agent in configured_agents {
        for key in identity_keys(agent) {
            configured.insert(key, agent.name.as_str());
        }
    }
    for agent in runtime_agents {
        for key in identity_keys(agent) {
            if let Some(previous) = configured.get(key) {
                return Err(management_error(format!(
                    "Runtime agent '{}' collides with configured agent '{previous}' on name or alias '{key}'.",
                    agent.name
                )));
            }
        }
    }
    Ok(())
}

/// The `mcp:` split `crate::discovery::frontmatter`'s private `parse_tool_refs` applies to an
/// agent file's `tools:` list, replicated locally.
fn tool_refs(entries: &[String]) -> Vec<ToolRef> {
    entries
        .iter()
        .map(|entry| match entry.strip_prefix("mcp:") {
            Some(mcp_name) => ToolRef::Mcp(mcp_name.to_string()),
            None => ToolRef::Builtin(entry.clone()),
        })
        .collect()
}

/// `toAgentConfig(name, definition)` (`:325-369`): `source: "runtime"`, `filePath:
/// "runtime:<name>"`, the name-sensitive defaults for the three inherit/mode fields, aliases
/// normalized against the name. The `assertNoIdentityCollisions([agent], …)` at `:367` is
/// structurally unreachable here — [`normalize_agent_aliases`] has already de-duplicated the
/// aliases and removed the name, and `local_name == name` contributes no key — and is kept for
/// fidelity.
fn to_agent_definition(
    name: &str,
    validated: ValidatedDefinition,
) -> Result<AgentDefinition, SubagentError> {
    let ValidatedDefinition {
        definition,
        tool_budget,
        permission_rules,
        present_fields,
    } = validated;
    let aliases = normalize_agent_aliases(definition.aliases.clone().unwrap_or_default(), name);
    let max_subagent_depth = match definition.max_subagent_depth {
        None => None,
        Some(depth) => Some(u32::try_from(depth).map_err(|_| {
            management_error(
                "Runtime agent definition maxSubagentDepth must be a positive integer when provided."
                    .to_string(),
            )
        })?),
    };
    let output = if definition.output.is_some() || definition.output_mode.is_some() {
        Some(OutputSpec {
            path: definition.output.as_deref().map(PathBuf::from),
            mode: definition.output_mode,
        })
    } else {
        None
    };
    let agent = AgentDefinition {
        name: name.to_string(),
        local_name: name.to_string(),
        package_name: None,
        description: definition.description,
        aliases,
        tools: definition.tools.as_deref().map(tool_refs),
        exclude_tools: definition.exclude_tools,
        allow_nested_subagents: definition.allow_nested_subagents,
        extensions: definition.extensions,
        extensions_from_default: false,
        subagent_only_extensions: definition.subagent_only_extensions.unwrap_or_default(),
        model: definition.model.as_deref().map(ModelId::from),
        fallback_models: definition
            .fallback_models
            .unwrap_or_default()
            .into_iter()
            .map(ModelId::from)
            .collect(),
        thinking: definition.thinking.map(|thinking| match thinking {
            RuntimeThinking::Level(level) => level,
            RuntimeThinking::Off => "off".to_string(),
        }),
        system_prompt_mode: definition
            .system_prompt_mode
            .unwrap_or_else(|| default_system_prompt_mode(name)),
        inherit_project_context: definition
            .inherit_project_context
            .unwrap_or_else(|| default_inherit_project_context(name)),
        inherit_skills: definition.inherit_skills.unwrap_or(false),
        skills: definition.skills.unwrap_or_default(),
        default_reads: definition
            .default_reads
            .map(|reads| reads.into_iter().map(PathBuf::from).collect()),
        default_progress: definition.default_progress,
        output,
        completion_guard: definition.completion_guard,
        interactive: definition.interactive,
        max_subagent_depth,
        default_context: definition.default_context,
        default_async: definition.default_async,
        default_timeout_ms: definition.default_timeout_ms,
        memory: None,
        tool_budget,
        default_turn_budget: None,
        default_acceptance: definition.default_acceptance,
        acceptance_role: definition.acceptance_role,
        permission_rules,
        runner: definition.runner,
        disabled: None,
        system_prompt_body: definition.system_prompt,
        source: AgentSource::Runtime,
        file_path: PathBuf::from(format!("runtime:{name}")),
        present_fields,
        extra_fields: BTreeMap::new(),
        override_info: None,
        model_source: None,
        model_provider: None,
    };
    assert_no_identity_collisions(
        std::slice::from_ref(&agent),
        &format!("Runtime agent '{name}'"),
    )?;
    Ok(agent)
}

// -------------------------------------------------------------------------------------------
// The registry (`:66-74`, `:371-406`)
// -------------------------------------------------------------------------------------------

/// One `RuntimeAgentRecord` (`:66-69`). The record id is what [`RuntimeAgentRegistration::dispose`]
/// removes by — upstream filters on object identity (`entry !== record`, `:393`), never on name
/// equality, so a later registration re-using a disposed name is never removed by the old handle.
#[derive(Clone, Debug)]
struct RuntimeAgentRecord {
    id: u64,
    agent: AgentDefinition,
}

/// One owner's partition of pi's runtime agent registry (`RuntimeAgentRegistry.byPi`, `:71-74`).
/// See the module doc for the ownership mapping.
#[derive(Debug, Default)]
pub struct RuntimeAgentRegistry {
    records: Mutex<Vec<RuntimeAgentRecord>>,
    next_record_id: AtomicU64,
}

impl RuntimeAgentRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn records(&self) -> std::sync::MutexGuard<'_, Vec<RuntimeAgentRecord>> {
        self.records.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `registerRuntimeAgent({ pi, name, definition })` (`:371-398`), the public `registerAgent`
    /// of `src/api/agents.ts:2`: validate the name (`:373`, 128-char cap) and the definition
    /// (`:374`), build the agent (`:375`), refuse a reserved code-owned selection name
    /// (`validateCodeOwnedProfileRunner`, `:376-377`), refuse a builtin identity (`:378`), enforce
    /// the 200-per-owner cap (`:381`), refuse a runtime identity collision (`:382`), then record.
    ///
    /// Returns the registration handle whose [`RuntimeAgentRegistration::dispose`] removes exactly
    /// this record (`:386-397`). Dropping the handle does NOT dispose — upstream's returned object
    /// has no finalizer either; an embedder that wants the agent gone calls `dispose()`.
    ///
    /// # Errors
    ///
    /// Every refusal is a [`SubagentError::Management`] carrying upstream's message verbatim.
    pub fn register(
        self: &Arc<Self>,
        name: &str,
        definition: &RuntimeAgentDefinition,
    ) -> Result<RuntimeAgentRegistration, SubagentError> {
        self.register_value(name, &definition.to_value())
    }

    /// [`Self::register`] over an UNTYPED definition — the shape upstream's validator actually
    /// sees (`validateDefinition(value: unknown)`), and the shape a JSON-speaking embedder or a
    /// future event bridge hands over. Typed registration routes through here.
    ///
    /// # Errors
    ///
    /// As [`Self::register`], plus every type-shape refusal of `validateDefinition`
    /// (`Runtime agent definition must be an object.`, `… has unknown fields: …`, the per-field
    /// `must be a boolean when provided.`-style messages).
    pub fn register_value(
        self: &Arc<Self>,
        name: &str,
        definition: &Value,
    ) -> Result<RuntimeAgentRegistration, SubagentError> {
        let name = validate_string(
            Some(&Value::String(name.to_string())),
            "Runtime agent name",
            MAX_AGENT_NAME_LENGTH,
        )?;
        let validated = validate_definition(definition)?;
        let agent = to_agent_definition(&name, validated)?;
        // `validateCodeOwnedProfileRunner(agent)` (`:376-377`) over `[name, localName?, ...aliases]`.
        let selection_names: Vec<&str> = identity_keys(&agent);
        if let Some(message) = contract::validate_code_owned_profile_runner(
            &selection_names,
            agent
                .runner
                .as_ref()
                .and_then(AgentRunnerConfig::code_owned_adapter),
        ) {
            return Err(management_error(message));
        }
        assert_no_builtin_collision(&agent)?;
        let mut records = self.records();
        if records.len() >= MAX_RUNTIME_AGENTS_PER_OWNER {
            return Err(management_error(format!(
                "Runtime agent registry supports at most {MAX_RUNTIME_AGENTS_PER_OWNER} agents per Pi runtime."
            )));
        }
        let existing: Vec<AgentDefinition> =
            records.iter().map(|record| record.agent.clone()).collect();
        assert_no_runtime_collision(&agent, &existing)?;
        let id = self.next_record_id.fetch_add(1, Ordering::Relaxed);
        let agent_name = agent.name.clone();
        records.push(RuntimeAgentRecord { id, agent });
        drop(records);
        Ok(RuntimeAgentRegistration {
            registry: Arc::clone(self),
            record_id: id,
            agent_name,
            disposed: AtomicBool::new(false),
        })
    }

    /// `listRuntimeAgentConfigs(pi)` (`:404-406`): a fresh copy of every record's agent, in
    /// registration order.
    #[must_use]
    pub fn list(&self) -> Vec<AgentDefinition> {
        self.records()
            .iter()
            .map(|record| record.agent.clone())
            .collect()
    }

    /// `clearRuntimeAgentsForPi(pi)` (`:400-402`) — every record goes; outstanding registration
    /// handles become no-ops.
    pub fn clear(&self) {
        self.records().clear();
    }

    /// The number of live records (the value the 200-cap at `:381` is measured against).
    #[must_use]
    pub fn len(&self) -> usize {
        self.records().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records().is_empty()
    }

    fn remove_record(&self, record_id: u64) {
        self.records().retain(|record| record.id != record_id);
    }
}

/// `RuntimeAgentRegistration` (`:62-64`): the handle `registerRuntimeAgent` returns. Its only
/// operation is the idempotent [`Self::dispose`] (`:386-397`).
#[derive(Debug)]
pub struct RuntimeAgentRegistration {
    registry: Arc<RuntimeAgentRegistry>,
    record_id: u64,
    agent_name: String,
    disposed: AtomicBool,
}

impl RuntimeAgentRegistration {
    /// Remove this registration's record from its registry. Idempotent (`:389`, `if (disposed)
    /// return;`): a second call is a no-op, and a record already cleared by
    /// [`RuntimeAgentRegistry::clear`] is not an error.
    pub fn dispose(&self) {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.registry.remove_record(self.record_id);
    }

    /// Whether [`Self::dispose`] has run.
    #[must_use]
    pub fn is_disposed(&self) -> bool {
        self.disposed.load(Ordering::Acquire)
    }

    /// The registered agent's runtime name.
    #[must_use]
    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }
}

/// `mergeRuntimeAgents(pi, discovered, configuredAgents)` (`:423-429`): drop disabled runtime
/// agents (`:424`), no-op when none remain (`:425`), refuse a collision AMONG the runtime agents
/// (`:426`), refuse a collision against ANY configured agent (`:427`) — `configured` is every
/// on-disk tier regardless of the requested scope, so an agent hidden by scope precedence still
/// blocks (`extension/index.ts:533-539`, tests `runtime-agent-registration.test.ts:331-366`
/// @v0.64.0) — then APPEND the runtime agents after the discovered ones (`:428`). Runtime agents
/// never take part in the four-tier precedence merge and never receive settings overrides.
///
/// # Errors
///
/// [`SubagentError::Management`] with upstream's collision message verbatim; `discovered` is left
/// untouched on error.
pub fn merge_runtime_agents(
    runtime: &[AgentDefinition],
    discovered: &mut Vec<AgentDefinition>,
    configured: &[AgentDefinition],
) -> Result<(), SubagentError> {
    let runtime_agents: Vec<AgentDefinition> = runtime
        .iter()
        .filter(|agent| agent.disabled != Some(true))
        .cloned()
        .collect();
    if runtime_agents.is_empty() {
        return Ok(());
    }
    assert_no_identity_collisions(&runtime_agents, "Runtime agent registration")?;
    assert_no_configured_collision(configured, &runtime_agents)?;
    discovered.extend(runtime_agents);
    Ok(())
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
    use serde_json::json;

    fn registry() -> Arc<RuntimeAgentRegistry> {
        Arc::new(RuntimeAgentRegistry::new())
    }

    fn err_text(result: Result<RuntimeAgentRegistration, SubagentError>) -> String {
        match result {
            Ok(_) => panic!("expected a refusal"),
            Err(err) => err.to_string(),
        }
    }

    /// `toAgentConfig` (`:325-369`): source/filePath stamping and the name-sensitive defaults.
    #[test]
    fn to_agent_definition_stamps_runtime_source_and_defaults() {
        let reg = registry();
        let mut def = RuntimeAgentDefinition::new("Runtime helper", "Help at runtime.");
        def.aliases = Some(vec![
            "helper".into(),
            "helper".into(),
            "runtime-helper".into(),
        ]);
        def.model = Some("openai/gpt-5-mini".into());
        def.thinking = Some(RuntimeThinking::Off);
        def.tools = Some(vec!["read".into(), "mcp:srv.tool".into()]);
        reg.register("runtime-helper", &def).expect("registers");
        let agents = reg.list();
        assert_eq!(agents.len(), 1);
        let agent = &agents[0];
        assert_eq!(agent.source, AgentSource::Runtime);
        assert_eq!(agent.file_path, PathBuf::from("runtime:runtime-helper"));
        assert_eq!(agent.aliases, vec!["helper".to_string()]);
        assert_eq!(agent.system_prompt_mode, SystemPromptMode::Replace);
        assert!(!agent.inherit_project_context);
        assert!(!agent.inherit_skills);
        assert_eq!(agent.thinking.as_deref(), Some("off"));
        assert_eq!(agent.system_prompt_body, "Help at runtime.");
        assert_eq!(
            agent.tools,
            Some(vec![
                ToolRef::Builtin("read".into()),
                ToolRef::Mcp("srv.tool".into())
            ])
        );

        let reg2 = registry();
        reg2.register("delegate-like", &RuntimeAgentDefinition::new("d", "d."))
            .expect("registers");
        assert_eq!(reg2.list()[0].system_prompt_mode, SystemPromptMode::Replace);
        let reg3 = registry();
        // `delegate` itself is a builtin name; the name-sensitive default is only observable
        // through the helper.
        assert_eq!(
            default_system_prompt_mode("delegate"),
            SystemPromptMode::Append
        );
        assert!(default_inherit_project_context("delegate"));
        assert!(reg3.is_empty());
    }

    /// `validateString` (`:116-123`) on the name (`:373`).
    #[test]
    fn name_validation_matches_upstream_messages() {
        let reg = registry();
        let def = RuntimeAgentDefinition::new("d", "d.");
        assert_eq!(
            err_text(reg.register(" padded", &def)),
            "Runtime agent name must be a non-empty string without leading or trailing whitespace."
        );
        assert_eq!(
            err_text(reg.register(&"x".repeat(129), &def)),
            "Runtime agent name must be at most 128 characters."
        );
        assert_eq!(
            err_text(reg.register("nul\0name", &def)),
            "Runtime agent name must not contain NUL characters."
        );
    }

    /// `validateDefinition` (`:198-219`) shape and enum refusals, verbatim.
    #[test]
    fn definition_validation_matches_upstream_messages() {
        let reg = registry();
        assert_eq!(
            err_text(reg.register_value("a", &json!([]))),
            "Runtime agent definition must be an object."
        );
        assert_eq!(
            err_text(
                reg.register_value("a", &json!({"description":"d","systemPrompt":"p","foo":1}))
            ),
            "Runtime agent definition has unknown fields: foo."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &json!({"description":"d","systemPrompt":"p","systemPromptMode":"x"})
            )),
            "Runtime agent definition systemPromptMode must be 'append' or 'replace'."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &json!({"description":"d","systemPrompt":"p","defaultContext":"x"})
            )),
            "Runtime agent definition defaultContext must be 'fresh' or 'fork'."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &json!({"description":"d","systemPrompt":"p","thinking":true})
            )),
            "Runtime agent definition thinking must be a string or false when provided."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &json!({"description":"d","systemPrompt":"p","acceptanceRole":"x"})
            )),
            "Runtime agent definition acceptanceRole must be 'read-only' or 'writer'."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &json!({"description":"d","systemPrompt":"p","outputMode":"file-and-inline"})
            )),
            "Runtime agent definition outputMode must be 'inline' or 'file-only'."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &json!({"description":"d","systemPrompt":"p","aliases":"x"})
            )),
            "Runtime agent definition aliases must be an array of strings when provided."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &json!({"description":"d","systemPrompt":"p","aliases":["ok"," bad"]})
            )),
            "Runtime agent definition aliases[1] must be a non-empty string without leading or trailing whitespace."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &json!({"description":"d","systemPrompt":"p","defaultAsync":1})
            )),
            "Runtime agent definition defaultAsync must be a boolean when provided."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &json!({"description":"d","systemPrompt":"p","defaultTimeoutMs":0})
            )),
            "Runtime agent definition defaultTimeoutMs must be a positive integer when provided."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &json!({"description":"d","systemPrompt":"p","maxSubagentDepth":1.5})
            )),
            "Runtime agent definition maxSubagentDepth must be a positive integer when provided."
        );
        assert_eq!(
            err_text(reg.register_value("a", &json!({"systemPrompt":"p"}))),
            "Runtime agent definition description must be a non-empty string without leading or trailing whitespace."
        );
        assert_eq!(
            err_text(reg.register_value("a", &json!({"description":"d"}))),
            "Runtime agent definition systemPrompt must be a non-empty string without leading or trailing whitespace."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &json!({"description":"d".repeat(4097),"systemPrompt":"p"})
            )),
            "Runtime agent definition description must be at most 4096 characters."
        );
        assert!(reg.is_empty());
    }

    /// `validateRunner` (`:156-184`) refusals, verbatim.
    #[test]
    fn runner_validation_matches_upstream_messages() {
        let reg = registry();
        let with_runner =
            |runner: Value| json!({"description":"d","systemPrompt":"p","runner":runner});
        assert_eq!(
            err_text(reg.register_value("a", &with_runner(json!("pi")))),
            "Runtime agent definition runner must be an object when provided."
        );
        assert_eq!(
            err_text(reg.register_value("a", &with_runner(json!({"type":"pi","extra":1})))),
            "Runtime agent definition Pi runner supports only 'type'."
        );
        assert_eq!(
            err_text(reg.register_value("a", &with_runner(json!({"type":"bogus"})))),
            "Runtime agent definition runner.type must be 'pi', 'external-cli', or 'external-job'."
        );
        assert_eq!(
            err_text(reg.register_value("a", &with_runner(json!({"type":"external-cli"})))),
            "Runtime agent definition external-cli runner requires a non-empty command string."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &with_runner(json!({"type":"external-cli","command":"x","args":"no"}))
            )),
            "Runtime agent definition external-cli runner args must be an array of strings."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &with_runner(json!({"type":"external-cli","command":"x","adapter":"nope"}))
            )),
            format!(
                "Runtime agent definition external-cli runner adapter must be {}.",
                contract::CODE_OWNED_ADAPTER_LABEL
            )
        );
        assert_eq!(
            err_text(reg.register_value("a", &with_runner(json!({"type":"external-cli","command":"x","adapter":"codex-exec","args":["-v"]})))),
            "Runtime agent definition codex-exec adapter owns its argv; runner args are not supported."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &with_runner(json!({"type":"external-cli","command":"x","promptDelivery":"argv"}))
            )),
            "Runtime agent definition external-cli runner promptDelivery must be 'stdin'."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &with_runner(json!({"type":"external-cli","command":"x","bogus":1}))
            )),
            "Runtime agent definition external-cli runner has unsupported fields: bogus."
        );
        assert_eq!(
            err_text(reg.register_value("a", &with_runner(json!({"type":"external-job"})))),
            "Runtime agent definition external-job runner requires a non-empty trimmed provider string."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &with_runner(json!({"type":"external-job","provider":"p","options":[]}))
            )),
            "Runtime agent definition external-job runner options must be a JSON-serializable object."
        );
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &with_runner(json!({"type":"external-job","provider":"p","x":1}))
            )),
            "Runtime agent definition external-job runner has unsupported fields: x."
        );
        // A well-formed external-cli runner registers (the refusal to LAUNCH it is
        // `AgentRunnerConfig::refusal_reason`'s, at spawn time, not registration's).
        reg.register_value(
            "cli",
            &with_runner(
                json!({"type":"external-cli","command":" claude ","promptDelivery":"stdin"}),
            ),
        )
        .expect("registers");
        match &reg.list()[0].runner {
            Some(AgentRunnerConfig::ExternalCli(cli)) => {
                assert_eq!(cli.command, "claude");
                assert!(cli.prompt_delivery_stdin);
            }
            other => panic!("unexpected runner {other:?}"),
        }
    }

    /// Nested validators delegate to the crate's existing ones with upstream's labels
    /// (`:186-196`, `:247`; upstream test `runtime-agent-registration.test.ts:305-319`).
    #[test]
    fn nested_definition_fields_are_validated_with_upstream_labels() {
        let reg = registry();
        let base = |extra: Value| {
            let mut object = json!({"description":"Bad","systemPrompt":"Bad."});
            if let (Some(target), Some(source)) = (object.as_object_mut(), extra.as_object()) {
                for (key, value) in source {
                    target.insert(key.clone(), value.clone());
                }
            }
            object
        };
        let acceptance = err_text(reg.register_value(
            "a",
            &base(json!({"defaultAcceptance":{"level":"verified"}})),
        ));
        assert!(
            acceptance
                .contains("defaultAcceptance.verify must contain at least one runtime command"),
            "{acceptance}"
        );
        let budget = err_text(reg.register_value("a", &base(json!({"toolBudget":{"hard":0}}))));
        assert!(
            budget.contains("toolBudget.hard must be an integer >= 1"),
            "{budget}"
        );
        let permissions =
            err_text(reg.register_value("a", &base(json!({"permissions":{"bash":"deny"}}))));
        assert!(
            permissions.contains("permissions.bash is unsupported"),
            "{permissions}"
        );
    }

    /// `[CYRUP-DELTA]`: an unrepresentable field is validated upstream-style FIRST, then refused
    /// by name rather than dropped.
    #[test]
    fn unrepresentable_fields_are_refused_after_upstream_validation() {
        let reg = registry();
        assert_eq!(
            err_text(reg.register_value(
                "a",
                &json!({"description":"d","systemPrompt":"p","inheritGlobalContext":"yes"})
            )),
            "Runtime agent definition inheritGlobalContext must be a boolean when provided."
        );
        let refused = err_text(reg.register_value(
            "a",
            &json!({"description":"d","systemPrompt":"p","inheritGlobalContext":true}),
        ));
        assert!(
            refused.starts_with(
                "Runtime agent definition inheritGlobalContext is not supported by this cyrup build"
            ),
            "{refused}"
        );
        assert!(refused.contains("[CYRUP-DELTA] SUBA-084"), "{refused}");
        let mut typed = RuntimeAgentDefinition::new("d", "p");
        typed.mcp_direct_tools = Some(vec!["srv.tool".into()]);
        assert!(err_text(reg.register("a", &typed)).contains("mcpDirectTools is not supported"));
        assert!(reg.is_empty());
    }

    /// `registerRuntimeAgent` order (`:376-382`): the reserved-name guard fires before the builtin
    /// check, the cap before the runtime-collision check.
    #[test]
    fn reserved_selection_names_are_guarded_before_builtin_collision() {
        let reg = registry();
        let mut writer = RuntimeAgentDefinition::new("Unsafe", "Write.");
        writer.runner = Some(AgentRunnerConfig::ExternalCli(ExternalCliRunner {
            adapter: Some("claude-code-writer".into()),
            command: "claude".into(),
            args: Vec::new(),
            prompt_delivery_stdin: false,
            capabilities: None,
        }));
        let message = err_text(reg.register("claude-code", &writer));
        assert!(
            message.contains("reserved for the read-only 'claude-code' adapter"),
            "{message}"
        );
        writer.aliases = Some(vec!["codex-exec".into()]);
        let message = err_text(reg.register("runtime-writer", &writer));
        assert!(
            message.contains("Selection name 'codex-exec' is reserved"),
            "{message}"
        );
    }

    #[test]
    fn merge_appends_after_discovered_and_is_a_no_op_when_empty() {
        let reg = registry();
        reg.register("rt", &RuntimeAgentDefinition::new("d", "p"))
            .expect("registers");
        let mut discovered: Vec<AgentDefinition> = Vec::new();
        merge_runtime_agents(&[], &mut discovered, &[]).expect("no-op");
        assert!(discovered.is_empty());
        merge_runtime_agents(&reg.list(), &mut discovered, &[]).expect("merges");
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].name, "rt");
        // `mergeRuntimeAgents` filters `disabled === true` (`:424`).
        let mut disabled = reg.list();
        disabled[0].disabled = Some(true);
        let mut again: Vec<AgentDefinition> = Vec::new();
        merge_runtime_agents(&disabled, &mut again, &[]).expect("filters disabled");
        assert!(again.is_empty());
    }

    #[test]
    fn merge_refuses_identity_collisions_among_runtime_agents() {
        let reg_a = registry();
        let reg_b = registry();
        let mut a = RuntimeAgentDefinition::new("A", "A.");
        a.aliases = Some(vec!["shared".into()]);
        let mut b = RuntimeAgentDefinition::new("B", "B.");
        b.aliases = Some(vec!["shared".into()]);
        reg_a.register("runtime-a", &a).expect("registers");
        reg_b.register("runtime-b", &b).expect("registers");
        let mut runtime = reg_a.list();
        runtime.extend(reg_b.list());
        let mut discovered: Vec<AgentDefinition> = Vec::new();
        let err = merge_runtime_agents(&runtime, &mut discovered, &[]).expect_err("collides");
        assert_eq!(
            err.to_string(),
            "Runtime agent registration collision for 'shared' between 'runtime-a' and 'runtime-b'."
        );
        assert!(discovered.is_empty(), "discovered is untouched on error");
    }
}
