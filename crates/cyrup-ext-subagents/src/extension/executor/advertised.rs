//! SUBA-133 — the advertised-agent catalog's session state: pi's `advertisedAgents` /
//! `advertisedContext` pair and `refreshAdvertisedAgents` (`extension/index.ts:533-541` @v0.71.0).
//!
//! The catalog is discovered when the session starts and again after every successful agent
//! mutation (pi `onAgentsChanged`, `:636-642`), and read on every `before_agent_start` — so a turn
//! never walks the agent directories, and a manual edit outside the management surface shows up at
//! the next session start, exactly as upstream's cache behaves.

use std::path::{Path, PathBuf};

use crate::discovery::types::AgentDefinition;
use crate::extension::executor::SubagentExecutor;

/// pi `advertisedContext` + `advertisedAgents`.
#[derive(Debug, Default)]
pub(crate) struct AdvertisedAgents {
    /// The session's working directory, set at session start. `None` before the first session.
    cwd: Option<PathBuf>,
    /// The discovered agents that set `advertise: true`.
    agents: Vec<AgentDefinition>,
}

impl SubagentExecutor {
    /// pi `session_start`'s `advertisedContext = { cwd, model }; refreshAdvertisedAgents()`
    /// (`extension/index.ts:1189-1192` @v0.71.0).
    pub(crate) async fn start_advertised_agents(&self, cwd: &Path) {
        self.advertised_agents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cwd = Some(cwd.to_path_buf());
        self.refresh_advertised_agents().await;
    }

    /// pi `refreshAdvertisedAgents` (`:535-541`): withdraw the catalog, then rediscover it for the
    /// session's cwd. A discovery failure leaves it withdrawn until the next refresh — stale
    /// guidance is worse than none (`:641`).
    pub(crate) async fn refresh_advertised_agents(&self) {
        let cwd = {
            let mut state = self
                .advertised_agents
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.agents.clear();
            state.cwd.clone()
        };
        let Some(cwd) = cwd else {
            return;
        };
        let roots = self.config_snapshot().await.roots;
        match self
            .discovery_config(&cwd, &roots)
            .and_then(|cfg| crate::discovery::discover_agents(&cfg, None))
        {
            Ok(result) => {
                self.advertised_agents
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .agents = result
                    .agents
                    .into_iter()
                    .filter(|agent| agent.advertise == Some(true))
                    .collect();
            }
            Err(error) => tracing::warn!(
                target: "cyrup_ext_subagents::advertised",
                %error,
                "Failed to refresh advertised agents; catalog withdrawn until refresh"
            ),
        }
    }

    /// pi `buildAdvertisedAgentPrompt(advertisedAgents, resolveCurrentSubagentCapabilityCeiling(
    /// sessionId))` (`:825`): the catalog under this session's capability ceiling, read through the
    /// extension's env seam as every launch reads it. A malformed inherited ceiling advertises
    /// nothing — upstream's resolver throws there, and the hook changes no prompt.
    pub(crate) async fn advertised_agent_prompt(&self) -> Option<String> {
        let env_overrides = self.config_snapshot().await.env_overrides;
        let ceiling = crate::exec::capability_ceiling::resolve_current_capability_ceiling_from(
            self.current_session_id().as_deref(),
            &|key| match env_overrides.get(key) {
                Some(pinned) => pinned.clone(),
                None => std::env::var(key).ok(),
            },
        )
        .ok()?;
        let state = self
            .advertised_agents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::discovery::advertised::build_advertised_agent_prompt(&state.agents, ceiling.as_ref())
    }
}
