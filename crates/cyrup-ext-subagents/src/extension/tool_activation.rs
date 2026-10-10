//! SUBA-139 / SUBA-153 — the `subagents_enable` loader and `config.toolActivation` (pi
//! `src/extension/tool-activation.ts` @v0.76.1, byte-identical since v0.74.0 `92a8a1f5` #2595).
//!
//! A fresh parent session can start with a small, model-only loader tool instead of the full
//! `subagent` tool. Calling the loader adds `subagent` to the active set; the session drains that
//! change at the next turn boundary (`AgentSession::next_turn_tools`), so the full tool is on the
//! next model request of the same run. Both tools are registered at `init`: nothing is registered
//! late, activation only toggles the active set through [`HostServices::set_active_tools`].
//!
//! The selection is decided at `session_start` and `session_tree` only ([`apply_recorded_selection`],
//! upstream `:55-79`); `before_agent_start` keeps the loader selected
//! ([`before_agent_start_loader_edit`], upstream `:138-145`). Switching models changes nothing
//! (upstream `:134`).
//!
//! # `[CYRUP-DELTA]`s
//!
//! - **`auto` asks one more question than upstream.** Upstream's predicate
//!   ([`compat_adds_tools_without_checkpoint`], ported verbatim) reads the model's compat flags,
//!   which describe what pi-ai's adapters do with a mid-conversation tool change. cyrup's adapters
//!   do not do it yet (PROV-133 / "PROV-083b"): every tool change rebuilds the request's tool list
//!   and misses the prompt cache. [`adds_tools_without_checkpoint`] therefore also requires
//!   [`cyrup_provider::api::emits_native_tool_additions`], which is `false` for every api today,
//!   so `auto` behaves as `eager` for a fresh session until an adapter lands its emitter.
//!   `dynamic` keeps upstream's unconditional loader.
//! - **Unsupported host.** Upstream does not register the loader when the host lacks the three
//!   dynamic-tool functions (`:88-94`). cyrup only learns that at `session_start` (a host with no
//!   live dynamic-tool view answers `None` from [`HostServices::active_tools`]), so the loader is
//!   registered with `default_active() == false`: on such a host it is never selected and the
//!   session behaves exactly as it did before the loader existed.
//! - **`toolsAdded: []`.** Upstream counts a system message that has the KEY with an empty array
//!   as a selection record (`Object.hasOwn`, `:22-25`). cyrup's [`cyrup_core::SystemMessage`]
//!   holds both lists as `Vec`s that are absent when empty, so the two are indistinguishable here.
//!   pi's own writer never emits an empty `toolsAdded` (`utils/transcript.ts:20`), so only a
//!   hand-built fixture can tell them apart.
//! - **No `session_start`, no loader.** Upstream starts `loaderSelected` at `true` (`:135`), which
//!   it never reads before `session_start`; cyrup starts it `false`, see [`ToolActivationState`].
//! - **`Activation failed: ${message}`** (`:111-119`) has no arm: cyrup's `set_active_tools`
//!   cannot fail.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use cyrup_core::{
    CancelToken, Tool, ToolCallId, ToolError, ToolExposure, ToolResult, ToolUpdateSink,
};
use cyrup_ext::host::HostServices;
use serde_json::Value;

use crate::extension::TOOL_NAME as SUBAGENT_NAME;
use crate::extension::executor::SubagentExecutor;
use crate::registration::ToolActivationMode;

/// pi `const LOADER_NAME = "subagents_enable"` (`tool-activation.ts:12` @v0.76.1).
pub(crate) const LOADER_NAME: &str = "subagents_enable";

/// pi `label: "Enable Subagents"` (`tool-activation.ts:101` @v0.76.1).
const LOADER_LABEL: &str = "Enable Subagents";

/// pi's loader `description` (`tool-activation.ts:102` @v0.76.1), verbatim.
const LOADER_DESCRIPTION: &str = "Enable pi-subagents delegation and management tools without launching work. Call when delegation is authorized by the current request or applicable user/project instructions, or when managing existing runs. Direct execution is the default; complexity alone never authorizes delegation. Full tools are available on the next model request.";

/// pi's loader `promptSnippet` (`tool-activation.ts:103` @v0.76.1), verbatim.
const LOADER_PROMPT_SNIPPET: &str = "pi-subagents is installed. For authorized specialist, independent-review, or parallel work, call subagents_enable, then subagent. Authorization must come from the current request or applicable instructions; complexity alone is not authorization.";

/// pi's success text (`tool-activation.ts:127` @v0.76.1), before the advertised catalog. The
/// product name in "Start Pi with" is cyrup's binary, as the `bg_wait` description says
/// `cyrup -p`; the flag is the same (`crates/cyrup/src/cli/args.rs` `--exclude-tools`).
pub(crate) const LOADER_ENABLED_TEXT: &str = "Enabled: subagent. If your tool list includes subagent (possibly prefixed), call subagent({action:\"list\",capabilities:true}). Otherwise, wait for the next user prompt; do not retry now. Start cyrup with --exclude-tools subagents_enable to keep subagent always available.";

/// pi `let warnedUnsupportedHost = false` (`tool-activation.ts:14` @v0.76.1): module-level, so the
/// warning is logged once per process however many sessions start.
static WARNED_UNSUPPORTED_HOST: AtomicBool = AtomicBool::new(false);

/// pi `addsToolsWithoutCheckpoint(model)` (`tool-activation.ts:34-53` @v0.76.1), verbatim, over
/// the raw `ctx.model` JSON ([`HostServices::current_model_info`]): `compat` is read with no
/// defaults applied, so a key the catalog does not declare does not qualify.
///
/// Upstream's comment (`:34-35`): *"Mirrors pi-ai's per-API transcript handling: without these
/// flags, a mid-conversation tool change makes Pi resend the conversation under a new leading
/// system message, missing the prompt cache."* In cyrup the flags are necessary but not
/// sufficient — see [`adds_tools_without_checkpoint`].
pub(crate) fn compat_adds_tools_without_checkpoint(model: Option<&Value>) -> bool {
    let Some(model) = model else {
        return false;
    };
    let compat = model.get("compat");
    let flag = |key: &str| compat.and_then(|c| c.get(key)) == Some(&Value::Bool(true));
    if !flag("supportsMidConvoSystemMessages") {
        return false;
    }
    match model.get("api").and_then(Value::as_str) {
        Some("anthropic-messages") => flag("supportsMidConvoToolChanges"),
        Some("openai-completions") => flag("supportsMidConvoToolAdditions"),
        Some("openai-responses" | "openai-codex-responses" | "azure-openai-responses") => {
            flag("supportsAdditionalTools") || flag("supportsToolSearch")
        }
        _ => false,
    }
}

/// The gate `auto` actually uses `[CYRUP-DELTA]`: upstream's
/// [`compat_adds_tools_without_checkpoint`] AND cyrup's adapter for that api really sending the
/// change natively ([`cyrup_provider::api::emits_native_tool_additions`] — `true` for
/// `anthropic-messages` since PROV-133, still `false` for the rest). Without the second half, `auto`
/// would pick the loader for models whose catalog flags promise a cache-safe enable that cyrup's
/// request builders do not deliver, and the first `subagents_enable` would resend the whole
/// conversation uncached. Now that the Anthropic adapter emits the change natively, `auto` DOES
/// choose the lazy loader for a mid-convo-capable Anthropic model — the behaviour change PROV-133
/// was meant to buy, and the reason that row's closure had to flip the predicate too.
pub(crate) fn adds_tools_without_checkpoint(model: Option<&Value>) -> bool {
    compat_adds_tools_without_checkpoint(model)
        && model
            .and_then(|m| m.get("api"))
            .and_then(Value::as_str)
            .is_some_and(cyrup_provider::api::emits_native_tool_additions)
}

/// The system messages of `messages`, in order.
fn system_messages(
    messages: &[cyrup_session::AgentMessage],
) -> impl Iterator<Item = &cyrup_core::SystemMessage> {
    messages.iter().filter_map(|message| match message {
        cyrup_session::AgentMessage::Core(cyrup_core::Message::System(system)) => Some(system),
        _ => None,
    })
}

/// pi `hasNativeToolSelection(messages)` (`tool-activation.ts:22-25` @v0.76.1). See the module
/// doc for the `toolsAdded: []` delta.
fn has_native_tool_selection(messages: &[cyrup_session::AgentMessage]) -> bool {
    system_messages(messages)
        .any(|system| !system.tools_added.is_empty() || !system.tools_removed.is_empty())
}

/// pi `setSelection(pi, includeSubagent, includeLoader)` (`tool-activation.ts:27-32` @v0.76.1):
/// unrelated active tools keep their order, the two names go to the end (`subagent` first), and
/// the result is de-duplicated keeping the first occurrence (`[...new Set(next)]`).
pub(crate) fn set_selection(
    services: &dyn HostServices,
    include_subagent: bool,
    include_loader: bool,
) {
    let Some(active) = services.active_tools() else {
        return;
    };
    let mut next: Vec<String> = active
        .into_iter()
        .filter(|name| {
            (include_subagent || name != SUBAGENT_NAME) && (include_loader || name != LOADER_NAME)
        })
        .collect();
    if include_subagent {
        next.push(SUBAGENT_NAME.to_string());
    }
    if include_loader {
        next.push(LOADER_NAME.to_string());
    }
    let mut seen = HashSet::new();
    next.retain(|name| seen.insert(name.clone()));
    services.set_active_tools(&next);
}

/// pi `applyRecordedSelection(pi, ctx, mode)` (`tool-activation.ts:55-79` @v0.76.1): decide this
/// session's selection and return whether the loader is selected.
///
/// - Host without a dynamic-tool view, or a loader that is not registered (`--exclude-tools
///   subagents_enable`): `true`, selection untouched (`:57-59`).
/// - A transcript with native tool-selection records: replay them (`:62-74`). `auto` never adds
///   the loader to a transcript that did not declare it (`:70`), because that is itself a tool
///   change; `dynamic` always does.
/// - No records: `auto` goes eager only for an EMPTY session whose model cannot take a new tool
///   without a checkpoint; any history keeps the `subagent` it already sent (`:75-78`).
pub(crate) async fn apply_recorded_selection(
    services: &dyn HostServices,
    mode: ToolActivationMode,
) -> bool {
    let (Some(available), Some(_)) = (services.all_tool_names(), services.active_tools()) else {
        return true;
    };
    if !available.iter().any(|name| name == LOADER_NAME) {
        return true;
    }
    let messages = services.session_context_messages().await;
    if has_native_tool_selection(&messages) {
        // Replayed here, as upstream does (`:63`), rather than trusting the session's own restore:
        // the answer must not depend on which side restored first.
        let mut recorded: Vec<String> = Vec::new();
        for system in system_messages(&messages) {
            for tool in &system.tools_removed {
                recorded.retain(|name| *name != tool.name);
            }
            for tool in &system.tools_added {
                if !recorded.contains(&tool.name) {
                    recorded.push(tool.name.clone());
                }
            }
        }
        let has = |name: &str| recorded.iter().any(|recorded| recorded == name);
        // "auto" never adds the loader to a transcript that did not declare it: that is itself a
        // tool change.
        let loader_selected = mode == ToolActivationMode::Dynamic || has(LOADER_NAME);
        set_selection(services, has(SUBAGENT_NAME), loader_selected);
        return loader_selected;
    }
    // "auto" goes eager only for an empty session; any history keeps the tools it already sent.
    let eager = mode == ToolActivationMode::Auto
        && messages.is_empty()
        && !adds_tools_without_checkpoint(services.current_model_info().as_ref());
    let subagent_active = services
        .active_tools()
        .is_some_and(|active| active.iter().any(|name| name == SUBAGENT_NAME));
    set_selection(
        services,
        eager || (!messages.is_empty() && subagent_active),
        !eager,
    );
    !eager
}

/// pi's `before_agent_start` handler body (`tool-activation.ts:138-145` @v0.76.1), for a session
/// whose loader is selected: keep the loader in `systemPromptOptions.selectedTools` (pi `??=` seeds
/// an absent list from the active set) and in the active set, so another handler or a
/// `setActiveTools` that dropped it cannot strand the session without a way to enable `subagent`.
///
/// Returns the WHOLE edited options object when it changed (`EventPatch::SystemPromptAndInject`
/// takes the full object, not a delta), `None` when nothing needed editing. The active-set repair
/// is a side effect on `services` either way.
pub(crate) fn before_agent_start_loader_edit(
    services: &dyn HostServices,
    options: &Value,
) -> Option<Value> {
    if !services
        .all_tool_names()
        .is_some_and(|all| all.iter().any(|name| name == LOADER_NAME))
    {
        return None;
    }
    let active = services.active_tools().unwrap_or_default();
    let mut edited = options.clone();
    let mut changed = false;
    if let Some(object) = edited.as_object_mut() {
        let selected = object.entry("selectedTools").or_insert_with(|| {
            changed = true;
            Value::Array(active.iter().cloned().map(Value::String).collect())
        });
        if let Some(names) = selected.as_array_mut()
            && !names.iter().any(|name| name.as_str() == Some(LOADER_NAME))
        {
            names.push(Value::String(LOADER_NAME.to_string()));
            changed = true;
        }
    }
    if !active.iter().any(|name| name == LOADER_NAME) {
        let mut next = active;
        next.push(LOADER_NAME.to_string());
        services.set_active_tools(&next);
    }
    changed.then_some(edited)
}

/// Whether `services` is a host the loader can work on: one with a live dynamic-tool view
/// (pi's `typeof pi.getAllTools/getActiveTools/setActiveTools === "function"`, `:88`).
fn host_supports_dynamic_tools(services: &dyn HostServices) -> bool {
    services.active_tools().is_some() && services.all_tool_names().is_some()
}

/// The extension's per-process tool-activation state: the mode the loader was registered under
/// (unset when no loader was registered — `eager`, or a `ChildSafe` child) and pi's
/// `let loaderSelected = true` (`tool-activation.ts:135` @v0.76.1).
///
/// `[CYRUP-DELTA]` — `loader_selected` starts `false`, not `true`. pi always emits `session_start`
/// before the first `before_agent_start`, so its initial `true` is never read. A cyrup session
/// whose host never binds extensions (an SDK caller that skips `bind_extensions`) emits no
/// `session_start` at all; starting `false` keeps such a session exactly as it was before the
/// loader existed, instead of adding the loader beside an active `subagent`.
#[derive(Debug, Default)]
pub(crate) struct ToolActivationState {
    mode: std::sync::OnceLock<ToolActivationMode>,
    loader_selected: AtomicBool,
}

impl ToolActivationState {
    /// Record that `init` registered the loader under `mode`.
    pub(crate) fn loader_registered(&self, mode: ToolActivationMode) {
        let _ = self.mode.set(mode);
    }

    /// pi's `session_start` / `session_tree` handlers (`tool-activation.ts:136-137` @v0.76.1).
    /// A no-op when no loader was registered. On a host without a live dynamic-tool view this is
    /// upstream's unsupported-host fallback (`:88-94`): one warning per process, the loader left
    /// unselected, `subagent` left as it is.
    pub(crate) async fn on_session_start_or_tree(&self, services: Option<Arc<dyn HostServices>>) {
        let Some(mode) = self.mode.get().copied() else {
            return;
        };
        let Some(services) = services.filter(|s| host_supports_dynamic_tools(s.as_ref())) else {
            if !WARNED_UNSUPPORTED_HOST.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    "[cyrup-subagents] Dynamic tool activation needs a live session tool registry; \
                     keeping subagent eagerly available."
                );
            }
            self.loader_selected.store(false, Ordering::Relaxed);
            return;
        };
        let selected = apply_recorded_selection(services.as_ref(), mode).await;
        self.loader_selected.store(selected, Ordering::Relaxed);
    }

    /// pi's `before_agent_start` handler (`tool-activation.ts:138-145` @v0.76.1): the edited
    /// options when the loader is selected and had to be put back, else `None`.
    pub(crate) fn on_before_agent_start(
        &self,
        services: Option<Arc<dyn HostServices>>,
        options: &Value,
    ) -> Option<Value> {
        if self.mode.get().is_none() || !self.loader_selected.load(Ordering::Relaxed) {
            return None;
        }
        before_agent_start_loader_edit(services?.as_ref(), options)
    }
}

/// The `subagents_enable` loader tool (pi `tool-activation.ts:96-132` @v0.76.1).
pub(crate) struct SubagentsEnableTool {
    executor: Arc<SubagentExecutor>,
    /// pi `Type.Object({})` (`:97`) — deliberately WITHOUT `additionalProperties: false`, so a
    /// stray argument (DeepSeek sends `{ action: "enable" }`, #2492) still validates.
    parameters: Value,
}

impl SubagentsEnableTool {
    pub(crate) fn new(executor: Arc<SubagentExecutor>) -> Self {
        Self {
            executor,
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        }
    }
}

/// An `is_error` result carrying `details`, pi's `{ isError: true, content, details }`.
fn error_result(text: &str, details: Value) -> ToolResult {
    ToolResult {
        content: vec![cyrup_core::Content::text(text)],
        details: Some(details),
        is_error: true,
        ..ToolResult::default()
    }
}

#[async_trait]
impl Tool for SubagentsEnableTool {
    fn name(&self) -> &str {
        LOADER_NAME
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn description(&self) -> &str {
        LOADER_DESCRIPTION
    }

    fn label(&self) -> Option<&str> {
        Some(LOADER_LABEL)
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(LOADER_PROMPT_SNIPPET)
    }

    /// pi `...MODEL_ONLY_TOOL` (`shared/extension-context.ts:9`, #2586): the model calls it
    /// directly; codemode's callable view does not offer it.
    fn exposure(&self) -> ToolExposure {
        ToolExposure::ModelOnly
    }

    /// `[CYRUP-DELTA]` — the cyrup form of upstream's unsupported-host fallback; see the module
    /// doc. On a live host [`apply_recorded_selection`] selects it explicitly.
    fn default_active(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let services = self.executor.host_services();
        let available = services
            .as_ref()
            .and_then(|s| s.all_tool_names())
            .is_some_and(|all| all.iter().any(|name| name == SUBAGENT_NAME));
        let Some(services) = services.filter(|_| available) else {
            return Ok(error_result(
                "Cannot enable unavailable tools: subagent.",
                serde_json::json!({ "unavailable": [SUBAGENT_NAME] }),
            ));
        };
        set_selection(services.as_ref(), true, true);
        if !services
            .active_tools()
            .is_some_and(|active| active.iter().any(|name| name == SUBAGENT_NAME))
        {
            return Ok(error_result(
                "Activation failed: subagent.",
                serde_json::json!({ "missing": [SUBAGENT_NAME] }),
            ));
        }
        let mut text = LOADER_ENABLED_TEXT.to_string();
        if let Some(advertised) = self
            .executor
            .advertised_agent_prompt()
            .await
            .filter(|advertised| !advertised.is_empty())
        {
            text.push_str("\n\n");
            text.push_str(&advertised);
        }
        Ok(ToolResult {
            content: vec![cyrup_core::Content::text(text)],
            details: Some(serde_json::json!({ "enabled": [SUBAGENT_NAME] })),
            ..ToolResult::default()
        })
    }
}

#[cfg(test)]
#[path = "tool_activation_tests.rs"]
mod tests;
