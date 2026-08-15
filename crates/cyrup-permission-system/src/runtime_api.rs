//! The PUBLISH SEAM for the yolo-mode control surface — a 1:1 port of
//! `pi-permission-system` v0.8.0 `src/yolo-mode-api.ts:19-43` (`registerPiPermissionSystemRuntimeApi`
//! / `unregisterPiPermissionSystemRuntimeApi` / `getPiPermissionSystemRuntimeApi`), PERM-011 half A.
//!
//! **What upstream actually does, and why this file exists.** pi publishes a three-method object on
//! `globalThis.__piPermissionSystem` (`yolo-mode-api.ts:20-22, 26-29`), reads it back with
//! `getPiPermissionSystemRuntimeApi()` (`:41-43`), and deletes it on `session_shutdown` — but only
//! if the slot still holds the same object it registered (`:33-38`). That is a PROCESS-GLOBAL
//! SINGLE SLOT holding a vtable, published by one extension and read by any other code in the
//! process.
//!
//! The in-tree note that used to sit on [`crate::yolo_api`] said this was unportable because "cyrup
//! has no such realm". That was wrong about the mechanism: `globalThis` is JavaScript's spelling of
//! *a process-global*, and Rust has one — a `static`. Every consumer of this API in cyrup (a second
//! native extension, a mode entry point, a front-end) is in the SAME process, exactly as pi's
//! consumers are in the same realm. So the registry is a `static` here, registered and unregistered
//! at pi's own two points, with pi's identity-guarded delete.
//!
//! \[CYRUP-DELTA] — **the slot holds a `Weak`-backed handle, so it cannot resurrect a dead
//! session.** pi's registered value is a bag of closures capturing module-scope `let` bindings, and
//! the global keeps them alive until `session_shutdown` deletes the key; a caller that somehow got
//! the object after teardown would still see the last config. cyrup's handle borrows the extension
//! through a [`std::sync::Weak`] ([`crate::extension::PermissionSystemExtension::into_shared`]
//! installs it), so if the extension has been dropped the three methods report
//! [`EXTENSION_GONE_ERROR`] instead of answering from a stale copy. The registration/unregistration
//! POINTS are pi's; only the failure mode of a call that arrives after the extension is gone
//! differs, and pi's alternative there is to lie.

use std::sync::{Arc, Mutex};

use crate::yolo_api::{YoloModeControlOptions, YoloModeControlResult};

/// The error a control call reports when the registered extension has already been dropped. See
/// the CYRUP-DELTA in this module's docs — upstream has no analog because a JS closure keeps its
/// captured state alive forever.
pub const EXTENSION_GONE_ERROR: &str = "The permission-system extension is no longer loaded.";

/// pi `PiPermissionSystemRuntimeApi` (`yolo-mode-api.ts:13-17`) — the three methods, and only the
/// three methods, that upstream publishes.
pub trait PermissionSystemRuntimeApi: Send + Sync {
    /// pi `getYoloMode()` (`yolo-mode-api.ts:14`, bound to `() => extensionConfig.yoloMode` at
    /// `index.ts:1482`).
    fn get_yolo_mode(&self) -> bool;

    /// pi `setYoloMode(enabled, options?)` (`yolo-mode-api.ts:15`, bound to
    /// `setYoloModeFromRuntimeApi` at `index.ts:1483`).
    fn set_yolo_mode(
        &self,
        enabled: bool,
        options: &YoloModeControlOptions,
    ) -> YoloModeControlResult;

    /// pi `toggleYoloMode(options?)` (`yolo-mode-api.ts:16`, bound at `index.ts:1484` to
    /// `setYoloModeFromRuntimeApi(!extensionConfig.yoloMode, options)`).
    fn toggle_yolo_mode(&self, options: &YoloModeControlOptions) -> YoloModeControlResult;
}

/// `globalThis.__piPermissionSystem` (`yolo-mode-api.ts:20-22`): ONE slot, process-wide, holding at
/// most one published API.
///
/// `Mutex::new` is `const`, so no lazy initialization is needed and the slot exists before any
/// extension loads — the same "the property is simply absent until someone sets it" shape as the
/// JS global.
static RUNTIME_API: Mutex<Option<Arc<dyn PermissionSystemRuntimeApi>>> = Mutex::new(None);

/// Lock the slot, recovering from poison rather than panicking (the crate's no-panic policy; the
/// same recovery [`crate::extension::guard`] uses).
fn slot() -> std::sync::MutexGuard<'static, Option<Arc<dyn PermissionSystemRuntimeApi>>> {
    RUNTIME_API.lock().unwrap_or_else(|e| e.into_inner())
}

/// pi `registerPiPermissionSystemRuntimeApi(api)` (`yolo-mode-api.ts:23-29`): overwrite the slot
/// and hand the caller back the same object, so it can pass it to [`unregister_runtime_api`]
/// later — which is exactly what `index.ts:1481` (`runtimeApi = register…`) does with it.
pub fn register_runtime_api(
    api: Arc<dyn PermissionSystemRuntimeApi>,
) -> Arc<dyn PermissionSystemRuntimeApi> {
    *slot() = Some(Arc::clone(&api));
    api
}

/// pi `unregisterPiPermissionSystemRuntimeApi(api?)` (`yolo-mode-api.ts:31-38`).
///
/// The identity guard is the whole function: `if (api !== undefined && global !== undefined &&
/// global !== api) { return; }` — a late shutdown from session A must NOT delete the registration
/// session B has already installed over it. `Arc::ptr_eq` is the `!==` of `dyn` handles; note it is
/// deliberately compared on the fat pointer's DATA half only, which is what identity means here
/// (two `Arc` clones of the same object).
pub fn unregister_runtime_api(api: Option<&Arc<dyn PermissionSystemRuntimeApi>>) {
    let mut slot = slot();
    if let (Some(api), Some(current)) = (api, slot.as_ref())
        && !std::ptr::addr_eq(Arc::as_ptr(current), Arc::as_ptr(api))
    {
        return;
    }
    *slot = None;
}

/// pi `getPiPermissionSystemRuntimeApi()` (`yolo-mode-api.ts:40-43`): the published API, or `None`
/// when nothing is registered (upstream's `?? null`).
///
/// This is the seam PERM-011 half A was missing. A second native extension, a mode entry point or a
/// front-end reads yolo mode — and flips it — through here, without holding the extension.
#[must_use]
pub fn runtime_api() -> Option<Arc<dyn PermissionSystemRuntimeApi>> {
    slot().clone()
}

/// The slot is process-global, exactly as `globalThis.__piPermissionSystem` is, so every test that
/// touches it — here and in [`crate::extension`] — must serialize on this ONE lock. Same reasoning
/// as `ext_config::env_lock` (PERM-020): the crate's unit tests all run as threads in a single
/// process, and a global is only isolated while one thread owns it.
#[cfg(test)]
pub(crate) fn test_registry_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A stand-in publisher: the registry must not know anything about the extension.
    struct FakeApi {
        yolo: AtomicBool,
    }

    impl FakeApi {
        fn arc(yolo: bool) -> Arc<dyn PermissionSystemRuntimeApi> {
            Arc::new(Self { yolo: AtomicBool::new(yolo) })
        }
    }

    impl PermissionSystemRuntimeApi for FakeApi {
        fn get_yolo_mode(&self) -> bool {
            self.yolo.load(Ordering::SeqCst)
        }
        fn set_yolo_mode(
            &self,
            enabled: bool,
            _options: &YoloModeControlOptions,
        ) -> YoloModeControlResult {
            let changed = self.yolo.swap(enabled, Ordering::SeqCst) != enabled;
            YoloModeControlResult { yolo_mode: enabled, changed, persisted: false, error: None }
        }
        fn toggle_yolo_mode(&self, options: &YoloModeControlOptions) -> YoloModeControlResult {
            self.set_yolo_mode(!self.get_yolo_mode(), options)
        }
    }

    use super::test_registry_lock as registry_lock;

    /// PERM-011 half A. Before this module existed there was NO seam at all: a second extension
    /// could not reach `yolo_mode`/`set_yolo_mode`/`toggle_yolo_mode` under any spelling, which is
    /// what the item means by "the ported methods are dead code that reads as done".
    #[test]
    fn a_published_api_is_readable_and_writable_by_a_holder_of_nothing_else() {
        let _lock = registry_lock();
        assert!(runtime_api().is_none(), "nothing registered yet");

        let handle = register_runtime_api(FakeApi::arc(false));

        // The consumer's whole view of the world: a module-path call, no extension in hand.
        let api = runtime_api().expect("published");
        assert!(!api.get_yolo_mode());
        let result = api.toggle_yolo_mode(&YoloModeControlOptions::from_source("second-extension"));
        assert!(result.yolo_mode && result.changed);
        assert!(runtime_api().expect("still published").get_yolo_mode());

        unregister_runtime_api(Some(&handle));
        assert!(runtime_api().is_none(), "shutdown must clear the slot");
    }

    /// pi's identity guard (`yolo-mode-api.ts:33-37`): session A's late `session_shutdown` must not
    /// delete the registration session B installed over it. Without the guard this test sees
    /// `None` and B's consumers silently lose the API.
    #[test]
    fn a_stale_unregister_does_not_delete_a_newer_registration() {
        let _lock = registry_lock();
        let first = register_runtime_api(FakeApi::arc(false));
        let second = register_runtime_api(FakeApi::arc(true));

        unregister_runtime_api(Some(&first));
        let api = runtime_api().expect("the newer registration must survive");
        assert!(api.get_yolo_mode(), "and it must be the SECOND one");

        unregister_runtime_api(Some(&second));
        assert!(runtime_api().is_none());
    }

    /// pi `unregisterPiPermissionSystemRuntimeApi()` with no argument deletes unconditionally
    /// (`:33` — the guard's first conjunct is `api !== undefined`).
    #[test]
    fn an_argumentless_unregister_clears_whatever_is_there() {
        let _lock = registry_lock();
        let _handle = register_runtime_api(FakeApi::arc(true));
        unregister_runtime_api(None);
        assert!(runtime_api().is_none());
    }
}
