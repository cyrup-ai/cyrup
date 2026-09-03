//! Mid-session tool toggling and the system-prompt rebuild it forces.
//!
//! Pi `agent-session.ts:786-828,2304`. The active-tool view shared with
//! [`crate::LiveHostServices`] so a guest's `setActiveTools`/`getActiveTools` and the host-side
//! toggle read and mutate the same state, plus the per-turn tool/model baseline the run driver
//! reads.

use std::sync::Arc;

use cyrup_core::ModelRef;

use crate::tools::ToolInfo;

use super::AgentSession;

impl AgentSession {
    /// Names of the currently-active tools (Pi `getActiveToolNames`, agent-session.ts:786).
    pub fn active_tool_names(&self) -> Vec<String> {
        Self::lock(&self.dynamic_tools).active_names()
    }

    /// All enable-able tools with metadata (Pi `getAllTools`, agent-session.ts:794).
    pub fn all_tools(&self) -> Vec<ToolInfo> {
        Self::lock(&self.dynamic_tools).all()
    }

    /// One tool's definition by name (Pi `getToolDefinition`, agent-session.ts:806).
    pub fn tool_definition(&self, name: &str) -> Option<ToolInfo> {
        Self::lock(&self.dynamic_tools).get(name)
    }

    /// Push a rebuilt `(tools, system_prompt)` onto the agent for the next turn (Pi
    /// `setActiveToolsByName` tail, agent-session.ts:850-854). Shared by the host/CLI
    /// [`Self::set_active_tools_by_name`] path and the guest-driven drain in
    /// [`Self::apply_pending_control`] so both reach the live agent identically.
    pub(super) async fn push_active_tools(&self, tools: Vec<Arc<dyn cyrup_core::Tool>>, prompt: String) {
        self.agent.set_tools(tools).await;
        // The rebuilt prompt is the new BASE, not just this turn's value (Pi
        // `this._baseSystemPrompt = this._rebuildSystemPrompt(validToolNames)`, agent-session.ts:939).
        // Without this write the next run's `before_agent_start` reset in
        // [`Self::assemble_run_messages`] would restore the startup prompt and the model would be
        // described the startup tool set for the rest of the session.
        *Self::lock(&self.base_system_prompt) = prompt.clone();
        // …and what reaches the AGENT is `override ?? base` — pi's very next line,
        // `this.agent.state.systemPrompt = this._systemPromptOverride ?? this._baseSystemPrompt;`
        // (agent-session.ts:940 @v0.83.0). A rebuild triggered mid-run by a tool registration used to
        // overwrite a `before_agent_start` handler's sanitized prompt with the raw rebuilt one
        // (DRIFT-033); resolving through the override slot is what stops it.
        let effective = self.effective_system_prompt();
        self.agent.set_system_prompt(effective).await;
        // EXT-005: keep the guest-visible `ctx.getSystemPrompt()` mirror in step with the agent —
        // a tool-set rebuild rewrites the prompt (Pi `_rebuildSystemPrompt`, agent-session.ts:2304)
        // and a guest reading it back must see the rebuilt one.
        self.services
            .host_services
            .update_prompt_state(Some(prompt), self.services.settings.project_trusted());
    }

    /// Surface tools an extension registered AFTER its `init` to the LIVE agent (EXT-004; Pi
    /// `refreshTools` → `_refreshToolRegistry`, extensions/loader.ts:249-256 →
    /// agent-session.ts:2452-2546).
    ///
    /// `ExtensionHost::refresh_tools` re-materializes a late descriptor into an executable
    /// `Arc<dyn Tool>`, but that alone only changes the extension host's view. The model's tool
    /// array and the system prompt come from [`crate::tools::DynamicToolState`], which the builder
    /// snapshots ONCE — so without this the tool existed and could not be called. Merging here
    /// mirrors Pi's tail exactly: new names are auto-activated (`if (!previousRegistryNames.has(
    /// toolName)) nextActiveToolNames.push(toolName)` … `setActiveToolsByName(...)`,
    /// agent-session.ts:2534-2545) and the rebuilt `(tools, prompt)` is pushed to the agent.
    ///
    /// Cheap and idempotent: a relaxed atomic load short-circuits when nothing was registered.
    pub(crate) async fn refresh_extension_tools(&self) {
        match self.services.ext_host.refresh_tools() {
            Ok(false) => return,
            Ok(true) => {}
            Err(e) => {
                tracing::warn!(error = %e, "extension tool refresh failed; the late tool stays invisible");
                return;
            }
        }
        // `&[]` = "no built-in base": what comes back is exactly the extension-contributed set,
        // which is what merges into the registry (the built-ins are already in it).
        let ext_tools = match self.services.ext_host.active_tools(&[]) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "extension tool refresh failed; the late tool stays invisible");
                return;
            }
        };
        let push = { Self::lock(&self.dynamic_tools).merge_registered(ext_tools) };
        if let Some((tools, prompt)) = push {
            self.push_active_tools(tools, prompt).await;
        }
    }

    /// The TURN-BOUNDARY tool refresh (Pi `_installAgentNextTurnRefresh`, agent-session.ts:519-540).
    /// Returns the tool array the agent should run the NEXT turn of the current run with, for
    /// `PolicyHooks::prepare_next_turn` to hand back as a [`cyrup_agent::TurnUpdate`].
    ///
    /// Pi's version is one line — `tools: this.agent.state.tools.slice()` — because `setActiveTools`
    /// mutates `agent.state.tools` synchronously and the loop re-reads its context every turn.
    /// cyrup's loop snapshots the array at run start, so the live value has to be pushed back in;
    /// the value itself still comes from `agent.state`, which is the single authority every mutation
    /// path already writes to ([`Self::push_active_tools`]).
    ///
    /// The two drains ahead of that read are the EXISTING EXT-004 mechanism, called at a new time
    /// rather than reimplemented — and in the same order as the post-run drain in
    /// [`Self::apply_pending_agent_control`]: the refresh runs first so an explicit `setActiveTools`
    /// still has the last word. Both are cheap no-ops when nothing changed (a relaxed atomic load
    /// and an `Option` take), which is the common case on every turn of every run.
    ///
    /// The rebuilt system prompt IS propagated now, through [`Self::effective_system_prompt`] —
    /// `PolicyHooks::prepare_next_turn` reads it beside this array and returns both, mirroring pi's
    /// `systemPrompt: this._systemPromptOverride ?? this._baseSystemPrompt` (agent-session.ts:534
    /// @v0.83.0). The reason it used not to be is gone: cyrup now keeps pi's two slots, so re-pushing
    /// resolves back to a `before_agent_start` handler's SANITIZED prompt (the permission companion's
    /// `shouldExposeTool` shaping) rather than clobbering it with the raw rebuild (DRIFT-033). Both
    /// drains below still discard their locally rebuilt prompt string for the same reason they always
    /// did — the authority is the base slot [`Self::push_active_tools`] writes, not a drain's
    /// by-product.
    pub(crate) async fn next_turn_tools(&self) -> Vec<Arc<dyn cyrup_core::Tool>> {
        // EXT-004: a tool an extension registered from a LIVE handler during this run.
        self.refresh_extension_tools().await;
        // A guest's `setActiveTools` queued from an event handler / mid-turn tool hook, re-resolved
        // against the registry the refresh above just updated (the queue holds the requested NAMES
        // precisely so this resolution happens after it — see `PendingActiveTools`). Array only,
        // prompt discarded — see above, and the identical rule in `assemble_run_messages`.
        if let Some(names) = self.services.host_services.take_pending_active_tools() {
            let (tools, _rebuilt_prompt) = { Self::lock(&self.dynamic_tools).set_active(&names) };
            self.agent.set_tools(tools).await;
        }
        self.agent.tools().await
    }

    /// The agent's live model + thinking level, for the per-turn refresh to stamp over whatever the
    /// extension seam returned (AGENT-017). pi reads exactly these two off the AGENT — `model:
    /// this.agent.state.model` (`agent-session.ts:537` @v0.83.0) and `thinkingLevel:
    /// this.agent.state.thinkingLevel` (`:538`) — not the session's mirrors, and stamps them AFTER
    /// the `...previousSnapshot` spread so the session out-votes an extension override.
    /// `None` (a modelless agent) leaves `TurnUpdate.model` unset, so the running loop keeps its
    /// own baseline — a run cannot be in flight without one anyway.
    pub(crate) async fn next_turn_model_baseline(
        &self,
    ) -> (Option<ModelRef>, cyrup_core::ModelThinkingLevel) {
        let snap = self.agent.snapshot().await;
        (snap.model, snap.thinking_level)
    }

    /// Set the active tool set by name, rebuilding the base system prompt and re-pushing both the
    /// tool array and the prompt to the agent for the next turn (Pi `setActiveToolsByName`,
    /// agent-session.ts:812). Unknown names are ignored.
    pub async fn set_active_tools_by_name(&self, names: &[String]) {
        let (tools, prompt) = { Self::lock(&self.dynamic_tools).set_active(names) };
        self.push_active_tools(tools, prompt).await;
    }

    /// Register additional custom tools into the enable-able registry (Pi `customTools`, sdk.ts:71,384).
    ///
    /// Each tool goes through [`cyrup_ext::ExtensionHost::wrap_tool`] first, exactly as the
    /// BUILD-TIME custom-tool path does (`builder.rs`'s
    /// `registry_tools.extend(cfg.custom_tools.iter().map(|t| ext_host.wrap_tool(t.clone())))`),
    /// which is the parity this method claims. pi wraps its SDK custom tools together with
    /// everything else in one `wrapRegisteredTools` pass (`core/agent-session.ts:2513`, over the
    /// `allCustomTools` list built at `:2472-2478`), so there is no upstream shape in which an
    /// SDK-supplied tool runs unwrapped. Unwrapped, the tool executed with NO extension
    /// `tool_call`/`tool_result` hooks around it — the permission gate and every observer extension
    /// were blind to it — and it never derived `addedToolNames`.
    pub fn register_custom_tools(&self, tools: Vec<Arc<dyn cyrup_core::Tool>>) {
        let wrapped = tools.into_iter().map(|t| self.services.ext_host.wrap_tool(t)).collect();
        Self::lock(&self.dynamic_tools).register_custom(wrapped);
    }
}
