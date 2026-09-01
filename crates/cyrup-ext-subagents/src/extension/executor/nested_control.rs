//! The fanout child's nested-control inbox listener (pi `fanout-child.ts`).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::discovery::discover_agents;
use crate::extension::executor::SubagentExecutor;

impl SubagentExecutor {

    // ---------------------------------------------------------------------------------------
    // Nested-control inbox listener (T6, pi `fanout-child.ts:53-128`): serviced ONLY by a
    // `RegistrationMode::ChildSafe` process that inherited a nested route from its own parent's
    // env — a grandparent orchestrator's interrupt/resume request targeting a run nested two (or
    // more) levels deep is routed here rather than lost.
    // ---------------------------------------------------------------------------------------

    /// Start the listener (pi `startNestedControlInboxListener`, `fanout-child.ts:53-63,125-128` @v0.34.0):
    /// resolve the inherited nested route from the process env — a resolution error is swallowed
    /// (no listener), as is the "no inherited route" case (`Ok(None)`) — and, only when a real route
    /// was found, spawn the 200ms poll loop as a detached background task. Called once from
    /// [`crate::extension::RegistrationMode::ChildSafe`] `init()`.
    pub(crate) fn start_nested_control_inbox_listener(self: &Arc<Self>) {
        let route = match crate::spawn::nested_events::resolve_nested_route_from_env(|key| {
            std::env::var(key).ok()
        }) {
            Ok(Some(route)) => route,
            Ok(None) | Err(_) => return,
        };
        let executor = Arc::clone(self);
        tokio::spawn(async move { executor.run_nested_control_inbox_listener(route).await });
    }

    /// The 200ms poll loop body (pi's `setInterval(..., 200)`, `fanout-child.ts:64-125`; `.unref()`
    /// has no analog — this crate has no process-exit-blocking-task concern here since the listener
    /// only ever runs inside a fanout child's own detached OS process). Runs for the lifetime of the
    /// process; a poll-tick error is logged and the loop continues, exactly as pi's per-tick
    /// `try`/`catch` around the whole poll body.
    async fn run_nested_control_inbox_listener(
        self: Arc<Self>,
        route: crate::spawn::nested_events::NestedRoute,
    ) {
        let mut seen: HashSet<String> = HashSet::new();
        let mut pending_results: HashMap<String, crate::spawn::nested_events::NestedControlResultInput> =
            HashMap::new();
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(200));
        loop {
            ticker.tick().await;
            self.poll_nested_control_inbox_once(&route, &mut seen, &mut pending_results)
                .await;
        }
    }

    /// One poll tick: read every pending request, skip already-`seen` ones (pi's `seen`/`inFlight`
    /// dedup — this loop processes one tick to completion before the next ever ticks, so no request
    /// can be revisited mid-flight the way pi's concurrently-spawned per-request IIFEs could), resolve
    /// each new one, and write back its result — pi `fanout-child.ts:66-121`.
    async fn poll_nested_control_inbox_once(
        &self,
        route: &crate::spawn::nested_events::NestedRoute,
        seen: &mut HashSet<String>,
        pending_results: &mut HashMap<String, crate::spawn::nested_events::NestedControlResultInput>,
    ) {
        let requests = match crate::spawn::nested_events::read_nested_control_requests(route) {
            Ok(requests) => requests,
            Err(err) => {
                // pi `console.error("Failed to poll nested control inbox '...' for root '...':", error)`
                // (`fanout-child.ts:122-124`): logged, never fatal — the next tick tries again.
                eprintln!(
                    "Failed to poll nested control inbox '{}' for root '{}': {err}",
                    route.control_inbox.display(),
                    route.root_run_id
                );
                return;
            }
        };
        for (request, file_path) in requests {
            if seen.contains(&request.request_id) {
                continue;
            }
            let result = match pending_results.remove(&request.request_id) {
                // A prior tick already resolved this request but failed to WRITE the result — pi
                // retries the write with the SAME cached result rather than re-resolving it
                // (`fanout-child.ts:71-72`).
                Some(cached) => cached,
                None => {
                    let (ok, message) = self.resolve_nested_control_request(&request).await;
                    crate::spawn::nested_events::NestedControlResultInput {
                        ts: crate::time::now_epoch_millis(),
                        request_id: request.request_id.clone(),
                        target_run_id: request.target_run_id.clone(),
                        ok,
                        message,
                    }
                }
            };
            match crate::spawn::nested_events::write_nested_control_result(route, &result) {
                Ok(()) => {
                    // pi: mark `seen`, drop the pending cache, unlink the request file (unlink errors
                    // swallowed) — `fanout-child.ts:114-116`.
                    seen.insert(request.request_id.clone());
                    let _ = std::fs::remove_file(&file_path);
                }
                Err(err) => {
                    // pi: cache the resolved result for retry and KEEP the request file —
                    // `fanout-child.ts:109-113`.
                    eprintln!(
                        "Failed to write nested control result for request '{}' targeting '{}' via \
                         inbox '{}'; keeping request for retry: {err}",
                        request.request_id,
                        request.target_run_id,
                        route.control_inbox.display()
                    );
                    pending_results.insert(request.request_id.clone(), result);
                }
            }
        }
    }

    /// G98 / pi `applySingleAgentLaunchDefaults` (`subagent-executor.ts:1930-1947` @v0.43.0):
    /// resolve the named agent's OWN launch defaults so a SINGLE-agent launch can inherit them.
    ///
    /// **Why this lives on the executor rather than in `route_single`.** Upstream calls
    /// `applySingleAgentLaunchDefaults` exactly ONCE, at the shared `execute` entry
    /// (`subagent-executor.ts:3608` @v0.35.0), *before* any mode routing — so every surface that
    /// reaches `execute` inherits the defaults: the `subagent` tool, the child-safe fanout tool,
    /// the delegated route, and every slash command, because upstream's `/run` handler is
    /// `runSlashSubagent` → the same `executor.execute`. cyrup's `/run` is an INDEPENDENT entry
    /// point (`SubagentsExtension::dispatch_slash`'s `SlashCommandName::Run` arm) that never
    /// passes through `SubagentTool::execute`, so applying the defaults inside `route_single`
    /// covered the tool surface only: `/run reviewer "…"` silently ignored the agent's own
    /// `async:` and `timeoutMs:` frontmatter, while `subagent({agent:"reviewer",…})` honoured it.
    /// Same agent, same request, two behaviours. The shared owner both surfaces DO have is this
    /// executor, so the resolution lives here and each entry applies it once.
    ///
    /// Returns `(default_async, default_timeout_ms, default_turn_budget)` — pi's
    /// `agent.defaultAsync` / `agent.defaultTimeoutMs` / `agent.defaultTurnBudget`, all `None` for
    /// an unknown agent name (pi `:1588`'s `if (!agent) return params`, which leaves the existing
    /// "unknown agent" error path to report it) and all `None` on a discovery failure.
    ///
    /// The APPLICATION rules stay at the call sites, because they are fill-unset-only and each
    /// site knows its own "was this supplied?" question (pi `:1591-1594`): `async` applies only
    /// when the call omitted `async` entirely, `timeout_ms` only when it omitted BOTH `timeoutMs`
    /// and its alias `maxRuntimeMs`, and `turn_budget` only when it omitted `turnBudget`
    /// (SUBA-008, pi `:1940-1942`).
    #[must_use]
    pub(crate) fn single_agent_launch_defaults(
        cwd: &Path,
        agent: &str,
        roots: &crate::paths::Roots,
    ) -> (
        Option<bool>,
        Option<u64>,
        Option<crate::exec::turn_budget::ResolvedTurnBudget>,
    ) {
        SubagentExecutor::discovery_config(cwd, roots)
            .and_then(|cfg| discover_agents(&cfg, None))
            .ok()
            .and_then(|result| {
                result
                    .agents
                    .into_iter()
                    .find(|candidate| candidate.name == agent)
            })
            .map_or((None, None, None), |found| {
                (
                    found.default_async,
                    found.default_timeout_ms,
                    found.default_turn_budget,
                )
            })
    }

    /// Is `selector` a run this executor is currently driving in the FOREGROUND?
    ///
    /// pi's `resolveSubagentRunId` returns `{ kind: "foreground" }` for such a selector
    /// (`subagent-executor.ts:3211,3217` @v0.34.0) and `steer` refuses it with its own message
    /// rather than reporting the run as missing. cyrup's `foreground_controls` map — keyed by run
    /// id, populated for the lifetime of every foreground run — is the same registry (it is
    /// already what `foreground_fleet_entries` and `resolve_nested_control_request` read), so the
    /// classification is a lookup, not a new source of truth.
    ///
    /// Prefix selectors are honoured for the same reason `resolve_run_id` honours them: a user who
    /// can address a run by prefix everywhere else must get the same classification here, not a
    /// spurious "no async run found". An AMBIGUOUS prefix is deliberately NOT treated as a
    /// foreground match — that must fall through to the async resolver, whose
    /// [`crate::error::SubagentError::AmbiguousRunId`] is the accurate diagnosis.
    pub(crate) fn is_live_foreground_run(&self, selector: &str) -> bool {
        let controls = self
            .foreground_controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if controls.contains_key(selector) {
            return true;
        }
        let mut matches = controls.keys().filter(|id| id.starts_with(selector));
        matches.next().is_some() && matches.next().is_none()
    }

    /// G77 — `resolveSubagentRunId(...).kind === "nested"` for the one caller that has to refuse it
    /// ([`Self::control_stop`], pi `subagent-executor.ts:4791,4796` @v0.43.0, whose nested scope is
    /// `nestedResolutionScopeForExecutor(deps)`).
    ///
    /// This is `findNestedRunMatchesById` (`runs/shared/nested-events.ts:661-675` @v0.43.0) reduced
    /// to the boolean its one caller needs, and it walks the same three steps upstream does:
    /// `listNestedRoutes()` → `projectNestedEvents(route)` → search the projected registry, with
    /// upstream's own `catch { continue; }` on each route. (Upstream flattens via
    /// `collectScopedNestedRuns`; this uses [`crate::spawn::nested_events::find_nested_run`], pi's
    /// `findNestedRun`, whose depth-first walk over children, nested children and step children
    /// reaches at least the same set.)
    ///
    /// A selector found in ANY route is a nested run and therefore not a top-level async run of this
    /// session — exactly the distinction the refusal draws. Fail-open: an unreadable route
    /// contributes nothing rather than turning a stop into an error, so a corrupt projection can
    /// only ever cost the caller the more specific sentence, never a spurious refusal of a genuinely
    /// stoppable run.
    pub(crate) async fn resolves_to_nested_run(&self, selector: &str) -> bool {
        // One resolved value, not a deferred one: `Roots::nested_events` is the SAME tree the
        // containment guards validate every route against, so a scoped run and its guard can no
        // longer disagree about where "nested events" is.
        let root = self.config_snapshot().await.roots.nested_events();
        let Ok(routes) = crate::spawn::nested_events::list_nested_routes_in(&root) else {
            return false;
        };
        routes.iter().any(|route| {
            crate::spawn::nested_events::project_nested_events_in(&root, route)
                .ok()
                .is_some_and(|registry| {
                    crate::spawn::nested_events::find_nested_run(&registry.children, selector)
                        .is_some()
                })
        })
    }

    /// Resolve one nested control request against this executor's live `foreground_controls`
    /// registry — pi's per-request body inside `startNestedControlInboxListener`
    /// (`fanout-child.ts:73-104`). Guard order (exact): target-not-active -> `interrupt` action ->
    /// blank message -> no current agent -> intercom delivery. Returns `(ok, message)`, the exact
    /// pair `writeNestedControlResult`'s `{ok, message}` carries.
    pub(crate) async fn resolve_nested_control_request(
        &self,
        request: &crate::spawn::nested_events::NestedControlRequestRecord,
    ) -> (bool, String) {
        let target = request.target_run_id.as_str();
        let control = {
            let controls = self
                .foreground_controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            controls.get(target).cloned()
        };
        let Some(control) = control else {
            return (
                false,
                format!("Nested run {target} is not active in this fanout child."),
            );
        };
        if request.action == "interrupt" {
            // pi `ok = control.interrupt?.() === true`: a token not yet cancelled has an active step
            // to interrupt (fire it, report success); an already-cancelled token has none left.
            let ok = !control.interrupt.is_cancelled();
            control.interrupt.cancel();
            let message = if ok {
                format!("Interrupt requested for nested run {target}.")
            } else {
                format!("Nested run {target} has no active child step to interrupt.")
            };
            return (ok, message);
        }
        let trimmed = request.message.as_deref().map(str::trim).unwrap_or("");
        if trimmed.is_empty() {
            return (false, "Nested resume requires message.".to_string());
        }
        let Some(agent) = control.current_agent.clone() else {
            return (
                false,
                format!("Nested run {target} has no active child message route."),
            );
        };
        let index = control.current_index.unwrap_or(0);
        let intercom_target =
            crate::spawn::intercom_target::resolve_subagent_intercom_target(target, &agent, index);
        let ok = crate::tui::intercom::steer_with_default_timeout(
            self.steer.as_ref(),
            intercom_target.clone(),
            format!("Follow-up for nested run {target} ({agent}):\n\n{trimmed}"),
        )
        .await;
        let message = if ok {
            format!("Delivered follow-up to live nested run {target}.")
        } else {
            format!("Nested child intercom target is not registered: {intercom_target}")
        };
        (ok, message)
    }
}
