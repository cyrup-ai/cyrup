//! VL-S11a — `/subagents`, the admin surface. Port of pi `src/slash/subagents-admin.ts` @v0.68.0
//! (460 lines), registered at `slash-commands.ts:869-875` with upstream's own description
//! (*"Administer subagents: inspect metadata and update models, thinking, or prompts"*) and the
//! handler `openSubagentsAdmin(pi, ctx, args)` (`subagents-admin.ts:396`).
//!
//! Every function below names the upstream symbol and its v0.68.0 line. The shapes this module
//! exports — [`EditableOverrideField`] (`:117`), [`AgentSelection`] (`:119-123`),
//! [`AdminAction`] (`:418-423`) and [`SubagentsAdminError`] (`readOnlyAgentMessage`, `:150-160`) —
//! are the ones the slash handler [`crate::extension::host::SubagentsExtension::slash_subagents`]
//! drives.
//!
//! ## The three seams where cyrup's primitives differ from pi's, and what was done instead
//!
//! 1. **`allVisibleAgents` (`:32`) reads a MERGED view, not four raw tiers.** Upstream concatenates
//!    `d.project`/`d.user`/`d.package`/`d.builtin` — per-tier arrays that already carry their
//!    settings overrides (`buildAllDiscovery`, `agents.ts:2806-2848`). cyrup's tier scan
//!    ([`crate::discovery::scan_agent_tiers`]) runs BEFORE override application, so it cannot feed
//!    [`saves_through_settings`], which reads [`AgentDefinition::override_info`] and has no other
//!    data source. [`crate::discovery::discover_agents_all`] is therefore the entry point: the
//!    merged, override-applied, disabled-included management view. The consequence is that one
//!    runtime NAME resolves to one agent, so [`AgentSelection::Ambiguous`] arises only from
//!    [`agent_matches`]'s second clause — a package agent whose LOCAL name equals another agent's
//!    runtime name — rather than from same-name definitions in two tiers.
//! 2. **`metadataFor` (`:185`) is cyrup's `format_agent_detail`** (`discovery/management/render.rs:39`),
//!    reached through the public [`handle_management_action`] `get` dispatch because `render` is a
//!    private module of `discovery::management`. cyrup's renderer is RICHER than upstream's
//!    (aliases, fallback models, acceptance, acceptance role, extensions); the one upstream line it
//!    does not emit is `Override: <scope> (<path>)` (`:211`), which belongs in `render.rs` and is
//!    not this module's to add — writing a second renderer here is what the reuse rule exists to
//!    prevent.
//! 3. **`selectFromList` (`:216`) takes upstream's `ctx.ui.select` branch, never `ctx.ui.custom`.**
//!    Upstream picks between them on `typeof ctx.ui.custom === "function"` — a test cyrup cannot
//!    make: [`HostServices::custom`] is always present and answers `Option<String>`, whose `None`
//!    cannot be told apart from a dismissal (its default impl returns `None`, and so does pi's own
//!    RPC mode). Preferring it would turn "this host paints no custom overlay" into a spurious
//!    cancel. `ctx.ui.select` is upstream's own documented fallback (`:224-227`), and the flat
//!    title (`title` + `"\nCurrent: "` + subtitle) is upstream's too.

use std::path::{Path, PathBuf};

use cyrup_ext::host::HostServices;
use cyrup_ext::{DialogOptions, NotifyKind};
use serde_json::{Value, json};

use crate::discovery::management::helpers::{override_scope_str, source_str};
use crate::discovery::management::{ManagementRequest, handle_management_action};
use crate::discovery::settings_write::{
    merge_builtin_agent_override, remove_builtin_agent_override_fields,
};
use crate::discovery::types::{AgentDefinition, AgentSource, OverrideScope};
use crate::discovery::{AgentDiscoveryConfig, discover_agents_all};
use crate::error::SubagentError;
use crate::exec::output::normalize_lexically;
use crate::exec::spawn_plan::{THINKING_LEVELS, split_known_thinking_suffix};
use crate::extension::models::registry_models;

/// pi `INHERIT_MODEL_CHOICE` (`subagents-admin.ts:20`).
const INHERIT_MODEL_CHOICE: &str = "Default / inherit session model";
/// pi `INHERIT_THINKING_CHOICE` (`subagents-admin.ts:21`).
const INHERIT_THINKING_CHOICE: &str = "Default / inherit session thinking";

/// The rendering of an absent model/thinking value in [`metadata_summary`] (pi `:357-358`).
const DEFAULT_INHERIT_LABEL: &str = "default / inherit";

// -------------------------------------------------------------------------------------------
// Shapes
// -------------------------------------------------------------------------------------------

/// pi `EditableOverrideField` (`subagents-admin.ts:117`): the three fields `/subagents` can edit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EditableOverrideField {
    Model,
    Thinking,
    SystemPrompt,
}

impl EditableOverrideField {
    /// The settings key this field occupies inside `subagents.agentOverrides.<name>` — and, because
    /// upstream interpolates the same token into its refusals (`readOnlyAgentMessage`, `:152/:155`),
    /// the word that appears in [`SubagentsAdminError`]'s sentences. camelCase, exactly as
    /// `applyBuiltinOverride` records it in `BuiltinAgentOverrideInfo.fields` (`agents.ts:1418`).
    pub(crate) fn settings_key(self) -> &'static str {
        match self {
            EditableOverrideField::Model => "model",
            EditableOverrideField::Thinking => "thinking",
            EditableOverrideField::SystemPrompt => "systemPrompt",
        }
    }

    /// This field's EFFECTIVE value on `agent`, as a string — the left-hand side of
    /// `savesThroughSettings`'s final comparison (`:136`) and of `persistSettingsField`'s
    /// base comparison (`:284`, `:295`).
    ///
    /// pi's `thinking` is `string | false`; cyrup's is `Option<String>` where `Some("off")` IS the
    /// explicit off ([`AgentDefinition::thinking`]'s own doc), so upstream's
    /// `base.thinking === false ? "off" : base.thinking` collapse (`:288`) is already applied at
    /// parse time and needs no mapping here.
    pub(crate) fn value_of(self, agent: &AgentDefinition) -> Option<String> {
        match self {
            EditableOverrideField::Model => agent.model.as_ref().map(ToString::to_string),
            EditableOverrideField::Thinking => agent.thinking.clone(),
            // pi `AgentConfig.systemPrompt` is a plain `string`, never undefined.
            EditableOverrideField::SystemPrompt => Some(agent.system_prompt_body.clone()),
        }
    }
}

/// pi `AgentSelection` (`subagents-admin.ts:119-123`) — a closed enum, not an `Option` plus a
/// message string, so every caller has to handle all four outcomes.
pub(crate) enum AgentSelection {
    Selected(Box<AgentDefinition>),
    Cancelled,
    NotFound {
        agents: Vec<AgentDefinition>,
        requested: Option<String>,
    },
    Ambiguous {
        requested: String,
        matches: Vec<AgentDefinition>,
    },
}

/// pi `subagents-admin.ts:418-423` — the requested action token, parsed once from `args`'s SECOND
/// word, and the five labels the interactive picker offers (`:427`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AdminAction {
    ChangeModel,
    ChangeThinking,
    EditSystemPrompt,
    ShowDetails,
    Done,
}

impl AdminAction {
    /// pi `:420-423`: the argument token → action mapping. Lowercased by the caller. `Done` has no
    /// token upstream (it is only ever a picker choice), so it is not produced here.
    pub(crate) fn from_token(token: &str) -> Option<Self> {
        match token {
            "model" => Some(AdminAction::ChangeModel),
            "thinking" => Some(AdminAction::ChangeThinking),
            "prompt" | "system-prompt" | "edit" => Some(AdminAction::EditSystemPrompt),
            "details" | "info" => Some(AdminAction::ShowDetails),
            _ => None,
        }
    }

    /// pi's picker labels (`:427`), which are also the strings its `if/else` chain compares
    /// against (`:432`, `:439`, `:446`, `:451`) — so label and action can never drift apart here.
    pub(crate) fn label(self) -> &'static str {
        match self {
            AdminAction::ChangeModel => "Change model",
            AdminAction::ChangeThinking => "Change thinking level",
            AdminAction::EditSystemPrompt => "Edit system prompt",
            AdminAction::ShowDetails => "Show details",
            AdminAction::Done => "Done",
        }
    }

    /// The five labels in upstream's order (`:427`).
    pub(crate) const ALL: [AdminAction; 5] = [
        AdminAction::ChangeModel,
        AdminAction::ChangeThinking,
        AdminAction::EditSystemPrompt,
        AdminAction::ShowDetails,
        AdminAction::Done,
    ];

    fn from_label(label: &str) -> Option<Self> {
        AdminAction::ALL.into_iter().find(|a| a.label() == label)
    }
}

/// pi `readOnlyAgentMessage` (`subagents-admin.ts:150-160`) — the three refusals, verbatim.
///
/// Only the ENV VAR NAME in the third differs from upstream's (`PI_SUBAGENT_EXTRA_AGENT_DIRS` →
/// [`crate::discovery::EXTRA_AGENT_DIRS_ENV_VAR`]), because that is cyrup's own variable; the
/// sentence SHAPE is upstream's byte-for-byte.
///
/// Upstream surfaces these as a RESULT, not a throw: `saveAgentModel` returns the string
/// (`:317-318`) and `openSubagentsAdmin` posts it with `ctx.ui.notify(message, "info")` (`:437`).
/// This enum exists so the three sentences have ONE spelling; [`read_only_agent_message`] renders
/// it onto upstream's own success channel.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SubagentsAdminError {
    #[error(
        "Cannot update '{agent}' {field} because that agent is runtime-registered by an extension; \
         edit its source definition instead."
    )]
    RuntimeRegistered { agent: String, field: &'static str },
    #[error(
        "Cannot update '{agent}' {field} because that field is owned by its read-only package \
         definition."
    )]
    PackageOwned { agent: String, field: &'static str },
    #[error(
        "Cannot update '{agent}' because its definition in CYRUP_SUBAGENT_EXTRA_AGENT_DIRS is \
         read-only."
    )]
    ReadOnlyExtraDir { agent: String },
}

/// Everything `openSubagentsAdmin`'s `(pi, ctx)` pair carries that this port actually reads.
///
/// `services` is [`crate::extension::SubagentExecutor::host_services`] — pi's `ctx.ui` plus
/// `ctx.model`. `has_ui` is pi's `ctx.hasUI`, but see [`Self::interactive`].
pub(crate) struct AdminContext<'a> {
    pub(crate) cfg: &'a AgentDiscoveryConfig,
    pub(crate) cwd: &'a Path,
    pub(crate) services: Option<&'a dyn HostServices>,
    pub(crate) has_ui: bool,
    /// The already-resolved [`crate::discovery::EXTRA_AGENT_DIRS_ENV_VAR`] entries — always
    /// [`crate::paths::Roots::extra_agent_dirs`] of the SAME roots `cfg` was built from, so this
    /// check and discovery agree about which agents are extra-dir agents. Injected rather than
    /// read from the process environment, so a caller pins it with
    /// [`crate::paths::Roots::with_extra_agent_dirs`] instead of `std::env::set_var`.
    pub(crate) extra_agent_dirs: Vec<PathBuf>,
}

impl AdminContext<'_> {
    /// pi's `ctx.hasUI`, tightened by the one condition upstream has no analogue for: a session
    /// that reports a UI but bound no capability backend cannot be prompted at all. Treating that
    /// as interactive would turn every dialog into an instant dismissal — i.e. `/subagents scout`
    /// would silently print nothing — so it takes the no-UI text path instead, which is the
    /// honest answer and the one upstream's own `!ctx.hasUI` branch gives.
    fn interactive(&self) -> bool {
        self.has_ui && self.services.is_some()
    }
}

// -------------------------------------------------------------------------------------------
// Listing, labelling, matching (`:25`-`:63`)
// -------------------------------------------------------------------------------------------

/// pi `sourceRank` (`subagents-admin.ts:25-30`): `project` 0, `user` 1, `package` 2, everything
/// else 3.
///
/// cyrup already has this exact function — [`AgentSource::precedence_rank`] ranks
/// Project/Runtime 0, User 1, Package 2, Builtin 3 — so this is an alias, not a second table. The
/// `runtime_ranks_with_project` test below is what stops the two from drifting.
pub(crate) fn source_rank(source: AgentSource) -> u8 {
    source.precedence_rank()
}

/// pi `allVisibleAgents(pi, cwd)` (`subagents-admin.ts:32-39`): every non-disabled agent, sorted by
/// name then [`source_rank`].
///
/// Upstream hides disabled definitions from the panel while keeping them in the collision checks
/// (`:36`); cyrup's [`discover_agents_all`] is the management view (which by R-SA-013 INCLUDES
/// disabled agents) and its merge has already run every collision check, so the filter here is the
/// same statement. Runtime agents are already folded in by `run_discovery`
/// (pi's `mergeRuntimeAgents`, `:37`).
///
/// The sort uses byte order where upstream uses `localeCompare`; that is this crate's standing
/// convention for every discovery listing (`run_discovery` itself sorts `a.name.cmp(&b.name)`).
///
/// # Errors
///
/// Propagates R-SA-009's malformed-settings abort from [`discover_agents_all`].
pub(crate) fn all_visible_agents(
    cfg: &AgentDiscoveryConfig,
) -> Result<Vec<AgentDefinition>, SubagentError> {
    let mut agents: Vec<AgentDefinition> = discover_agents_all(cfg)?
        .agents
        .into_iter()
        .filter(|agent| agent.disabled != Some(true))
        .collect();
    agents.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| source_rank(a.source).cmp(&source_rank(b.source)))
    });
    Ok(agents)
}

/// pi `agentLabel` (`subagents-admin.ts:41-44`): `name [source] · model — description`, where the
/// model segment is present only when the agent pins a model.
pub(crate) fn agent_label(agent: &AgentDefinition) -> String {
    let model = match &agent.model {
        Some(model) => format!(" · {model}"),
        None => String::new(),
    };
    format!(
        "{} [{}]{model} — {}",
        agent.name,
        source_str(agent.source),
        agent.description
    )
}

/// pi `agentChoices` (`subagents-admin.ts:46-54`): label → agent, with the agent's file path
/// appended to any label that more than one agent produced, so the picker never offers two
/// indistinguishable rows.
///
/// Upstream builds a `Map`, whose semantics on a duplicate KEY are "keep the first position, take
/// the last value"; this reproduces them explicitly rather than relying on `Vec` order, because the
/// duplicate case is exactly the one the `filePath` suffix is there to make impossible and a silent
/// divergence would only show up when it failed to.
pub(crate) fn agent_choices(agents: &[AgentDefinition]) -> Vec<(String, &AgentDefinition)> {
    let labels: Vec<String> = agents.iter().map(agent_label).collect();
    let mut out: Vec<(String, &AgentDefinition)> = Vec::with_capacity(agents.len());
    for (index, agent) in agents.iter().enumerate() {
        let Some(label) = labels.get(index) else {
            continue;
        };
        let count = labels.iter().filter(|other| *other == label).count();
        let key = if count == 1 {
            label.clone()
        } else {
            format!("{label} · {}", agent.file_path.display())
        };
        // `iter().position(..)` then `get_mut(..)`, not `iter_mut().find(..)`: the latter keeps a
        // mutable borrow of `out` alive across BOTH match arms, and the `None` arm pushes.
        match out.iter().position(|(existing, _)| *existing == key) {
            Some(index) => {
                if let Some(slot) = out.get_mut(index) {
                    slot.1 = agent;
                }
            }
            None => out.push((key, agent)),
        }
    }
    out
}

/// pi `agentSelectItems` (`subagents-admin.ts:56-58`): the picker's values, which for the agent
/// picker are the labels themselves (`{ value: label, label }`).
pub(crate) fn agent_select_items(by_label: &[(String, &AgentDefinition)]) -> Vec<String> {
    by_label.iter().map(|(label, _)| label.clone()).collect()
}

/// pi `agentMatches` (`subagents-admin.ts:60-63`):
/// `agent.name === name || frontmatterNameForConfig(agent) === name`, on the TRIMMED request.
///
/// `frontmatterNameForConfig` (`agents/identity.ts:24-30`) returns `config.localName` whenever it
/// is set, and cyrup's [`AgentDefinition::local_name`] is always set (R-SA-008 computes `name` FROM
/// it), so upstream's two fallbacks — strip the `package.` prefix, else the runtime name — are
/// unreachable here and `local_name` is the whole of it.
pub(crate) fn agent_matches(agent: &AgentDefinition, raw_name: &str) -> bool {
    let name = raw_name.trim();
    agent.name == name || agent.local_name == name
}

// -------------------------------------------------------------------------------------------
// Read-only / settings-vs-frontmatter routing (`:125`-`:160`)
// -------------------------------------------------------------------------------------------

/// pi `savesThroughSettings` (`subagents-admin.ts:125-137`): does an edit to `field` belong in
/// `settings.json`, or in the agent's own frontmatter?
///
/// ```text
/// if (agent.source === "builtin") return true;                       // :126
/// if (agent.source === "package") return true;                       // :127
/// if (!agent.override) return false;                                 // :128
/// if (agent.override.fields?.includes(field) === true) return true;  // :129
/// if (agent.source !== agent.override.scope) return false;           // :133
/// return agent[field] !== agent.override.base[field];                // :136
/// ```
///
/// Clause `:129` reads [`crate::discovery::types::AgentOverrideInfo::fields`] and has no other data
/// source. Clause `:133` is the one that keeps a lower-scope override from being rewritten on
/// behalf of a higher-scope custom agent that merely inherited it; clause `:136` compares the
/// EFFECTIVE value with the pre-override base so an override on one field does not redirect edits
/// to an unrelated frontmatter-owned field.
///
/// **`:128` is a test on the PROVENANCE and `:129` a test on the KEY SET, and they are not the
/// same test.** `fields` can be EMPTY on an agent that carries an `override_info`: cyrup's
/// `clear_builtin_thinking` (pi `clearBuiltinThinking`, `agents.ts:1466-1470`) records provenance
/// for the global `subagents.disableThinking` flag while contributing no key, because that flag is
/// not a `subagents.agentOverrides.<name>.thinking` entry. Writing `override_info.is_some()` in
/// place of clause `:129` would therefore send `/subagents` off to rewrite a per-agent override
/// that does not exist; the disable-thinking agent is covered by the base comparison at `:133-136`
/// instead, which is exactly why upstream keeps the two clauses apart.
///
/// The keys compared here are the on-disk camelCase spellings
/// [`crate::discovery::types::AgentOverrideConfig`] serializes (`model`, `thinking`,
/// `systemPrompt`), which are pi's `EditableOverrideField` strings (`:117`) unchanged.
pub(crate) fn saves_through_settings(
    agent: &AgentDefinition,
    field: EditableOverrideField,
) -> bool {
    if matches!(agent.source, AgentSource::Builtin | AgentSource::Package) {
        return true;
    }
    let Some(info) = agent.override_info.as_ref() else {
        return false;
    };
    if info.fields.contains(field.settings_key()) {
        return true;
    }
    if !source_is_scope(agent.source, info.scope) {
        return false;
    }
    field.value_of(agent) != field.value_of(&info.base_snapshot)
}

/// pi's `agent.source !== agent.override.scope` (`subagents-admin.ts:133`), which compares a
/// five-valued string against a two-valued one. Only `user`/`project` can ever be equal; `builtin`,
/// `package` and `runtime` never are (and the first two never reach this line).
fn source_is_scope(source: AgentSource, scope: OverrideScope) -> bool {
    matches!(
        (source, scope),
        (AgentSource::User, OverrideScope::User) | (AgentSource::Project, OverrideScope::Project)
    )
}

/// pi `isReadOnlyExtraAgent` (`subagents-admin.ts:139-148`): is this agent's file inside one of the
/// extra agent directories, which are a read-only fallback stream?
///
/// Upstream's containment test is
/// `relative !== "" && relative !== ".." && !relative.startsWith("../") && !path.isAbsolute(relative)`
/// (`:146`) — i.e. the file is STRICTLY inside the root. Here that is
/// [`Path::strip_prefix`] (component-wise, so `/extra-dirs` is not a prefix of `/extra-dirsX`)
/// plus a non-equality check (so the directory itself is not "inside" itself).
///
/// Both sides are made absolute against `cwd` and lexically normalized first, which is Node's
/// `path.resolve` (`:142`, `:144`). [`std::path::absolute`] alone is not enough: it deliberately
/// KEEPS `..` components, so `<root>/../elsewhere/a.md` would strip as if it were contained.
///
/// `agent.source !== "user"` short-circuits (`:141`): the extras stream is prepended to the User
/// tier and nothing else.
pub(crate) fn is_read_only_extra_agent(
    agent: &AgentDefinition,
    cwd: &Path,
    extra_dirs: &[PathBuf],
) -> bool {
    if extra_dirs.is_empty() || agent.source != AgentSource::User {
        return false;
    }
    let file = resolve_against(cwd, &agent.file_path);
    extra_dirs.iter().any(|dir| {
        let root = resolve_against(cwd, dir);
        file != root && file.strip_prefix(root.as_path()).is_ok()
    })
}

/// Node `path.resolve(base, p)`'s pure-lexical half: make `p` absolute against `base` when it is
/// relative, then normalize away `.`/`..` with [`normalize_lexically`].
fn resolve_against(base: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    normalize_lexically(&joined)
}

/// pi `readOnlyAgentMessage` (`subagents-admin.ts:150-160`): the refusal for this agent+field, or
/// `None` when the edit may proceed. Order is upstream's — runtime, then package, then the
/// extra-directory check.
///
/// **The PACKAGE branch is defensive at v0.68.0, not a live production path**, and the port keeps
/// it that way on purpose. Every caller consults this function only AFTER
/// [`saves_through_settings`] answered `false` (`:317`, `:340`, `:372`, `:384`), and that function
/// returns `true` for every package agent at `:127` — so a package agent is always routed into the
/// settings-override branch and never reaches this refusal. That is the designed behaviour, not an
/// oversight: a package definition is read-only, but a settings override ON it is not. The two
/// branches that ARE reachable are `Runtime` (no `override_info`, so `:128` returns `false`) and
/// the extra-directory case (a `User`-tier agent, same reason).
pub(crate) fn read_only_agent_message(
    agent: &AgentDefinition,
    field: EditableOverrideField,
    cwd: &Path,
    extra_dirs: &[PathBuf],
) -> Option<SubagentsAdminError> {
    match agent.source {
        AgentSource::Runtime => Some(SubagentsAdminError::RuntimeRegistered {
            agent: agent.name.clone(),
            field: field.settings_key(),
        }),
        AgentSource::Package => Some(SubagentsAdminError::PackageOwned {
            agent: agent.name.clone(),
            field: field.settings_key(),
        }),
        _ if is_read_only_extra_agent(agent, cwd, extra_dirs) => {
            Some(SubagentsAdminError::ReadOnlyExtraDir {
                agent: agent.name.clone(),
            })
        }
        _ => None,
    }
}

// -------------------------------------------------------------------------------------------
// Rendering (`:185`, `:354`)
// -------------------------------------------------------------------------------------------

/// pi `metadataFor` (`subagents-admin.ts:185-214`) → cyrup's `format_agent_detail`
/// (`discovery/management/render.rs:39-140`), reached through the public `get` dispatch. See this
/// module's doc, seam 2, for why it is not called directly and what the one difference is.
///
/// # Errors
///
/// Propagates R-SA-009's malformed-settings abort from the re-discovery `get` performs.
pub(crate) async fn metadata_for(
    cfg: &AgentDiscoveryConfig,
    agent: &AgentDefinition,
) -> Result<String, SubagentError> {
    let request = ManagementRequest {
        agent: Some(agent.name.as_str()),
        chain_name: None,
        agent_scope: None,
        config: None,
        current_session_model: None,
        proactive_skills: None,
    };
    Ok(handle_management_action(cfg, "get", &request).await?.text)
}

/// pi `metadataSummary` (`subagents-admin.ts:353-360`): the compact one-liner the interactive
/// picker's title carries, deliberately WITHOUT the full system-prompt dump `metadataFor` ends
/// with. Three fields joined by `" · "`.
pub(crate) fn metadata_summary(agent: &AgentDefinition) -> String {
    let model = agent
        .model
        .as_ref()
        .map_or_else(|| DEFAULT_INHERIT_LABEL.to_string(), ToString::to_string);
    // pi `agent.thinking === false ? "off" : agent.thinking ?? "default / inherit"` — cyrup's
    // `Some("off")` IS the explicit off, so the ternary is already resolved.
    let thinking = agent
        .thinking
        .clone()
        .unwrap_or_else(|| DEFAULT_INHERIT_LABEL.to_string());
    format!(
        "Source: {} · Model: {model} · Thinking: {thinking}",
        source_str(agent.source)
    )
}

// -------------------------------------------------------------------------------------------
// The UI primitives (`:216`, `:230`, `:246`, `:265`)
// -------------------------------------------------------------------------------------------

/// pi `selectFromList` (`subagents-admin.ts:216-228`), flat-title branch — see this module's doc,
/// seam 3, for why that branch and not `ctx.ui.custom`.
///
/// `None` is upstream's `undefined`: a dismissal, a timeout, or no interactive surface. Every
/// caller treats it as a cancel.
fn select_from_list(
    services: &dyn HostServices,
    title: &str,
    subtitle: Option<&str>,
    values: &[String],
) -> Option<String> {
    let flat_title = match subtitle {
        Some(subtitle) => format!("{title}\nCurrent: {subtitle}"),
        None => title.to_string(),
    };
    let options = Value::Array(values.iter().cloned().map(Value::String).collect());
    let choice = services.select(&flat_title, &options, &DialogOptions::default())?;
    // pi `items.find((item) => item.value === choice)?.value ?? labelToValue.get(choice)` (`:227`):
    // value and label are the same string on every list this module builds, so one lookup covers
    // both arms, and a host answering something that is on neither list is `undefined`/cancel.
    values.iter().find(|value| **value == choice).cloned()
}

/// pi `modelFullId` (`subagents-admin.ts:73-75`): `` `${provider}/${id}` ``.
fn model_full_id(model: &cyrup_provider::Model) -> String {
    format!("{}/{}", model.provider.as_str(), model.id.as_str())
}

/// pi `liveAvailableModels(ctx)` (`subagents-admin.ts:77-92`) → [`registry_models`].
///
/// Upstream's body is a best-effort `ctx.modelRegistry.refresh({ allowNetwork: false })` followed
/// by `getAvailable()`, with three `ctx.ui.notify(…, "warning")` degrades. cyrup's registry is
/// [`cyrup_provider::catalog::builtin_catalog`] — a static, already-loaded catalog with no refresh
/// step and therefore no failure mode to warn about, so the whole refresh half has no counterpart
/// and is not simulated. See [`registry_models`]' own `[CYRUP-DELTA]` for the credential-blindness
/// difference from pi's `getAvailable()`.
fn live_available_models() -> &'static [cyrup_provider::Model] {
    registry_models()
}

/// pi `findModelInfo` (`shared/model-info.ts:74-86`): resolve a model string — bare or
/// fully-qualified, with or without a thinking suffix — against the available list.
///
/// Ported here rather than reused: `extension/models`' own resolver
/// (`resolve_model_candidate`) answers a different question (it returns a full id STRING, falling
/// back to the input when nothing matches) and is private to that module. This one returns the
/// matched registry entry or nothing, which is what [`supported_thinking_levels`] needs.
fn find_model_info<'a>(
    model: Option<&str>,
    available: &'a [cyrup_provider::Model],
    preferred_provider: Option<&str>,
) -> Option<&'a cyrup_provider::Model> {
    let model = model.filter(|m| !m.is_empty())?;
    if available.is_empty() {
        return None;
    }
    let (base_model, _) = split_known_thinking_suffix(model);
    if let Some(exact) = available
        .iter()
        .find(|entry| model_full_id(entry) == base_model)
    {
        return Some(exact);
    }
    let matches: Vec<&cyrup_provider::Model> = available
        .iter()
        .filter(|entry| entry.id.as_str() == base_model)
        .collect();
    if let Some(preferred) = preferred_provider
        && let Some(hit) = matches
            .iter()
            .copied()
            .find(|entry| entry.provider.as_str() == preferred)
    {
        return Some(hit);
    }
    match matches.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// pi `getSupportedThinkingLevels` (`shared/model-info.ts:88-101`): the reasoning levels this model
/// offers.
///
/// * no model info → every level except `max`
/// * `reasoning === false` → `["off"]` only
/// * no `thinkingLevelMap` → every level except `max`
/// * otherwise → every level whose map entry is not `null`, and for `xhigh`/`max` only when the map
///   names it at all
pub(crate) fn supported_thinking_levels(model: Option<&cyrup_provider::Model>) -> Vec<String> {
    let all_but_max = || -> Vec<String> {
        THINKING_LEVELS
            .iter()
            .filter(|level| **level != "max")
            .map(|level| (*level).to_string())
            .collect()
    };
    let Some(model) = model else {
        return all_but_max();
    };
    if !model.reasoning {
        return vec!["off".to_string()];
    }
    let Some(map) = model.thinking_level_map.as_ref() else {
        return all_but_max();
    };
    THINKING_LEVELS
        .iter()
        .filter(|level| match map.get(**level) {
            // pi `mapped === null` — the level is explicitly unsupported.
            Some(None) => false,
            Some(Some(_)) => true,
            // pi `mapped === undefined`: the two top rungs must be NAMED to count.
            None => !matches!(**level, "xhigh" | "max"),
        })
        .map(|level| (*level).to_string())
        .collect()
}

/// pi `chooseModel` (`subagents-admin.ts:230-244`).
///
/// `None` is upstream's `null` (`:242`, cancelled); `Some(None)` is its `undefined` (`:243`, the
/// inherit choice); `Some(Some(id))` is a concrete `provider/id`.
fn choose_model(services: &dyn HostServices, agent: &AgentDefinition) -> Option<Option<String>> {
    let models = live_available_models();
    let agent_model = agent.model.as_ref().map(ToString::to_string);
    let current = agent_model
        .clone()
        .unwrap_or_else(|| INHERIT_MODEL_CHOICE.to_string());

    let mut values = vec![INHERIT_MODEL_CHOICE.to_string()];
    // pi `:234-236`: an agent pinned to a model the registry does not list keeps that model
    // selectable, so opening the picker cannot silently drop it.
    if let Some(model) = agent_model.as_ref()
        && !models
            .iter()
            .any(|entry| model_full_id(entry) == model.as_str())
    {
        values.push(model.clone());
    }
    for model in models {
        values.push(model_full_id(model));
    }

    let choice = select_from_list(
        services,
        &format!("Select model for {}", agent.name),
        Some(current.as_str()),
        &values,
    )?;
    if choice == INHERIT_MODEL_CHOICE {
        Some(None)
    } else {
        Some(Some(choice))
    }
}

/// pi `chooseThinking` (`subagents-admin.ts:246-263`).
///
/// Same three-state return as [`choose_model`]. `session_model` is pi's `ctx.model`, rendered
/// `provider/id` by [`HostServices::current_model`].
fn choose_thinking(
    services: &dyn HostServices,
    agent: &AgentDefinition,
    session_model: Option<&str>,
) -> Option<Option<String>> {
    let available = live_available_models();
    let agent_model = agent.model.as_ref().map(ToString::to_string);
    let effective_model = agent_model
        .clone()
        .or_else(|| session_model.map(ToString::to_string));
    // pi `ctx.model?.provider` — the provider half of the live session model.
    let preferred_provider = session_model.and_then(|m| m.split('/').next());
    let model_info = find_model_info(effective_model.as_deref(), available, preferred_provider);
    let levels = supported_thinking_levels(model_info);

    // pi `agent.thinking === false ? "off" : agent.thinking ?? INHERIT_THINKING_CHOICE` (`:251`).
    let current = agent
        .thinking
        .clone()
        .unwrap_or_else(|| INHERIT_THINKING_CHOICE.to_string());
    let mut values = vec![INHERIT_THINKING_CHOICE.to_string()];
    values.extend(levels);
    // pi `:253`: an agent carrying a level this model does not offer keeps it selectable, inserted
    // directly after the inherit choice.
    if current != INHERIT_THINKING_CHOICE && !values.contains(&current) {
        values.insert(1, current.clone());
    }

    let model_note = match (&agent_model, &effective_model) {
        (Some(model), _) => format!("Model: {model}"),
        (None, Some(effective)) => format!("Session model: {effective}"),
        (None, None) => "Model: default / inherit".to_string(),
    };

    let subtitle = format!("{model_note} · {current}");
    let choice = select_from_list(
        services,
        &format!("Select thinking level for {}", agent.name),
        Some(subtitle.as_str()),
        &values,
    )?;
    if choice == INHERIT_THINKING_CHOICE {
        Some(None)
    } else {
        Some(Some(choice))
    }
}

/// Is there a project settings scope at all? pi's `d.projectSettingsPath` (`:268`, `:270`), which
/// is `null` outside any project.
///
/// #84: this used to ALSO require `cfg.project_root`, on the theory that the path was always
/// present and the root was the real question. Neither held: the producer
/// (`extension/executor/resolve.rs`, `discovery_config_on_disk`) set the path for every cwd, and
/// `project_root` falls back to the cwd too, so both were always `Some` and the check could not
/// fail — `/subagents` offered a project scope outside any project. The producer now yields `None`
/// exactly when pi's `getProjectAgentSettingsPath` returns `null`, so the path IS the answer, and
/// it is the one [`persist_settings_field`] will actually write.
fn project_settings_path(cfg: &AgentDiscoveryConfig) -> Option<&Path> {
    cfg.override_settings.project_settings_path.as_deref()
}

/// The scope → settings-file path resolution `mergeBuiltinAgentOverride(ctx.cwd, name, scope, …)`
/// performs internally upstream and cyrup's writers leave to the caller — the same resolution
/// `discovery/management/tier_actions.rs:127-140` does for `disable`/`enable`/`reset`.
fn scope_settings_path(cfg: &AgentDiscoveryConfig, scope: OverrideScope) -> Option<PathBuf> {
    match scope {
        OverrideScope::User => Some(cfg.override_settings.user_settings_path.clone()),
        OverrideScope::Project => project_settings_path(cfg).map(Path::to_path_buf),
    }
}

/// pi `chooseOverrideScope` (`subagents-admin.ts:265-273`).
///
/// `None` is upstream's `undefined` — the human dismissed the scope picker, which cancels the save.
fn choose_override_scope(ctx: &AdminContext<'_>, agent: &AgentDefinition) -> Option<OverrideScope> {
    let has_project = project_settings_path(ctx.cfg).is_some();
    if let Some(info) = agent.override_info.as_ref() {
        // pi `:268`: a PROJECT agent carrying a USER-scope override edits at project scope when a
        // project settings file exists, so the edit shadows rather than rewrites the shared
        // user-scope entry. Otherwise the existing scope is kept.
        let promote = agent.source == AgentSource::Project
            && info.scope == OverrideScope::User
            && has_project;
        return Some(if promote {
            OverrideScope::Project
        } else {
            info.scope
        });
    }
    if !has_project || !ctx.interactive() {
        return Some(OverrideScope::User);
    }
    let services = ctx.services?;
    let choice = services.select(
        &format!("Save subagent override for {}", agent.name),
        &json!(["user", "project"]),
        &DialogOptions::default(),
    )?;
    match choice.as_str() {
        "user" => Some(OverrideScope::User),
        "project" => Some(OverrideScope::Project),
        _ => None,
    }
}

// -------------------------------------------------------------------------------------------
// Persistence (`:275`-`:394`)
// -------------------------------------------------------------------------------------------

/// pi's `shadowsLowerScope` (`subagents-admin.ts:283`): the field being saved at PROJECT scope
/// already has a USER-scope contributor, so simply deleting the project key would let the user
/// value win again instead of clearing the edit.
///
/// Reads [`crate::discovery::types::AgentOverrideInfo::field_scopes`] and has no other data source.
///
/// It CANNOT be answered from [`crate::discovery::types::AgentOverrideInfo::scope`]: that field
/// names the LAST pass that applied (pi spreads `{...meta}` first at `agents.ts:1416`), not the
/// only scope involved, and `base_snapshot` is likewise sticky — pi's
/// `base: agent.override?.base ?? cloneOverrideBase(agent)` (`:1417`) keeps the state from before
/// the FIRST override, not the last. `field_scopes` is the per-key truth.
///
/// A two-scope entry is custom-agent-only. A builtin's entries are always singletons because
/// `applyBuiltinOverrides` (`agents.ts:1472-1512`) is winner-take-all — upstream's shape, not a
/// gap, so a builtin never takes the restore-the-base branch below.
pub(crate) fn shadows_lower_scope(
    agent: &AgentDefinition,
    scope: OverrideScope,
    field: EditableOverrideField,
) -> bool {
    scope == OverrideScope::Project
        && agent.override_info.as_ref().is_some_and(|info| {
            info.field_scopes
                .get(field.settings_key())
                .is_some_and(|scopes| scopes.contains(&OverrideScope::User))
        })
}

/// pi `persistSettingsField` (`subagents-admin.ts:275-305`): write (or clear) one field of
/// `subagents.agentOverrides.<name>` at `scope`, returning the file written and whether an override
/// now stands.
///
/// Three branches, upstream's:
/// 1. [`shadows_lower_scope`] and the new value is absent-or-equal-to-base (`:284-294`) → write an
///    EXPLICIT value that restores the base (`false` when the base has none), because removing the
///    project key would uncover the user one.
/// 2. the new value is absent-or-equal-to-base (`:295-300`) → remove just this field.
/// 3. otherwise (`:301-304`) → merge the new value in.
///
/// # Errors
///
/// [`SubagentError::MalformedSettings`] for an unreadable/non-object settings file,
/// [`SubagentError::Spawn`] for a write failure (both from the settings writers), and
/// [`SubagentError::Management`] when `scope` is `Project` but there is no project scope here.
pub(crate) async fn persist_settings_field(
    cfg: &AgentDiscoveryConfig,
    agent: &AgentDefinition,
    scope: OverrideScope,
    field: EditableOverrideField,
    value: Option<&str>,
) -> Result<(PathBuf, bool), SubagentError> {
    let Some(path) = scope_settings_path(cfg, scope) else {
        return Err(SubagentError::Management(
            "Project override is not available here: no project config root (.cyrup or .agents) \
             was found above the cwd. Use agentScope: 'user' or run from inside a project."
                .to_string(),
        ));
    };
    // pi `agent.override?.base ?? buildBuiltinBase(agent)` (`:282`, `buildBuiltinBase` at `:94`):
    // the pre-override definition when one was recorded, else the agent as it stands (which is the
    // same thing — nothing has overridden it).
    let base: &AgentDefinition = agent
        .override_info
        .as_ref()
        .map_or(agent, |info| info.base_snapshot.as_ref());
    let base_value = field.value_of(base);
    let clears = value.is_none() || value.map(ToString::to_string) == base_value;

    if shadows_lower_scope(agent, scope, field) && clears {
        let restored = match value {
            Some(value) => Value::String(value.to_string()),
            // pi `value ?? base.model ?? false` (`:286`) / `value ?? base.thinking ?? false`
            // (`:288`) / `value ?? base.systemPrompt` (`:289`, always a string).
            None => match base_value {
                Some(base_value) => Value::String(base_value),
                None => Value::Bool(false),
            },
        };
        let mut fields = serde_json::Map::new();
        fields.insert(field.settings_key().to_string(), restored);
        merge_builtin_agent_override(&path, &agent.name, &fields).await?;
        return Ok((path, true));
    }

    if clears {
        remove_builtin_agent_override_fields(&path, &agent.name, &[field.settings_key()]).await?;
        return Ok((path, false));
    }

    let mut fields = serde_json::Map::new();
    fields.insert(
        field.settings_key().to_string(),
        Value::String(value.unwrap_or_default().to_string()),
    );
    merge_builtin_agent_override(&path, &agent.name, &fields).await?;
    Ok((path, true))
}

/// The non-settings half of `saveAgent*`: pi's
/// `fs.writeFileSync(updated.filePath, serializeAgent(editableAgentConfig(agent), {
/// preserveFrontmatterFields: preservedAgentFrontmatterFields(agent, { … }) }))`
/// (`subagents-admin.ts:319-324`, `:342-347`, `:374-377`).
///
/// Routed through the public `update` management dispatch, which is the ONE in-tree owner of that
/// exact sequence — `editable_base` (pi `editableAgentConfig`) → `update_agent` →
/// `preserved_frontmatter_fields` → `write_agent_file` (`discovery/management/agent_crud.rs:135-172`).
/// Calling those directly is not possible (`agent_crud`/`frontmatter_write` are private modules of
/// `discovery::management`) and would in any case be a second copy of the sequence.
///
/// `None` clears the field: cyrup's config parser reads a JSON `false` as pi's `delete`
/// (`discovery/management/config_parse.rs:148-169`, `:243-262`).
///
/// # Errors
///
/// [`SubagentError::Management`] carrying the management layer's own refusal text when the write is
/// refused, plus whatever [`handle_management_action`] propagates.
async fn write_agent_frontmatter_field(
    cfg: &AgentDiscoveryConfig,
    agent: &AgentDefinition,
    field: EditableOverrideField,
    value: Option<&str>,
) -> Result<(), SubagentError> {
    let payload = match value {
        Some(value) => Value::String(value.to_string()),
        None => Value::Bool(false),
    };
    let mut config_map = serde_json::Map::new();
    config_map.insert(field.settings_key().to_string(), payload);
    let config = Value::Object(config_map);
    // Disambiguate to the tier this agent actually came from, so an update never lands on a
    // same-named definition in the other writable scope (pi resolves by `agent.filePath`; cyrup's
    // management layer resolves by name plus this hint).
    let agent_scope = match agent.source {
        AgentSource::User => Some("user"),
        AgentSource::Project => Some("project"),
        _ => None,
    };
    let request = ManagementRequest {
        agent: Some(agent.name.as_str()),
        chain_name: None,
        agent_scope,
        config: Some(&config),
        current_session_model: None,
        proactive_skills: None,
    };
    let outcome = handle_management_action(cfg, "update", &request).await?;
    if outcome.is_error {
        return Err(SubagentError::Management(outcome.text));
    }
    Ok(())
}

/// pi `saveAgentModel` (`subagents-admin.ts:307-328`).
///
/// `Ok(None)` is upstream's `null` (`:310`) — the scope picker was dismissed, so nothing is posted.
async fn save_agent_model(
    ctx: &AdminContext<'_>,
    agent: &AgentDefinition,
    selected_model: Option<&str>,
) -> Result<Option<String>, SubagentError> {
    if saves_through_settings(agent, EditableOverrideField::Model) {
        let Some(scope) = choose_override_scope(ctx, agent) else {
            return Ok(None);
        };
        let (path, overridden) = persist_settings_field(
            ctx.cfg,
            agent,
            scope,
            EditableOverrideField::Model,
            selected_model,
        )
        .await?;
        return Ok(Some(if overridden {
            format!(
                "Saved {} settings override for '{}' with model '{}' in {}.",
                override_scope_str(scope),
                agent.name,
                // pi interpolates `${selectedModel}` unconditionally (`:313`), which renders the
                // literal `undefined` on the restore-the-base branch above. Reproduced rather than
                // "fixed": the string is upstream's, and silently substituting a different value
                // would diverge exactly where the two implementations are hardest to compare.
                selected_model.unwrap_or("undefined"),
                path.display()
            )
        } else {
            format!(
                "Cleared model settings override for '{}' in {}.",
                agent.name,
                path.display()
            )
        }));
    }

    if let Some(refusal) = read_only_agent_message(
        agent,
        EditableOverrideField::Model,
        ctx.cwd,
        &ctx.extra_agent_dirs,
    ) {
        return Ok(Some(refusal.to_string()));
    }
    write_agent_frontmatter_field(ctx.cfg, agent, EditableOverrideField::Model, selected_model)
        .await?;
    Ok(Some(match selected_model {
        Some(model) => format!(
            "Updated '{}' model to '{model}' in {}.",
            agent.name,
            agent.file_path.display()
        ),
        None => format!(
            "Cleared '{}' model in {}.",
            agent.name,
            agent.file_path.display()
        ),
    }))
}

/// pi `saveAgentThinking` (`subagents-admin.ts:330-351`) — [`save_agent_model`]'s twin.
async fn save_agent_thinking(
    ctx: &AdminContext<'_>,
    agent: &AgentDefinition,
    selected_thinking: Option<&str>,
) -> Result<Option<String>, SubagentError> {
    if saves_through_settings(agent, EditableOverrideField::Thinking) {
        let Some(scope) = choose_override_scope(ctx, agent) else {
            return Ok(None);
        };
        let (path, overridden) = persist_settings_field(
            ctx.cfg,
            agent,
            scope,
            EditableOverrideField::Thinking,
            selected_thinking,
        )
        .await?;
        return Ok(Some(if overridden {
            format!(
                "Saved {} settings override for '{}' with thinking '{}' in {}.",
                override_scope_str(scope),
                agent.name,
                selected_thinking.unwrap_or("undefined"),
                path.display()
            )
        } else {
            format!(
                "Cleared thinking settings override for '{}' in {}.",
                agent.name,
                path.display()
            )
        }));
    }

    if let Some(refusal) = read_only_agent_message(
        agent,
        EditableOverrideField::Thinking,
        ctx.cwd,
        &ctx.extra_agent_dirs,
    ) {
        return Ok(Some(refusal.to_string()));
    }
    write_agent_frontmatter_field(
        ctx.cfg,
        agent,
        EditableOverrideField::Thinking,
        selected_thinking,
    )
    .await?;
    Ok(Some(match selected_thinking {
        Some(thinking) => format!(
            "Updated '{}' thinking to '{thinking}' in {}.",
            agent.name,
            agent.file_path.display()
        ),
        None => format!(
            "Cleared '{}' thinking in {}.",
            agent.name,
            agent.file_path.display()
        ),
    }))
}

/// pi `saveAgentSystemPrompt` (`subagents-admin.ts:362-379`). The prompt is right-trimmed first
/// (`:363`, `systemPrompt.replace(/\s+$/, "")`).
async fn save_agent_system_prompt(
    ctx: &AdminContext<'_>,
    agent: &AgentDefinition,
    system_prompt: &str,
) -> Result<Option<String>, SubagentError> {
    let next_prompt = system_prompt.trim_end();
    if saves_through_settings(agent, EditableOverrideField::SystemPrompt) {
        let Some(scope) = choose_override_scope(ctx, agent) else {
            return Ok(None);
        };
        let (path, overridden) = persist_settings_field(
            ctx.cfg,
            agent,
            scope,
            EditableOverrideField::SystemPrompt,
            Some(next_prompt),
        )
        .await?;
        return Ok(Some(if overridden {
            format!(
                "Saved {} settings override for '{}' system prompt in {}.",
                override_scope_str(scope),
                agent.name,
                path.display()
            )
        } else {
            format!(
                "Cleared system prompt settings override for '{}' in {}.",
                agent.name,
                path.display()
            )
        }));
    }

    if let Some(refusal) = read_only_agent_message(
        agent,
        EditableOverrideField::SystemPrompt,
        ctx.cwd,
        &ctx.extra_agent_dirs,
    ) {
        return Ok(Some(refusal.to_string()));
    }
    write_agent_frontmatter_field(
        ctx.cfg,
        agent,
        EditableOverrideField::SystemPrompt,
        Some(next_prompt),
    )
    .await?;
    Ok(Some(format!(
        "Updated '{}' system prompt in {}.",
        agent.name,
        agent.file_path.display()
    )))
}

/// pi `editSystemPrompt` (`subagents-admin.ts:381-394`): refuse early for a read-only agent whose
/// edit would go to frontmatter, open the editor, then either report "left unchanged" or save.
///
/// The `projectOwnsLowerScopeOverride` exception (`:389-390`) is why an UNCHANGED prompt can still
/// be written: a project agent shadowing a user-scope override must materialize the value at
/// project scope, and skipping the write because the text matched would leave the shadow unmade.
async fn edit_system_prompt(
    ctx: &AdminContext<'_>,
    agent: &AgentDefinition,
) -> Result<Option<String>, SubagentError> {
    let save_through_settings = saves_through_settings(agent, EditableOverrideField::SystemPrompt);
    if !save_through_settings
        && let Some(refusal) = read_only_agent_message(
            agent,
            EditableOverrideField::SystemPrompt,
            ctx.cwd,
            &ctx.extra_agent_dirs,
        )
    {
        return Ok(Some(refusal.to_string()));
    }
    let Some(services) = ctx.services else {
        return Ok(None);
    };
    let Some(edited) = services.editor(
        &format!("Edit '{}' system prompt", agent.name),
        &agent.system_prompt_body,
    ) else {
        return Ok(None);
    };
    let project_owns_lower_scope_override = agent.source == AgentSource::Project
        && agent
            .override_info
            .as_ref()
            .is_some_and(|info| info.scope == OverrideScope::User)
        && project_settings_path(ctx.cfg).is_some();
    if edited.trim_end() == agent.system_prompt_body.trim_end()
        && !(save_through_settings && project_owns_lower_scope_override)
    {
        return Ok(Some(format!(
            "System prompt for '{}' left unchanged.",
            agent.name
        )));
    }
    save_agent_system_prompt(ctx, agent, &edited).await
}

// -------------------------------------------------------------------------------------------
// Selection + entry point (`:162`, `:396`)
// -------------------------------------------------------------------------------------------

/// pi `selectAgent` (`subagents-admin.ts:162-183`).
pub(crate) fn select_agent(
    ctx: &AdminContext<'_>,
    agents: Vec<AgentDefinition>,
    args: &str,
) -> AgentSelection {
    let requested = args.split_whitespace().next().unwrap_or("").to_string();
    if agents.is_empty() {
        return AgentSelection::NotFound {
            agents,
            requested: (!requested.is_empty()).then_some(requested),
        };
    }

    if !requested.is_empty() {
        let matches: Vec<AgentDefinition> = agents
            .iter()
            .filter(|agent| agent_matches(agent, &requested))
            .cloned()
            .collect();
        return match matches.len() {
            0 => AgentSelection::NotFound {
                agents,
                requested: Some(requested),
            },
            1 => match matches.into_iter().next() {
                Some(agent) => AgentSelection::Selected(Box::new(agent)),
                None => AgentSelection::Cancelled,
            },
            _ if !ctx.interactive() => AgentSelection::Ambiguous { requested, matches },
            _ => pick_from(
                ctx,
                &matches,
                &format!("Multiple subagents named '{requested}'"),
            ),
        };
    }

    if !ctx.interactive() {
        return AgentSelection::NotFound {
            agents,
            requested: None,
        };
    }
    pick_from(ctx, &agents, "Select subagent")
}

/// pi `:172-174` / `:180-182`: build the disambiguated label map, show it, and resolve the choice.
fn pick_from(ctx: &AdminContext<'_>, agents: &[AgentDefinition], title: &str) -> AgentSelection {
    let Some(services) = ctx.services else {
        return AgentSelection::Cancelled;
    };
    let by_label = agent_choices(agents);
    let values = agent_select_items(&by_label);
    match select_from_list(services, title, None, &values) {
        Some(choice) => match by_label
            .into_iter()
            .find(|(label, _)| *label == choice)
            .map(|(_, agent)| AgentDefinition::clone(agent))
        {
            Some(agent) => AgentSelection::Selected(Box::new(agent)),
            None => AgentSelection::Cancelled,
        },
        None => AgentSelection::Cancelled,
    }
}

/// pi `:405-406`'s available-subagent listing, shared by both not-found spellings.
fn available_list(agents: &[AgentDefinition]) -> String {
    if agents.is_empty() {
        return "- (none)".to_string();
    }
    agents
        .iter()
        .map(|agent| format!("- {} ({})", agent.name, source_str(agent.source)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// pi `openSubagentsAdmin(pi, ctx, args)` (`subagents-admin.ts:396-460`).
///
/// Upstream posts every outcome with `sendAdminMessage` (`:65-71`, a `pi.sendMessage` with
/// `customType: "subagents-admin"`) and returns `void`. cyrup's slash seam returns the ONE
/// transcript entry `NativeExtension::execute_command` renders, so each `sendAdminMessage` call is
/// this function's return value and a `return` with nothing posted is `Ok(String::new())`.
///
/// Both branches terminate on their own:
/// * `has_ui == false` (`:412-416`) renders [`metadata_for`] and returns — it is upstream's text
///   path, not an unsupported one.
/// * `has_ui == true` runs selection → action → persist once and returns the receipt; upstream's
///   handler is likewise single-shot, not a loop (`Done` and a dismissal both fall through `:431-454`
///   with nothing posted).
///
/// # Errors
///
/// Propagates R-SA-009's malformed-settings abort from discovery. A failure INSIDE the save is
/// upstream's `catch` (`:455-459`): notified at error level and returned as
/// `Failed to update '<name>': <message>`, not raised.
pub(crate) async fn open_subagents_admin(
    ctx: &AdminContext<'_>,
    args: &str,
) -> Result<String, SubagentError> {
    let agents = all_visible_agents(ctx.cfg)?;
    let agent = match select_agent(ctx, agents, args) {
        AgentSelection::Cancelled => return Ok(String::new()),
        AgentSelection::Ambiguous { requested, matches } => {
            return Ok(format!(
                "Subagent '{requested}' is ambiguous. Choose a scope in interactive mode:\n{}",
                matches
                    .iter()
                    .map(|agent| format!(
                        "- {}: {}",
                        source_str(agent.source),
                        agent.file_path.display()
                    ))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        AgentSelection::NotFound { agents, requested } => {
            return Ok(match requested {
                Some(requested) => format!(
                    "Subagent '{requested}' not found.\n\nAvailable subagents:\n{}",
                    available_list(&agents)
                ),
                None => format!("Available subagents:\n{}", available_list(&agents)),
            });
        }
        AgentSelection::Selected(agent) => agent,
    };

    if !ctx.interactive() {
        // pi `:412-415`: non-interactive (tool/headless) emits the full metadata as the
        // inspection result.
        return metadata_for(ctx.cfg, &agent).await;
    }
    let Some(services) = ctx.services else {
        return metadata_for(ctx.cfg, &agent).await;
    };

    // pi `:418-429`: the requested action token, else the picker.
    let action = match args
        .split_whitespace()
        .nth(1)
        .map(str::to_lowercase)
        .as_deref()
        .and_then(AdminAction::from_token)
    {
        Some(action) => Some(action),
        None => {
            let labels: Vec<String> = AdminAction::ALL
                .into_iter()
                .map(|action| action.label().to_string())
                .collect();
            select_from_list(
                services,
                &format!("Administer {}\n{}", agent.name, metadata_summary(&agent)),
                None,
                &labels,
            )
            .as_deref()
            .and_then(AdminAction::from_label)
        }
    };

    let saved = match action {
        Some(AdminAction::ChangeModel) => match choose_model(services, &agent) {
            Some(selected) => save_agent_model(ctx, &agent, selected.as_deref()).await,
            None => return Ok(String::new()),
        },
        Some(AdminAction::ChangeThinking) => {
            // pi `ctx.model` (`:248-249`), the live parent session model.
            let session_model = services.current_model();
            match choose_thinking(services, &agent, session_model.as_deref()) {
                Some(selected) => save_agent_thinking(ctx, &agent, selected.as_deref()).await,
                None => return Ok(String::new()),
            }
        }
        Some(AdminAction::EditSystemPrompt) => edit_system_prompt(ctx, &agent).await,
        Some(AdminAction::ShowDetails) => {
            // pi `:452-453`: full metadata is opt-in here (it used to be posted unconditionally).
            return metadata_for(ctx.cfg, &agent).await;
        }
        // `Done`, an unrecognized label, and a dismissal all fall through with nothing posted.
        Some(AdminAction::Done) | None => return Ok(String::new()),
    };

    match saved {
        Ok(Some(message)) => {
            services.notify(&message, NotifyKind::Info);
            Ok(message)
        }
        Ok(None) => Ok(String::new()),
        // pi `:455-459`.
        Err(error) => {
            let message = error.to_string();
            services.notify(&message, NotifyKind::Error);
            Ok(format!("Failed to update '{}': {message}", agent.name))
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use cyrup_core::ModelId;

    use super::*;
    use crate::discovery::management::test_support::sample_agent;
    use crate::discovery::types::AgentOverrideInfo;

    fn agent(source: AgentSource, name: &str, path: &str) -> AgentDefinition {
        let mut a = sample_agent(source, PathBuf::from(path));
        a.name = name.to_string();
        a.local_name = name.to_string();
        a
    }

    fn override_info(
        scope: OverrideScope,
        base: AgentDefinition,
        fields: &[&str],
        field_scopes: &[(&str, &[OverrideScope])],
    ) -> AgentOverrideInfo {
        AgentOverrideInfo {
            scope,
            settings_path: PathBuf::from("/settings.json"),
            base_snapshot: Box::new(base),
            fields: fields.iter().map(|f| (*f).to_string()).collect(),
            field_scopes: field_scopes
                .iter()
                .map(|(field, scopes)| {
                    (
                        (*field).to_string(),
                        scopes.iter().copied().collect::<BTreeSet<_>>(),
                    )
                })
                .collect::<BTreeMap<_, _>>(),
        }
    }

    /// pi `sourceRank` (`subagents-admin.ts:25-30`) is `project` 0 < `user` 1 < `package` 2 <
    /// everything else 3, and `/subagents`' listing sorts on it.
    ///
    /// Catches: re-ordering [`AgentSource::precedence_rank`]'s match arms (e.g. making Builtin
    /// outrank Project), which would silently reverse the picker's tie-break for two agents that
    /// share a name.
    #[test]
    fn source_rank_matches_upstream_order() {
        assert_eq!(source_rank(AgentSource::Project), 0);
        assert_eq!(source_rank(AgentSource::User), 1);
        assert_eq!(source_rank(AgentSource::Package), 2);
        assert_eq!(source_rank(AgentSource::Builtin), 3);
    }

    /// pi's `sourceRank` returns 3 for anything that is not project/user/package, and cyrup ranks
    /// `Runtime` WITH `Project` (`discovery/types.rs:75-78`) because a runtime agent outranks a
    /// project one at name resolution.
    ///
    /// Catches: a future edit that gives `Runtime` its own rank, which would change where a
    /// runtime-registered agent sorts in the picker relative to its configured namesake.
    #[test]
    fn runtime_ranks_with_project() {
        assert_eq!(
            source_rank(AgentSource::Runtime),
            source_rank(AgentSource::Project)
        );
    }

    /// pi `agentLabel` (`:41-44`): `name [source] · model — description`, with the model segment
    /// omitted entirely when the agent pins none.
    ///
    /// Catches: dropping the ` · ` separator or the em dash, either of which makes two labels
    /// collide and sends [`agent_choices`] down its file-path-disambiguation branch for agents
    /// that are not actually ambiguous.
    #[test]
    fn agent_label_renders_model_only_when_pinned() {
        let mut a = agent(AgentSource::User, "scout", "/u/scout.md");
        assert_eq!(agent_label(&a), "scout [user] — reviews things");
        a.model = Some(ModelId::from("anthropic/claude"));
        assert_eq!(
            agent_label(&a),
            "scout [user] · anthropic/claude — reviews things"
        );
    }

    /// pi `agentChoices` (`:46-54`): a label produced by exactly one agent is used bare; a label
    /// produced by more than one gets ` · <filePath>` appended so the rows stay distinguishable.
    ///
    /// Catches: emitting the file path unconditionally (which would make every picker row noisy)
    /// or never (which would make two same-named agents indistinguishable and the choice
    /// arbitrary).
    #[test]
    fn agent_choices_disambiguates_only_collisions() {
        let unique = vec![
            agent(AgentSource::User, "a", "/u/a.md"),
            agent(AgentSource::User, "b", "/u/b.md"),
        ];
        let labels = agent_select_items(&agent_choices(&unique));
        assert_eq!(
            labels,
            vec![agent_label(&unique[0]), agent_label(&unique[1])]
        );

        // Same name, same source, same description => identical labels.
        let colliding = vec![
            agent(AgentSource::User, "dup", "/one/dup.md"),
            agent(AgentSource::User, "dup", "/two/dup.md"),
        ];
        let labels = agent_select_items(&agent_choices(&colliding));
        assert_eq!(labels.len(), 2);
        assert!(labels[0].ends_with(" · /one/dup.md"), "{labels:?}");
        assert!(labels[1].ends_with(" · /two/dup.md"), "{labels:?}");
    }

    /// pi `agentMatches` (`:60-63`): the trimmed request equals the runtime name OR the
    /// frontmatter (local) name.
    ///
    /// Catches: dropping the `local_name` clause, which makes `/subagents foo` unable to reach a
    /// packaged `pkg.foo`; and dropping the trim, which makes `/subagents  scout ` miss.
    #[test]
    fn agent_matches_runtime_and_local_names() {
        let mut packaged = agent(AgentSource::Package, "pkg.foo", "/p/foo.md");
        packaged.local_name = "foo".to_string();
        packaged.package_name = Some("pkg".to_string());
        assert!(agent_matches(&packaged, "pkg.foo"));
        assert!(agent_matches(&packaged, "foo"));
        assert!(agent_matches(&packaged, " foo "));
        assert!(!agent_matches(&packaged, "fo"));
    }

    /// pi `selectAgent` (`:167-176`): one match selects it, none is not-found, several with no UI
    /// is the AMBIGUOUS outcome — which in cyrup arises from `agentMatches`' local-name clause
    /// (a package agent's local name colliding with another agent's runtime name).
    ///
    /// Catches: collapsing `Ambiguous` into `NotFound`, which would answer "not found" for a name
    /// that matched twice and lose the per-scope file list the message carries.
    #[test]
    fn select_agent_resolves_exact_ambiguous_and_missing() {
        let cfg = AgentDiscoveryConfig::default();
        let ctx = AdminContext {
            cfg: &cfg,
            cwd: Path::new("/work"),
            services: None,
            has_ui: false,
            extra_agent_dirs: Vec::new(),
        };
        let mut packaged = agent(AgentSource::Package, "pkg.foo", "/p/foo.md");
        packaged.local_name = "foo".to_string();
        let plain = agent(AgentSource::User, "foo", "/u/foo.md");
        let other = agent(AgentSource::User, "bar", "/u/bar.md");
        let agents = vec![packaged, plain, other];

        match select_agent(&ctx, agents.clone(), "bar") {
            AgentSelection::Selected(a) => assert_eq!(a.name, "bar"),
            _ => panic!("expected an exact selection"),
        }
        match select_agent(&ctx, agents.clone(), "foo") {
            AgentSelection::Ambiguous { requested, matches } => {
                assert_eq!(requested, "foo");
                assert_eq!(matches.len(), 2);
            }
            _ => panic!("expected ambiguity"),
        }
        match select_agent(&ctx, agents.clone(), "nope") {
            AgentSelection::NotFound { requested, .. } => {
                assert_eq!(requested.as_deref(), Some("nope"));
            }
            _ => panic!("expected not-found"),
        }
        // No name + no UI is upstream's `:179` listing branch, not a cancel.
        match select_agent(&ctx, agents, "") {
            AgentSelection::NotFound { requested, agents } => {
                assert!(requested.is_none());
                assert_eq!(agents.len(), 3);
            }
            _ => panic!("expected the listing branch"),
        }
    }

    /// pi `isReadOnlyExtraAgent` (`:139-148`): an agent whose file sits inside an extra directory
    /// — including a SUBDIRECTORY of one — is read-only; one that merely shares a string prefix
    /// with an extra directory is not.
    ///
    /// Catches: implementing the containment test with `starts_with` on the rendered strings,
    /// which would wrongly lock `/extra-dirs-backup/a.md` against the root `/extra-dirs`; and
    /// dropping the subdirectory case by comparing parents instead of prefixes.
    #[test]
    fn is_read_only_extra_agent_uses_path_containment() {
        let cwd = Path::new("/work");
        let dirs = vec![PathBuf::from("/extra")];

        let inside = agent(AgentSource::User, "a", "/extra/a.md");
        assert!(is_read_only_extra_agent(&inside, cwd, &dirs));

        let nested = agent(AgentSource::User, "a", "/extra/team/a.md");
        assert!(is_read_only_extra_agent(&nested, cwd, &dirs));

        let string_prefix_only = agent(AgentSource::User, "a", "/extra-backup/a.md");
        assert!(!is_read_only_extra_agent(&string_prefix_only, cwd, &dirs));

        // Node `path.resolve` normalizes `..` away before the containment test; `std::path::absolute`
        // would not, so this escapes only if the lexical normalization is in place.
        let escaping = agent(AgentSource::User, "a", "/extra/../elsewhere/a.md");
        assert!(!is_read_only_extra_agent(&escaping, cwd, &dirs));

        // `:141` — the extras stream is a USER-tier fallback and nothing else.
        let project = agent(AgentSource::Project, "a", "/extra/a.md");
        assert!(!is_read_only_extra_agent(&project, cwd, &dirs));

        // `:140` — no configured extras means no refusal at all.
        assert!(!is_read_only_extra_agent(&inside, cwd, &[]));
    }

    /// pi `savesThroughSettings` (`:125-137`), whole decision table.
    ///
    /// Catches: dropping the `fields` clause (`:129`) — a builtin's model edit would still route to
    /// settings via the source check, but a CUSTOM agent carrying a settings-applied `model` would
    /// start rewriting its own frontmatter, silently un-shadowing the override; dropping the
    /// scope-equality clause (`:133`) — a project agent that merely inherited a user-scope override
    /// would rewrite it on behalf of every other agent sharing it; and dropping the base comparison
    /// (`:136`) — an override on `thinking` would redirect `model` edits into settings too.
    #[test]
    fn saves_through_settings_decision_table() {
        let base = agent(AgentSource::User, "a", "/u/a.md");

        // :126/:127 — bundled tiers always save through settings.
        for source in [AgentSource::Builtin, AgentSource::Package] {
            let bundled = agent(source, "a", "/b/a.md");
            assert!(saves_through_settings(
                &bundled,
                EditableOverrideField::Model
            ));
        }

        // :128 — a custom agent with no override at all writes its own frontmatter.
        let plain = agent(AgentSource::User, "a", "/u/a.md");
        assert!(!saves_through_settings(
            &plain,
            EditableOverrideField::Model
        ));

        // :129 — the field is named in `fields`, whatever the values say.
        let mut listed = plain.clone();
        listed.override_info = Some(override_info(
            OverrideScope::User,
            base.clone(),
            &["model"],
            &[],
        ));
        assert!(saves_through_settings(
            &listed,
            EditableOverrideField::Model
        ));

        // :133 — override scope does not match the agent's own tier: frontmatter.
        let mut inherited = agent(AgentSource::Project, "a", "/p/a.md");
        let mut changed_base = base.clone();
        changed_base.model = None;
        inherited.model = Some(ModelId::from("x/y"));
        inherited.override_info = Some(override_info(
            OverrideScope::User,
            changed_base.clone(),
            &["thinking"],
            &[],
        ));
        assert!(!saves_through_settings(
            &inherited,
            EditableOverrideField::Model
        ));

        // :136 — same scope, field not listed, effective value differs from base: settings.
        let mut differs = agent(AgentSource::User, "a", "/u/a.md");
        differs.model = Some(ModelId::from("x/y"));
        differs.override_info = Some(override_info(
            OverrideScope::User,
            changed_base.clone(),
            &["thinking"],
            &[],
        ));
        assert!(saves_through_settings(
            &differs,
            EditableOverrideField::Model
        ));

        // :136 — same scope, field not listed, value EQUALS base: frontmatter.
        let mut same = differs.clone();
        let mut same_base = changed_base;
        same_base.model = Some(ModelId::from("x/y"));
        same.override_info = Some(override_info(
            OverrideScope::User,
            same_base,
            &["thinking"],
            &[],
        ));
        assert!(!saves_through_settings(&same, EditableOverrideField::Model));
    }

    /// pi `readOnlyAgentMessage` (`:150-160`), all three sentences byte-for-byte, with cyrup's own
    /// env-var name in the third.
    ///
    /// Catches: collapsing the three into one generic refusal (the bytes change), reordering the
    /// runtime/package checks (a runtime agent would report the package sentence), and swapping the
    /// `field` token for a prettified label.
    #[test]
    fn read_only_agent_message_sentences() {
        let cwd = Path::new("/work");
        let runtime = agent(AgentSource::Runtime, "live", "runtime:live");
        assert_eq!(
            read_only_agent_message(&runtime, EditableOverrideField::Model, cwd, &[])
                .unwrap()
                .to_string(),
            "Cannot update 'live' model because that agent is runtime-registered by an extension; \
             edit its source definition instead."
        );

        let packaged = agent(AgentSource::Package, "pkg.a", "/p/a.md");
        assert_eq!(
            read_only_agent_message(&packaged, EditableOverrideField::SystemPrompt, cwd, &[])
                .unwrap()
                .to_string(),
            "Cannot update 'pkg.a' systemPrompt because that field is owned by its read-only \
             package definition."
        );

        let extra = agent(AgentSource::User, "x", "/extra/x.md");
        assert_eq!(
            read_only_agent_message(
                &extra,
                EditableOverrideField::Thinking,
                cwd,
                &[PathBuf::from("/extra")]
            )
            .unwrap()
            .to_string(),
            "Cannot update 'x' because its definition in CYRUP_SUBAGENT_EXTRA_AGENT_DIRS is \
             read-only."
        );

        let writable = agent(AgentSource::User, "w", "/u/w.md");
        assert!(
            read_only_agent_message(&writable, EditableOverrideField::Model, cwd, &[]).is_none()
        );
    }

    /// pi `metadataSummary` (`:353-360`): exactly three fields joined by `" · "`, with the
    /// unpinned cases rendering `default / inherit`.
    ///
    /// Catches: leaking the full system prompt into the picker title (the reason upstream added
    /// this function), and rendering an absent model as an empty string, which would make the
    /// title read `Model:  · Thinking: …`.
    #[test]
    fn metadata_summary_is_a_three_field_one_liner() {
        let mut a = agent(AgentSource::Builtin, "scout", "/b/scout.md");
        a.system_prompt_body = "a very long prompt".to_string();
        assert_eq!(
            metadata_summary(&a),
            "Source: builtin · Model: default / inherit · Thinking: default / inherit"
        );
        a.model = Some(ModelId::from("p/m"));
        a.thinking = Some("off".to_string());
        assert_eq!(
            metadata_summary(&a),
            "Source: builtin · Model: p/m · Thinking: off"
        );
        assert!(!metadata_summary(&a).contains("a very long prompt"));
    }

    /// pi's `shadowsLowerScope` (`:283`): true only when saving at PROJECT scope for a field whose
    /// `fieldScopes` entry also names `user`.
    ///
    /// Catches: reading `override.scope` instead of `field_scopes` (which cannot see a second
    /// contributor at all), and forgetting the `scope === "project"` guard, which would make a
    /// user-scope clear write a redundant explicit value instead of removing the key.
    #[test]
    fn shadows_lower_scope_reads_field_scopes() {
        let base = agent(AgentSource::User, "a", "/u/a.md");
        let mut a = agent(AgentSource::Project, "a", "/p/a.md");
        a.override_info = Some(override_info(
            OverrideScope::Project,
            base.clone(),
            &["model", "thinking"],
            &[
                ("model", &[OverrideScope::User, OverrideScope::Project]),
                ("thinking", &[OverrideScope::Project]),
            ],
        ));
        assert!(shadows_lower_scope(
            &a,
            OverrideScope::Project,
            EditableOverrideField::Model
        ));
        // Only project contributed `thinking`, so clearing it there just removes the key.
        assert!(!shadows_lower_scope(
            &a,
            OverrideScope::Project,
            EditableOverrideField::Thinking
        ));
        // Saving at USER scope is never a shadow.
        assert!(!shadows_lower_scope(
            &a,
            OverrideScope::User,
            EditableOverrideField::Model
        ));
        // No override info at all.
        let plain = agent(AgentSource::Project, "a", "/p/a.md");
        assert!(!shadows_lower_scope(
            &plain,
            OverrideScope::Project,
            EditableOverrideField::Model
        ));
    }

    /// pi `:418-423` + `:427`: the argument tokens and the picker labels are two spellings of one
    /// action set.
    ///
    /// Catches: dropping an alias (`system-prompt`/`edit`/`info`), which makes a documented
    /// argument fall through to the picker; and a label typo, which makes the picker's choice
    /// match nothing and silently do nothing.
    #[test]
    fn admin_action_tokens_and_labels_round_trip() {
        assert_eq!(
            AdminAction::from_token("model"),
            Some(AdminAction::ChangeModel)
        );
        assert_eq!(
            AdminAction::from_token("thinking"),
            Some(AdminAction::ChangeThinking)
        );
        for token in ["prompt", "system-prompt", "edit"] {
            assert_eq!(
                AdminAction::from_token(token),
                Some(AdminAction::EditSystemPrompt)
            );
        }
        for token in ["details", "info"] {
            assert_eq!(
                AdminAction::from_token(token),
                Some(AdminAction::ShowDetails)
            );
        }
        assert_eq!(AdminAction::from_token("done"), None);
        for action in AdminAction::ALL {
            assert_eq!(AdminAction::from_label(action.label()), Some(action));
        }
    }

    /// pi `getSupportedThinkingLevels` (`shared/model-info.ts:88-101`), all four branches.
    ///
    /// Catches: offering `max` for a model that never named it (the picker would write a level the
    /// provider rejects), and returning the full ladder for a non-reasoning model.
    #[test]
    fn supported_thinking_levels_matches_upstream_branches() {
        assert_eq!(
            supported_thinking_levels(None),
            vec!["off", "minimal", "low", "medium", "high", "xhigh"]
        );

        let mut model = registry_models()
            .first()
            .cloned()
            .expect("the builtin catalog ships at least one model");

        model.reasoning = false;
        model.thinking_level_map = None;
        assert_eq!(supported_thinking_levels(Some(&model)), vec!["off"]);

        model.reasoning = true;
        assert_eq!(
            supported_thinking_levels(Some(&model)),
            vec!["off", "minimal", "low", "medium", "high", "xhigh"]
        );

        model.thinking_level_map = Some(
            [
                ("off".to_string(), Some("0".to_string())),
                ("minimal".to_string(), None),
                ("low".to_string(), Some("1".to_string())),
                ("max".to_string(), Some("9".to_string())),
            ]
            .into_iter()
            .collect(),
        );
        // `minimal` is explicitly null => dropped. `medium`/`high` are unnamed non-top rungs =>
        // kept. `xhigh` is an unnamed TOP rung => dropped. `max` is named => kept.
        assert_eq!(
            supported_thinking_levels(Some(&model)),
            vec!["off", "low", "medium", "high", "max"]
        );
    }
}
