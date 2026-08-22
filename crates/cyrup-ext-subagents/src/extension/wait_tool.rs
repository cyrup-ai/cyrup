//! The `wait` tool (SUBA-004; pi `extension/index.ts:509-527` + `runs/background/wait.ts`).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::{CancelToken, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};

use crate::extension::executor::SubagentExecutor;

/// The `wait` tool's registered name. Deliberately pi's v0.33/v0.34 name — upstream renamed it to
/// `subagent_wait` in `9245034` (2026-07-14), eight days AFTER v0.34.0, which is post-baseline
/// drift this port does not pull in.
pub(crate) const WAIT_TOOL_NAME: &str = "wait";

/// pi's `wait` tool description (`extension/index.ts:512-518`), rebranded to cyrup's binary/env
/// names. The trailing sentence is appended only when the tool is configured off, exactly as
/// upstream appends its own "Configured behavior:" note.
fn wait_tool_description(enabled: bool) -> String {
    let base = "Block until background (async) subagent runs started in this session finish, then \
                return.\n\nUse this after launching async subagents when you have no independent \
                work left and must not end your turn — for example inside a skill that has to run \
                to completion, or any non-interactive run (`cyrup -p ...`) where the whole task is \
                a single turn and ending it would abandon the still-running children.\n\n\
                • { } — return as soon as the FIRST active run finishes (default). Ideal for a \
                rolling fleet: launch N, wait, spawn a replacement for the one that finished, wait \
                again — keeping N in flight.\n\
                • { all: true } — block until EVERY active run in this session is finished.\n\
                • { id: \"...\" } — wait for one specific run (id or prefix) to finish.\n\
                • { timeoutMs: 600000 } — stop waiting after N ms (the runs keep going regardless; \
                default 30 min)\n\n\
                wait also returns when a run needs attention (a child that went idle or blocked \
                for a decision), not only on completion — so a stuck child never stalls the loop; \
                the summary names the run(s) to inspect/nudge/resume/interrupt. It polls the \
                authoritative on-disk run records (which also reconciles crashed runners), keeps \
                the turn alive for normal notification delivery, and resolves early if the turn is \
                aborted.";
    if enabled {
        base.to_string()
    } else {
        format!(
            "{base}\n\nConfigured behavior: wait is disabled by config.waitTool or \
             {} and returns immediately without blocking.",
            crate::background::wait::WAIT_TOOL_ENABLED_ENV
        )
    }
}

/// JSON Schema for [`WaitTool`]'s parameters (pi `WaitParams`, `runs/background/wait.ts:96-108` @v0.34.0).
fn wait_tool_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "Optional run id (or unambiguous prefix) to wait for. Omitted: wait across every active run."
            },
            "all": {
                "type": "boolean",
                "description": "Block until EVERY active run is finished. Default false: return as soon as the first one finishes."
            },
            "timeoutMs": {
                "type": "integer",
                "minimum": 1,
                "description": "Give up after this many milliseconds (default 1800000 = 30 minutes). The runs are detached and keep going."
            }
        },
        "additionalProperties": false
    })
}

/// The `wait` tool (SUBA-004): the ONLY way an orchestrator can block on a background subagent run
/// without ending its turn. See [`crate::background::wait`] for the loop itself, including the two
/// escape hatches (timeout + cancellation) that keep a wedged child from hanging the orchestrator.
///
/// Registered alongside [`crate::extension::SubagentTool`] in the [`crate::extension::host::registration::RegistrationMode::Full`] arm only: a fanout child
/// has no business blocking on its parent's whole async root (the same reasoning that makes
/// `control_status`'s no-id listing child-unsafe).
pub struct WaitTool {
    executor: Arc<SubagentExecutor>,
    cwd: PathBuf,
    parameters: serde_json::Value,
    description: String,
}

impl WaitTool {
    /// `enabled` is the already-resolved [`crate::background::wait::resolve_wait_tool_enabled`]
    /// verdict, captured at registration time exactly as pi captures `waitToolConfig` at extension
    /// load — so the advertised description and the runtime behavior can never disagree.
    #[must_use]
    pub fn new(executor: Arc<SubagentExecutor>, cwd: PathBuf, enabled: bool) -> Self {
        Self {
            executor,
            cwd,
            parameters: wait_tool_parameters(),
            description: wait_tool_description(enabled),
        }
    }

    /// The effective enabled verdict for this cwd: `CYRUP_SUBAGENT_WAIT_TOOL_ENABLED` over
    /// `config.waitTool` over pi's enabled-by-default. A malformed env value degrades to enabled
    /// (and is surfaced when the tool actually runs) rather than failing extension registration.
    pub(crate) async fn resolve_enabled(executor: &SubagentExecutor) -> bool {
        let cfg = executor.config_snapshot().await;
        let env = std::env::var(crate::background::wait::WAIT_TOOL_ENABLED_ENV).ok();
        crate::background::wait::resolve_wait_tool_enabled(cfg.wait_tool.as_ref(), env.as_deref())
            .unwrap_or(true)
    }
}

#[async_trait]
impl Tool for WaitTool {
    fn name(&self) -> &str {
        WAIT_TOOL_NAME
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn label(&self) -> Option<&str> {
        Some("Wait")
    }

    /// Blocks the calling turn. `cancel` is the host's own token for this tool call (pi's
    /// `AbortSignal`) and is threaded straight into the wait loop — aborting the turn releases the
    /// wait immediately instead of after the remaining poll interval.
    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let parsed: crate::background::wait::WaitParams = serde_json::from_value(params)
            .map_err(|e| ToolError::new(format!("invalid wait tool call: {e}")))?;
        // Re-resolved per call (not cached from registration) so a mid-session config/env change
        // takes effect; the registration-time verdict only fixes the advertised description.
        let enabled = Self::resolve_enabled(&self.executor).await;
        // SUBA-031: re-read per call for the same reason `enabled` is — pi reads
        // `deps.state.currentSessionId` at wait time, so a session switch between registration and
        // the call scopes the wait to the session that actually issued it.
        let deps = crate::background::wait::WaitDeps::for_cwd(
            &self.cwd,
            enabled,
            self.executor.current_session_id(),
        )
        // SUBA-034: subscribe this wait to the orchestrator's completion bus, so a result observed
        // by THIS process's watcher releases the wait immediately rather than one poll interval
        // later. The poll under it is unchanged and remains the source of truth.
        .with_completion_bus(Some(self.executor.completion_bus()));
        match crate::background::wait::wait_for_subagents(&parsed, &cancel, &deps).await {
            Ok(text) => Ok(ToolResult {
                content: vec![cyrup_core::Content::text(text)],
                details: Some(serde_json::json!({ "mode": "management" })),
                terminate: false,
                ..Default::default()
            }),
            Err(message) => Err(ToolError::new(message)),
        }
    }
}
