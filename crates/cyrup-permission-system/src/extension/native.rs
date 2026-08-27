//! The [`NativeExtension`] trait implementation — the host-facing surface: identity, the
//! late-bound capability backend, `init`'s subscriptions + registrations, the
//! `/permission-system` command dispatch and the event handler that drives every layer.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use cyrup_core::ExtensionId;
use cyrup_ext::{
    EventKind, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension,
    NotifyKind,
};

use crate::skill;
use crate::status;

use super::consts::PERMISSION_SYSTEM_COMMAND;
use super::{PermissionSystemExtension, guard};

// Doc-link-only imports: these are named by prose relocated verbatim from the single-file
// `extension.rs`, where they were in scope for real code. `#[cfg(doc)]` keeps those intra-doc
// links resolving without adding an import the compiled build does not use.
#[cfg(doc)]
use crate::ask::LocalAskChannel;

#[async_trait]
impl NativeExtension for PermissionSystemExtension {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    /// Ambient (SEAM-071/SEAM-074): upstream `@gotgenes/pi-permission-system` is an installed
    /// package in the PATH tier that `noExtensions` collapses (`resource-loader.ts:451-453`
    /// @v0.83.0). A subagent CHILD keeps the gate — pi re-injects it by path via
    /// `resolvePermissionSystemExtension` (`pi-subagents/src/runs/shared/pi-args.ts:413-417`
    /// @v0.47.1) — so the builder's `SUBAGENT_CHILD_RUNTIME_NATIVES` carve-out, not this flag, is
    /// what keeps a pinned-allowlist child from failing OPEN.
    fn is_ambient(&self) -> bool {
        true
    }

    /// P-1 (reconciliation §2 item 1): capture the late-bound live capability backend BEFORE `init`
    /// (the builder threads its `LiveHostServices` via `load_native_with_services`). The in-session
    /// `ask` dialog (`resolve_ask`) prompts through it via [`LocalAskChannel`]. Set-once; a second
    /// bind is ignored (the session's backend is stable).
    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        let _ = self.host_services.set(services);
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        // ToolCall is the deciding gate (honored by the block/mutate dispatcher `ExtHooks` drives).
        // BeforeAgentStart runs the context-hygiene shaping (pi `index.ts:2134-2190`): it shapes the
        // active tool set (`setActiveTools`), sanitizes the system prompt (tools section + denied
        // guideline bullets, and hides ask/deny skills while caching the skill-read enforcement entries,
        // pi `:2174-2176`), and syncs the yolo pill — returning the sanitized prompt as a `[mutate]`.
        // Input captures `/skill:<name>` explicit requests (pi `:2192-2206`). Session{Start,Shutdown}
        // clear the in-session store + dedup + skill state (and set/clear the status pill).
        // ResourcesDiscover runs pi's `resources_discover` reload branch (`index.ts:2103-2118`):
        // re-reads `config.json`, rebuilds the `PermissionManager` from the current cwd, and
        // invalidates the agent-start cache. No LLM-visible TOOL is registered — the gate is
        // invisible to the model (pi registers none either). The one HUMAN-visible registration is
        // the `/permission-system` slash command below, which pi registers at `index.ts:1502-1512`.
        api.subscribe(&[
            EventKind::ToolCall,
            EventKind::BeforeAgentStart,
            EventKind::Input,
            EventKind::SessionStart,
            EventKind::SessionShutdown,
            EventKind::ResourcesDiscover,
        ]);
        // pi `pi.registerCommand("permission-system", { description, handler })`
        // (v0.8.0 `index.ts:1502-1512`). This is what makes `ExtensionConfig::save` reachable at
        // all: before it, every `.save(` call site in this crate lived inside `#[cfg(test)]`, so the
        // v0.8.0 save semantics (non-extension keys preserved, corrupt file refused, symlink written
        // through) could not be observed by anything a human could run. The registration lands in
        // `ExtensionRegistry`'s command table via `load_native_body`, and `/permission-system` routes
        // back here through `ExtensionHost::execute_native_command` → [`Self::execute_command`].
        api.register_command(
            PERMISSION_SYSTEM_COMMAND,
            cyrup_ext::CommandDescriptor {
                description: crate::common::PERMISSION_SYSTEM_COMMAND_DESCRIPTION.to_string(),
                // The modal's two setting ids (`config-modal.ts:27,34`) as completions.
                completions: vec!["debug".to_string(), "yoloMode".to_string()],
            },
        );
        // PERM-011 half A / pi `runtimeApi = registerPiPermissionSystemRuntimeApi(…)`
        // (`index.ts:1481-1485`). Upstream registers it in the activation body, one statement
        // before the command registration above; this is that same body. Retracted in
        // `session_shutdown` (`:1868-1870`).
        self.publish_runtime_api();
        Ok(())
    }

    /// Service the `/permission-system` command (pi `createPermissionSystemCommandHandler`,
    /// v0.8.0 `common.ts:188-198`).
    ///
    /// The `has_ui` guard is upstream's, verbatim in effect (`common.ts:192-195`): with no
    /// interactive UI the handler notifies a `warning` and returns without touching the config.
    ///
    /// It returns `Ok(None)` afterwards, NOT the sentence it just notified. Per the convention on
    /// [`cyrup_ext::NativeExtension::execute_command`], an `Ok(Some(text))` is surfaced by the
    /// session as an **Info** notification, so returning the same sentence would put it on screen
    /// twice — once as the `warning` this level deliberately chose, once as an Info duplicate. The
    /// handler owns the level here, so it owns the whole notification.
    async fn execute_command(
        &self,
        name: &str,
        args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        if name != PERMISSION_SYSTEM_COMMAND {
            return Err(ExtError::Component(format!(
                "cyrup-permission-system has no handler for command `{name}`"
            )));
        }
        // pi `common.ts:192-195`.
        if !ctx.has_ui {
            if let Some(services) = self.host_services.get() {
                services.notify(
                    crate::common::PERMISSION_SYSTEM_COMMAND_REQUIRES_UI,
                    NotifyKind::Warning,
                );
            }
            return Ok(None);
        }
        // pi `openPermissionSystemSettingsModal(ctx, { getConfig, setConfig, getConfigPath })`
        // (`index.ts:1504-1511`). `None` here means the handler already notified at its own level.
        Ok(self.run_permission_system_command(args))
    }

    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::ToolCall { call_id, name, input } => {
                // PERM-005 / pi `tool_call` (`index.ts:2210`): every tool call re-enters
                // `startForwardedPermissionPolling`, so a watcher that could not attach at session
                // start (unresolved session id, UI attached late) is armed here instead of never.
                // Idempotent — see `maybe_start_forwarding_watcher`.
                self.maybe_start_forwarding_watcher(ctx);
                self.decide(call_id.as_str(), name, input, ctx).await
            }
            HostEvent::BeforeAgentStart { system_prompt, .. } => {
                // pi `before_agent_start` (`index.ts:2134-2190`): shape the active tool set
                // (`setActiveTools`), sanitize the system prompt (tools section + denied guideline
                // bullets, and hide ask/deny skills while caching the enforcement entries the skill-read
                // gate reads at every `tool_call`), and sync the yolo status pill — returning the
                // sanitized prompt as a `[mutate]`.
                //
                // PERM-024 / pi `before_agent_start` (v0.8.0 `index.ts:1875-1878`): the handler's
                // first two statements are `runtimeContext = ctx; refreshExtensionConfig(ctx);`,
                // i.e. `config.json` is re-read at the TOP OF EVERY TURN — before the watcher is
                // re-armed and before any shaping. Without it an operator's mid-session
                // `yoloMode`/`debug` edit took effect only at the next session start or resource
                // reload.
                //
                // The CONFIG half only (`refresh_extension_config`, not
                // `refresh_config_and_manager`): pi does not rebuild the `PermissionManager` here
                // and does not invalidate the agent-start cache here — doing either per turn would
                // defeat the cache PERM-013 just landed.
                self.refresh_extension_config();
                // PERM-005 / pi `before_agent_start` (`index.ts:1878`): re-enter
                // `startForwardedPermissionPolling`, so each turn re-arms the forwarding watcher
                // (and tears it down if the UI has gone away). Idempotent.
                self.maybe_start_forwarding_watcher(ctx);
                self.on_before_agent_start(system_prompt, ctx)
            }
            HostEvent::Input { text, .. } => {
                // PERM-005 / pi `input` (`index.ts:2194`): re-enter `startForwardedPermissionPolling`
                // on every user turn. Idempotent.
                self.maybe_start_forwarding_watcher(ctx);
                // pi `index.ts:2192-2206`: a `/skill:<name>` slash command is a direct user action —
                // remember it so its skill-file reads bypass the skill-read ask/deny (pi `:2243`).
                if let Some(name) = skill::extract_skill_name_from_input(text) {
                    guard(&self.explicitly_requested_skill_names).insert(name);
                }
                HookOutcome::Noop
            }
            HostEvent::SessionStart { reason, .. } => {
                // pi `index.ts:2089,2092`: clear session store + dedup + explicit-skill set; refresh.
                guard(&self.session_approvals).clear();
                guard(&self.dedup).clear();
                guard(&self.explicitly_requested_skill_names).clear();
                // pi `resetShownWarnings()` (`index.ts:2079`, the first statement of
                // `refreshSessionRuntimeState`): a new session re-arms every load warning, so a
                // still-malformed policy file is reported again rather than staying suppressed by
                // the previous session's dedup set.
                self.warnings.reset();
                // pi `refreshSessionRuntimeState` (`index.ts:2077-2085`, called unconditionally from
                // every `session_start`): re-read `config.json` from disk and rebuild the
                // `PermissionManager`'s policy paths from the CURRENT session `ctx.cwd` (not just the
                // process's original cwd) — a session can start in a different working directory than
                // the one the extension was constructed with. Also invalidates the agent-start cache
                // (clears `active_skill_entries`), superseding the plain clear this arm did before.
                // pi `handleSessionStart` (`handlers/lifecycle.ts:54-60`): read project trust
                // ONCE and use it for the refresh and the warning both. An untrusted project has
                // its policy scopes withheld (`configureForCwd(projectTrusted ? ctx.cwd :
                // undefined)`, `permission-session.ts:106-110`, #644), so a repo's checked-in
                // `.cyrup/agent` file cannot widen the allow set before the human grants trust.
                let project_trusted = self.project_trusted(ctx);
                self.refresh_config_and_manager(project_trusted.then_some(ctx.cwd.as_path()));
                // pi warns AFTER the refresh (`:58-60`), so the review entry can never claim a
                // scope the manager has not actually been rebuilt with.
                if !project_trusted {
                    self.warn_project_untrusted(&ctx.cwd, "session_start");
                }
                // PERM-001 / pi `process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId`
                // (`pi-subagents/src/extension/index.ts:599` @v0.34.0): publish this parent session's id as
                // the process-wide anchor a subagent child's forwarded ask addresses, BEFORE the
                // watcher that services those asks starts. Without it the detached background hop
                // spawns children with no anchor and every one of their asks fail-closed denies.
                self.publish_parent_session_anchor();
                // pi `startForwardedPermissionPolling` via `refreshSessionRuntimeState`
                // (`index.ts:2084`): in the PARENT role, on a session WITH a UI, spawn the forwarding
                // watcher (a detached tokio task, OUTSIDE the 5s dispatch budget) that services
                // subagent children's forwarded asks. This is the FIRST of four re-entry points
                // (PERM-005) — see the `BeforeAgentStart` / `Input` / `ToolCall` arms.
                self.maybe_start_forwarding_watcher(ctx);
                // PERM-026: the yolo status pill is NO LONGER synced here. Upstream reaches it from
                // inside `refreshExtensionConfig` → `applyExtensionConfigSideEffects`
                // (v0.8.0 `index.ts:1364-1366`), which `refreshSessionRuntimeState` calls at
                // `:1821`; `refresh_config_and_manager` above now does the same, so a second write
                // here would only duplicate it — and keeping it here is exactly what let the
                // `resources_discover` arm, which never had one, go stale.
                //
                // PERM-027 / pi `:1834-1843`: a session_start whose `reason` is `"reload"` records
                // a `lifecycle.reload` line, so an operator diagnosing "did my policy edit take
                // effect" can tell a reload from a fresh start in the debug trail. Gated on the
                // reason exactly as upstream is: a `"startup"` session writes none.
                if reason == "reload" {
                    self.write_debug_entry(
                        "lifecycle.reload",
                        &json!({
                            "triggeredBy": "session_start",
                            "reason": reason,
                            "cwd": ctx.cwd.to_string_lossy(),
                        }),
                    );
                }
                HookOutcome::Noop
            }
            HostEvent::ResourcesDiscover { reason, .. } => {
                // pi `pi.on("resources_discover", …)` (v0.8.0 `index.ts:1844-1859`). The WHOLE body
                // is gated on `event.reason === "reload"` (`:1845`) — a `"startup"` discovery does
                // nothing here, because `session_start` has already refreshed everything.
                //
                // Cyrup's `HostEvent::ResourcesDiscover` used to carry no `reason`, so this arm
                // treated every dispatch as the reload case; the field now exists
                // (`cyrup-ext/src/event.rs:349`, EXT-016) and `facade::aggregate_resources`
                // genuinely sends `"startup"` for the discovery pass, so the gate is both
                // expressible and load-bearing.
                if reason != "reload" {
                    return HookOutcome::Noop;
                }
                // pi `resetShownWarnings()` (`:1846`, the reload branch's first statement).
                self.warnings.reset();
                guard(&self.dedup).clear();
                // pi `refreshExtensionConfig` + `createPermissionManagerForCwd` +
                // `invalidateAgentStartCache` (`:1848-1852`).
                // pi `handleResourcesDiscover` (`handlers/lifecycle.ts:92-96`): the reload path
                // re-evaluates trust the same way, so a trust grant since session start
                // re-includes the project scope and a revocation drops it again.
                let project_trusted = self.project_trusted(ctx);
                self.refresh_config_and_manager(project_trusted.then_some(ctx.cwd.as_path()));
                if !project_trusted {
                    self.warn_project_untrusted(&ctx.cwd, "resources_discover");
                }
                // PERM-027 / pi `writeDebugEntry("lifecycle.reload", …)` (`:1853-1857`). pi's `cwd`
                // is `runtimeContext?.cwd ?? null`; cyrup's `ctx` is always live at dispatch, so
                // the null arm is unreachable rather than dropped.
                self.write_debug_entry(
                    "lifecycle.reload",
                    &json!({
                        "triggeredBy": "resources_discover",
                        "reason": reason,
                        "cwd": ctx.cwd.to_string_lossy(),
                    }),
                );
                HookOutcome::Noop
            }
            HostEvent::SessionShutdown { .. } => {
                // pi `index.ts:2122,2123,2128,2130,2131`: clear the status pill + stores + dedup + skill
                // state; tear down watcher.
                if let Some(s) = self.host_services.get() {
                    status::clear_status(s);
                }
                guard(&self.session_approvals).clear();
                guard(&self.dedup).clear();
                guard(&self.explicitly_requested_skill_names).clear();
                // pi `invalidateAgentStartCache()` (v0.8.0 `index.ts:1871`) — the WHOLE cache, not
                // just the skill entries: a shutdown must not leave a live prompt-state key that a
                // later session could hit (PERM-013).
                self.invalidate_agent_start_cache();
                // pi `resetShownWarnings()` (`index.ts:2125`).
                self.warnings.reset();
                // PERM-011 half A / pi `unregisterPiPermissionSystemRuntimeApi(runtimeApi ??
                // undefined); … runtimeApi = null;` (`index.ts:1868` and `:1870`): upstream runs it
                // after `resetShownWarnings()` (`:1866`) and before the polling teardown
                // (`stopForwardedPermissionPolling`, `:1872`), which is where it sits here. The one
                // neighbour that differs is `invalidateAgentStartCache()` — pi runs it at `:1871`,
                // AFTER the unregister, and this handler already ran it above; that ordering is
                // this port's, predates PERM-011 and is unobservable (the cache and the published
                // slot share no state). A session that has ended must not leave a control surface
                // published that can still flip yolo mode.
                self.retract_runtime_api();
                self.stop_forwarding_watcher();
                // PERM-001 / pi `delete process.env[SUBAGENT_PARENT_SESSION_ENV]`
                // (`pi-subagents/src/extension/index.ts:619` @v0.34.0): drop the published anchor so a stale
                // id from the session that just ended never addresses a subsequently-started
                // session's spool on this same long-lived process. PARENT role only, symmetric with
                // `publish_parent_session_anchor` — a CHILD never published and must not clear the
                // anchor its own descendants still need.
                if self.install_watcher {
                    cyrup_ext_subagents::clear_parent_session_anchor();
                }
                HookOutcome::Noop
            }
            _ => HookOutcome::Noop,
        }
    }
}
