//! Mid-session tool toggling and the system-prompt rebuild it forces.
//!
//! Pi `agent-session.ts:786-828,2304`. The active-tool view shared with
//! [`crate::LiveHostServices`] so a guest's `setActiveTools`/`getActiveTools` and the host-side
//! toggle read and mutate the same state, plus the per-turn tool/model baseline the run driver
//! reads.

use std::sync::Arc;

use cyrup_core::{ModelRef, ToolLoadout};

use crate::tools::{BuiltPrompt, ToolInfo};

use super::AgentSession;

impl AgentSession {
    /// Names of the currently-active tools (Pi `getActiveToolNames`, agent-session.ts:786).
    pub fn active_tool_names(&self) -> Vec<String> {
        Self::lock(&self.dynamic_tools).active_names()
    }

    /// The names of the tools other tools can call through `ctx.executeTool()` — the active
    /// `direct` tools and every registered `codemode` or `deferred` one (Pi
    /// `getCallableToolNames`, `agent-session.ts:1457-1460` @v1.0.1). A tool that is callable is
    /// not necessarily declared to the model: that is [`Self::active_tool_names`] minus the
    /// declarations a `prepare_loadout` hook hid.
    pub fn callable_tool_names(&self) -> Vec<String> {
        Self::lock(&self.dynamic_tools).callable_names()
    }

    /// All enable-able tools with metadata (Pi `getAllTools`, agent-session.ts:794).
    pub fn all_tools(&self) -> Vec<ToolInfo> {
        Self::lock(&self.dynamic_tools).all()
    }

    /// One tool's definition by name (Pi `getToolDefinition`, agent-session.ts:806).
    pub fn tool_definition(&self, name: &str) -> Option<ToolInfo> {
        Self::lock(&self.dynamic_tools).get(name)
    }

    /// Push a rebuilt `(loadout, system_prompt)` for the next turn (Pi `setActiveToolsByName` tail,
    /// agent-session.ts:850-854). Shared by the host/CLI [`Self::set_active_tools_by_name`] path and
    /// the guest-driven drain in [`Self::apply_pending_control`] so both reach the live agent
    /// identically.
    pub(super) async fn push_active_tools(&self, loadout: ToolLoadout, prompt: BuiltPrompt) {
        self.apply_loadout(loadout).await;
        // The rebuilt prompt is the new BASE, not just this turn's value (Pi
        // `this._baseSystemPrompt = this._rebuildSystemPrompt(validToolNames)`, agent-session.ts:939).
        // Without this write the next run's `before_agent_start` reset in
        // [`Self::assemble_run_messages`] would restore the startup prompt and the model would be
        // described the startup tool set for the rest of the session.
        //
        // That is the whole of it: the agent holds no prompt (CODE-014), so nothing is pushed to it.
        // The base's SECTIONS reach the model as a diff row the next time the prompt is reconciled
        // with the transcript — at the start of the next run, and at every turn boundary of a run in
        // flight (`PolicyHooks::prepare_next_turn`) — which is pi's `_preparePromptAndToolLoadout`.
        // A `before_agent_start` handler's replacement lives in its own slot and is projected onto
        // the request, so a rebuild cannot undo it (DRIFT-033).
        *Self::lock(&self.base_prompt) = prompt;
        // EXT-005: keep the guest-visible `ctx.getSystemPrompt()` mirror in step with the session —
        // a tool-set rebuild rewrites the prompt (Pi `_rebuildSystemPrompt`, agent-session.ts:2304)
        // and a guest reading it back must see the rebuilt one. Between runs that is the base; in
        // a run it is the run's options until the turn boundary refreshes them (EXT-084).
        self.sync_prompt_mirror();
    }

    /// Hand a resolved loadout to the agent, reporting the `prepare_loadout` hooks that failed while
    /// it was resolved (pi `emitError({ event: "prepare_loadout" })`, `agent-session.ts:1556-1561`
    /// @v1.0.1) — a failed hook changes nothing, but it is not silent.
    pub(super) async fn apply_loadout(&self, loadout: ToolLoadout) {
        for failure in loadout.hook_failures() {
            self.services
                .ext_host
                .report_loadout_failure(&failure.tool, failure.message.clone());
        }
        self.agent.set_loadout(loadout).await;
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
        //
        // #2835: FILTERED, through the session-scoped selection resolved once at build time. This is
        // the SECOND of pi's two `_refreshToolRegistry` call paths (`:2612` via `refreshTools`; the
        // builder is `:2812`), and upstream reads `_allowedToolNames`/`_excludedToolNames` — session
        // FIELDS, not call-site locals — on both for exactly this reason. A tool registered after
        // `init`, which is precisely what the #2835 regression's `dynamic_tool` is, arrives here and
        // nowhere else: leaving this path unfiltered would keep the whole defect reachable while
        // every builder-level test passed.
        //
        // The REGISTERED set, not the active one: a `codemode`/`deferred`/`hidden` tool is
        // registered without being activated, and `merge_registered` is what decides activation
        // (pi `_isActivatedOnRegistration`, `agent-session.ts:3554` @v1.0.1).
        let ext_tools = match self.services.ext_host.registered_tools_filtered(
            &[],
            self.services.allowed_tool_names.as_ref(),
            &self.services.excluded_tool_names,
        ) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "extension tool refresh failed; the late tool stays invisible");
                return;
            }
        };
        let push = { Self::lock(&self.dynamic_tools).merge_registered(ext_tools) };
        if let Some((loadout, prompt)) = push {
            self.push_active_tools(loadout, prompt).await;
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
    /// The rebuilt system prompt follows the tool set: both drains below store the prompt they
    /// rebuild as the base, and `PolicyHooks::prepare_next_turn` reconciles that base with the
    /// transcript right after this returns (CODE-014; pi rebuilds its prompt options from
    /// `getActiveToolNames()` on every turn, `agent-session.ts:880-886` @v1.0.0). Before CODE-014 the
    /// drains discarded their rebuilt prompt, because storing it and pushing it to the agent would
    /// have clobbered a `before_agent_start` handler's sanitized replacement (DRIFT-033); that
    /// replacement now has a slot and a projection of its own, so there is nothing left to protect.
    pub(crate) async fn next_turn_tools(&self) -> ToolLoadout {
        // EXT-004: a tool an extension registered from a LIVE handler during this run.
        self.refresh_extension_tools().await;
        // A guest's `setActiveTools` queued from an event handler / mid-turn tool hook, re-resolved
        // against the registry the refresh above just updated (the queue holds the requested NAMES
        // precisely so this resolution happens after it — see `PendingActiveTools`). The rebuilt
        // prompt becomes the base, which the next reconciliation with the transcript writes.
        if let Some(names) = self.services.host_services.take_pending_active_tools() {
            let (loadout, prompt) = { Self::lock(&self.dynamic_tools).set_active(&names) };
            self.push_active_tools(loadout, prompt).await;
        }
        self.agent.loadout().await
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
        let (loadout, prompt) = { Self::lock(&self.dynamic_tools).set_active(names) };
        self.push_active_tools(loadout, prompt).await;
    }

    /// Restore the loadout the session's transcript now declares (pi `_restoreToolsFromTranscript`,
    /// `agent-session.ts:1762-1769` @v1.0.1), after `/tree` navigation has changed which branch the
    /// transcript is. `context` is the navigated branch's raw projection.
    ///
    /// The restored names replace the active set and stay pending until a tool of that name
    /// registers. A branch whose transcript declares nothing leaves the loadout as it is.
    pub(super) async fn restore_tools_from_transcript(
        &self,
        context: &[cyrup_session::AgentMessage],
    ) {
        let declared = crate::tools::declared_tool_names(context).map(|names| {
            names
                .into_iter()
                .filter(|name| {
                    crate::tools::is_allowed_tool(
                        self.services.allowed_tool_names.as_ref(),
                        &self.services.excluded_tool_names,
                        name,
                    )
                })
                .collect::<Vec<String>>()
        });
        let push = {
            let mut tools = Self::lock(&self.dynamic_tools);
            tools.clear_pending();
            declared.map(|names| tools.restore_declared(&names))
        };
        if let Some((loadout, prompt)) = push {
            self.push_active_tools(loadout, prompt).await;
        }
    }

    /// A run starts: the restored tools that did not register by now are dropped, so a tool that
    /// never registers does not stay pending (pi `_runAgentPrompt`, `agent-session.ts:1778-1782`
    /// @v1.0.1).
    pub(super) fn clear_pending_tools(&self) {
        Self::lock(&self.dynamic_tools).clear_pending();
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
        let wrapped = tools
            .into_iter()
            .map(|t| self.services.ext_host.wrap_tool(t))
            .collect();
        Self::lock(&self.dynamic_tools).register_custom(wrapped);
    }
}
