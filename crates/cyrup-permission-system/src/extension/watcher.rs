//! The PARENT role's forwarding-watcher lifecycle: the idempotent start, the shutdown teardown,
//! the two `#[cfg(test)]` seams that prove idempotence, and the process-wide parent-session
//! anchor a detached child reads to address its parent's spool.

use std::sync::Arc;

use cyrup_ext::HostCtx;

use crate::forwarding;

use super::{PermissionSystemExtension, guard};

/// The `no-ui` marker state last settled for one session id
/// ([`PermissionSystemExtension::sync_unattended_marker`]).
pub(super) struct UnattendedSync {
    session_id: String,
    /// Whether a marker is in place for `session_id`: written by this process, or one a process with a
    /// UI could not remove.
    marked: bool,
}

impl PermissionSystemExtension {
    /// pi `startForwardedPermissionPolling` (`index.ts:1983-2031`): in the PARENT role
    /// (`install_watcher`), on a session WITH a UI and a captured live backend, ensure the forwarding
    /// watcher is running.
    ///
    /// **IDEMPOTENT** — this is the crux of PERM-005. Upstream re-enters this function on FOUR hooks
    /// (`refreshSessionRuntimeState`/`session_start` `:2084`, `before_agent_start` `:2137`, `input`
    /// `:2194`, `tool_call` `:2210`), and cyrup now calls it from the same four places, so it fires on
    /// every turn. The `is_finished()` check below makes N calls yield exactly ONE live watcher — pi's
    /// analog is `if (permissionForwardingWatcher && watchedPermissionForwardingRequestsDir ===
    /// location.requestsDir) { …; return; }` (`index.ts:1996-2000`), which likewise keeps the existing
    /// watcher rather than re-arming one per hook.
    ///
    /// **STOPS on the disqualifying branch** (PERM-005): pi's early return is
    /// `if (!ctx.hasUI || isSubagentExecutionContext(ctx)) { stopForwardedPermissionPolling(); return; }`
    /// (`index.ts:1984-1987`) — it TEARS DOWN a live watcher rather than leaving one orphaned. Cyrup's
    /// guard used to return without stopping, so a UI that detached mid-session left the watcher
    /// prompting into a dead backend.
    ///
    /// A missing `host_services` backend is NOT a disqualifier — it is the "cannot attach yet" case
    /// (pi's `if (!location) return;`, `:1991-1993`), which upstream leaves running for the next hook.
    pub(super) fn maybe_start_forwarding_watcher(&self, ctx: &HostCtx) {
        // PERM-031: publish the live `has_ui` for the detached watcher BEFORE the disqualifying
        // branch, so a scan already in flight sees the new value even on the teardown path. pi gets
        // this for free — `permissionForwardingContext` holds the ctx object itself and
        // `processForwardedPermissionRequests` re-reads `ctx.hasUI` (`index.ts:1114`).
        //
        // Called from all four of pi's `startForwardedPermissionPolling` hooks
        // (`session_start`/`before_agent_start`/`input`/`tool_call`), which is every event arm that
        // carries a ctx, so this is the exact set of moments upstream reassigns `runtimeContext`.
        self.has_ui
            .store(ctx.has_ui, std::sync::atomic::Ordering::Relaxed);
        // \[CYRUP-DELTA] The same four hooks carry `ctx.isProjectTrusted`, which decides whether the
        // project's own policy layers may relax anything ([`crate::types::AgentPermissions::tightened`]).
        guard(&self.manager).set_project_trusted(ctx.is_project_trusted());
        self.sync_unattended_marker(ctx.has_ui);
        if !self.install_watcher || !ctx.has_ui {
            // pi `:1985`: a non-parent / headless context tears the watcher DOWN, it does not merely
            // decline to start one.
            self.stop_forwarding_watcher();
            return;
        }
        let Some(services) = self.host_services.get() else {
            return;
        };
        let mut slot = guard(&self.watcher);
        // Re-entrancy guard: keep a live watcher; only replace a finished one.
        if slot.as_ref().is_some_and(|h| !h.is_finished()) {
            return;
        }
        *slot = Some(forwarding::spawn_forwarding_watcher(
            self.agent_dir.clone(),
            services.clone(),
            Arc::clone(&self.config),
            Arc::clone(&self.logger),
            Arc::clone(&self.has_ui),
        ));
    }

    /// \[CYRUP-DELTA] Keep the root session's `no-ui` marker in the forwarding spool in step with
    /// `has_ui`: present while this process has the session and no UI, withdrawn when it has one.
    /// PARENT role only; a child answers for nobody, and a session id the host cannot name has no
    /// spool to mark.
    ///
    /// Called from every event arm that carries a ctx (it rides on
    /// [`Self::maybe_start_forwarding_watcher`]), so the file system is touched only when the state
    /// changes: the session id and the state last synced are remembered. The FIRST sync for a session
    /// id settles the file whatever it holds, because a session id outlives the process that marked
    /// it (`-c`, `--session <file>`): a UI that resumes a session a `-p` run marked has to withdraw
    /// that run's marker, not only one of its own. A marker whose writer has died is ignored by the
    /// child's side anyway ([`forwarding::session_is_unattended`]); this is what removes one whose
    /// writer is still there, and the file of one that is not.
    ///
    /// A marker is not withdrawn at `SessionShutdown`: it is stamped with the process that wrote it,
    /// so it stops meaning anything when that process exits, and it is swept up by the next headless
    /// start.
    fn sync_unattended_marker(&self, has_ui: bool) {
        if !self.install_watcher {
            return;
        }
        let Some(session_id) = self
            .host_services
            .get()
            .and_then(|services| services.session_id())
            .and_then(|id| forwarding::normalize_session_id(&id))
        else {
            return;
        };
        let mut synced = guard(&self.unattended_marker);
        if synced
            .as_ref()
            .is_some_and(|last| last.session_id == session_id && last.marked != has_ui)
        {
            return;
        }
        let marked = if has_ui {
            // A marker that could not be removed counts as in place, so the next sync tries again.
            !forwarding::clear_session_unattended(
                &self.agent_dir,
                &session_id,
                Some(self.logger.as_ref()),
            )
        } else {
            forwarding::mark_session_unattended(
                &self.agent_dir,
                &session_id,
                Some(self.logger.as_ref()),
            )
        };
        *synced = Some(UnattendedSync { session_id, marked });
    }

    /// pi `stopForwardedPermissionPolling` (`index.ts:1970-1981`, called from `session_shutdown`
    /// `:2131` and from the disqualified branch of `startForwardedPermissionPolling` `:1985`): abort
    /// the forwarding watcher task. Idempotent — a no-op when no watcher is installed.
    pub(super) fn stop_forwarding_watcher(&self) {
        if let Some(handle) = guard(&self.watcher).take() {
            handle.abort();
        }
    }

    /// Test seam: is a live (unfinished) forwarding-watcher task currently installed? Used by the
    /// PERM-005 idempotence regression tests to assert that N `maybe_start_forwarding_watcher` calls
    /// yield exactly one watcher.
    #[cfg(test)]
    pub(super) fn has_live_forwarding_watcher(&self) -> bool {
        guard(&self.watcher)
            .as_ref()
            .is_some_and(|h| !h.is_finished())
    }

    /// Test seam (PERM-005): how many watcher TASKS currently exist, counted independently of the
    /// `watcher` slot.
    ///
    /// Every spawned watcher moves its own clone of the shared `config` handle into the task future,
    /// synchronously at `tokio::spawn` time, so `Arc::strong_count` MINUS the non-watcher holders is
    /// the number of watcher futures still alive. This is the assertion that would catch a
    /// non-idempotent start: the slot only ever holds ONE `JoinHandle`, so overwriting it would hide
    /// a leaked task, whereas the leaked task's `Arc` clone cannot hide.
    ///
    /// The non-watcher holders are exactly three and are structural, not incidental:
    ///
    /// 1. the extension's own `self.config` field;
    /// 2. `self.logger`, which must share the SAME handle so a config reload re-arms the audit
    ///    trail (pi's `extensionLogger` reads the module-scope `extensionConfig` binding,
    ///    `index.ts:146-150`);
    /// 3. `self.controller` (PERM-007), which must share the SAME handle so the settings modal's
    ///    writer and this extension's reader are one cell — pi's controller literal closes over the
    ///    same module-scope `extensionConfig` binding the logger reads (`getConfig: () =>
    ///    extensionConfig`, v0.8.0 `index.ts:1507`, in the `registerCommand` at `:1502-1512`).
    ///
    /// Adding a FOURTH holder without updating this constant makes the count read one watcher too
    /// many — which is precisely how this seam is meant to fail: loudly, at the assertion, rather
    /// than silently under-counting a leak. `a_fresh_extension_holds_no_watcher_config_handles`
    /// pins the baseline directly, so drift is caught even when no watcher is armed.
    #[cfg(test)]
    pub(super) fn live_watcher_task_count(&self) -> usize {
        /// `self.config` + `self.logger` + `self.controller` — see the note above.
        const NON_WATCHER_CONFIG_HOLDERS: usize = 3;
        Arc::strong_count(&self.config).saturating_sub(NON_WATCHER_CONFIG_HOLDERS)
    }

    /// PERM-001 — publish this PARENT session's own id as the process-wide parent-session anchor
    /// (`cyrup_ext_subagents::publish_parent_session_anchor`), the address a subagent child's
    /// forwarded ask writes to.
    ///
    /// This is the cyrup placement of pi's `process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId`
    /// (`pi-subagents/src/extension/index.ts:599` @v0.34.0). Upstream, the SUBAGENTS extension does it, into
    /// the real process environment, so every descendant — foreground, background, detached, at any
    /// hop — inherits the anchor for free. `cyrup-ext-subagents` cannot: it is
    /// `#![forbid(unsafe_code)]` and `std::env::set_var` is `unsafe` in edition 2024, so it keeps
    /// the captured anchor in a private executor field and threads it explicitly — which reaches
    /// only the FOREGROUND spawn path. A background run crosses two OS process boundaries, and
    /// neither carried the anchor, so a background child's `ask` addressed a null target and
    /// `forwarding::wait_for_forwarded_approval` fail-closed DENIED it with no prompt ever reaching
    /// the operator.
    ///
    /// This crate is the anchor's sole consumer and, unlike `cyrup-ext-subagents`, sits in the root
    /// process with the live session id in hand at exactly pi's moment (`SessionStart`), so it is
    /// the natural publisher of the memory-safe register that stands in for pi's `process.env`
    /// slot. `cyrup_ext_subagents::background::spawn_detached` reads it back when it builds the
    /// hop-1 env overlay, restoring the inheritance chain pi gets for free.
    ///
    /// PARENT role only (`install_watcher`). A CHILD must never publish its own id: a depth-2
    /// grandchild would then address its immediate parent's spool instead of continuing to thread
    /// the root's anchor, which is the direct-parent depth-1 semantics
    /// `cyrup_ext_subagents::PARENT_SESSION_ENV_VAR` documents. Publishing is also unconditional in
    /// `has_ui` (pi's `index.ts:599` is), so a UI-less parent that later gains one still has a
    /// correctly-addressed anchor in place; the watcher, not the anchor, is what `has_ui` gates.
    pub(super) fn publish_parent_session_anchor(&self) {
        if !self.install_watcher {
            return;
        }
        if let Some(services) = self.host_services.get()
            && let Some(id) = services.session_id()
        {
            cyrup_ext_subagents::publish_parent_session_anchor(&id);
        }
    }
}
