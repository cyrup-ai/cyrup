//! The extension `config.json` lifecycle: load, the `session_start` / `resources_discover`
//! refresh of both the config and the manager, the dedup-once warning memo, and the two v0.8.0
//! config WRITERS' shared plumbing.

use std::path::Path;
use std::sync::Arc;

use serde_json::json;

use crate::agent_start_cache::AgentStartCache;
use crate::ext_config::ExtensionConfig;
use crate::status;

use super::warnings::manager_with_warnings;
use super::{PermissionSystemExtension, guard};

// Doc-link-only imports: these are named by prose relocated verbatim from the single-file
// `extension.rs`, where they were in scope for real code. `#[cfg(doc)]` keeps those intra-doc
// links resolving without adding an import the compiled build does not use.
#[cfg(doc)]
use cyrup_ext::{HostServices, NotifyKind};

impl PermissionSystemExtension {
    /// Read `config.json` ONCE, through the resolved path (pi `loadPermissionSystemConfig()`,
    /// `extension-config.ts:117-138`).
    ///
    /// pi's entry point calls this exactly once at load — `loadExtensionConfigState()`
    /// (`index.ts:1350-1354`) is invoked at `index.ts:1473`, the `enabled` master switch tests the
    /// module-scope `extensionConfig` it just populated (`:1475-1477`), and everything downstream
    /// reuses that same object. cyrup's [`crate::permission_extension_for_env`] is the analog of that entry
    /// point, so it performs THE load and hands the result to the `*_with_config` constructor; the
    /// public constructors keep their standalone signature by doing the load themselves.
    ///
    /// Loading twice was not merely wasteful: [`ExtensionConfig::load`] `eprintln!`s on a malformed
    /// or unreadable config, so an operator with a corrupt `config.json` saw the identical warning
    /// printed twice per session build where pi prints it once.
    ///
    /// v0.7.1's `derive_parts` (which this replaces) also derived a
    /// `cyrup-permission-system-approvals.json` path for the `PermanentApprovalStore`; upstream
    /// deleted that store in v0.8.0 (commit `a33ac2c`), so no such file is read any more — see
    /// [`crate::stores`].
    pub(super) fn load_config(agent_dir: &Path) -> ExtensionConfig {
        ExtensionConfig::load(&Self::config_path_for(agent_dir))
    }

    /// pi `refreshSessionRuntimeState` (`index.ts:2077-2085`) + the `resources_discover` "reload"
    /// branch (`index.ts:2103-2118`): re-read `config.json` from disk into `self.config`, rebuild
    /// `self.manager`'s policy paths from the CURRENT `cwd` (not the process's original one), and
    /// invalidate the agent-start cache (`invalidateAgentStartCache`, `:1575-1581`) by clearing the
    /// cached active-skill enforcement entries. Shared by both the `session_start` handler and a
    /// `resources_discover` reload.
    /// Also surfaces the extension-config load warning (pi `refreshExtensionConfig`,
    /// `index.ts:1600-1618`) — this is the one place a malformed `config.json` becomes visible,
    /// since construction happens before any host backend is attached.
    pub(super) fn refresh_config_and_manager(&self, cwd: &Path) {
        // pi order (`refreshSessionRuntimeState`, v0.8.0 `index.ts:1819-1826`): config first,
        // manager second, agent-start cache invalidated third.
        self.refresh_extension_config();
        *guard(&self.manager) = manager_with_warnings(
            Self::manager_paths_for(&self.agent_dir, cwd),
            &self.warnings,
        );
        self.invalidate_agent_start_cache();
    }

    /// pi `refreshExtensionConfig(ctx?)` (v0.8.0 `index.ts:1383-1386`) = `loadExtensionConfigState()`
    /// (`:1350-1354`) + `applyExtensionConfigSideEffects(result, ctx)` (`:1356-1381`), in that
    /// order. The **config half only** — no manager rebuild, no agent-start-cache invalidation.
    ///
    /// Split out of [`Self::refresh_config_and_manager`] for PERM-024: pi calls this on THREE
    /// surfaces (`session_start` via `refreshSessionRuntimeState` `:1821`, the `resources_discover`
    /// reload branch `:1848`, and `before_agent_start` `:1877`) but rebuilds the manager and
    /// invalidates the cache on only the first two. Calling the combined function from
    /// `before_agent_start` would rebuild the `PermissionManager` and blow away the agent-start
    /// cache on every single turn — the exact per-turn cost PERM-013's cache exists to remove.
    ///
    /// The side-effect ORDER inside `applyExtensionConfigSideEffects` is pi's and is load-bearing:
    /// status pill (`:1364-1366`) → warning memo (`:1368-1374`) → `config.loaded` debug entry
    /// (`:1376-1381`). PERM-026 was the status sync being absent from here entirely, so a
    /// `resources_discover` reload changed the live gating behaviour while the pill kept the stale
    /// value until the next `before_agent_start` repainted it.
    pub(super) fn refresh_extension_config(&self) {
        let loaded = ExtensionConfig::load_with_result(&Self::config_path_for(&self.agent_dir));
        let (created, debug, yolo_mode) =
            (loaded.created, loaded.config.debug, loaded.config.yolo_mode);
        // pi `setExtensionConfig(result.config)` inside `loadExtensionConfigState` (`:1352`).
        *guard(&self.config) = loaded.config;
        // PERM-026 / pi `:1364-1366`: `if (runtimeContext?.hasUI) { syncPermissionSystemStatus(...) }`
        // — reached on EVERY refresh surface, which is why a reload re-syncs the pill upstream.
        // `sync_status_when_possible` is the ported form of that guard (see its doc for why the
        // `hasUI` half collapses into "is a backend attached").
        {
            let config = guard(&self.config).clone();
            self.sync_status_when_possible(&config);
        }
        self.report_config_warning(loaded.warning.clone());
        // pi `writeDebugEntry("config.loaded", …)` (`:1376-1381`) — emitted AFTER the new config
        // is installed, so a reload that turns `debug` ON records its own arrival as the trail's
        // first line.
        self.write_debug_entry(
            "config.loaded",
            &json!({
                "created": created,
                "warning": loaded.warning,
                "debug": debug,
                "yoloMode": yolo_mode,
            }),
        );
    }

    /// pi `invalidateAgentStartCache()` (v0.8.0 `index.ts:1326-1331`): drop the cached skill
    /// enforcement entries AND both `before_agent_start` cache keys, so the next turn recomputes
    /// from scratch. Called from `session_start` (`:1823`), the `resources_discover` reload branch
    /// (`:1852`) and `session_shutdown` (`:1871`) — never from `before_agent_start` itself.
    pub(super) fn invalidate_agent_start_cache(&self) {
        // pi `activeSkillEntries = []` (`:1327`).
        guard(&self.active_skill_entries).clear();
        // pi `:1328-1330`.
        *guard(&self.agent_start_cache) = AgentStartCache::default();
    }

    /// pi `refreshExtensionConfig`'s warning branch (`index.ts:1610-1618`): report a NEW warning
    /// once and remember it; clear the memo when the load comes back clean, so a later recurrence
    /// is reported again.
    fn report_config_warning(&self, warning: Option<String>) {
        let Some(warning) = warning else {
            *guard(&self.last_config_warning) = None;
            return;
        };
        // Scoped so the memo lock is released before the sink is touched — `notify` takes its own
        // lock and reaches the host.
        let is_new = {
            let mut last = guard(&self.last_config_warning);
            let is_new = last.as_deref() != Some(warning.as_str());
            if is_new {
                *last = Some(warning.clone());
            }
            is_new
        };
        if is_new {
            self.warnings.notify(&warning);
        }
    }

    // ===================================================== the two v0.8.0 config WRITERS (G133/F1)
    //
    // `ExtensionConfig::save` (the atomic merge-into-the-existing-document write, v0.8.0
    // `extension-config.ts:240-293`) landed with NO non-test call site, which made all three of the
    // behaviours it exists to guarantee — non-extension keys preserved, a corrupt file refused, a
    // symlinked config written through — unobservable in cyrup, because cyrup never saved this
    // config at all. The two functions below are upstream's two callers of it; `execute_command`
    // (the `/permission-system` handler, pi `index.ts:1502-1512`) is what reaches them.

    /// pi `syncPermissionSystemStatusWhenPossible(config, ctx?)` (v0.8.0 `index.ts:1388-1400`):
    /// reflect `yoloMode` on the live status bar after a config write.
    ///
    /// \[CYRUP-DELTA] pi's two branches — an explicitly-passed `ctx` (`:1392-1395`, the
    /// `saveExtensionConfig` case) versus the module-scope `runtimeContext?.hasUI` fallback
    /// (`:1397-1399`, the `setYoloModeFromRuntimeApi` case) — exist because pi's `ui.setStatus` is
    /// only reachable through whichever `ExtensionContext` object is at hand. cyrup's status seam is
    /// [`crate::status::sync_status`] over the ONE late-bound [`HostServices`] backend the session
    /// attaches, which is the same object no matter which handler is running, so both branches
    /// collapse into this single reachability test. The `hasUI` half is not re-imposed, for the
    /// reason [`crate::extension::warnings::WarningSink::notify`] documents: `HostServices::set_status` already no-ops on a
    /// backend with no status surface, and re-imposing it would blank the pill in modes that do
    /// render one. This is the same test the `SessionStart` / `BeforeAgentStart` arms already use.
    pub(super) fn sync_status_when_possible(&self, config: &ExtensionConfig) {
        if let Some(services) = self.host_services.get() {
            status::sync_status(services, config);
        }
    }

    /// pi `saveExtensionConfig(next, ctx)` (v0.8.0 `index.ts:1402-1420`) — registered as the config
    /// modal's `setConfig` (`index.ts:1508`), i.e. what runs when the human flips a row in
    /// `/permission-system`.
    ///
    /// The ORDER is the contract, and it is the reason this is a function and not three inlined
    /// statements: normalize, WRITE, and only then touch anything in memory. A failed write returns
    /// the cause and has changed NOTHING — no live config, no status pill, no `lastConfigWarning`
    /// reset, no debug entry — so cyrup can never end a turn with an in-memory config that
    /// disagrees with the file the next `session_start` will re-read.
    ///
    /// \[CYRUP-DELTA] Two shape differences, neither behavioural:
    /// - pi takes `ctx: ExtensionCommandContext` purely to reach `ctx.ui.notify` (`:1407`) and to
    ///   pass to `syncPermissionSystemStatusWhenPossible` (`:1413`). Both of those reach the live
    ///   [`HostServices`] backend in cyrup, which this extension already holds, so there is no ctx
    ///   parameter to thread and nothing is lost by its absence.
    /// - pi returns `void`; this returns whether the save landed, and on failure the RAW cause. Its
    ///   one upstream caller recovers the same fact by re-reading `controller.getConfig()` straight
    ///   after (`config-modal.ts:79`), which is exactly what the returned `Result` saves the cyrup
    ///   caller from having to do.
    ///
    /// **The `Err` MUST be surfaced by the caller**, at [`NotifyKind::Error`]. pi notifies inline
    /// here (`:1407`); cyrup hands the cause up one level instead so the human gets ONE error toast
    /// carrying both the what ("YOLO mode is unchanged (off)") and the why (the raw save error),
    /// rather than two toasts saying half each. See [`Self::run_permission_system_command`], the
    /// only caller, and the `Ok(None)` convention documented on
    /// [`cyrup_ext::NativeExtension::execute_command`].
    ///
    /// PERM-007: the BODY now lives on the shared
    /// [`crate::config_modal::ConfigController`] — pi's own `{getConfig, setConfig, getConfigPath}`
    /// indirection (`config-modal.ts:8-12`, registered `index.ts:1504-1511`) — so the `'static`
    /// settings overlay commits through the identical writer this method does. Nothing about the
    /// ordering contract moved with it; see [`crate::config_modal::ConfigController::set_config`].
    pub fn save_extension_config(&self, next: &ExtensionConfig) -> Result<(), String> {
        self.controller.set_config(next)
    }

    /// PERM-007 — the shared config controller, so a caller that needs the WRITER without holding
    /// this extension (an overlay, a runtime-API consumer) can take an `Arc` of it.
    #[must_use]
    pub fn config_controller(&self) -> Arc<crate::config_modal::ConfigController> {
        Arc::clone(&self.controller)
    }
}
