//! `handleList`/`handleGet`/`handleModels`/`handleCreate`/`handleUpdate`/`handleDelete` — the six
//! non-tier-aware management actions. Split out of `discovery/management.rs`'s own
//! "handleList / handleGet / handleModels / handleCreate / handleUpdate / handleDelete" section.
//! Reached only via [`crate::discovery::management::handle_management_action`].

use std::collections::HashMap;

use super::super::types::{
    AgentDefinition, AgentDiscoveryDiagnostic, AgentSource, ChainDefinition,
    ChainDiscoveryDiagnostic, ChainStepConfig, SystemPromptMode,
};
use super::super::{discover_agents_all, find_blocking_agent_diagnostic, AgentDiscoveryConfig};
use super::agent_crud::{create_agent, delete_agent, rename_agent, update_agent, AgentFields, AgentMutationOutcome};
use super::chain_crud::{create_chain_with_steps, delete_chain, update_chain_full};
use super::config_parse::{apply_agent_config, config_object, parse_package_config, parse_step_list};
use super::helpers::{disambiguation_scope, normalize_list_scope, pick_scope_dir, sanitize_name, source_str};
use super::lookup::{
    available_agent_names, available_chain_names, distinct_agent_names, find_agents, find_chains,
    name_exists_in_scope, resolve_target, unknown_chain_agents, TargetKind,
};
use super::render::{format_agent_detail, format_chain_detail, format_model_source};
use super::{ManagementOutcome, ManagementRequest, BUILTIN_AGENT_NAMES};
use crate::error::SubagentError;

/// Builtin/Package agents are visible under every list scope; SUBA-084 adds Runtime to that set —
/// pi `effectiveAgentsForScope` (`agent-management.ts:132-141` @v0.64.0) merges the runtime agents
/// into the scope-narrowed list unconditionally (`mergeRuntimeAgents(owner, { agents },
/// allAgents(d))`), so a `user`/`project` listing still shows them.
fn agent_in_list_scope(source: AgentSource, scope: Option<AgentSource>) -> bool {
    scope.is_none()
        || matches!(source, AgentSource::Builtin | AgentSource::Package | AgentSource::Runtime)
        || Some(source) == scope
}

fn chain_in_list_scope(source: AgentSource, scope: Option<AgentSource>) -> bool {
    scope.is_none() || source == AgentSource::Package || Some(source) == scope
}

/// SUBA-086 — pi `diagnosticsForScope` (`agent-management.ts:177-181` @v0.64.0): `both` keeps
/// everything; `user` drops PROJECT-sourced diagnostics and `project` drops USER-sourced ones
/// (builtin/package/runtime diagnostics survive either named scope).
fn diagnostics_for_scope(
    diagnostics: &[AgentDiscoveryDiagnostic],
    scope: Option<AgentSource>,
) -> Vec<AgentDiscoveryDiagnostic> {
    let excluded = match scope {
        None => return diagnostics.to_vec(),
        Some(AgentSource::User) => AgentSource::Project,
        Some(_) => AgentSource::User,
    };
    diagnostics.iter().filter(|d| d.source != excluded).cloned().collect()
}

/// SUBA-086 — pi `appendAgentDiagnosticLines` (`agent-management.ts:818-825` @v0.64.0;
/// `:760-764` @v0.57.0): nothing when empty, else a blank separator, `Invalid agent
/// definitions:` and one `- <name ?? filePath> (<source>): <error>` line per diagnostic.
fn append_agent_diagnostic_lines(lines: &mut Vec<String>, diagnostics: &[AgentDiscoveryDiagnostic]) {
    if diagnostics.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("Invalid agent definitions:".to_string());
    for d in diagnostics {
        lines.push(format!("- {} ({}): {}", d.label(), source_str(d.source), d.error));
    }
}

/// SUBA-086 — the two-probe lookup `get`/`models` run before their ambiguity/not-found branches
/// (`agent-management.ts:985-989,1084-1089` @v0.64.0): the raw name first, then — only when it
/// differs — the `sanitizeName`d spelling.
fn blocking_diagnostic_for_request<'a>(
    requested: &str,
    matches: &[AgentDefinition],
    diagnostics: &'a [AgentDiscoveryDiagnostic],
) -> Option<&'a AgentDiscoveryDiagnostic> {
    let candidates: Vec<&AgentDefinition> = matches.iter().collect();
    let raw = requested.trim();
    let sanitized = sanitize_name(raw);
    find_blocking_agent_diagnostic(raw, &candidates, diagnostics).or_else(|| {
        (sanitized != raw)
            .then(|| find_blocking_agent_diagnostic(&sanitized, &candidates, diagnostics))
            .flatten()
    })
}

/// pi's name-sensitive create defaults (`agents.ts:36-45`): `delegate` -> `Append`/inherit-context,
/// else `Replace`/no-inherit; `inheritSkills` always defaults false. Replicated locally (matching
/// this crate's established "each module keeps its own small helper" convention) rather than making
/// `frontmatter.rs`'s private equivalents `pub(crate)`.
fn default_system_prompt_mode(local_name: &str) -> SystemPromptMode {
    if local_name == "delegate" {
        SystemPromptMode::Append
    } else {
        SystemPromptMode::Replace
    }
}

fn default_inherit_project_context(local_name: &str) -> bool {
    local_name == "delegate"
}

/// The base definition to edit: pi `editableAgentConfig` (`agent-management.ts:217-267`) un-applies a
/// settings override so an update writes the agent's own base values, never the override-applied
/// ones. Settings overrides are inert today (C2), so `override_info` is always `None` and this is a
/// clone — kept forward-compatible for the moment C2 lands.
pub(crate) fn editable_base(target: &AgentDefinition) -> AgentDefinition {
    let mut base = match &target.override_info {
        Some(info) => (*info.base_snapshot).clone(),
        None => target.clone(),
    };
    // pi `editableAgentConfig` (`agent-management.ts:243`):
    // `...(agent.extensionsFromDefault ? {} : agent.extensions !== undefined ? { extensions: [...] } : {})`
    // — an `extensions` list that came from `subagents.defaultExtensions` is NOT the agent's own
    // data, so it is dropped here rather than BAKED into the `.md` file by the next update. (cyrup's
    // `base_snapshot` is a whole-definition clone, unlike pi's field-subset `cloneOverrideBase`, so
    // this guard also covers pi's `agents.ts:582` exclusion on the override-restore baseline.)
    if base.extensions_from_default {
        base.extensions = None;
        base.extensions_from_default = false;
    }
    base
}

/// pi `handleList` (`agent-management.ts:753-788` @v0.43.0 — thirty-six lines).
///
/// The proactive-skill block is spliced in exactly where upstream splices it: BETWEEN the `Chains:`
/// block and the chain diagnostics, preceded by one blank line and only when it has lines
/// (`agent-management.ts:784`'s
/// `...(proactiveSuggestions.length ? ["", ...proactiveSuggestions] : [])`). Its two inputs — pi's
/// `ctx.config?.proactiveSkillSubagents` and the result of its `discoverAvailableSkills(ctx.cwd)`
/// closure — arrive on [`super::ManagementRequest::proactive_skills`]; see
/// [`super::ProactiveSkillsInput`] for why the availability scan is pre-resolved by the async
/// caller rather than run lazily here.
///
/// The recommender consults the SAME `agents`/`chains` bindings this function already rendered
/// (upstream passes its own post-filter `agents` and `chains` locals), so a scope-filtered or
/// disabled-filtered listing recommends only from what it listed.
///
/// There is no companion-suggestion block to port: upstream
/// deleted `companionSuggestionLines` from `handleList`'s `ManagementContext` and from its rendered
/// lines in `3ac0ef5` ("Make supervisor coordination native", 2026-07-03), together with the whole
/// `extension/companion-suggestions.ts` module.
pub(crate) fn handle_list(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    let scope = normalize_list_scope(req.agent_scope);
    let d = discover_agents_all(cfg)?;

    let mut agents: Vec<&AgentDefinition> = d
        .agents
        .iter()
        .filter(|a| agent_in_list_scope(a.source, scope))
        .filter(|a| !a.disabled.unwrap_or(false))
        .collect();
    agents.sort_by(|a, b| a.name.cmp(&b.name));

    let mut chains: Vec<&ChainDefinition> = d
        .chains
        .iter()
        .filter(|c| chain_in_list_scope(c.source, scope))
        .collect();
    chains.sort_by(|a, b| a.name.cmp(&b.name));

    let diagnostics: Vec<&ChainDiscoveryDiagnostic> = d
        .diagnostics
        .iter()
        .filter(|e| scope.is_none() || Some(e.source) == scope)
        .collect();

    let mut lines: Vec<String> = Vec::new();
    lines.push("Executable agents:".to_string());
    if agents.is_empty() {
        lines.push("- (none)".to_string());
    } else {
        for a in &agents {
            let ctx = a
                .default_context
                .map(|c| format!(", context: {}", super::helpers::context_str(c)))
                .unwrap_or_default();
            // pi `agent-management.ts:774` @ v0.43.0 appends `, aliases: <a, b>` after the optional
            // context segment and before the `: <description>` separator.
            let aliases = if a.aliases.is_empty() {
                String::new()
            } else {
                format!(", aliases: {}", a.aliases.join(", "))
            };
            lines.push(format!(
                "- {} ({}{}{}): {}",
                a.name,
                source_str(a.source),
                ctx,
                aliases,
                a.description
            ));
        }
    }
    lines.push(String::new());
    lines.push("Chains:".to_string());
    if chains.is_empty() {
        lines.push("- (none)".to_string());
    } else {
        for c in &chains {
            lines.push(format!("- {} ({}): {}", c.name, source_str(c.source), c.description));
        }
    }
    // SUBA-086 — pi `handleList` (`agent-management.ts:946-947` @v0.64.0) appends the agent
    // diagnostics BEFORE the proactive suggestions, and hands it `d.agentDiagnostics` UNFILTERED
    // (unlike `get`/`models`, which go through `diagnosticsForScope`) — so a `user`-scoped
    // listing still shows a broken project file. Ported as written.
    append_agent_diagnostic_lines(&mut lines, &d.agent_diagnostics);
    // pi `agent-management.ts:765-770,784` @v0.43.0: the proactive suggestions are computed from the same
    // filtered `agents`/`chains` this listing rendered, and spliced in after `Chains:` and before
    // `Chain diagnostics:` — with a leading blank line, and only when non-empty.
    if let Some(proactive) = &req.proactive_skills {
        let agent_inputs: Vec<crate::discovery::skills::ProactiveAgentInput> = agents
            .iter()
            .map(|a| crate::discovery::skills::proactive_agent_input(a))
            .collect();
        let chain_inputs: Vec<crate::discovery::skills::ProactiveChainInput> = chains
            .iter()
            .map(|c| crate::discovery::skills::proactive_chain_input(c))
            .collect();
        let suggestions =
            crate::discovery::skills::build_proactive_skill_subagent_recommendation_lines(
                &agent_inputs,
                &chain_inputs,
                proactive.setting,
                // The availability scan already happened (see `ProactiveSkillsInput`); this closure
                // is the sync shim that hands its result to the recommender, and — exactly like
                // upstream's — is never called when the feature is disabled.
                || Ok::<_, std::convert::Infallible>(proactive.available_skills.to_vec()),
            );
        if !suggestions.is_empty() {
            lines.push(String::new());
            lines.extend(suggestions);
        }
    }
    if !diagnostics.is_empty() {
        lines.push(String::new());
        lines.push("Chain diagnostics:".to_string());
        for e in &diagnostics {
            lines.push(format!("- {}: {}", e.file_path.display(), e.message));
        }
    }
    Ok(ManagementOutcome::ok(lines.join("\n")))
}

/// pi `handleGet` (`agent-management.ts:871-906`).
pub(crate) fn handle_get(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    if req.agent.is_none() && req.chain_name.is_none() {
        return Ok(ManagementOutcome::err("Specify 'agent' or 'chainName' for get."));
    }
    let has_both = req.agent.is_some() && req.chain_name.is_some();
    let d = discover_agents_all(cfg)?;
    let mut blocks: Vec<String> = Vec::new();
    let mut any_found = false;
    if let Some(agent_name) = req.agent {
        let matches = find_agents(&d, agent_name, None);
        // SUBA-086 — pi `handleGet` (`agent-management.ts:1084-1089` @v0.64.0) consults the
        // blocking diagnostic BEFORE the ambiguity and not-found branches, so a name whose only
        // (or outranking) definition is malformed reports the parse error by name instead of
        // `not found`. `get` takes no `agentScope` here, so the diagnostics are unscoped like
        // the `find_agents(.., None)` match set beside them.
        let diagnostics = diagnostics_for_scope(&d.agent_diagnostics, None);
        let distinct = distinct_agent_names(&matches);
        if let Some(diagnostic) = blocking_diagnostic_for_request(agent_name, &matches, &diagnostics) {
            let msg = format!("Agent '{agent_name}' has invalid configuration: {}", diagnostic.error);
            if !has_both {
                return Ok(ManagementOutcome::err(msg));
            }
            blocks.push(msg);
        } else if distinct.len() > 1 {
            // pi `handleGet` @ v0.43.0 (`agent-management.ts:871-885`) checks AMBIGUITY next: a
            // match set spanning several distinct canonical names is refused before the not-found
            // branch.
            let msg = format!(
                "Ambiguous agent alias or name '{}': {}",
                agent_name,
                distinct.join(", ")
            );
            if !has_both {
                return Ok(ManagementOutcome::err(msg));
            }
            blocks.push(msg);
        } else if matches.is_empty() {
            let avail = available_agent_names(&d);
            let msg = format!(
                "Agent '{}' not found. Available: {}.",
                agent_name,
                if avail.is_empty() { "none".to_string() } else { avail.join(", ") }
            );
            if !has_both {
                return Ok(ManagementOutcome::err(msg));
            }
            blocks.push(msg);
        } else {
            any_found = true;
            for a in &matches {
                blocks.push(format_agent_detail(a));
            }
        }
    }
    if let Some(chain_name) = req.chain_name {
        let matches = find_chains(&d, chain_name, None);
        if matches.is_empty() {
            let avail = available_chain_names(&d);
            let msg = format!(
                "Chain '{}' not found. Available: {}.",
                chain_name,
                if avail.is_empty() { "none".to_string() } else { avail.join(", ") }
            );
            if !has_both {
                return Ok(ManagementOutcome::err(msg));
            }
            blocks.push(msg);
        } else {
            any_found = true;
            for c in &matches {
                blocks.push(format_chain_detail(c));
            }
        }
    }
    Ok(ManagementOutcome { text: blocks.join("\n\n"), is_error: !any_found })
}

/// pi `handleModels` (`agent-management.ts:802-869`): the live parent session model is now threaded
/// in via [`super::ManagementRequest::current_session_model`] (from
/// [`cyrup_ext::host::HostServices::current_model`]), so `Current session model` renders the real
/// `provider/id` and an inheriting persona's effective model falls back to it; both degrade to
/// `(unavailable)`/`(unresolved)` only when there is genuinely no live session (headless /
/// SDK-embedder). The requested-filter validation, override provenance, and disabled state are
/// faithful. NB: the live `/subagents-models` slash + `subagent` tool `models` action route through
/// [`crate::extension::SubagentExecutor::run_models_report`] (which has its own `HostServices`
/// handle); this handler is the management-layer twin, reached via
/// [`super::handle_management_action`] and this crate's tests.
pub(crate) fn handle_models(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    let requested = req.agent.map(str::trim).filter(|s| !s.is_empty());
    if let Some(name) = requested
        && !BUILTIN_AGENT_NAMES.contains(&name)
    {
        return Ok(ManagementOutcome::err(format!(
            "Builtin agent '{name}' not found. Available: {}.",
            BUILTIN_AGENT_NAMES.join(", ")
        )));
    }
    let d = discover_agents_all(cfg)?;
    let builtin_by_name: HashMap<&str, &AgentDefinition> = d
        .agents
        .iter()
        .filter(|a| a.source == AgentSource::Builtin)
        .map(|a| (a.name.as_str(), a))
        .collect();

    if let Some(name) = requested {
        let Some(agent) = builtin_by_name.get(name) else {
            return Ok(ManagementOutcome::err(format!("Builtin agent '{name}' not found.")));
        };
        let resolved = agent
            .model
            .as_ref()
            .map(ToString::to_string)
            .or_else(|| req.current_session_model.map(str::to_string))
            .unwrap_or_else(|| "(unresolved)".to_string());
        let mut lines = vec![
            "Builtin subagent model".to_string(),
            String::new(),
            format!("Agent: {name}"),
            "Effective model:".to_string(),
            format!("  {resolved}"),
            format!("Source: {}", format_model_source(agent, req.current_session_model)),
        ];
        if let Some(info) = &agent.override_info {
            lines.push("Override file:".to_string());
            lines.push(format!("  {}", info.settings_path.display()));
        }
        if agent.disabled == Some(true) {
            lines.push("Disabled: true".to_string());
        }
        lines.push("Current session model:".to_string());
        lines.push(format!("  {}", req.current_session_model.unwrap_or("(unavailable)")));
        return Ok(ManagementOutcome::ok(lines.join("\n")));
    }

    let mut lines = vec![
        "Builtin subagent models".to_string(),
        String::new(),
        "Current session model:".to_string(),
        format!("  {}", req.current_session_model.unwrap_or("(unavailable)")),
        String::new(),
    ];
    for name in BUILTIN_AGENT_NAMES {
        match builtin_by_name.get(name) {
            None => {
                lines.push(name.to_string());
                lines.push("  model:".to_string());
                lines.push("    (builtin definition not found)".to_string());
                lines.push("  source: missing".to_string());
                lines.push(String::new());
            }
            Some(agent) => {
                let resolved = agent
                    .model
                    .as_ref()
                    .map(ToString::to_string)
                    .or_else(|| req.current_session_model.map(str::to_string))
                    .unwrap_or_else(|| "(unresolved)".to_string());
                let source = format!(
                    "{}{}",
                    format_model_source(agent, req.current_session_model),
                    if agent.disabled == Some(true) { "; disabled" } else { "" }
                );
                lines.push(name.to_string());
                lines.push("  model:".to_string());
                lines.push(format!("    {resolved}"));
                lines.push(format!("  source: {source}"));
                lines.push(String::new());
            }
        }
    }
    // SUBA-086 — pi `handleModels` (`agent-management.ts:1074` @v0.64.0): `if (!requestedAgent)
    // appendAgentDiagnosticLines(lines, diagnosticsForScope(discovered.agentDiagnostics, scope))`
    // — only the all-builtins listing carries the block (a requested agent is answered above).
    // cyrup's `models` takes no `agentScope`, so the filter is the `both` identity.
    append_agent_diagnostic_lines(&mut lines, &diagnostics_for_scope(&d.agent_diagnostics, None));
    Ok(ManagementOutcome::ok(lines.join("\n")))
}

/// pi `handleCreate` (`agent-management.ts:908-975`). Model/skills registry warnings are deferred
/// (see `discovery/management.rs`'s own C3 section header, preserved on
/// [`crate::discovery::management::handle_management_action`]); the create + name-collision +
/// shadow-note + unknown-agent warnings are faithful.
pub(crate) fn handle_create(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    use serde_json::Value;
    let cfg_map = match config_object(req.config) {
        Ok(Some(map)) => map,
        Ok(None) => return Ok(ManagementOutcome::err("config required for create.")),
        Err(e) => return Ok(ManagementOutcome::err(e)),
    };
    let name_raw = match cfg_map.get("name").and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => s.to_string(),
        _ => {
            return Ok(ManagementOutcome::err("config.name is required and must be a non-empty string."))
        }
    };
    let description = match cfg_map.get("description").and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => {
            return Ok(ManagementOutcome::err("config.description is required and must be a non-empty string."))
        }
    };
    let local_name = sanitize_name(&name_raw);
    if local_name.is_empty() {
        return Ok(ManagementOutcome::err("config.name is invalid after sanitization. Use letters, numbers, spaces, or hyphens."));
    }
    let package_name = match parse_package_config(cfg_map.get("package")) {
        Ok(pkg) => pkg,
        Err(e) => return Ok(ManagementOutcome::err(e)),
    };
    let runtime_name = AgentDefinition::qualified_name(&local_name, package_name.as_deref());
    let scope = match cfg_map.get("scope") {
        None => AgentSource::User,
        Some(Value::String(s)) if s == "user" => AgentSource::User,
        Some(Value::String(s)) if s == "project" => AgentSource::Project,
        _ => return Ok(ManagementOutcome::err("config.scope must be 'user' or 'project'.")),
    };
    let is_chain = cfg_map.contains_key("steps");
    let d = discover_agents_all(cfg)?;

    let Some(scope_dir) = pick_scope_dir(cfg, scope, is_chain) else {
        return Ok(ManagementOutcome::err(format!(
            "no {} {} directory is configured.",
            source_str(scope),
            if is_chain { "chain" } else { "agent" }
        )));
    };

    if name_exists_in_scope(&d, scope, &runtime_name, None) {
        return Ok(ManagementOutcome::err(format!(
            "Name '{runtime_name}' already exists in {} scope. Use update instead.",
            source_str(scope)
        )));
    }

    let mut warnings: Vec<String> = Vec::new();
    if !is_chain
        && d.agents
            .iter()
            .any(|a| a.source == AgentSource::Builtin && a.name == runtime_name)
    {
        warnings.push(format!("Note: this shadows the builtin agent '{runtime_name}'."));
    }

    if is_chain {
        let steps = match parse_step_list(cfg_map.get("steps")) {
            Ok(s) => s,
            Err(e) => return Ok(ManagementOutcome::err(e)),
        };
        let created = create_chain_with_steps(
            &scope_dir,
            scope,
            &local_name,
            package_name.clone(),
            &description,
            steps.clone(),
        )?;
        let missing = unknown_chain_agents(&d, &steps);
        if !missing.is_empty() {
            warnings.push(format!(
                "Warning: chain steps reference unknown agents: {}.",
                missing.join(", ")
            ));
        }
        let mut lines = vec![format!(
            "Created chain '{runtime_name}' at {}.",
            created.file_path.display()
        )];
        lines.extend(warnings);
        return Ok(ManagementOutcome::ok(lines.join("\n")));
    }

    let mut fields = AgentFields {
        system_prompt_mode: Some(default_system_prompt_mode(&local_name)),
        inherit_project_context: Some(default_inherit_project_context(&local_name)),
        inherit_skills: Some(false),
        package_name: Some(package_name.clone()),
        system_prompt_body: Some(String::new()),
        ..AgentFields::default()
    };
    // On CREATE the target's name is the just-built runtime name (pi `agent-management.ts:953-965`
    // constructs the `AgentConfig` with `name: runtimeName` before calling `applyAgentConfig`).
    if let Err(e) = apply_agent_config(&mut fields, &cfg_map, &runtime_name) {
        return Ok(ManagementOutcome::err(e));
    }
    let Some(created) = create_agent(&scope_dir, scope, &local_name, &description, &fields)? else {
        // Pre-validated above via `parse_package_config`, so the low-level silent-skip path is
        // unreachable in practice; surface pi's own invalid-package text rather than panicking.
        return Ok(ManagementOutcome::err("config.package is invalid after sanitization."));
    };
    let mut lines = vec![format!(
        "Created agent '{runtime_name}' at {}.",
        created.file_path.display()
    )];
    lines.extend(warnings);
    Ok(ManagementOutcome::ok(lines.join("\n")))
}

/// pi `handleUpdate` (`agent-management.ts:977-1088`). Model/fallback/skills registry warnings are
/// deferred; rename, package repackaging, unknown-agent warnings, and the still-referenced-after-
/// rename warning are faithful.
pub(crate) fn handle_update(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    use serde_json::Value;
    if req.agent.is_none() && req.chain_name.is_none() {
        return Ok(ManagementOutcome::err("Specify 'agent' or 'chainName' for update."));
    }
    if req.agent.is_some() && req.chain_name.is_some() {
        return Ok(ManagementOutcome::err("Specify either 'agent' or 'chainName', not both."));
    }
    let cfg_map = match config_object(req.config) {
        Ok(Some(map)) => map,
        Ok(None) => return Ok(ManagementOutcome::err("config required for update.")),
        Err(e) => return Ok(ManagementOutcome::err(e)),
    };
    let scope_hint = disambiguation_scope(req.agent_scope);

    if let Some(agent_name) = req.agent {
        let d = discover_agents_all(cfg)?;
        let matches = find_agents(&d, agent_name, scope_hint);
        let available = available_agent_names(&d);
        let target = match resolve_target(TargetKind::Agent, agent_name, matches, &available, req.agent_scope) {
            Ok(t) => t,
            Err(outcome) => return Ok(outcome),
        };
        if cfg_map.contains_key("name")
            && !matches!(cfg_map.get("name").and_then(Value::as_str), Some(s) if !s.trim().is_empty())
        {
            return Ok(ManagementOutcome::err("config.name must be a non-empty string when provided."));
        }
        if cfg_map.contains_key("description")
            && !matches!(cfg_map.get("description").and_then(Value::as_str), Some(s) if !s.trim().is_empty())
        {
            return Ok(ManagementOutcome::err("config.description must be a non-empty string when provided."));
        }
        let old_name = target.name.clone();
        let mut new_local = target.local_name.clone();
        if cfg_map.contains_key("name") {
            new_local = sanitize_name(cfg_map.get("name").and_then(Value::as_str).unwrap_or(""));
            if new_local.is_empty() {
                return Ok(ManagementOutcome::err("config.name is invalid after sanitization."));
            }
        }
        let mut new_pkg = target.package_name.clone();
        if cfg_map.contains_key("package") {
            match parse_package_config(cfg_map.get("package")) {
                Ok(pkg) => new_pkg = pkg,
                Err(e) => return Ok(ManagementOutcome::err(e)),
            }
        }
        let mut fields = AgentFields::default();
        if let Err(e) = apply_agent_config(&mut fields, &cfg_map, &old_name) {
            return Ok(ManagementOutcome::err(e));
        }
        fields.local_name = Some(new_local.clone());
        fields.package_name = Some(new_pkg.clone());
        if cfg_map.contains_key("description") {
            fields.description = Some(
                cfg_map
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string(),
            );
        }
        let base = editable_base(&target);
        let Some(updated) = update_agent(&base, &fields)? else {
            return Ok(ManagementOutcome::err("config.package is invalid after sanitization."));
        };
        let new_runtime = updated.definition.name.clone();
        let final_outcome: AgentMutationOutcome = if new_runtime != old_name {
            rename_agent(&updated.definition, &new_local)?
        } else {
            updated
        };
        let mut warnings: Vec<String> = Vec::new();
        if new_runtime != old_name {
            let refs: Vec<String> = discover_agents_all(cfg)?
                .chains
                .iter()
                .filter(|c| c.steps.iter().any(|s| s.agent.as_deref() == Some(old_name.as_str())))
                .map(|c| format!("{} ({})", c.name, source_str(c.source)))
                .collect();
            if !refs.is_empty() {
                warnings.push(format!(
                    "Warning: chains still reference '{old_name}': {}.",
                    refs.join(", ")
                ));
            }
        }
        let headline = if new_runtime == old_name {
            format!("Updated agent '{new_runtime}' at {}.", final_outcome.file_path.display())
        } else {
            format!(
                "Updated agent '{old_name}' to '{new_runtime}' at {}.",
                final_outcome.file_path.display()
            )
        };
        let mut lines = vec![headline];
        lines.extend(warnings);
        return Ok(ManagementOutcome::ok(lines.join("\n")));
    }

    // Chain update.
    let chain_name = req.chain_name.unwrap_or_default();
    let d = discover_agents_all(cfg)?;
    let matches = find_chains(&d, chain_name, scope_hint);
    let available = available_chain_names(&d);
    let target = match resolve_target(TargetKind::Chain, chain_name, matches, &available, req.agent_scope) {
        Ok(t) => t,
        Err(outcome) => return Ok(outcome),
    };
    if cfg_map.contains_key("name")
        && !matches!(cfg_map.get("name").and_then(Value::as_str), Some(s) if !s.trim().is_empty())
    {
        return Ok(ManagementOutcome::err("config.name must be a non-empty string when provided."));
    }
    if cfg_map.contains_key("description")
        && !matches!(cfg_map.get("description").and_then(Value::as_str), Some(s) if !s.trim().is_empty())
    {
        return Ok(ManagementOutcome::err("config.description must be a non-empty string when provided."));
    }
    let old_name = target.name.clone();
    let mut new_local = target.local_name.clone();
    if cfg_map.contains_key("name") {
        new_local = sanitize_name(cfg_map.get("name").and_then(Value::as_str).unwrap_or(""));
        if new_local.is_empty() {
            return Ok(ManagementOutcome::err("config.name is invalid after sanitization."));
        }
    }
    let mut new_pkg = target.package_name.clone();
    if cfg_map.contains_key("package") {
        match parse_package_config(cfg_map.get("package")) {
            Ok(pkg) => new_pkg = pkg,
            Err(e) => return Ok(ManagementOutcome::err(e)),
        }
    }
    let mut new_steps: Option<Vec<ChainStepConfig>> = None;
    if cfg_map.contains_key("steps") {
        match parse_step_list(cfg_map.get("steps")) {
            Ok(s) => new_steps = Some(s),
            Err(e) => return Ok(ManagementOutcome::err(e)),
        }
    }
    let new_description = if cfg_map.contains_key("description") {
        cfg_map
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    } else {
        target.description.clone()
    };
    let mut warnings: Vec<String> = Vec::new();
    let steps = match &new_steps {
        Some(ns) => {
            let missing = unknown_chain_agents(&d, ns);
            if !missing.is_empty() {
                warnings.push(format!(
                    "Warning: chain steps reference unknown agents: {}.",
                    missing.join(", ")
                ));
            }
            ns.clone()
        }
        None => target.steps.clone(),
    };
    let updated = update_chain_full(&target, &new_local, new_pkg.clone(), &new_description, steps)?;
    let headline = if updated.name == old_name {
        format!("Updated chain '{}' at {}.", updated.name, updated.file_path.display())
    } else {
        format!(
            "Updated chain '{old_name}' to '{}' at {}.",
            updated.name,
            updated.file_path.display()
        )
    };
    let mut lines = vec![headline];
    lines.extend(warnings);
    Ok(ManagementOutcome::ok(lines.join("\n")))
}

/// pi `handleDelete` (`agent-management.ts:1090-1109`).
pub(crate) fn handle_delete(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    if req.agent.is_none() && req.chain_name.is_none() {
        return Ok(ManagementOutcome::err("Specify 'agent' or 'chainName' for delete."));
    }
    if req.agent.is_some() && req.chain_name.is_some() {
        return Ok(ManagementOutcome::err("Specify either 'agent' or 'chainName', not both."));
    }
    let scope_hint = disambiguation_scope(req.agent_scope);
    if let Some(agent_name) = req.agent {
        let d = discover_agents_all(cfg)?;
        let matches = find_agents(&d, agent_name, scope_hint);
        let available = available_agent_names(&d);
        let target = match resolve_target(TargetKind::Agent, agent_name, matches, &available, req.agent_scope) {
            Ok(t) => t,
            Err(outcome) => return Ok(outcome),
        };
        delete_agent(&target)?;
        let refs: Vec<String> = discover_agents_all(cfg)?
            .chains
            .iter()
            .filter(|c| c.steps.iter().any(|s| s.agent.as_deref() == Some(target.name.as_str())))
            .map(|c| format!("{} ({})", c.name, source_str(c.source)))
            .collect();
        let mut lines = vec![format!("Deleted agent '{}' at {}.", target.name, target.file_path.display())];
        if !refs.is_empty() {
            lines.push(format!(
                "Warning: chains reference deleted agent '{}': {}.",
                target.name,
                refs.join(", ")
            ));
        }
        return Ok(ManagementOutcome::ok(lines.join("\n")));
    }
    let chain_name = req.chain_name.unwrap_or_default();
    let d = discover_agents_all(cfg)?;
    let matches = find_chains(&d, chain_name, scope_hint);
    let available = available_chain_names(&d);
    let target = match resolve_target(TargetKind::Chain, chain_name, matches, &available, req.agent_scope) {
        Ok(t) => t,
        Err(outcome) => return Ok(outcome),
    };
    delete_chain(&target)?;
    Ok(ManagementOutcome::ok(format!(
        "Deleted chain '{}' at {}.",
        target.name,
        target.file_path.display()
    )))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::path::PathBuf;

    use super::*;
    use super::super::frontmatter_write::serialize_agent;
    use super::super::test_support::sample_agent;

    /// G101: an `extensions` list that came from `subagents.defaultExtensions` is NOT the agent's
    /// own data. `editable_base` (pi `editableAgentConfig`, `agent-management.ts:243`) must drop it
    /// so a management update never BAKES the settings default into the `.md` file — where it would
    /// outlive the setting and stop tracking it.
    #[test]
    fn a_settings_defaulted_extension_list_is_never_baked_into_the_agent_file() {
        let mut agent = sample_agent(AgentSource::User, PathBuf::from("/seer.md"));
        agent.extensions = Some(vec!["shared-ext".to_string()]);
        agent.extensions_from_default = true;

        let base = editable_base(&agent);
        assert_eq!(base.extensions, None, "a defaulted list must not survive into the edit base");
        assert!(!base.extensions_from_default);
        assert!(
            !serialize_agent(&base, None).contains("extensions:"),
            "the serialized file must carry no extensions line at all"
        );

        // An agent's OWN declared list is untouched by the same path.
        let mut own = sample_agent(AgentSource::User, PathBuf::from("/seer.md"));
        own.extensions = Some(vec!["own-ext".to_string()]);
        own.extensions_from_default = false;
        assert_eq!(editable_base(&own).extensions, Some(vec!["own-ext".to_string()]));
        assert!(serialize_agent(&editable_base(&own), None).contains("extensions: own-ext"));
    }
}
