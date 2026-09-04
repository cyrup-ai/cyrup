//! Per-session executor state: the root parent-session anchor, the intercom presence target,
//! and pi's `lastParentModel` memory.

use cyrup_core::ModelId;

use crate::extension::executor::SubagentExecutor;

/// pi's `state.lastParentModel` alongside the `state.currentSessionId` it is scoped to — the two
/// fields `rememberParentModel` reads and writes (`subagent-executor.ts:284-291` @v0.43.0).
///
/// `session_id` is the session `last` was observed under, so a change of session drops the memory
/// rather than leaking one session's model into the next (pi's `if (state.currentSessionId !==
/// sessionId) delete state.lastParentModel`).
#[derive(Debug, Default)]
pub(crate) struct ParentModelMemory {
    session_id: Option<String>,
    last: Option<ModelId>,
}
impl SubagentExecutor {

    /// The captured parent-session anchor (`CYRUP_SUBAGENT_PARENT_SESSION`, R-SA-P1), if the root
    /// `SessionStart` handler has resolved it from [`cyrup_ext::host::HostServices::session_id`].
    #[must_use]
    pub fn root_parent_session(&self) -> Option<String> {
        self.root_parent_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    /// SUBA-031 — the LIVE session identity, cyrup's analog of pi's `state.currentSessionId` /
    /// `ctx.currentSessionId` (`async-execution.ts:1042` @v0.43.0).
    ///
    /// Read straight off the bound P-1 backend on every call rather than off
    /// [`Self::root_parent_session`], for two reasons: `root_parent_session` is captured once at a
    /// PARENT-role `SessionStart` (so a fanout child, which legitimately owns its own async root,
    /// has none), and a session SWITCH inside one process moves the live id while the captured
    /// anchor deliberately keeps addressing the root. Everything that scopes a listing wants the
    /// former; only forwarding wants the latter.
    ///
    /// `None`/empty (headless, unpersisted, or no host services bound) means "no session identity",
    /// which every consumer treats as pi treats a falsy `sessionId`: no filter, no stamp.
    #[must_use]
    pub fn current_session_id(&self) -> Option<String> {
        self.host_services()
            .and_then(|services| services.session_id())
            .filter(|id| !id.is_empty())
    }

    /// Capture the canonical parent-session anchor from the live session id (P-2), at the root
    /// orchestrator's `SessionStart` (depth 0). Reads [`cyrup_ext::host::HostServices::session_id`]
    /// off the bound P-1 backend; a `None`/empty id (headless / unpersisted / no live session) leaves
    /// the slot unset, so the spawn-site resolution falls through to the inherited env value.
    pub fn capture_parent_session_anchor(&self) {
        if let Some(services) = self.host_services()
            && let Some(id) = services.session_id()
            && !id.is_empty()
        {
            *self
                .root_parent_session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(id);
            // Capture the session NAME too (may be absent): it feeds this orchestrator's own intercom
            // presence target (`orchestrator_presence_target(name, id)`), the address a spawned
            // child's `contact_supervisor` relays to. An absent/empty name falls through to the
            // `subagent-chat-<id8>` alias inside that resolver, so only a real name is stored here.
            if let Some(name) = services.session_name().filter(|n| !n.trim().is_empty()) {
                *self
                    .root_parent_session_name
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(name);
            }
        }
    }

    /// Clear the captured parent-session anchor (pi `delete process.env[SUBAGENT_PARENT_SESSION_ENV]`,
    /// `extension/index.ts:645`), called from `session_shutdown` so a stale id/name from the
    /// session that just ended never leaks into a subsequently-started session on this same
    /// long-lived process (e.g. an SDK embedder / test harness that starts multiple sessions
    /// against one `SubagentExecutor`). Detached background runs already spawned are wholly
    /// unaffected — this only clears THIS orchestrator's own anchor for future spawns.
    pub fn clear_parent_session_anchor(&self) {
        *self
            .root_parent_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .root_parent_session_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// This root orchestrator's own intercom presence target — the address a spawned child's
    /// `contact_supervisor` relays to (pi `resolveIntercomSessionTarget(pi.getSessionName(),
    /// sessionManager.getSessionId())`, `subagent-executor.ts:893`). Byte-identical to the string the
    /// intercom companion registers this session's broker presence under
    /// (`cyrup-intercom`'s `build_registration` derives it from the SAME `HostServices`), so the
    /// two independently-produced strings match at the broker. `None` when no live session id was
    /// captured (headless / SDK-embedder) — the spawn site then writes no child-bridge env, so the
    /// child registers no supervisor bridge (the clean no-intercom path).
    #[must_use]
    pub fn orchestrator_intercom_target(&self) -> Option<String> {
        let id = self.root_parent_session()?;
        let name_guard = self
            .root_parent_session_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let name = name_guard.as_deref().filter(|s| !s.trim().is_empty());
        Some(crate::spawn::intercom_target::orchestrator_presence_target(name, &id))
    }

    /// The live PARENT session's current model as a `provider/id` [`ModelId`] — pi's `ctx.model`
    /// (`pi-subagents/src/runs/shared/model-fallback.ts:196-220`), the model an inheriting subagent
    /// (a persona with no `model:` of its own, run with no per-call override) resolves to. Read off
    /// the bound P-1 [`cyrup_ext::host::HostServices`] backend
    /// ([`cyrup_ext::host::HostServices::current_model`], returned by `LiveHostServices` as
    /// `"{provider}/{model}"`), the SAME live-session seam `session_id`/`session_file`/
    /// `inject_message` already reach. `None` when no live session backend is bound (headless /
    /// SDK-embedder) or it has no active model yet — the ladder then falls through to the persona's
    /// own `model`/`fallback_models` exactly as before (see [`crate::exec::fallback::resolve_model_inheritance`]).
    #[must_use]
    pub fn inherited_session_model(&self) -> Option<ModelId> {
        self.host_services().and_then(|s| s.current_model()).map(ModelId::from)
    }

    /// pi `normalizeParentModel` (`runs/shared/model-fallback.ts:33-39` @v0.43.0): a live `ctx.model`
    /// only becomes a `ParentModel` when it carries BOTH a non-empty `provider` and a non-empty
    /// `id`. cyrup's [`ModelId`] is the joined `"{provider}/{id}"` form
    /// ([`SubagentExecutor::inherited_session_model`]), so the same predicate is "splits on `/` into
    /// two non-empty halves" — which rejects `""`, `"anthropic"`, `"/sonnet"` and `"anthropic/"`,
    /// each of which upstream would have turned into `undefined` rather than a usable parent model.
    fn normalize_parent_model(model: Option<ModelId>) -> Option<ModelId> {
        model.filter(|id| {
            id.as_str()
                .split_once('/')
                .is_some_and(|(provider, model)| !provider.is_empty() && !model.is_empty())
        })
    }

    /// pi `rememberParentModel(deps.state, requestSessionId, ctx.model)`
    /// (`subagent-executor.ts:284-291`, called at `:4345` @v0.43.0) — the parent-session model an
    /// inheriting subagent resolves to, read through this session's memory rather than straight off
    /// the live host.
    ///
    /// The state machine, in upstream's order:
    ///
    /// 1. a change of session id CLEARS the memory (one session's model must never leak into the
    ///    next), and the new id is recorded;
    /// 2. the live read is normalized ([`Self::normalize_parent_model`]);
    /// 3. with no resolvable session id there is nothing to key a memory on, so the live read is
    ///    returned WITHOUT being remembered (pi's `if (!sessionId) return parentModel`);
    /// 4. a live read that normalized successfully is remembered, overwriting any earlier value —
    ///    so a parent that switches model mid-session immediately re-anchors the memory;
    /// 5. the return is `live ?? remembered`: the live read always wins when it exists, and the
    ///    memory is consulted ONLY when it does not.
    ///
    /// Step 5 is the whole point, and it is why this is not equivalent to calling
    /// [`Self::inherited_session_model`] directly, which is what every execution path here used to
    /// do. `HostServices::current_model` is a live probe of the bound session backend and can
    /// legitimately answer `None` — no active model yet, a backend momentarily unbound, a model
    /// cleared and being re-selected. A dispatch that lands in that window previously inherited
    /// NOTHING and fell through to the persona's own `model`/`fallback_models`, silently running a
    /// different model than the one the session had been on (or failing on an empty ladder for a
    /// persona that declares neither). With the memory, it inherits the model this session last
    /// genuinely reported.
    ///
    /// Upstream's `preserveActiveSession ? normalizeParentModel(ctx.model) : rememberParentModel(…)`
    /// branch (`:4343-4345`) selects the un-remembered read for workflow children and scheduled
    /// owner executors. Neither exists in this crate (both arrive with `workflowScript`, which is
    /// unported), so only the `rememberParentModel` arm is reachable and only it is ported.
    ///
    /// Deliberately NOT used by the `models` REPORT surfaces
    /// ([`Self::run_models_report`] and `route_management_action`'s `current_session_model`):
    /// upstream's `handleModels` reads `ctx.model` directly (`agents/agent-management.ts:811-812`),
    /// never `requestParentModel`, because that surface reports what the session IS on and must say
    /// `(unavailable)` when the answer is genuinely unavailable rather than repeating a stale one.
    #[must_use]
    pub fn remembered_parent_model(&self) -> Option<ModelId> {
        let session_id = self.root_parent_session();
        let live = Self::normalize_parent_model(self.inherited_session_model());
        let mut memory = self
            .parent_model_memory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if memory.session_id != session_id {
            memory.last = None;
        }
        memory.session_id.clone_from(&session_id);
        if session_id.is_none() {
            return live;
        }
        if live.is_some() {
            memory.last.clone_from(&live);
        }
        live.or_else(|| memory.last.clone())
    }

    /// Full session-teardown housekeeping (pi `session_shutdown`, `extension/index.ts:644-680`,
    /// minus the pieces this crate has no analog for — see `on_event`'s `SessionShutdown` arm doc
    /// for the exact mapping): stop the completion watcher, abort+clear the background job
    /// tracker's poll loop and in-memory job map, and clear the captured parent-session anchor.
    /// Detached background runs already spawned are left running to completion untouched
    /// (R-SA-071/DI-SA-8) — this only resets THIS process's own live session-scoped state.
    pub async fn teardown_session(&self) {
        self.stop_completion_watcher().await;
        self.tracker.stop_and_clear().await;
        self.clear_parent_session_anchor();
        // SUBA-084 — pi `clearRuntimeAgentsForPi(pi)` in the runtime cleanup
        // (`extension/index.ts:971` @v0.64.0): a rebuilt session never inherits the previous
        // session's in-process agents; outstanding registration handles become no-ops.
        self.runtime_agents().clear();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use crate::background::RunId;
    use crate::background::RunMode;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// A [`cyrup_ext::host::HostServices`] double whose current model and session id can BOTH be
    /// changed after binding — [`SubagentExecutor::set_host_services`] writes a `OnceLock`, so a
    /// test that needs the parent's model to move mid-session has to move it behind the handle.
    #[derive(Default)]
    struct MutableModelHost {
        model: std::sync::Mutex<Option<String>>,
        session: std::sync::Mutex<Option<String>>,
    }

    impl MutableModelHost {
        fn set_model(&self, model: Option<&str>) {
            *self
                .model
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                model.map(str::to_string);
        }
    }

    impl cyrup_ext::host::HostServices for MutableModelHost {
        fn current_model(&self) -> Option<String> {
            self.model
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
        fn session_id(&self) -> Option<String> {
            self.session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    /// pi `rememberParentModel` (`subagent-executor.ts:284-291` @v0.43.0), the whole state machine.
    ///
    /// The behaviour that makes this NOT a live read of `ctx.model`: within one session the live
    /// read wins whenever it exists AND re-anchors the memory, but when the live read comes back
    /// empty the LAST well-formed model this session reported is returned instead of `None`. Before
    /// this port every execution path called `inherited_session_model()` directly, so a dispatch
    /// landing in that window inherited nothing at all.
    #[test]
    fn the_parent_model_is_remembered_per_session_and_survives_a_gap_in_the_live_read() {
        let executor = SubagentExecutor::new();
        let host = Arc::new(MutableModelHost::default());
        *host
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some("session-one".to_string());
        host.set_model(Some("anthropic/opus"));
        executor.set_host_services(host.clone());
        executor.capture_parent_session_anchor();
        assert_eq!(executor.root_parent_session().as_deref(), Some("session-one"));

        // A live read that resolves is returned AND remembered.
        assert_eq!(
            executor.remembered_parent_model(),
            Some(ModelId::from("anthropic/opus"))
        );

        // The parent switches model mid-session: the live read still wins, and the memory
        // re-anchors onto the new value rather than pinning the first one it ever saw.
        host.set_model(Some("anthropic/sonnet"));
        assert_eq!(
            executor.remembered_parent_model(),
            Some(ModelId::from("anthropic/sonnet"))
        );

        // The live probe goes quiet (no active model yet / backend momentarily unbound). THIS is
        // the case the two accessors disagree on, and the reason the memory exists.
        host.set_model(None);
        assert_eq!(
            executor.inherited_session_model(),
            None,
            "the live read is genuinely unavailable here"
        );
        assert_eq!(
            executor.remembered_parent_model(),
            Some(ModelId::from("anthropic/sonnet")),
            "the remembered parent model must carry the session through a gap in the live read"
        );

        // A live read that cannot normalize is the same as no live read (pi `normalizeParentModel`
        // requires a non-empty provider AND a non-empty id).
        for malformed in ["", "anthropic", "/sonnet", "anthropic/"] {
            host.set_model(Some(malformed));
            assert_eq!(
                executor.remembered_parent_model(),
                Some(ModelId::from("anthropic/sonnet")),
                "'{malformed}' is not a usable parent model and must not overwrite the memory"
            );
        }

        // A change of session CLEARS the memory — one session's model must never leak into the
        // next (pi's `if (state.currentSessionId !== sessionId) delete state.lastParentModel`).
        host.set_model(None);
        *executor
            .root_parent_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some("session-two".to_string());
        assert_eq!(
            executor.remembered_parent_model(),
            None,
            "session-one's model must not be inherited by session-two"
        );
    }

    /// pi's `if (!sessionId) return parentModel` (`subagent-executor.ts:288`): with no resolvable
    /// session id there is nothing to key a memory on, so the live read is passed through WITHOUT
    /// being remembered — a headless/SDK-embedder executor must not start accumulating a model
    /// under a null session and handing it back later.
    #[test]
    fn with_no_session_id_the_parent_model_is_passed_through_but_never_remembered() {
        let executor = SubagentExecutor::new();
        let host = Arc::new(MutableModelHost::default());
        host.set_model(Some("anthropic/opus"));
        executor.set_host_services(host.clone());
        executor.capture_parent_session_anchor();
        assert_eq!(
            executor.root_parent_session(),
            None,
            "the double reports no session id, so no anchor is captured"
        );

        assert_eq!(
            executor.remembered_parent_model(),
            Some(ModelId::from("anthropic/opus")),
            "the live read is still passed through"
        );
        host.set_model(None);
        assert_eq!(
            executor.remembered_parent_model(),
            None,
            // Deliberately narrow: this shows only that with no live model and no session id,
            // nothing is SURFACED. It does not establish that nothing was STORED — the memory is
            // keyed on a session id that is None here, so a stored value would be unreachable
            // either way and this assertion could not tell the two apart.
            "with no live model and a null session id, no model is surfaced"
        );
    }

    /// The background path is not a hole in the policy: the resolved scope is baked into the
    /// serialized `RunnerConfig` the detached hop-2 runner is handed over `--config`, which is the
    /// only channel by which anything reaches that separate OS process.
    #[test]
    fn the_model_scope_reaches_the_detached_runner_through_the_serialized_config() {
        let scope = crate::exec::model_scope::ModelScopeConfig {
            enforce: Some(true),
            strict: None,
            allow: Some(vec!["anthropic/*".to_string()]),
        };
        let config = crate::background::runner_main::RunnerConfig {
            // SUBA-021: unbudgeted on this path (see the field doc).
            usage_budget: None,
            turn_budget: None,
            permission_rules: None,
            // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
            // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
            timeout_ms: None,
            deadline_at_ms: None,
            share: None,
            artifacts_dir: None,
            artifact_config: crate::artifacts::ArtifactConfig::default(),
            run_id: RunId::new(),
            mode: RunMode::Single,
            steps: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            session_file: None,
            session_id: None,
            global_concurrency_limit: 4,
            worktree_base_dir: None,
            max_subagent_depth: 2,
            async_root: PathBuf::new(),
            results_dir: PathBuf::new(),
            resolved_agents: BTreeMap::new(),
            original_task: String::new(),
            chain_dir: None,
            orchestrator_intercom_target: None,
            inherited_session_model: None,
            model_scope: Some(scope.clone()),
            nested_route: None,
            nested_self: None,
            dynamic_fanout_max_items: None,
            control: None,
            include_progress: None,
        };
        let json = serde_json::to_value(&config).expect("config serializes");
        assert_eq!(
            json.get("modelScope").and_then(|v| v.get("allow")),
            Some(&serde_json::json!(["anthropic/*"])),
            "the policy must be present in the on-disk config handed to the child: {json}"
        );
        let round_tripped: crate::background::runner_main::RunnerConfig =
            serde_json::from_value(json).expect("config round-trips");
        assert_eq!(round_tripped.model_scope, Some(scope));
    }

}
