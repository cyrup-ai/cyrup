//! The `bg_wait` tool (SUBA-004; pi `runs/background/wait-tool.ts:8-45` @v0.68.0 +
//! `runs/background/subagent-wait.ts`).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::{
    CancelToken, TerminateHint, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};

use crate::extension::executor::SubagentExecutor;

/// The background-wait tool's registered name — pi `wait-tool.ts:38` @v0.68.0, `name: "bg_wait"`.
///
/// **No alias.** `registerWaitTool` registers exactly one tool (`:44`), and the old `wait` spelling
/// is not kept here either: nothing in this workspace is pinned to it.
///
/// This const is the single source of the name for every reader, and that is load-bearing rather
/// than tidy. `watchdog/permission_arbiter.rs`'s `INTERNAL_TOOLS` is pi `permissions.ts:8`, whose
/// whole job is to make this tool un-gateable so a parent policy cannot strand a child mid-run —
/// and a second literal there is exactly how that set came to name a tool that was never
/// registered.
pub(crate) const WAIT_TOOL_NAME: &str = "bg_wait";

/// The label the host shows for this tool — pi `wait-tool.ts:39` @v0.68.0, `label: "Background
/// Wait"`. A const rather than an inline literal so the stale-spelling guard
/// (`the_advertised_text_never_spells_the_tool_the_old_way`, below) can read it without
/// constructing a [`WaitTool`], which needs a live [`SubagentExecutor`].
pub(crate) const WAIT_TOOL_LABEL: &str = "Background Wait";

/// pi's `bg_wait` tool description (`wait-tool.ts:16-27` @v0.68.0), rebranded to cyrup's
/// binary/env names and cut to the four parameters cyrup's schema actually accepts
/// ([`wait_tool_parameters`] closes with `additionalProperties: false`, so advertising upstream's
/// `stopOnAttention` or its provider-job vocabulary here would advertise calls the host rejects).
/// The trailing sentence is appended only when the tool is configured off, exactly as upstream
/// appends its own "Configured behavior:" note (`:27`).
///
/// Every self-reference is spliced in from [`WAIT_TOOL_NAME`] rather than spelled out: this text
/// is the only thing that tells a model what to call, so it must not be able to name anything but
/// the registered tool.
fn wait_tool_description(enabled: bool) -> String {
    let mut text = String::from(
        "Block until background (async) subagent runs started in this session finish, then \
         return.\n\nOrdinary async subagent runs already notify this session natively when they \
         complete or need attention; in an interactive chat, return control instead of calling \
         this merely to wait. Use it when you have no independent work left and must not end your \
         turn — for example inside a skill that has to run to completion, or any non-interactive \
         run (`cyrup -p ...`) where the whole task is a single turn and ending it would abandon \
         the still-running children.\n\n\
         • { } — return as soon as the FIRST active run finishes (default). Ideal for a rolling \
         fleet: launch N, wait, spawn a replacement for the one that finished, wait again — \
         keeping N in flight.\n\
         • { all: true } — block until EVERY active run in this session is finished.\n\
         • { id: \"...\" } — wait for one specific run (id or prefix) to finish.\n\
         • { id: \"...\", nonBlocking: true } — resolve the prefix once, persist an exact-run wake \
         subscription, and return immediately. This session is woken on completion, failure, \
         attention, reconciliation failure, or timeout — including in a later turn, and including \
         after the result payload has been cleaned up. Requires id; cannot be combined with all. \
         Inspect armed subscriptions with subagent({ action: \"status\" }).\n\
         • { timeoutMs: 600000 } — stop waiting after N ms (the runs keep going regardless; \
         default 30 min)\n\n",
    );
    text.push_str(WAIT_TOOL_NAME);
    text.push_str(
        " also returns when a run needs attention (a child that went idle or blocked for a \
         decision), not only on completion — so a stuck child never stalls the loop; the summary \
         names the run(s) to inspect/nudge/resume/interrupt. It polls the authoritative on-disk \
         run records (which also reconciles crashed runners), keeps the turn alive for normal \
         notification delivery, and resolves early if the turn is aborted.",
    );
    if !enabled {
        // pi `wait-tool.ts:27`: "Configured behavior: bg_wait is disabled by config.waitTool or
        // PI_SUBAGENT_WAIT_TOOL_ENABLED and returns immediately without blocking."
        text.push_str("\n\nConfigured behavior: ");
        text.push_str(WAIT_TOOL_NAME);
        text.push_str(" is disabled by config.waitTool or ");
        text.push_str(crate::background::wait::WAIT_TOOL_ENABLED_ENV);
        text.push_str(" and returns immediately without blocking.");
    }
    text
}

/// JSON Schema for [`WaitTool`]'s parameters (pi `WaitParams`, `runs/background/wait.ts:96-108` @v0.34.0).
pub(crate) fn wait_tool_parameters() -> serde_json::Value {
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
            },
            // SCOPE_11. This key and `WaitParams::non_blocking` MUST land together: the schema
            // below closes with `additionalProperties: false`, so a caller passing `nonBlocking`
            // against an un-edited schema is rejected by the host before `serde` sees it — and
            // `WaitParams` carries `#[serde(default)]` without `deny_unknown_fields`, so an
            // un-edited struct would silently DROP the key. Either half alone is a no-op, in
            // opposite directions.
            "nonBlocking": {
                "type": "boolean",
                "description": "Arm a durable wake subscription for the exact run `id` resolves to and return immediately instead of blocking. Requires id; cannot be combined with all. The session is woken on completion, failure, attention, reconciliation failure, or timeout, even in a later turn."
            }
        },
        "additionalProperties": false
    })
}

/// The `bg_wait` tool (SUBA-004): the ONLY way an orchestrator can block on a background subagent
/// run without ending its turn. See [`crate::background::wait`] for the loop itself, including the two
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
        Some(WAIT_TOOL_LABEL)
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
            .map_err(|e| ToolError::new(format!("invalid {WAIT_TOOL_NAME} tool call: {e}")))?;
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
            &self.executor.config_snapshot().await.roots,
        )
        // SUBA-034: subscribe this wait to the orchestrator's completion bus, so a result observed
        // by THIS process's watcher releases the wait immediately rather than one poll interval
        // later. The poll under it is unchanged and remains the source of truth.
        .with_completion_bus(Some(self.executor.completion_bus()))
        // ASYNC_NOTIFY_BUG_REPORT F3.4 — share the executor's inline-answer ledger, so the runs
        // this wait answers inline are not re-announced as standalone notifications.
        .with_inline_answers(Some(self.executor.inline_answers()))
        // The executor-owned consumed-payload record, so a completion the watcher already
        // delivered and deleted still resolves for a wait that lands a moment later.
        .with_wait_completions(self.executor.wait_completions())
        // SCOPE_11 — pi `wait-tool.ts:33`'s
        // `...(subscriptions && ctx?.hasUI ? { subscribe: (input) => subscriptions.arm(input) } : {})`.
        //
        // Upstream's `ctx?.hasUI` is a per-CALL tool context; cyrup's `Tool::execute` has no such
        // parameter, and its `has_ui` lives on `HostCtx` at the `SessionStart` edge instead. The
        // gate is therefore applied where the fact is actually known — the manager is INSTALLED
        // only for a session with a UI (`extension/host/native_impl.rs`'s `SessionStart` arm) — so
        // a headless runtime simply has no manager here and `wait_for_subagents` takes pi's own
        // `!deps.subscribe` refusal. Same observable, one fewer place for the two to disagree.
        .with_subscribe(self.executor.wait_subscriptions().map(|manager| {
            let arming: Arc<dyn crate::background::wait::WaitSubscriptionArming> = manager;
            crate::background::wait::WaitSubscribeHook::new(arming)
        }))
        // VL-S11b — pi `deps.state.foregroundRuns` (`subagent-wait.ts:226`): the executor's
        // remembered-run map, which is where a DETACHED foreground run lives and the only place it
        // ever lives (upstream never writes one to disk — `foreground-history.ts:67-69` — and
        // cyrup's `persist.rs` refuses one). Without this the candidate set is async runs only and
        // `bg_wait({ id })` cannot see a detached run at all.
        .with_detached_foreground(Some(self.executor.detached_foreground_hook()));
        let outcome = crate::background::wait::wait_for_subagents(&parsed, &cancel, &deps).await;
        // cyrup's `Tool::execute` returns `Result<ToolResult, ToolError>` and the host maps `Err`
        // onto the result's error flag (`cyrup-core/src/tool.rs`: "Tools signal failure by
        // returning `Err(ToolError)`"), which is the same observable as pi's `isError`.
        //
        // It carries no loss. Upstream's error `result(…, true)` sites all pass TWO arguments —
        // the third, `completions`, is supplied only at the two terminal returns
        // (`subagent-wait.ts:752`, `:769`) — so the sole errored-result-with-completions case is
        // the `failOnFailedRuns`/`failOnAttention` flip (`:751`/`:768`). The `bg_wait` TOOL leaves
        // both flags `false` (`WaitDeps::for_cwd`, `background/wait.rs`, overridden nowhere
        // above), so for THIS caller `is_error()` implies `completions.is_empty()`. Auto-drain —
        // the one caller that does set them — takes the whole `WaitOutcome` and keeps everything.
        if outcome.is_error() {
            return Err(ToolError::new(outcome.text));
        }
        // pi `completionUsage(completions)` → `AgentToolResult.usage` (`subagent-wait.ts:318`,
        // `:338`, `:345`); `ToolResult::usage` is `Option<cyrup_core::Usage>`, which is exactly
        // `completion_usage`'s return type — no conversion.
        let usage = outcome.usage();
        // pi's `details` — `mode`, `results: []`, `completions` when non-empty, and the
        // `wait: { reason: "window_elapsed", … }` block a timed-out wait now carries instead of
        // failing the call.
        let details = Some(outcome.details());
        Ok(ToolResult {
            content: vec![cyrup_core::Content::text(outcome.text)],
            usage,
            details,
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::{WAIT_TOOL_LABEL, WAIT_TOOL_NAME, wait_tool_description};
    use crate::registration::tool_description::{
        COMPACT_SUBAGENT_TOOL_DESCRIPTION, SUBAGENT_SAFETY_GUIDANCE,
    };

    /// Every place `text` still spells the wait tool the OLD way: a `wait` that is NOT part of
    /// `bg_wait` (or any other `*_wait`) and that is used as a tool NAME — either called
    /// (`wait(`, `wait({`) or referred to as "wait tool".
    ///
    /// The English word is deliberately allowed: this prose legitimately says "wait for one
    /// specific run", "launch N, wait, spawn a replacement", "wait again" and "waiting", none of
    /// which a model can mistake for a tool name. Matching is case-insensitive so a re-capitalised
    /// `Wait(` cannot slip through, and each hit is returned with trailing context so the failure
    /// message points at the sentence.
    fn stale_tool_name_spellings(text: &str) -> Vec<String> {
        // Every index here comes from `match_indices`, so both slices land on char boundaries;
        // the prose carries `—` and `•`, which is why the captured context is taken by `chars()`
        // rather than by a byte range.
        let lowered = text.to_ascii_lowercase();
        let mut hits = Vec::new();
        for (index, _) in lowered.match_indices("wait") {
            let previous = lowered[..index].chars().next_back();
            if matches!(previous, Some(before) if before.is_alphanumeric() || before == '_') {
                // `bg_wait(`, `subagent_wait`, … — a qualified name, not the bare one.
                continue;
            }
            let rest = &lowered[index + "wait".len()..];
            if rest.starts_with('(') || rest.starts_with(" tool") {
                hits.push(lowered[index..].chars().take(48).collect());
            }
        }
        hits
    }

    /// SLASH_SURFACE §I.3 — the crate's own advertised text names the REGISTERED tool and nothing
    /// else. A model only ever learns the name from these four strings, so a stale one teaches it
    /// to call a tool that is not registered ("unknown tool"), which no type check catches.
    ///
    /// Exactly what this catches: any of the checked texts spelling a tool call as `wait(...)` /
    /// `wait({...})` or naming "the wait tool" instead of `bg_wait` — for example reverting
    /// `registration/tool_description.rs`'s two "use the bg_wait tool" sentences, or re-inlining a
    /// literal into [`wait_tool_description`] in place of [`WAIT_TOOL_NAME`]. It also fails if
    /// [`WAIT_TOOL_NAME`] is changed without the descriptions following, because each description
    /// is asserted to CONTAIN the const's current value.
    ///
    /// The label is checked for the stale spelling only: upstream's label is prose
    /// (`"Background Wait"`, `wait-tool.ts:39`), not the tool name, so it cannot contain it.
    #[test]
    fn the_advertised_text_never_spells_the_tool_the_old_way() {
        let enabled = wait_tool_description(true);
        let disabled = wait_tool_description(false);
        let described = [
            ("wait_tool_description(true)", enabled.as_str()),
            ("wait_tool_description(false)", disabled.as_str()),
            ("SUBAGENT_SAFETY_GUIDANCE", SUBAGENT_SAFETY_GUIDANCE),
            (
                "COMPACT_SUBAGENT_TOOL_DESCRIPTION",
                COMPACT_SUBAGENT_TOOL_DESCRIPTION,
            ),
        ];
        for (label, text) in described {
            assert!(
                text.contains(WAIT_TOOL_NAME),
                "{label} never names the registered tool `{WAIT_TOOL_NAME}`"
            );
            assert!(
                stale_tool_name_spellings(text).is_empty(),
                "{label} still spells the tool the old way: {:?}",
                stale_tool_name_spellings(text)
            );
        }
        assert!(
            stale_tool_name_spellings(WAIT_TOOL_LABEL).is_empty(),
            "the tool label still spells the tool the old way: {WAIT_TOOL_LABEL}"
        );
        // The disabled note is upstream's own sentence (`wait-tool.ts:27`) and names the tool once
        // more than the enabled form does.
        assert!(
            disabled.contains(&format!(
                "Configured behavior: {WAIT_TOOL_NAME} is disabled by"
            )),
            "{disabled}"
        );
    }
}
