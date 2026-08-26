//! Discovery-driven lookup helpers (pi `findAgents`/`findChains`/`availableNames`/
//! `nameExistsInScope`/`unknownChainAgents` + `resolveTarget`). Split out of
//! `discovery/management.rs`'s own "discovery-driven lookup helpers" section. Every item here is
//! called only from `handlers.rs` (and, for `available_agent_names`/`name_exists_in_scope`,
//! `tier_actions.rs` too).

use std::collections::BTreeSet;
use std::path::Path;

use super::super::types::{AgentDefinition, AgentSource, ChainDefinition, ChainStepConfig};
use super::super::{AgentDiscoveryResult, AgentNameResolution, resolve_agent_name};
use super::helpers::{disambiguation_scope, sanitize_name, source_str};
use super::ManagementOutcome;

/// pi `findAgents` (`agent-management.ts:114-126` @ v0.43.0): ALIAS-AWARE lookup over the management
/// (disabled-inclusive) view, optionally narrowed to one scope, sorted by source label.
///
/// The upstream shape, verbatim:
/// 1. Resolve `raw` against the scoped list with [`resolve_agent_name`].
/// 2. If that neither matched nor was ambiguous, retry with the sanitized name (only when the
///    sanitized form actually differs). An AMBIGUOUS first attempt is NOT retried — the retry guard
///    is `!resolved.agent && !resolved.error`.
/// 3. On a hit, return EVERY definition sharing the resolved CANONICAL name (so a user file
///    shadowing a builtin still yields both tiers, which is what `resolve_target`'s
///    both-scopes/read-only messages are built on).
/// 4. On a miss OR an ambiguity, fall back to the per-candidate membership probe
///    `resolveAgentName(raw, [agent]).agent` — which, run against a ONE-element list, can never be
///    ambiguous, so this is exactly "every agent whose own name/localName/aliases answer to `raw`
///    (or to the sanitized form)". That is what surfaces the several distinct canonical names an
///    ambiguity error must list.
pub(crate) fn find_agents(
    d: &AgentDiscoveryResult,
    name: &str,
    scope: Option<AgentSource>,
) -> Vec<AgentDefinition> {
    let raw = name.trim();
    let sanitized = sanitize_name(raw);
    let scoped: Vec<AgentDefinition> = d
        .agents
        .iter()
        .filter(|a| scope.is_none() || Some(a.source) == scope)
        .cloned()
        .collect();

    let mut resolved = resolve_agent_name(raw, &scoped);
    if matches!(resolved, AgentNameResolution::NotFound) && sanitized != raw {
        resolved = resolve_agent_name(&sanitized, &scoped);
    }

    let mut matches: Vec<AgentDefinition> = if let Some(agent) = resolved.agent() {
        let canonical = agent.name.clone();
        scoped.iter().filter(|a| a.name == canonical).cloned().collect()
    } else {
        scoped
            .iter()
            .filter(|a| {
                let one = std::slice::from_ref(*a);
                resolve_agent_name(raw, one).agent().is_some()
                    || (sanitized != raw
                        && resolve_agent_name(&sanitized, one).agent().is_some())
            })
            .cloned()
            .collect()
    };
    matches.sort_by(|a, b| source_str(a.source).cmp(source_str(b.source)));
    matches
}

/// The DISTINCT canonical names present in a match set, sorted — pi's
/// `[...new Set(matches.map(m => m.name))].sort((a, b) => a.localeCompare(b))`
/// (`agent-management.ts:624-626,880-882` @v0.43.0). More than one entry means the requested name/alias is
/// ambiguous and every caller must refuse rather than pick.
pub(crate) fn distinct_agent_names<'a>(
    matches: impl IntoIterator<Item = &'a AgentDefinition>,
) -> Vec<String> {
    matches.into_iter().map(|a| a.name.clone()).collect::<BTreeSet<_>>().into_iter().collect()
}

/// pi `findChains` (`agent-management.ts:128-134`).
pub(crate) fn find_chains(
    d: &AgentDiscoveryResult,
    name: &str,
    scope: Option<AgentSource>,
) -> Vec<ChainDefinition> {
    let raw = name.trim();
    let sanitized = sanitize_name(raw);
    let mut matches: Vec<ChainDefinition> = d
        .chains
        .iter()
        .filter(|c| scope.is_none() || Some(c.source) == scope)
        .filter(|c| c.name == raw || c.name == sanitized)
        .cloned()
        .collect();
    matches.sort_by(|a, b| source_str(a.source).cmp(source_str(b.source)));
    matches
}

/// pi `availableNames(cwd, "agent")` (`agent-management.ts:108-112`): unique, sorted runtime names.
pub(crate) fn available_agent_names(d: &AgentDiscoveryResult) -> Vec<String> {
    d.agents
        .iter()
        .map(|a| a.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn available_chain_names(d: &AgentDiscoveryResult) -> Vec<String> {
    d.chains
        .iter()
        .map(|c| c.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// pi `nameExistsInScope` (`agent-management.ts:154-163`): whether an agent OR chain with this
/// runtime name already exists in the given writable scope (excluding one path, used on rename).
pub(crate) fn name_exists_in_scope(
    d: &AgentDiscoveryResult,
    scope: AgentSource,
    name: &str,
    exclude: Option<&Path>,
) -> bool {
    for a in &d.agents {
        if a.source == scope && a.name == name && Some(a.file_path.as_path()) != exclude {
            return true;
        }
    }
    for c in &d.chains {
        if c.source == scope && c.name == name && Some(c.file_path.as_path()) != exclude {
            return true;
        }
    }
    false
}

/// pi `unknownChainAgents` (`agent-management.ts:169-174`): step agents that resolve to no known
/// agent name, unique and sorted. Dynamic (agent-less) steps are skipped.
pub(crate) fn unknown_chain_agents(d: &AgentDiscoveryResult, steps: &[ChainStepConfig]) -> Vec<String> {
    // pi v0.43.0 (`agent-management.ts:169-174`) replaced the `new Set(allAgents(d).map(a => a.name))`
    // membership test with `!resolveAgentName(agentName, agents).agent`, so a step that names an
    // ALIAS is known and no longer warns. An ambiguous name yields no `.agent` and is therefore
    // reported as unknown — upstream's behaviour, and defensible: the chain cannot be run either way.
    let mut missing = BTreeSet::new();
    for step in steps {
        if let Some(agent) = &step.agent
            && resolve_agent_name(agent, &d.agents).agent().is_none()
        {
            missing.insert(agent.clone());
        }
    }
    missing.into_iter().collect()
}

/// Shared shape over the two writable-target kinds (agent/chain) so [`resolve_target`] is one
/// implementation.
pub(crate) trait MutableTarget: Clone {
    fn source(&self) -> AgentSource;
    fn file_path(&self) -> &Path;
    /// The target's CANONICAL name — pi widened `resolveTarget`'s bound to
    /// `T extends { name: string; … }` (`agent-management.ts:617`) precisely so it could reject a
    /// match set spanning several distinct names.
    fn target_name(&self) -> &str;
}

impl MutableTarget for AgentDefinition {
    fn source(&self) -> AgentSource {
        self.source
    }
    fn file_path(&self) -> &Path {
        &self.file_path
    }
    fn target_name(&self) -> &str {
        &self.name
    }
}

impl MutableTarget for ChainDefinition {
    fn source(&self) -> AgentSource {
        self.source
    }
    fn file_path(&self) -> &Path {
        &self.file_path
    }
    fn target_name(&self) -> &str {
        &self.name
    }
}

#[derive(Clone, Copy)]
pub(crate) enum TargetKind {
    Agent,
    Chain,
}

impl TargetKind {
    fn cap(self) -> &'static str {
        match self {
            TargetKind::Agent => "Agent",
            TargetKind::Chain => "Chain",
        }
    }
    fn low(self) -> &'static str {
        match self {
            TargetKind::Agent => "agent",
            TargetKind::Chain => "chain",
        }
    }
}

/// pi `resolveTarget` (`agent-management.ts:617-646`): pick the single writable target for a
/// mutating action, producing pi's exact read-only / not-found / disambiguation messages as an
/// error [`ManagementOutcome`].
pub(crate) fn resolve_target<T: MutableTarget>(
    kind: TargetKind,
    name: &str,
    matches: Vec<T>,
    available: &[String],
    scope_hint_raw: Option<&str>,
) -> Result<T, ManagementOutcome> {
    // pi `agent-management.ts:624-627` @ v0.43.0, ahead of every other branch: a match set spanning
    // several DISTINCT canonical names means the requested string was an ambiguous alias (or an
    // ambiguous name), and a mutating action must refuse outright rather than silently mutate one of
    // them. Names are listed sorted, de-duplicated.
    let distinct: Vec<String> = matches
        .iter()
        .map(|m| m.target_name().to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if distinct.len() > 1 {
        return Err(ManagementOutcome::err(format!(
            "Ambiguous {} alias or name '{}': {}",
            kind.low(),
            name,
            distinct.join(", ")
        )));
    }
    let mutable: Vec<T> = matches
        .iter()
        .filter(|m| m.source().is_writable())
        .cloned()
        .collect();
    if mutable.is_empty() {
        if !matches.is_empty() {
            return Err(ManagementOutcome::err(format!(
                "{} '{}' is read-only and cannot be modified. Create a same-named {} in user or project scope to override it.",
                kind.cap(),
                name,
                kind.low()
            )));
        }
        let avail = if available.is_empty() {
            "none".to_string()
        } else {
            available.join(", ")
        };
        return Err(ManagementOutcome::err(format!(
            "{} '{}' not found. Available: {}.",
            kind.cap(),
            name,
            avail
        )));
    }
    if mutable.len() == 1 {
        return mutable
            .into_iter()
            .next()
            .ok_or_else(|| ManagementOutcome::err("internal error: empty mutable set".to_string()));
    }
    let Some(scope) = disambiguation_scope(scope_hint_raw) else {
        let paths: Vec<String> = mutable
            .iter()
            .map(|m| format!("{}: {}", source_str(m.source()), m.file_path().display()))
            .collect();
        return Err(ManagementOutcome::err(format!(
            "{} '{}' exists in both scopes. Specify agentScope: 'user' or 'project'.\n{}",
            kind.cap(),
            name,
            paths.join("\n")
        )));
    };
    let scoped: Vec<T> = mutable.into_iter().filter(|m| m.source() == scope).collect();
    if scoped.is_empty() {
        return Err(ManagementOutcome::err(format!(
            "{} '{}' not found in scope '{}'.",
            kind.cap(),
            name,
            source_str(scope)
        )));
    }
    if scoped.len() > 1 {
        let paths: Vec<String> = scoped
            .iter()
            .map(|m| m.file_path().display().to_string())
            .collect();
        return Err(ManagementOutcome::err(format!(
            "Multiple {}s named '{}' found in scope '{}': {}",
            kind.low(),
            name,
            source_str(scope),
            paths.join(", ")
        )));
    }
    scoped
        .into_iter()
        .next()
        .ok_or_else(|| ManagementOutcome::err("internal error: empty scoped set".to_string()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::path::PathBuf;

    use super::*;
    use super::super::test_support::sample_agent;

    #[test]
    fn resolve_target_rejects_package_source_with_read_only_message() {
        // Package tier is not populated by discovery in a bare test cfg, so exercise the
        // management-layer read-only gate (pi resolveTarget) directly against a Package-sourced
        // match — the exact path a `subagent update/delete` on a packaged agent takes (R-SA-014).
        let mut pkg = sample_agent(AgentSource::Package, PathBuf::from("/pkg/acme.tool.md"));
        pkg.name = "acme.tool".to_string();
        let outcome = resolve_target(TargetKind::Agent, "acme.tool", vec![pkg], &[], None)
            .expect_err("a package-sourced target must be rejected as read-only");
        assert!(outcome.is_error);
        assert!(
            outcome.text.contains("Agent 'acme.tool' is read-only and cannot be modified"),
            "{}",
            outcome.text
        );
    }
}
