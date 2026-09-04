//! R-SA-013 call-site-dependent `disabled` visibility, and the R-SA-014 read-only-source guard
//! every mutating function in `agent_crud`/`chain_crud` calls first. Split out of
//! `discovery/management.rs`'s own "R-SA-013"/"R-SA-014" banner sections.

use super::super::types::{AgentDefinition, AgentSource, ChainDefinition};
use crate::error::SubagentError;

/// Three independently testable views over the *same* underlying agent set, differing only in
/// how `AgentDefinition::disabled` is treated (R-SA-013). Each is exposed as its own function
/// (rather than one function taking a boolean) so call sites are self-documenting and so a future
/// change to any one view's semantics cannot accidentally also change another's.
pub struct AgentVisibility;

impl AgentVisibility {
    /// Management/introspection listing: used for CRUD and re-enabling. MUST include disabled
    /// agents (R-SA-013) — a caller needs to *see* a disabled agent in order to re-enable it, so
    /// this view is deliberately unfiltered. Returns every entry in `agents`, in the same order.
    pub fn management(agents: &[AgentDefinition]) -> Vec<&AgentDefinition> {
        agents.iter().collect()
    }

    /// Delegation/execution-time selection: the view actual runtime dispatch uses to resolve a
    /// requested agent name. MUST exclude disabled agents (R-SA-013) — a disabled agent is not a
    /// valid delegation target regardless of how it is named.
    pub fn delegation(agents: &[AgentDefinition]) -> Vec<&AgentDefinition> {
        agents
            .iter()
            .filter(|a| !a.disabled.unwrap_or(false))
            .collect()
    }

    /// Human-facing list view (e.g. a `/subagents-list`-style command's default rendering).
    /// Filtered independently of [`Self::delegation`] (R-SA-013's "these are two distinct,
    /// independently testable behaviors" framing extends to keeping this call site textually
    /// separate from delegation's, not merely reusing its result under a different name) — a
    /// list view legitimately might diverge from delegation's filter in the future (e.g. gaining
    /// a `--show-disabled` flag that flips only this view's predicate without touching
    /// delegation's), so the two are never collapsed into a single shared function even though
    /// their current predicate is identical.
    pub fn list(agents: &[AgentDefinition]) -> Vec<&AgentDefinition> {
        agents
            .iter()
            .filter(|a| !a.disabled.unwrap_or(false))
            .collect()
    }
}

/// Chain-definition analog of [`AgentVisibility`]. `ChainDefinition` (func-SA §4.1) has no
/// `disabled` field of its own in the current data model, so all three views are currently
/// identical (unfiltered) passthroughs — kept as distinct functions for the same
/// forward-compatibility reason as [`AgentVisibility::list`] above, and so call sites read
/// identically to their agent counterparts.
pub struct ChainVisibility;

impl ChainVisibility {
    pub fn management(chains: &[ChainDefinition]) -> Vec<&ChainDefinition> {
        chains.iter().collect()
    }

    pub fn delegation(chains: &[ChainDefinition]) -> Vec<&ChainDefinition> {
        chains.iter().collect()
    }

    pub fn list(chains: &[ChainDefinition]) -> Vec<&ChainDefinition> {
        chains.iter().collect()
    }
}

/// Reject a management operation targeting a non-writable [`AgentSource`] (R-SA-014). Called
/// first, before any filesystem access, by every mutating function in this module.
pub(crate) fn require_writable_source(
    source: AgentSource,
    target_name: &str,
) -> Result<(), SubagentError> {
    if source.is_writable() {
        Ok(())
    } else {
        Err(SubagentError::ReadOnlySource(target_name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::path::PathBuf;

    use super::super::test_support::sample_agent;
    use super::*;

    fn agent_named(name: &str, disabled: Option<bool>) -> AgentDefinition {
        let mut a = sample_agent(
            AgentSource::Project,
            PathBuf::from(format!("/proj/{name}.md")),
        );
        a.name = name.to_string();
        a.local_name = name.to_string();
        a.disabled = disabled;
        a
    }

    #[test]
    fn management_view_includes_disabled_agents() {
        let agents = vec![
            agent_named("enabled-one", None),
            agent_named("disabled-one", Some(true)),
            agent_named("explicitly-enabled", Some(false)),
        ];
        let visible = AgentVisibility::management(&agents);
        assert_eq!(
            visible.len(),
            3,
            "management view MUST include disabled agents"
        );
        assert!(visible.iter().any(|a| a.name == "disabled-one"));
    }

    #[test]
    fn delegation_view_excludes_disabled_agents() {
        let agents = vec![
            agent_named("enabled-one", None),
            agent_named("disabled-one", Some(true)),
            agent_named("explicitly-enabled", Some(false)),
        ];
        let visible = AgentVisibility::delegation(&agents);
        assert_eq!(
            visible.len(),
            2,
            "delegation view MUST exclude disabled agents"
        );
        assert!(!visible.iter().any(|a| a.name == "disabled-one"));
        assert!(visible.iter().any(|a| a.name == "enabled-one"));
        assert!(visible.iter().any(|a| a.name == "explicitly-enabled"));
    }

    #[test]
    fn list_view_excludes_disabled_agents_independently_of_delegation() {
        let agents = vec![
            agent_named("enabled-one", None),
            agent_named("disabled-one", Some(true)),
        ];
        let list_visible = AgentVisibility::list(&agents);
        let delegation_visible = AgentVisibility::delegation(&agents);
        // Same current predicate, but the two are asserted independently (distinct function
        // calls, distinct assertions) so a future divergence between them is caught by whichever
        // assertion regresses, not silently passed by a single shared check.
        assert_eq!(list_visible.len(), 1);
        assert_eq!(delegation_visible.len(), 1);
        assert!(!list_visible.iter().any(|a| a.name == "disabled-one"));
        assert!(!delegation_visible.iter().any(|a| a.name == "disabled-one"));
    }

    #[test]
    fn three_visibility_views_diverge_exactly_on_disabled_agents() {
        let agents = vec![
            agent_named("a", None),
            agent_named("b", Some(true)),
            agent_named("c", Some(false)),
        ];
        assert_eq!(AgentVisibility::management(&agents).len(), 3);
        assert_eq!(AgentVisibility::delegation(&agents).len(), 2);
        assert_eq!(AgentVisibility::list(&agents).len(), 2);
    }

    #[test]
    fn chain_visibility_views_are_all_unfiltered_passthroughs() {
        let chains = vec![
            super::super::test_support::sample_chain(
                AgentSource::User,
                PathBuf::from("/user/a.chain.json"),
            ),
            super::super::test_support::sample_chain(
                AgentSource::Project,
                PathBuf::from("/proj/a.chain.json"),
            ),
        ];
        assert_eq!(ChainVisibility::management(&chains).len(), 2);
        assert_eq!(ChainVisibility::delegation(&chains).len(), 2);
        assert_eq!(ChainVisibility::list(&chains).len(), 2);
    }
}
