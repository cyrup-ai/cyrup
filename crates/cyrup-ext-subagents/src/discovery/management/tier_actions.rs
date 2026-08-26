//! SUBA-005: the tier-aware / settings-writing management actions (pi `handleEject`/
//! `handleDisable`/`handleEnable`/`handleReset`, `agent-management.ts:1111-1240`). Split out of
//! `discovery/management.rs`'s own "SUBA-005" section.
//!
//! These four are the last of pi's ten `ManagementAction`s to be ported. They differ from the six
//! CRUD actions in `handlers.rs` in two ways that shape everything below:
//!
//!  1. **They are tier-aware.** `eject`/`reset` must see the *bundled* (builtin/package) source file
//!     even when a same-named user/project file shadows it out of the R-SA-001 merge, and must
//!     separately see that shadowing file. `discover_agents_all` returns only the merge winner, so
//!     these two read [`crate::discovery::scan_agent_tiers`] (the raw, unmerged four-tier scan —
//!     pi's `d.builtin`/`d.package`/`d.user`/`d.project`) *in addition to* the merged view they use
//!     for name/chain-collision checks and the "Available: …" listing.
//!  2. **`disable`/`enable`/`reset` WRITE `settings.json`,** via
//!     [`crate::discovery::settings_write`]. They are the only management actions that mutate
//!     anything other than an agent `.md` file.
//!
//! pi's distinguishing behavior — faithfully reproduced — is that `disable`/`enable` do not trust
//! the write: they RE-RUN discovery afterwards and report an error if the agent's effective state did
//! not actually change, naming the higher-precedence scope that is winning. A settings write that is
//! silently overruled by the other scope is exactly the failure a user cannot debug on their own.

use std::path::PathBuf;

use super::super::types::AgentSource;
use super::super::{resolve_agent_name, AgentDiscoveryConfig, AgentDiscoveryResult, AgentNameResolution};
use super::agent_crud::agent_file_path;
use super::helpers::{override_scope_str, pick_scope_dir, sanitize_name, source_str};
use super::lookup::{available_agent_names, name_exists_in_scope};
use super::{ManagementOutcome, ManagementRequest};
use crate::error::SubagentError;

/// pi `actionScope` (`agent-management.ts:84-88`): unlike the CRUD actions' `asDisambiguationScope`
/// (where an absent/unrecognized `agentScope` means "both, disambiguate later"), these four actions
/// each write to exactly ONE scope, so an absent `agentScope` defaults to `user` and anything other
/// than `user`/`project` is a hard validation error naming the action.
fn action_scope(scope: Option<&str>, action: &str) -> Result<AgentSource, ManagementOutcome> {
    match scope {
        None => Ok(AgentSource::User),
        Some("user") => Ok(AgentSource::User),
        Some("project") => Ok(AgentSource::Project),
        _ => Err(ManagementOutcome::err(format!(
            "agentScope must be 'user' or 'project' for {action}."
        ))),
    }
}

/// pi `resolveEffectiveAgent` (`agent-management.ts:138-152` @ v0.43.0, renamed from
/// `pickEffectiveAgent` when it became alias-aware): the single highest-precedence agent answering
/// to `name` — verbatim, by alias, or (only when neither matched) after [`sanitize_name`].
///
/// Three outcomes, matching pi's `{ agent?, error? }`:
/// * `Ok(Some(agent))` — resolved.
/// * `Ok(None)` — nothing answers; the caller emits its own "not found. Available: …".
/// * `Err(message)` — the name/alias is AMBIGUOUS; the caller surfaces the message verbatim. This
///   outcome did not exist before aliases, and it must not be collapsed into `Ok(None)`: a
///   "not found" message for a name that matched two agents would be actively misleading.
///
/// The sanitized retry is gated on the first attempt being a clean MISS (pi's
/// `!resolved.agent && !resolved.error`) — an ambiguous raw name is never retried.
///
/// pi reduces over its concatenated per-tier arrays by `AGENT_SOURCE_PRECEDENCE`; cyrup's
/// `discover_agents_all` has *already* performed that reduction per name (R-SA-001), so the reduce
/// below normally sees a single element — it is kept so the precedence rule is stated, not implied.
fn resolve_effective_agent(
    d: &AgentDiscoveryResult,
    name: &str,
) -> Result<Option<super::super::types::AgentDefinition>, String> {
    let raw = name.trim();
    let mut resolved = resolve_agent_name(raw, &d.agents);
    if matches!(resolved, AgentNameResolution::NotFound) {
        let sanitized = sanitize_name(raw);
        if sanitized != raw {
            resolved = resolve_agent_name(&sanitized, &d.agents);
        }
    }
    if let Some(err) = resolved.error() {
        return Err(err.to_string());
    }
    let Some(agent) = resolved.agent() else {
        return Ok(None);
    };
    let canonical = agent.name.clone();
    Ok(d.agents
        .iter()
        .filter(|a| a.name == canonical)
        .min_by_key(|a| a.source.precedence_rank())
        .cloned())
}

/// The bundled (read-only) tiers in pi's own `[...d.package, ...d.builtin]` search order
/// (`agent-management.ts:917`, `:1005`) — package first, so a package agent shadowing a same-named
/// builtin is the one `eject`/`reset` treat as "the bundled default", matching R-SA-001's
/// Package-beats-Builtin precedence.
fn find_bundled<'a>(
    tiers: &'a crate::discovery::merge::TieredAgents,
    raw: &str,
    sanitized: &str,
) -> Option<&'a super::super::types::AgentDefinition> {
    tiers
        .package
        .iter()
        .chain(tiers.builtin.iter())
        .find(|a| a.name == raw || a.name == sanitized)
}

/// The raw (unmerged) writable tier for `scope` — pi's `scope === "user" ? d.user : d.project`.
fn writable_tier(
    tiers: &crate::discovery::merge::TieredAgents,
    scope: AgentSource,
) -> &[super::super::types::AgentDefinition] {
    match scope {
        AgentSource::Project => &tiers.project,
        _ => &tiers.user,
    }
}

/// The `settings.json` path these actions write for `scope`, or pi's verbatim refusal when the
/// project scope does not exist at all (`agent-management.ts:1157`, mirrored for enable/reset at `:1181`/`:1210`).
///
/// `project_settings_path` is `None` **only** when the discovery config was built with no project
/// root; an existing project root whose `settings.json` has not been created yet is `Some(path)`
/// (the writers below `mkdir -p` and create it). pi's `.pi or .agents` wording is rebranded to
/// cyrup's own config-directory names, matching this crate's standing `.pi` -> `.cyrup` rename.
fn scope_settings_path(
    cfg: &AgentDiscoveryConfig,
    scope: AgentSource,
) -> Result<PathBuf, ManagementOutcome> {
    match scope {
        AgentSource::Project => cfg.override_settings.project_settings_path.clone().ok_or_else(|| {
            ManagementOutcome::err(
                "Project override is not available here: no project config root (.cyrup or .agents) \
                 was found above the cwd. Use agentScope: 'user' or run from inside a project.",
            )
        }),
        _ => Ok(cfg.override_settings.user_settings_path.clone()),
    }
}

/// Re-read BOTH `settings.json` files from disk into a fresh copy of `cfg`.
///
/// **Required after any settings write, and easy to get wrong.** pi's post-write verification calls
/// `discoverAgentsAll(ctx.cwd)`, which re-reads the settings files as part of discovery. cyrup's
/// [`crate::discovery::discover_agents_all`] instead consumes the already-parsed
/// [`AgentDiscoveryConfig::override_settings`] snapshot its caller loaded — so re-running it with
/// the SAME `cfg` after a write re-applies the PRE-write settings and reports the exact opposite of
/// what happened (a successful disable reads back as "still enabled", a successful enable as "still
/// disabled"). Every `disable`/`enable` verification below goes through this function.
fn with_settings_reread(cfg: &AgentDiscoveryConfig) -> Result<AgentDiscoveryConfig, SubagentError> {
    let mut refreshed = cfg.clone();
    refreshed.override_settings = crate::discovery::load_layered_override_settings(
        &cfg.override_settings.user_settings_path,
        cfg.override_settings.project_settings_path.as_deref(),
    )?;
    Ok(refreshed)
}

/// pi `handleEject` (`agent-management.ts:1111-1147`): copy a read-only builtin/package agent file
/// verbatim into a writable scope so it can be customized, refusing rather than clobbering whenever
/// the destination is already occupied.
///
/// Deliberately a **byte-for-byte file copy**, not a re-serialization of the parsed
/// [`crate::discovery::types::AgentDefinition`]: an ejected file must be the bundled author's
/// original text (comments, field order, prose formatting and any frontmatter key this crate's
/// parser ignores all survive), which round-tripping through
/// [`crate::discovery::management::frontmatter_write::write_agent_file`] would not preserve.
pub(crate) fn handle_eject(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    let Some(agent_param) = req.agent else {
        return Ok(ManagementOutcome::err("Specify 'agent' for eject."));
    };
    let raw = agent_param.trim();
    let sanitized = sanitize_name(raw);
    let scope = match action_scope(req.agent_scope, "eject") {
        Ok(s) => s,
        Err(outcome) => return Ok(outcome),
    };

    let d = crate::discovery::discover_agents_all(cfg)?;
    let tiers = crate::discovery::scan_agent_tiers(cfg);
    let Some(source) = find_bundled(&tiers, raw, &sanitized) else {
        let avail = available_agent_names(&d);
        return Ok(ManagementOutcome::err(format!(
            "Agent '{raw}' not found or is not a bundled/package agent. eject copies a builtin or \
             package agent to {} scope so it can be customized. Available: {}.",
            source_str(scope),
            if avail.is_empty() { "none".to_string() } else { avail.join(", ") }
        )));
    };
    let runtime_name = source.name.clone();

    if let Some(existing) = writable_tier(&tiers, scope).iter().find(|a| a.name == runtime_name) {
        return Ok(ManagementOutcome::err(format!(
            "Agent '{runtime_name}' is already a custom {} agent at {}. Edit it with {{ action: \"update\", agent: \"{runtime_name}\" }} or delete it first.",
            source_str(scope),
            existing.file_path.display()
        )));
    }
    // The remaining collision pi's `nameExistsInScope` guards against is a same-named CHAIN (the
    // same-named-agent case is already answered above, from the raw tier rather than the merge).
    if name_exists_in_scope(&d, scope, &runtime_name, None) {
        return Ok(ManagementOutcome::err(format!(
            "An agent or chain named '{runtime_name}' already exists in {} scope. Remove or rename it first.",
            source_str(scope)
        )));
    }

    let Some(target_dir) = pick_scope_dir(cfg, scope, false) else {
        return Ok(ManagementOutcome::err(format!(
            "No {} agents directory is configured to eject into.",
            source_str(scope)
        )));
    };
    std::fs::create_dir_all(&target_dir).map_err(SubagentError::Spawn)?;
    let target_path = agent_file_path(&target_dir, &runtime_name);
    if target_path.exists() {
        // Reachable only when the destination holds a file discovery REFUSED to parse as an agent
        // (missing `name`/`description`, R-SA-005) — otherwise the tier check above would have
        // fired. Refuse rather than overwrite: the file is someone's, whatever it is.
        return Ok(ManagementOutcome::err(format!(
            "File already exists at {} but is not a valid agent definition. Remove or rename it first.",
            target_path.display()
        )));
    }
    let content = match std::fs::read_to_string(&source.file_path) {
        Ok(content) => content,
        Err(e) => {
            return Ok(ManagementOutcome::err(format!(
                "Failed to read source agent at {}: {e}",
                source.file_path.display()
            )));
        }
    };
    std::fs::write(&target_path, content).map_err(SubagentError::Spawn)?;
    Ok(ManagementOutcome::ok(format!(
        "Ejected agent '{runtime_name}' from {} to {} scope at {}. Edit it there to customize; it \
         shadows the bundled {} agent of the same name.",
        source_str(source.source),
        source_str(scope),
        target_path.display(),
        source_str(source.source)
    )))
}

/// pi `handleDisable` (`agent-management.ts:1149-1171`): write `{ disabled: true }` into
/// `subagents.agentOverrides.<name>` at `scope`, then RE-DISCOVER and verify the agent actually
/// became invisible — reporting an error (naming the winning scope) if a higher-precedence override
/// overruled the write.
pub(crate) async fn handle_disable(cfg: &AgentDiscoveryConfig, req: &ManagementRequest<'_>) -> Result<ManagementOutcome, SubagentError> {
    let Some(agent_param) = req.agent else {
        return Ok(ManagementOutcome::err("Specify 'agent' for disable."));
    };
    let raw = agent_param.trim();
    let scope = match action_scope(req.agent_scope, "disable") {
        Ok(s) => s,
        Err(outcome) => return Ok(outcome),
    };
    let settings_path = match scope_settings_path(cfg, scope) {
        Ok(p) => p,
        Err(outcome) => return Ok(outcome),
    };

    let d = crate::discovery::discover_agents_all(cfg)?;
    // pi `agent-management.ts:987-988` @ v0.43.0: the AMBIGUITY outcome is surfaced verbatim and
    // short-circuits ahead of the not-found message.
    let effective = match resolve_effective_agent(&d, raw) {
        Err(msg) => return Ok(ManagementOutcome::err(msg)),
        Ok(Some(agent)) => agent,
        Ok(None) => {
            let avail = available_agent_names(&d);
            return Ok(ManagementOutcome::err(format!(
                "Agent '{raw}' not found. Available: {}.",
                if avail.is_empty() { "none".to_string() } else { avail.join(", ") }
            )));
        }
    };
    let runtime_name = effective.name;

    let mut fields = serde_json::Map::new();
    fields.insert("disabled".to_string(), serde_json::Value::Bool(true));
    crate::discovery::settings_write::merge_builtin_agent_override(&settings_path, &runtime_name, &fields)
        .await?;

    // pi re-runs `discoverAgentsAll` and inspects the effective agent again: the write is only a
    // success if the agent is ACTUALLY disabled now. `discover_agents_all` is the management view,
    // which by R-SA-013 still lists disabled agents — so a disabled agent is found here with
    // `disabled: Some(true)`, which is precisely the signal being checked. The re-read
    // ([`with_settings_reread`]) is what makes this a verification rather than a replay of the
    // pre-write snapshot.
    let after = resolve_effective_agent(
        &crate::discovery::discover_agents_all(&with_settings_reread(cfg)?)?,
        raw,
    )
    .ok()
    .flatten();
    if after.as_ref().and_then(|a| a.disabled) == Some(true) {
        return Ok(ManagementOutcome::ok(format!(
            "Disabled agent '{runtime_name}' via {} settings override at {}. It is now hidden from \
             runtime discovery and {{ action: \"list\" }}.",
            source_str(scope),
            settings_path.display()
        )));
    }
    let winning = after
        .as_ref()
        .and_then(|a| a.override_info.as_ref())
        .map_or("project", |o| override_scope_str(o.scope));
    Ok(ManagementOutcome::err(format!(
        "Wrote a disabled override for '{runtime_name}' at {}, but the agent is still enabled. A \
         higher-precedence {winning} override is likely winning. Try agentScope: '{winning}'.",
        settings_path.display()
    )))
}

/// pi `handleEnable` (`agent-management.ts:1173-1199`): remove ONLY the `disabled` field from
/// `subagents.agentOverrides.<name>` at `scope` (an agent's other overrides — its model, tools,
/// thinking budget — survive being re-enabled), then re-discover and verify.
pub(crate) async fn handle_enable(cfg: &AgentDiscoveryConfig, req: &ManagementRequest<'_>) -> Result<ManagementOutcome, SubagentError> {
    let Some(agent_param) = req.agent else {
        return Ok(ManagementOutcome::err("Specify 'agent' for enable."));
    };
    let raw = agent_param.trim();
    let scope = match action_scope(req.agent_scope, "enable") {
        Ok(s) => s,
        Err(outcome) => return Ok(outcome),
    };
    let settings_path = match scope_settings_path(cfg, scope) {
        Ok(p) => p,
        Err(outcome) => return Ok(outcome),
    };

    let d = crate::discovery::discover_agents_all(cfg)?;
    // pi `agent-management.ts:987-988` @ v0.43.0: the AMBIGUITY outcome is surfaced verbatim and
    // short-circuits ahead of the not-found message.
    let effective = match resolve_effective_agent(&d, raw) {
        Err(msg) => return Ok(ManagementOutcome::err(msg)),
        Ok(Some(agent)) => agent,
        Ok(None) => {
            let avail = available_agent_names(&d);
            return Ok(ManagementOutcome::err(format!(
                "Agent '{raw}' not found. Available: {}.",
                if avail.is_empty() { "none".to_string() } else { avail.join(", ") }
            )));
        }
    };
    let runtime_name = effective.name;

    let removed = crate::discovery::settings_write::remove_builtin_agent_override_fields(
        &settings_path,
        &runtime_name,
        &["disabled"],
    ).await?;
    // Re-read from disk before verifying — see [`with_settings_reread`].
    let after = resolve_effective_agent(
        &crate::discovery::discover_agents_all(&with_settings_reread(cfg)?)?,
        raw,
    )
    .ok()
    .flatten();

    if let Some(after) = after.as_ref()
        && after.disabled != Some(true)
    {
        return Ok(ManagementOutcome::ok(if removed {
            format!(
                "Enabled agent '{runtime_name}' (removed disabled override at {}).",
                settings_path.display()
            )
        } else {
            format!("Agent '{runtime_name}' is already enabled.")
        }));
    }
    if let Some(info) = after.as_ref().and_then(|a| a.override_info.as_ref())
        && override_scope_str(info.scope) != source_str(scope)
    {
        return Ok(ManagementOutcome::err(format!(
            "Agent '{runtime_name}' is still disabled via a {} scope override at {}. Specify \
             agentScope: '{}' to enable it.",
            override_scope_str(info.scope),
            info.settings_path.display(),
            override_scope_str(info.scope)
        )));
    }
    let (hint_scope, hint_path) = after
        .as_ref()
        .and_then(|a| a.override_info.as_ref())
        .map_or_else(
            || (source_str(scope).to_string(), settings_path.display().to_string()),
            |o| (override_scope_str(o.scope).to_string(), o.settings_path.display().to_string()),
        );
    Ok(ManagementOutcome::err(format!(
        "Agent '{runtime_name}' is still disabled after removing the {} disabled override. It may \
         be hidden via subagents.disableBuiltins in {hint_scope} settings at {hint_path}.",
        source_str(scope)
    )))
}

/// pi `handleReset` (`agent-management.ts:1201-1240`): undo BOTH halves of a customization at
/// `scope` — delete the custom `.md` file that shadows a bundled agent, and delete the whole
/// `subagents.agentOverrides.<name>` entry — returning the agent to its bundled default.
///
/// Distinct from `delete` (which removes a custom agent that has no bundled default and leaves
/// settings alone) and from `enable` (which removes only the `disabled` field). Reset with nothing
/// to reset is a **success**, not an error, and says so.
pub(crate) async fn handle_reset(cfg: &AgentDiscoveryConfig, req: &ManagementRequest<'_>) -> Result<ManagementOutcome, SubagentError> {
    let Some(agent_param) = req.agent else {
        return Ok(ManagementOutcome::err("Specify 'agent' for reset."));
    };
    let raw = agent_param.trim();
    let sanitized = sanitize_name(raw);
    let scope = match action_scope(req.agent_scope, "reset") {
        Ok(s) => s,
        Err(outcome) => return Ok(outcome),
    };
    let settings_path = match scope_settings_path(cfg, scope) {
        Ok(p) => p,
        Err(outcome) => return Ok(outcome),
    };

    let d = crate::discovery::discover_agents_all(cfg)?;
    let tiers = crate::discovery::scan_agent_tiers(cfg);
    let Some(bundled) = find_bundled(&tiers, raw, &sanitized) else {
        let custom = tiers
            .user
            .iter()
            .chain(tiers.project.iter())
            .find(|a| a.name == raw || a.name == sanitized);
        if let Some(custom) = custom {
            return Ok(ManagementOutcome::err(format!(
                "Agent '{raw}' has no bundled default to reset to. Use {{ action: \"delete\", agent: \"{}\" }} to remove the custom {} agent.",
                custom.name,
                source_str(custom.source)
            )));
        }
        let avail = available_agent_names(&d);
        return Ok(ManagementOutcome::err(format!(
            "Agent '{raw}' not found. Available: {}.",
            if avail.is_empty() { "none".to_string() } else { avail.join(", ") }
        )));
    };
    let runtime_name = bundled.name.clone();
    let bundled_source = bundled.source;

    let mut lines: Vec<String> = Vec::new();
    if let Some(custom) = writable_tier(&tiers, scope)
        .iter()
        .find(|a| a.name == raw || a.name == sanitized)
    {
        std::fs::remove_file(&custom.file_path).map_err(SubagentError::Spawn)?;
        lines.push(format!(
            "Deleted custom {} agent file at {}.",
            source_str(scope),
            custom.file_path.display()
        ));
    }
    if crate::discovery::settings_write::remove_builtin_agent_override(&settings_path, &runtime_name).await? {
        lines.push(format!(
            "Removed {} settings override at {}.",
            source_str(scope),
            settings_path.display()
        ));
    }

    if lines.is_empty() {
        let other_scope = match scope {
            AgentSource::Project => AgentSource::User,
            _ => AgentSource::Project,
        };
        let other_custom = writable_tier(&tiers, other_scope)
            .iter()
            .any(|a| a.name == raw || a.name == sanitized);
        // pi reads `bundled.override?.scope` off its per-tier (override-applied) builtin entry;
        // cyrup applies overrides during the merge, so the equivalent provenance lives on the merged
        // winner for this name — which, in this branch (no customization at `scope`), is the bundled
        // agent itself unless the OTHER scope shadows it, exactly the case this hint is about.
        let has_other_override = d
            .agents
            .iter()
            .find(|a| a.name == runtime_name)
            .and_then(|a| a.override_info.as_ref())
            .is_some_and(|o| override_scope_str(o.scope) == source_str(other_scope));
        let note = if other_custom || has_other_override {
            format!(
                " Customization exists in {0} scope; specify agentScope: '{0}' to reset it.",
                source_str(other_scope)
            )
        } else {
            String::new()
        };
        return Ok(ManagementOutcome::ok(format!(
            "Agent '{runtime_name}' has no {} customization to reset.{note} It is at its bundled {} default.",
            source_str(scope),
            source_str(bundled_source)
        )));
    }
    lines.push(format!(
        "Reset agent '{runtime_name}' to its bundled {} default.",
        source_str(bundled_source)
    ));
    Ok(ManagementOutcome::ok(lines.join("\n")))
}
