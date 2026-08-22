//! Yolo mode: the flag, its two writers, and the process-global runtime API through which a
//! second extension reads or flips it without holding this one.

use std::sync::Arc;

use serde_json::json;

use crate::ext_config::ExtensionConfig;
use crate::yolo_api::{YoloModeControlOptions, YoloModeControlResult};

use super::consts::YOLO_PERSIST_FALLBACK_ERROR;
use super::{PermissionSystemExtension, guard};

/// PERM-011 half A — the object published on the process-global registry, i.e. cyrup's spelling of
/// pi's `{getYoloMode, setYoloMode, toggleYoloMode}` literal (`index.ts:1481-1485`).
///
/// It is a distinct type rather than an `impl` on the extension because the registry must hold a
/// handle that does NOT keep a finished session's extension alive; see the CYRUP-DELTA on
/// [`crate::runtime_api`]. The three methods delegate straight to the ported inherent methods, so
/// there is exactly ONE implementation of each behaviour — in particular of `set_yolo_mode`'s
/// persist-failure invariant.
struct PublishedRuntimeApi {
    ext: std::sync::Weak<PermissionSystemExtension>,
}

impl crate::runtime_api::PermissionSystemRuntimeApi for PublishedRuntimeApi {
    /// pi `getYoloMode: () => extensionConfig.yoloMode` (`index.ts:1482`). A dropped extension
    /// reports `false` — "no gate is running, so nothing is being auto-approved" — which is the
    /// fail-CLOSED reading of the flag.
    fn get_yolo_mode(&self) -> bool {
        self.ext.upgrade().is_some_and(|ext| ext.yolo_mode())
    }

    /// pi `setYoloMode: setYoloModeFromRuntimeApi` (`index.ts:1483`).
    fn set_yolo_mode(
        &self,
        enabled: bool,
        options: &YoloModeControlOptions,
    ) -> YoloModeControlResult {
        match self.ext.upgrade() {
            Some(ext) => ext.set_yolo_mode(enabled, options),
            None => Self::gone(),
        }
    }

    /// pi `toggleYoloMode: (options) => setYoloModeFromRuntimeApi(!extensionConfig.yoloMode,
    /// options)` (`index.ts:1484`).
    fn toggle_yolo_mode(&self, options: &YoloModeControlOptions) -> YoloModeControlResult {
        match self.ext.upgrade() {
            Some(ext) => ext.toggle_yolo_mode(options),
            None => Self::gone(),
        }
    }
}

impl PublishedRuntimeApi {
    /// The extension is gone: report the SAME shape a refused persist reports — nothing changed,
    /// nothing was written, and the reason is stated (`yolo-mode-api.ts:6-11`).
    fn gone() -> YoloModeControlResult {
        YoloModeControlResult {
            yolo_mode: false,
            changed: false,
            persisted: false,
            error: Some(crate::runtime_api::EXTENSION_GONE_ERROR.to_string()),
        }
    }
}

impl PermissionSystemExtension {
    /// pi `getYoloMode: () => extensionConfig.yoloMode` (v0.8.0 `index.ts:1482`).
    #[must_use]
    pub fn yolo_mode(&self) -> bool {
        guard(&self.config).yolo_mode
    }

    /// pi `setYoloModeFromRuntimeApi(enabled, options)` (v0.8.0 `index.ts:1422-1469`), exposed
    /// upstream as the runtime API's `setYoloMode` (`:1483`).
    ///
    /// The security-relevant property, and the whole reason this is not just "assign the field and
    /// save": when `persist` is on and the write FAILS, the in-memory yolo mode is left exactly as
    /// it was and the result reports `changed: false, persisted: false` with the error
    /// (`:1438-1451`). A caller must never be told that auto-approval was turned on (or off) when
    /// the gate's live config — and the file the next session will load — still says the opposite.
    ///
    /// \[CYRUP-DELTA] pi's first statement is a runtime `typeof enabled !== "boolean"` guard
    /// returning an unchanged result with `"setYoloMode(enabled) requires a boolean value."`
    /// (`:1423-1430`). That branch is **unrepresentable in Rust**: `enabled` is typed `bool`, so no
    /// caller can reach it and there is nothing to check at runtime. It is not ported and no
    /// stand-in is invented for it — the compiler enforces the same precondition earlier and more
    /// completely. This is a language difference, not a dropped behaviour; the `error` field of
    /// [`YoloModeControlResult`] remains, because the persist-failure path (`:1449`) still uses it.
    pub fn set_yolo_mode(
        &self,
        enabled: bool,
        options: &YoloModeControlOptions,
    ) -> YoloModeControlResult {
        // pi `normalizePermissionSystemConfig({ ...extensionConfig, yoloMode: enabled })` (`:1432`).
        // Cloned out of the mutex first so nothing below runs while the live config is locked.
        let current = guard(&self.config).clone();
        let normalized = ExtensionConfig { yolo_mode: enabled, ..current.clone() }.normalized();
        // pi `const persisted = options.persist !== false` (`:1433`).
        let persisted = options.persists();
        // pi `const changed = extensionConfig.yoloMode !== normalized.yoloMode` (`:1434`).
        let changed = current.yolo_mode != normalized.yolo_mode;

        if persisted {
            // pi `const saved = savePermissionSystemConfig(normalized)` (`:1437`).
            let saved = normalized.save(&Self::config_path_for(&self.agent_dir));
            if !saved.success {
                // pi `saved.error ?? "Failed to persist pi-permission-system config."` (`:1439`).
                let error = saved
                    .error
                    .unwrap_or_else(|| YOLO_PERSIST_FALLBACK_ERROR.to_string());
                // pi `writeDebugEntry("yolo_mode.update_failed", {...})` (`:1440-1444`).
                self.write_debug_entry(
                    "yolo_mode.update_failed",
                    &json!({
                        "error": error,
                        "requestedYoloMode": normalized.yolo_mode,
                        "source": options.source_or_default(),
                    }),
                );
                // pi `:1445-1450`: `yoloMode: extensionConfig.yoloMode` — the UNCHANGED live value,
                // read fresh rather than reported from `normalized`.
                return YoloModeControlResult {
                    yolo_mode: guard(&self.config).yolo_mode,
                    changed: false,
                    persisted: false,
                    error: Some(error),
                };
            }
            // pi `lastConfigWarning = null` (`:1452`) — inside the `persisted` branch, so a
            // `persist: false` call deliberately leaves the memo alone (nothing was written).
            *guard(&self.last_config_warning) = None;
        }

        // pi `setExtensionConfig(normalized)` (`:1455`).
        *guard(&self.config) = normalized.clone();
        // pi `syncPermissionSystemStatusWhenPossible(normalized)` — no ctx (`:1456`).
        self.sync_status_when_possible(&normalized);
        // pi `writeDebugEntry("yolo_mode.updated", {...})` (`:1457-1462`).
        self.write_debug_entry(
            "yolo_mode.updated",
            &json!({
                "changed": changed,
                "persisted": persisted,
                "source": options.source_or_default(),
                "yoloMode": normalized.yolo_mode,
            }),
        );
        // pi `:1464-1468` — note `error` is absent, not `null`.
        YoloModeControlResult { yolo_mode: normalized.yolo_mode, changed, persisted, error: None }
    }

    /// pi `toggleYoloMode: (options?) => setYoloModeFromRuntimeApi(!extensionConfig.yoloMode,
    /// options)` (v0.8.0 `index.ts:1484`).
    pub fn toggle_yolo_mode(&self, options: &YoloModeControlOptions) -> YoloModeControlResult {
        self.set_yolo_mode(!self.yolo_mode(), options)
    }

    /// PERM-011 half A / pi `runtimeApi = registerPiPermissionSystemRuntimeApi({getYoloMode,
    /// setYoloMode, toggleYoloMode})` (`index.ts:1481-1485`): publish the three-method control
    /// surface on the process-global registry, and keep what the registry handed back so
    /// `session_shutdown` can retract exactly this registration (pi's module-scope `runtimeApi`,
    /// `:159`).
    ///
    /// Called from `init` — cyrup's analog of upstream's activation body, which is where the
    /// registration sits: AFTER the `enabled` early return (`:1475-1477`, cyrup's
    /// [`permission_extension_for_env`] returning `None`) and before any handler registration.
    ///
    /// A no-op when [`Self::self_ref`] was never installed, i.e. the extension was built by value
    /// rather than through [`Self::into_shared`]. That is the honest state and not a silent
    /// failure: an extension that was never activated has published nothing upstream either.
    pub(super) fn publish_runtime_api(&self) {
        let Some(weak) = self.self_ref.get() else { return };
        let api: Arc<dyn crate::runtime_api::PermissionSystemRuntimeApi> =
            Arc::new(PublishedRuntimeApi { ext: weak.clone() });
        *guard(&self.runtime_api) = Some(crate::runtime_api::register_runtime_api(api));
    }

    /// PERM-011 half A / pi `unregisterPiPermissionSystemRuntimeApi(runtimeApi ?? undefined);
    /// runtimeApi = null;` (`index.ts:1868-1870`), in `session_shutdown`.
    ///
    /// The handle is passed so the registry's identity guard can decline to clear a NEWER
    /// registration — the whole reason pi stores the returned object rather than calling the
    /// argumentless form.
    pub(super) fn retract_runtime_api(&self) {
        let published = guard(&self.runtime_api).take();
        crate::runtime_api::unregister_runtime_api(published.as_ref());
    }
}
