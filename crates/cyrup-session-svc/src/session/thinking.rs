//! Reasoning-level and transport control.
//!
//! Pi `agent-session.ts` `setThinkingLevel`/`cycleThinkingLevel`. The levels the active model
//! supports, the setter that re-pushes them to the agent and the bash session env, and the
//! transport override.

use cyrup_core::ModelThinkingLevel;
use cyrup_ext::HostEvent;

use crate::error::SessionServiceError;
use crate::event::AgentSessionEvent;

use super::AgentSession;

impl AgentSession {
    /// The agent's current thinking level (Pi `thinkingLevel` getter, agent-session.ts:763).
    pub async fn thinking_level(&self) -> ModelThinkingLevel {
        self.agent.snapshot().await.thinking_level
    }

    /// The thinking levels the active model supports (Pi `getAvailableThinkingLevels`,
    /// agent-session.ts:1816-1819). A non-reasoning model supports only `off`.
    pub fn available_thinking_levels(&self) -> Vec<ModelThinkingLevel> {
        // Pi `if (!this.model) return [...THINKING_LEVEL_OPTIONS];` (agent-session.ts:1817), and
        // `THINKING_LEVEL_OPTIONS` is the FULL ladder — `["off","minimal","low","medium","high",
        // "xhigh","max"]` (`core/defaults.ts:4-12`). It is `EXTENDED_THINKING_LEVELS` here.
        //
        // This used to return a five-rung list against a citation (`agent-session.ts:297
        // THINKING_LEVELS`) that does not exist at 0.84.3; with no model resolved yet that silently
        // hid `xhigh`/`max` from `/thinking` and from the picker.
        let Some(model) = ({ Self::lock(&self.compaction_model).clone() }) else {
            return cyrup_provider::EXTENDED_THINKING_LEVELS.to_vec();
        };
        cyrup_provider::get_supported_thinking_levels(&model)
    }

    /// Whether the active model supports reasoning/thinking (Pi `supportsThinking`,
    /// agent-session.ts:1729-1731).
    pub fn supports_thinking(&self) -> bool {
        // Pi `return !!this.model?.reasoning;` (agent-session.ts:1730) — false with no model.
        Self::lock(&self.compaction_model).as_ref().is_some_and(|m| m.reasoning)
    }

    /// Set the thinking level, clamping to the model's capabilities, persisting a
    /// `thinking_level_change` entry and emitting the `thinking_level_select` ext event + the
    /// facade event — but only when the effective level actually changes (Pi `setThinkingLevel`,
    /// agent-session.ts:1677-1698).
    pub async fn set_thinking_level(
        &self,
        level: ModelThinkingLevel,
    ) -> Result<ModelThinkingLevel, SessionServiceError> {
        let model = { Self::lock(&self.compaction_model).clone() };
        // Pi `_clampThinkingLevel`: `return this.model ? clampThinkingLevel(this.model, level)
        // : "off";` (agent-session.ts:1608-1610) — a modelless session clamps everything to off.
        let effective = match model.as_ref() {
            Some(m) => cyrup_provider::clamp_thinking_level(m, level),
            None => ModelThinkingLevel::Off,
        };
        let previous = self.agent.snapshot().await.thinking_level;
        self.agent.set_thinking_level(effective).await;
        // Republish `CYRUP_REASONING_LEVEL` for the NEXT `bash` child (Pi re-reads `ctx.thinkingLevel`
        // on every `resolveSpawnContext`, bash.ts:180). Pushed BEFORE the no-change early return so
        // the handle is authoritative even when this call is a clamp-to-the-same-value no-op.
        self.bash_session_env
            .set_reasoning_level(crate::builder::thinking_level_to_str(effective));
        if effective == previous {
            return Ok(effective);
        }
        let level_str = crate::builder::thinking_level_to_str(effective);
        self.manager.lock().await.append_thinking_level_change(&level_str)?;
        // Only with a model installed: a guest's `ctx.model` stays `undefined` on a modelless
        // session (pi's `ExtensionContext.model` is the optional `session.model`).
        if let (Some(mr), Some(m)) = (Self::lock(&self.model).clone(), model.as_ref()) {
            self.services.host_services.update_model(
                mr,
                m.context_window,
                Some(level_str.clone()),
            );
        }
        self.fanout_emit(AgentSessionEvent::ThinkingLevelChanged { level: level_str.clone() }).await;
        let cancel = self.session_cancel.child_token();
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(
                &HostEvent::ThinkingLevelSelect {
                    level: level_str,
                    // pi's `previousLevel` (`extensions/types.ts:802-806`), read off the agent
                    // snapshot above and emitted only on a real change — the `effective ==
                    // previous` early return is pi's `if (isChanging)` guard
                    // (`agent-session.ts:1688-1697`).
                    previous_level: Some(
                        crate::builder::thinking_level_to_str(previous).to_string(),
                    ),
                },
                &cancel,
            )
            .await;
        Ok(effective)
    }

    /// Apply a `transport` settings value to the RUNNING agent — the second half of pi's
    /// `/settings` "Transport" handler:
    ///
    /// ```ts
    /// onTransportChange: (transport) => {
    ///     this.settingsManager.setTransport(transport);
    ///     this.session.agent.transport = transport;   // interactive-mode.ts:4215
    /// },
    /// ```
    ///
    /// cyrup did only the persist half, so cycling the row wrote JSON that nothing re-read until the
    /// next process start. `s` is the persisted `TransportSetting` string (`"auto" | "sse" |
    /// "websocket" | "websocket-cached"`); an unrecognized value falls back to `auto`, matching
    /// `getTransport()`'s `?? "auto"` (settings-manager.ts:751). Returns the transport actually
    /// applied. Takes effect from the NEXT run, exactly as pi's `createLoopConfig` read does
    /// (agent.ts:442).
    pub async fn set_transport(&self, s: &str) -> cyrup_provider::Transport {
        let t = crate::builder::parse_transport(s);
        self.agent.set_transport(Some(t)).await;
        t
    }

    /// Cycle to the next thinking level (Pi `cycleThinkingLevel`, agent-session.ts:1551). Returns
    /// `None` when the model does not support thinking.
    pub async fn cycle_thinking_level(&self) -> Result<Option<ModelThinkingLevel>, SessionServiceError> {
        if !self.supports_thinking() {
            return Ok(None);
        }
        let levels = self.available_thinking_levels();
        if levels.is_empty() {
            return Ok(None);
        }
        let current = self.thinking_level().await;
        let idx = levels.iter().position(|l| *l == current).unwrap_or(0);
        let Some(&next) = levels.get((idx + 1) % levels.len()) else {
            return Ok(None);
        };
        Ok(Some(self.set_thinking_level(next).await?))
    }
}
