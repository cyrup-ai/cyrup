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
/// constructing a [`WaitTool`] from either source.
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
///
/// SUBA-232 — the second paragraph is the one place the two audiences differ, as upstream's own
/// `child ? … : …` (`wait-tool.ts:19` @ad11b7ab): a child has no native completion notifier, so it
/// is told to collect its owned descendants with a blocking wait before it answers.
fn wait_tool_description(enabled: bool, audience: WaitAudience) -> String {
    let mut text = String::from(
        "Block until background (async) subagent runs started in this session finish, then \
         return.\n\n",
    );
    match audience {
        WaitAudience::Orchestrator => text.push_str(
            "Ordinary async subagent runs already notify this session natively when they \
             complete or need attention; in an interactive chat, return control instead of \
             calling this merely to wait. Use it when you have no independent work left and must \
             not end your turn — for example inside a skill that has to run to completion, or any \
             non-interactive run (`cyrup -p ...`) where the whole task is a single turn and ending \
             it would abandon the still-running children.",
        ),
        // pi `wait-tool.ts:19` @ad11b7ab, verbatim, with the registered name spliced in.
        WaitAudience::Child => {
            text.push_str(
                "This child runtime does not install the root session's native completion \
                 notifier. Use blocking ",
            );
            text.push_str(WAIT_TOOL_NAME);
            text.push_str(
                " to collect your owned descendants during this turn, then read the returned \
                 result references before synthesizing findings. Automatic draining at agent_end \
                 keeps owned work alive but does not synthesize its results.",
            );
        }
    }
    text.push_str(
        "\n\n\
         • { } — return as soon as the FIRST active run finishes (default). Ideal for a rolling \
         fleet: launch N, wait, spawn a replacement for the one that finished, wait again — \
         keeping N in flight.\n\
         • { all: true } — block until EVERY active run in this session is finished.\n\
         • { id: \"...\" } — wait for one specific run (id or prefix) to finish.\n\
         • { id: \"...\", nonBlocking: true } — resolve the prefix once, persist an exact-run wake \
         subscription, and return immediately. The originating interactive session is woken on \
         completion, failure, attention, reconciliation failure, or timeout — including in a later turn, and including \
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
         run records (which also reconciles crashed runners), and resolves early if the turn is \
         aborted.",
    );
    // SUBA-192 — pi `wait-tool.ts:27` @ad11b7ab (`ecc9bc41`), verbatim. Upstream shortened it after
    // the pin (`b8f58f18`); parity is measured at the pin.
    text.push_str(
        "\nA message the user sends while a blocking wait is open ends the wait with a non-error \
         user_input result; active work keeps running.",
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

/// Which session a [`WaitTool`] serves — the one thing its description varies on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaitAudience {
    /// The root orchestrator: completions reach it natively.
    Orchestrator,
    /// A subagent child (SUBA-232): nothing notifies it, so a blocking wait is how it collects.
    Child,
}

/// pi `subagent-wait.ts:665-667` @ad11b7ab, with the registered name spliced in: a wait with no
/// session identity has nothing to scope its listing to, and an unscoped listing would name every
/// session's runs in this checkout — a child's wait, its parent's.
fn no_session_refusal() -> String {
    format!("{WAIT_TOOL_NAME} requires an active session identity to scope background work safely.")
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

/// The `bg_wait` tool (SUBA-004): the way a session blocks on its background subagent runs without
/// ending its turn. See [`crate::background::wait`] for the loop itself, including the three escape
/// hatches (timeout, cancellation, and a message from the operator) that keep a wedged child from
/// hanging the caller.
///
/// **Two registration sites, as upstream's two `registerWaitTool` calls** (`extension/index.ts:721`
/// and `subagent-prompt-runtime.ts:520` @ad11b7ab):
///
/// - the orchestrator's [`crate::extension::host::registration::RegistrationMode::Full`] arm, over
///   its executor ([`WaitTool::new`]);
/// - EVERY subagent child, fan-out or plain, by the child prompt runtime
///   ([`WaitTool::new_child`], SUBA-232) — scoped to the child's own session, so it collects the
///   runs the child launched and nothing of its parent's.
///
/// **Deliberately NOT the `ChildSafe` arm.** A fan-out child loads that arm AND the prompt runtime
/// into one session, and the registry is first-wins across extensions — so a second registration
/// there would win (it loads first), with the orchestrator's description and a user-input set
/// nothing feeds. Upstream registers none there either (`fanout-child.ts` makes no
/// `registerWaitTool` call).
pub struct WaitTool {
    source: WaitSource,
    cwd: PathBuf,
    parameters: serde_json::Value,
    description: String,
}

/// Where a [`WaitTool`] call reads its session, roots, user-input set and enabled verdict.
enum WaitSource {
    /// The orchestrator's tool, over its executor.
    Orchestrator(Arc<SubagentExecutor>),
    /// A subagent child's tool (SUBA-232), over the prompt runtime's own state.
    Child(ChildWaitSource),
}

/// SUBA-232 — what a CHILD's `bg_wait` reads at call time, none of it through an executor: the
/// child prompt runtime has none (a plain child loads no subagents extension, and a fan-out child's
/// is a separate extension the runtime holds no handle to).
///
/// - The session id comes off the late-bound [`cyrup_ext::host::HostServices`] slot the runtime
///   already shares with its watchdog and its `agent_end` drain, read per call as upstream reads
///   `deps.state.currentSessionId` at wait time.
/// - The roots are the runtime's one value, which its `agent_end` drain lists under too (production
///   reads them with [`crate::paths::Roots::from_env`], as the executor's are), and the cwd is the
///   SESSION's — the `ctx.cwd` the runtime last saw, as the drain lists under — so the pair is
///   exactly the drain's: an async run the child launches lands in that per-cwd root stamped with
///   the child's session id, which is what this wait's session-scoped listing finds. Upstream
///   lists a second scope too, the inherited nested route's (`waitRunScopes`,
///   `subagent-wait.ts:281-286`, fed `nestedRootRunId` by `spr.ts:519-520` @ad11b7ab), because
///   its launches mint one. cyrup's do not — no production path mints a nested route (see the
///   note on the role env in `exec/spawn_plan.rs`) — so a child's launches only ever land in the
///   per-cwd root, and that second scope would always be empty.
/// - The user-input set is the runtime's, fed from its `input` subscription — upstream's
///   `wait-tool.ts:31-35` listener, which a child's `bg_wait` registers as the root's does.
///
/// What a child's wait does NOT carry: no completion bus (no results watcher runs in a child, so it
/// polls — as its drain does) and no wake-subscription manager (`nonBlocking` takes upstream's own
/// refusal, as `spr.ts:520` passes none).
///
/// `[CYRUP-DELTA]` nor a detached-foreground hook, and that one is a gap. `/subagents-detach` and
/// its keybinding are orchestrator-only, but the intercom-coordination detach is not: a fan-out
/// child's `ChildSafe` executor remembers a foreground run whose grandchild blocked in
/// `contact_supervisor` as `"detached"` (`exec/drive_attempt.rs`, then
/// `extension/executor/foreground_history/record.rs`), and the tool result tells the child to
/// `bg_wait({ id })` it. Upstream's child wait finds that run, because the fan-out executor and the
/// prompt runtime share one `runtimeState` (`child-hooks.ts:151-154`, `fanout-child.ts:165`,
/// `subagent-prompt-runtime.ts:504` @ad11b7ab). This runtime holds no handle to the `ChildSafe`
/// executor, so the child's wait answers "No active run matched" instead. Filed as `SUBA-247`.
#[derive(Clone)]
pub struct ChildWaitSource {
    services: Arc<std::sync::Mutex<Option<Arc<dyn cyrup_ext::host::HostServices>>>>,
    roots: crate::paths::Roots,
    user_input_waits: crate::background::wait::UserInputWaits,
    /// The session's cwd as the runtime last saw it on an event (`HostCtx::cwd`) — `Tool::execute`
    /// carries no context, so the runtime records it. `None` until the first event, when the
    /// tool's construction-time cwd stands in.
    session_cwd: Arc<std::sync::Mutex<Option<PathBuf>>>,
}

impl ChildWaitSource {
    /// `services` is the runtime's late-bound host-services slot; `user_input_waits` the set its
    /// `input` subscription feeds.
    #[must_use]
    pub fn new(
        services: Arc<std::sync::Mutex<Option<Arc<dyn cyrup_ext::host::HostServices>>>>,
        roots: crate::paths::Roots,
        user_input_waits: crate::background::wait::UserInputWaits,
    ) -> Self {
        Self {
            services,
            roots,
            user_input_waits,
            session_cwd: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Record the session's cwd off an event's `HostCtx` — the runtime calls this for every event
    /// it handles, so the wait lists the same partition the `agent_end` drain does.
    pub fn observe_session_cwd(&self, cwd: &std::path::Path) {
        let mut slot = self
            .session_cwd
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.as_deref() != Some(cwd) {
            *slot = Some(cwd.to_path_buf());
        }
    }

    fn session_cwd(&self) -> Option<PathBuf> {
        self.session_cwd
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn session_id(&self) -> Option<String> {
        self.services
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|services| services.session_id())
    }
}

impl WaitTool {
    /// `enabled` is the already-resolved [`crate::background::wait::resolve_wait_tool_enabled`]
    /// verdict, captured at registration time exactly as pi captures `waitToolConfig` at extension
    /// load — so the advertised description and the runtime behavior can never disagree.
    #[must_use]
    pub fn new(executor: Arc<SubagentExecutor>, cwd: PathBuf, enabled: bool) -> Self {
        Self {
            source: WaitSource::Orchestrator(executor),
            cwd,
            parameters: wait_tool_parameters(),
            description: wait_tool_description(enabled, WaitAudience::Orchestrator),
        }
    }

    /// SUBA-232 — a subagent child's `bg_wait` (pi `subagent-prompt-runtime.ts:520` @ad11b7ab),
    /// with the child description (`wait-tool.ts:19`). `cwd` must be the child session's own, the
    /// one its async launches are partitioned under. `enabled` is resolved as
    /// [`Self::resolve_child_enabled`] does, at registration, for the advertised description.
    #[must_use]
    pub fn new_child(source: ChildWaitSource, cwd: PathBuf, enabled: bool) -> Self {
        Self {
            source: WaitSource::Child(source),
            cwd,
            parameters: wait_tool_parameters(),
            description: wait_tool_description(enabled, WaitAudience::Child),
        }
    }

    /// SUBA-232 — a child's enabled verdict: [`crate::background::wait::WAIT_TOOL_ENABLED_ENV`]
    /// over pi's enabled-by-default, the env inherited from the parent.
    ///
    /// `[CYRUP-DELTA]` upstream ships the parent's RESOLVED `waitTool` config to each child in its
    /// launch config (`child-launch.ts:285-287` @ad11b7ab: `enabled`, and `defaultTimeoutMs` when
    /// set), so a `"waitTool": false` in the parent's `config.json` disables the child's wait too.
    /// cyrup's spawn plan carries no such field, so only the env var reaches the child; a
    /// config-only disable stops at the parent. Filed as `SUBA-242` (cyrup has no
    /// `defaultTimeoutMs` to ship at all: `SUBA-245`).
    pub(crate) fn resolve_child_enabled(env: Option<&str>) -> bool {
        crate::background::wait::resolve_wait_tool_enabled(None, env).unwrap_or(true)
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
    ///
    /// SUBA-192 — a message the operator sends while this call blocks releases it too, as a
    /// non-error `user_input` yield: the call opens its own entry in its source's
    /// [`crate::background::wait::UserInputWaits`] — the executor's for the orchestrator, the child
    /// prompt runtime's for a child — and that owner's `input` handler ends it.
    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        // pi `wait-tool.ts:37-38` — the call's own user-input signal, opened FIRST, before any
        // `.await`: a steer that lands while this call is still resolving its config must already
        // find it listening. Held to the end of the call and deregistered on drop (`:51-53`'s
        // `finally`), on every return path including the parse error just below.
        let user_input = match &self.source {
            WaitSource::Orchestrator(executor) => executor.user_input_waits().open(),
            WaitSource::Child(child) => child.user_input_waits.open(),
        };
        let parsed: crate::background::wait::WaitParams = serde_json::from_value(params)
            .map_err(|e| ToolError::new(format!("invalid {WAIT_TOOL_NAME} tool call: {e}")))?;
        let executor = match &self.source {
            WaitSource::Orchestrator(executor) => executor,
            WaitSource::Child(child) => {
                // Per call, as the orchestrator's below: a mid-session env change takes effect.
                let enabled = Self::resolve_child_enabled(
                    std::env::var(crate::background::wait::WAIT_TOOL_ENABLED_ENV)
                        .ok()
                        .as_deref(),
                );
                let session_id = child.session_id();
                // pi `:665-667` — after the disabled check (which `wait_for_subagents` answers),
                // before any listing.
                if enabled && session_id.is_none() {
                    return Err(ToolError::new(no_session_refusal()));
                }
                let cwd = child.session_cwd().unwrap_or_else(|| self.cwd.clone());
                let deps = crate::background::wait::WaitDeps::for_cwd(
                    &cwd,
                    enabled,
                    session_id,
                    &child.roots,
                )
                // SUBA-192, the child's half — pi `userInputSignal: userInput.signal`
                // (`wait-tool.ts:46`), fed from the runtime's `input` subscription.
                .with_user_input(Some(user_input.token()));
                let outcome =
                    crate::background::wait::wait_for_subagents(&parsed, &cancel, &deps).await;
                return outcome_to_result(outcome);
            }
        };
        // Re-resolved per call (not cached from registration) so a mid-session config/env change
        // takes effect; the registration-time verdict only fixes the advertised description.
        let enabled = Self::resolve_enabled(executor).await;
        // SUBA-031: re-read per call for the same reason `enabled` is — pi reads
        // `deps.state.currentSessionId` at wait time, so a session switch between registration and
        // the call scopes the wait to the session that actually issued it.
        let session_id = executor.current_session_id();
        // pi `:665-667` — the same refusal a child's wait applies. In production the executor's
        // session is attached before any tool can run, so this is the guard, not a path.
        if enabled && session_id.is_none() {
            return Err(ToolError::new(no_session_refusal()));
        }
        let deps = crate::background::wait::WaitDeps::for_cwd(
            &self.cwd,
            enabled,
            session_id,
            &executor.config_snapshot().await.roots,
        )
        // SUBA-034: subscribe this wait to the orchestrator's completion bus, so a result observed
        // by THIS process's watcher releases the wait immediately rather than one poll interval
        // later. The poll under it is unchanged and remains the source of truth.
        .with_completion_bus(Some(executor.completion_bus()))
        // ASYNC_NOTIFY_BUG_REPORT F3.4 — share the executor's inline-answer ledger, so the runs
        // this wait answers inline are not re-announced as standalone notifications.
        .with_inline_answers(Some(executor.inline_answers()))
        // The executor-owned consumed-payload record, so a completion the watcher already
        // delivered and deleted still resolves for a wait that lands a moment later.
        .with_wait_completions(executor.wait_completions())
        // SCOPE_11 — pi `wait-tool.ts:33`'s
        // `...(subscriptions && ctx?.hasUI ? { subscribe: (input) => subscriptions.arm(input) } : {})`.
        //
        // Upstream's `ctx?.hasUI` is a per-CALL tool context; cyrup's `Tool::execute` has no such
        // parameter, and its `has_ui` lives on `HostCtx` at the `SessionStart` edge instead. The
        // gate is therefore applied where the fact is actually known — the manager is INSTALLED
        // only for a session with a UI (`extension/host/native_impl.rs`'s `SessionStart` arm) — so
        // a headless runtime simply has no manager here and `wait_for_subagents` takes pi's own
        // `!deps.subscribe` refusal. Same observable, one fewer place for the two to disagree.
        .with_subscribe(executor.wait_subscriptions().map(|manager| {
            let arming: Arc<dyn crate::background::wait::WaitSubscriptionArming> = manager;
            crate::background::wait::WaitSubscribeHook::new(arming)
        }))
        // VL-S11b — pi `deps.state.foregroundRuns` (`subagent-wait.ts:226`): the executor's
        // remembered-run map, which is where a DETACHED foreground run lives and the only place it
        // ever lives (upstream never writes one to disk — `foreground-history.ts:67-69` — and
        // cyrup's `persist.rs` refuses one). Without this the candidate set is async runs only and
        // `bg_wait({ id })` cannot see a detached run at all.
        .with_detached_foreground(Some(executor.detached_foreground_hook()))
        // SUBA-192 — pi `userInputSignal: userInput.signal` (`wait-tool.ts:46`).
        .with_user_input(Some(user_input.token()));
        let outcome = crate::background::wait::wait_for_subagents(&parsed, &cancel, &deps).await;
        outcome_to_result(outcome)
    }
}

/// The one mapping from a wait's outcome to the tool's result, shared by both sources.
fn outcome_to_result(
    outcome: crate::background::wait::WaitOutcome,
) -> Result<ToolResult, ToolError> {
    // cyrup's `Tool::execute` returns `Result<ToolResult, ToolError>` and the host maps `Err`
    // onto the result's error flag (`cyrup-core/src/tool.rs`: "Tools signal failure by
    // returning `Err(ToolError)`"), which is the same observable as pi's `isError`.
    //
    // It carries no loss. Upstream's error `result(…, true)` sites all pass TWO arguments —
    // the third, `completions`, is supplied only at the two terminal returns
    // (`subagent-wait.ts:752`, `:769`) — so the sole errored-result-with-completions case is
    // the `failOnFailedRuns`/`failOnAttention` flip (`:751`/`:768`). The `bg_wait` TOOL leaves
    // both flags `false` (`WaitDeps::for_cwd`, `background/wait.rs`, overridden nowhere in
    // either source), so for THIS caller `is_error()` implies `completions.is_empty()`. Auto-drain —
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

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::{WAIT_TOOL_LABEL, WAIT_TOOL_NAME, WaitAudience, wait_tool_description};
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
        let enabled = wait_tool_description(true, WaitAudience::Orchestrator);
        let disabled = wait_tool_description(false, WaitAudience::Orchestrator);
        let child = wait_tool_description(true, WaitAudience::Child);
        let child_disabled = wait_tool_description(false, WaitAudience::Child);
        let described = [
            ("wait_tool_description(true)", enabled.as_str()),
            ("wait_tool_description(false)", disabled.as_str()),
            ("the child description", child.as_str()),
            ("the disabled child description", child_disabled.as_str()),
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

    /// SUBA-192 — the description tells the model what a `user_input` result means, in pi's own
    /// sentence (`wait-tool.ts:27` @ad11b7ab, added by `ecc9bc41`), whether or not the tool is
    /// configured off: a model that sees the yield must know the work is still running and that
    /// the call did not fail.
    #[test]
    fn the_description_says_a_user_message_ends_the_wait_without_an_error() {
        let sentence = "A message the user sends while a blocking wait is open ends the wait with a \
                        non-error user_input result; active work keeps running.";
        for audience in [WaitAudience::Orchestrator, WaitAudience::Child] {
            for enabled in [true, false] {
                let description = wait_tool_description(enabled, audience);
                assert!(
                    description.contains(sentence),
                    "wait_tool_description({enabled}, {audience:?}) lacks upstream's user_input \
                     sentence: {description}"
                );
            }
        }
    }

    /// SUBA-232 — a child is told to collect its owned descendants, in upstream's own sentence
    /// (`wait-tool.ts:19` @ad11b7ab), and is never told it is notified natively; the orchestrator
    /// is told the opposite, and never the child's sentence. Everything after that paragraph is
    /// one shared text.
    ///
    /// **Gutting mutation this fails on:** hand the child the orchestrator's paragraph (the
    /// audience ignored) — the child sentence is then absent.
    #[test]
    fn a_child_is_told_to_collect_its_descendants_and_the_orchestrator_is_not() {
        let child_sentence = "This child runtime does not install the root session's native \
                              completion notifier. Use blocking bg_wait to collect your owned \
                              descendants during this turn, then read the returned result \
                              references before synthesizing findings. Automatic draining at \
                              agent_end keeps owned work alive but does not synthesize its \
                              results.";
        let notified = "Ordinary async subagent runs already notify this session natively";
        let child = wait_tool_description(true, WaitAudience::Child);
        let orchestrator = wait_tool_description(true, WaitAudience::Orchestrator);
        assert!(child.contains(child_sentence), "{child}");
        assert!(!child.contains(notified), "{child}");
        // Nothing in the shared text may promise a child the notification it does not have.
        assert!(!child.contains("notification delivery"), "{child}");
        assert!(orchestrator.contains(notified), "{orchestrator}");
        assert!(!orchestrator.contains(child_sentence), "{orchestrator}");
        let tail = |text: &str| {
            text.split_once("\n\n")
                .and_then(|(_, rest)| rest.split_once("\n\n"))
                .map(|(_, shared)| shared.to_string())
        };
        assert_eq!(
            tail(&child),
            tail(&orchestrator),
            "the rest of the text is shared"
        );
    }

    /// pi `subagent-wait.ts:665-667` @ad11b7ab — an enabled wait with no session identity is
    /// refused with upstream's sentence before it lists anything, from either source: a child
    /// whose host has no session yet, and an orchestrator whose executor has none attached.
    /// Unrefused, the wait would list the cwd's whole partition, and a child's would block on its
    /// parent's runs.
    ///
    /// **Gutting mutation this fails on:** drop either source's refusal — that call then lists and
    /// returns `Ok`.
    #[tokio::test]
    async fn a_wait_without_a_session_identity_is_refused_with_upstreams_text() {
        use cyrup_core::{CancelToken, Tool, ToolCallId};
        let root = tempfile::tempdir().expect("tempdir");
        let child = super::WaitTool::new_child(
            super::ChildWaitSource::new(
                std::sync::Arc::new(std::sync::Mutex::new(None)),
                crate::paths::Roots::sandboxed(root.path()),
                crate::background::wait::UserInputWaits::default(),
            ),
            root.path().to_path_buf(),
            true,
        );
        let orchestrator = super::WaitTool::new(
            std::sync::Arc::new(crate::extension::executor::SubagentExecutor::new()),
            root.path().to_path_buf(),
            true,
        );
        for (who, tool) in [("child", &child), ("orchestrator", &orchestrator)] {
            let err = tool
                .execute(
                    ToolCallId::from("call-1"),
                    serde_json::json!({}),
                    CancelToken::new(),
                    Box::new(|_| {}),
                )
                .await
                .expect_err(who);
            assert_eq!(
                err.message,
                "bg_wait requires an active session identity to scope background work safely.",
                "{who}"
            );
        }
    }
}
