//! Port of `pi-permission-system` v0.8.0 `src/yolo-mode-api.ts` — the yolo-mode CONTROL surface's
//! option and result shapes, consumed by
//! [`crate::extension::PermissionSystemExtension::set_yolo_mode`] /
//! [`crate::extension::PermissionSystemExtension::toggle_yolo_mode`] (pi
//! `setYoloModeFromRuntimeApi`, `index.ts:1422-1469`).
//!
//! **What is NOT ported, and why.** Upstream's `registerPiPermissionSystemRuntimeApi` /
//! `unregisterPiPermissionSystemRuntimeApi` / `getPiPermissionSystemRuntimeApi`
//! (`yolo-mode-api.ts:23-43`) publish the three-method `PiPermissionSystemRuntimeApi` object on
//! `globalThis.__piPermissionSystem`, because in pi every extension shares ONE JavaScript realm and
//! a mutable global is the only cross-extension handle available. cyrup has no such realm: a native
//! built-in is an `Arc<dyn NativeExtension>` the host owns, and
//! [`crate::permission_extension_for_env`] hands the binary a `dyn` handle whose concrete type is
//! erased. So the three methods are ported as INHERENT methods on the extension —
//! [`crate::extension::PermissionSystemExtension::yolo_mode`] (pi `getYoloMode`, `:1482`),
//! `set_yolo_mode` (`:1483`) and `toggle_yolo_mode` (`:1484`).
//!
//! **PERM-011 — correction to what this doc used to claim.** It said the three methods are "reached
//! through the `/permission-system` command". They are NOT, and deliberately so: the command routes
//! BOTH of its rows through `save_extension_config`, matching upstream, where `setConfig` is
//! `saveExtensionConfig` (`index.ts:1508`) and the runtime API is a SEPARATE surface
//! (`index.ts:1481-1485`). Routing the yolo row through `set_yolo_mode` would emit
//! `yolo_mode.updated` where pi emits `config.saved`. So at HEAD these three methods have exactly
//! one in-crate caller (`toggle_yolo_mode` → `set_yolo_mode`) and no external one.
//!
//! What is missing is the PUBLISH SEAM, not the methods: `cyrup-ext` has no
//! extension-provided-API registry for one native extension to call another's methods, which is
//! the direct analog of `globalThis.__piPermissionSystem`. Until that lands in `cyrup-ext`, no
//! other extension or front-end can read or flip yolo mode. Tracked as PERM-011 half A; the
//! permission-request EVENT channel (`PERMISSION_REQUEST_EVENT_CHANNEL`, `index.ts:150`, emitted at
//! `:1606`/`:1612`/`:1626`) is half B and needs the same crate's bus accessor.

/// pi `getNonEmptyString(options.source) ?? "runtime-api"` (`index.ts:1443,1460`): the `source`
/// recorded in the `yolo_mode.*` debug entries when the caller supplies none.
pub const DEFAULT_YOLO_CONTROL_SOURCE: &str = "runtime-api";

/// pi `YoloModeControlOptions` (`yolo-mode-api.ts:1-4`).
///
/// Both fields are `Option` for the same reason both are `?:` upstream: "absent" and "present"
/// are distinguishable, and the `persist` check keys on that distinction — see [`Self::persists`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct YoloModeControlOptions {
    /// pi `persist?: boolean` (`yolo-mode-api.ts:2`).
    pub persist: Option<bool>,
    /// pi `source?: string` (`yolo-mode-api.ts:3`) — a free-form caller label recorded in the
    /// `yolo_mode.updated` / `yolo_mode.update_failed` debug entries.
    pub source: Option<String>,
}

impl YoloModeControlOptions {
    /// pi `const persisted = options.persist !== false` (`index.ts:1433`): ONLY the literal `false`
    /// suppresses the write. Absent — and, upstream, any other value — still persists.
    #[must_use]
    pub fn persists(&self) -> bool {
        self.persist != Some(false)
    }

    /// pi `getNonEmptyString(options.source) ?? "runtime-api"` (`index.ts:1443,1460`, over
    /// `common.ts:15-22`): a TRIMMED non-empty `source`, else [`DEFAULT_YOLO_CONTROL_SOURCE`].
    #[must_use]
    pub fn source_or_default(&self) -> String {
        self.source
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map_or_else(|| DEFAULT_YOLO_CONTROL_SOURCE.to_string(), ToString::to_string)
    }

    /// The in-memory-only variant (pi `{ persist: false }`).
    #[must_use]
    pub fn transient(source: impl Into<String>) -> Self {
        Self { persist: Some(false), source: Some(source.into()) }
    }

    /// The persisting variant with an explicit caller label (pi `{ source }`).
    #[must_use]
    pub fn from_source(source: impl Into<String>) -> Self {
        Self { persist: None, source: Some(source.into()) }
    }
}

/// pi `YoloModeControlResult` (`yolo-mode-api.ts:6-11`).
///
/// The load-bearing invariant, which [`crate::extension::PermissionSystemExtension::set_yolo_mode`]
/// upholds and `tests/config_command.rs` pins: when a requested persist FAILS, `yolo_mode` reports
/// the UNCHANGED in-memory value, `changed`/`persisted` are both `false`, and `error` is set (pi
/// `index.ts:1438-1451`). A caller must never be told yolo mode changed when the gate's live config
/// still says otherwise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YoloModeControlResult {
    /// pi `yoloMode` (`yolo-mode-api.ts:7`): the yolo mode now in effect.
    pub yolo_mode: bool,
    /// pi `changed` (`:8`): whether this call moved the value.
    pub changed: bool,
    /// pi `persisted` (`:9`): whether the value was written to `config.json`.
    pub persisted: bool,
    /// pi `error?` (`:10`).
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    /// pi `options.persist !== false` (`index.ts:1433`) — absent persists, `true` persists, only
    /// `false` does not.
    #[test]
    fn only_an_explicit_false_suppresses_the_write() {
        assert!(YoloModeControlOptions::default().persists());
        assert!(YoloModeControlOptions { persist: Some(true), source: None }.persists());
        assert!(!YoloModeControlOptions { persist: Some(false), source: None }.persists());
        assert!(!YoloModeControlOptions::transient("t").persists());
        assert!(YoloModeControlOptions::from_source("s").persists());
    }

    /// pi `getNonEmptyString(options.source) ?? "runtime-api"` (`index.ts:1443,1460`).
    #[test]
    fn source_falls_back_to_runtime_api_for_absent_empty_and_blank() {
        assert_eq!(YoloModeControlOptions::default().source_or_default(), "runtime-api");
        assert_eq!(YoloModeControlOptions::from_source("").source_or_default(), "runtime-api");
        assert_eq!(YoloModeControlOptions::from_source("   ").source_or_default(), "runtime-api");
        assert_eq!(YoloModeControlOptions::from_source("  cli  ").source_or_default(), "cli");
    }
}
