//! Port of pi `status.ts` — the single status pill the extension surfaces: a `"yolo"` segment keyed
//! by [`EXTENSION_ID`] when `yoloMode` is on, cleared otherwise. Driven through the live UI-effect
//! seam [`cyrup_ext::HostServices::set_status`] (pi `ui.setStatus(key, text?)`), synced at
//! `session_start`/`before_agent_start` and cleared at `session_shutdown` (pi `index.ts:2122`),
//! from `extension/config.rs` and `extension/native.rs`.

use std::sync::Arc;

use cyrup_ext::HostServices;

use crate::ext_config::ExtensionConfig;
use crate::extension::EXTENSION_ID;

/// pi `PERMISSION_SYSTEM_YOLO_STATUS_VALUE` (`status.ts:7`).
pub const YOLO_STATUS_VALUE: &str = "yolo";

/// pi `getPermissionSystemStatus` (`status.ts:11-13`): `Some("yolo")` when yolo is on, else `None`.
#[must_use]
pub fn permission_system_status(config: &ExtensionConfig) -> Option<&'static str> {
    if config.yolo_mode {
        Some(YOLO_STATUS_VALUE)
    } else {
        None
    }
}

/// pi `syncPermissionSystemStatus` (`status.ts:15-20`): set (or clear) the `"yolo"` pill on the live
/// UI keyed by [`EXTENSION_ID`], reflecting the current `yoloMode`.
pub fn sync_status(services: &Arc<dyn HostServices>, config: &ExtensionConfig) {
    services.set_status(EXTENSION_ID, permission_system_status(config));
}

/// pi teardown (`index.ts:2122`): clear the pill on shutdown (`ui.setStatus(KEY, undefined)`).
pub fn clear_status(services: &Arc<dyn HostServices>) {
    services.set_status(EXTENSION_ID, None);
}
