//! pi's `notifyWarning` + `shownWarnings` pair and the one [`PermissionManager`] factory bound to
//! it, so no construction site can silently drop a policy-load warning.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use cyrup_ext::{HostServices, NotifyKind};

use crate::manager::{ManagerPaths, PermissionManager};

use super::guard;

/// pi's `notifyWarning` + `shownWarnings` pair (`index.ts:1573,1586-1592`): the ONE user-visible
/// sink every policy-file / config-file load warning funnels into, deduped for the life of a
/// session so a per-tool-call reload storm cannot spam the same message.
///
/// Before this existed, [`PermissionManager::with_on_warning`] was called only from unit tests, so
/// in production a malformed `cyrup-permissions.jsonc` fell back to `ask`-everything **in total
/// silence** — indistinguishable from a policy that genuinely says `ask`.
///
/// Holds the SAME late-bound `Arc<OnceLock<Arc<dyn HostServices>>>` the extension does, so a
/// manager built during construction (before the host attaches its backend) still delivers once
/// the backend lands — that late binding is why this is a shared handle and not a captured
/// `Arc<dyn HostServices>`.
pub(super) struct WarningSink {
    host_services: Arc<OnceLock<Arc<dyn HostServices>>>,
    /// pi `shownWarnings` (`index.ts:1573`).
    shown: Mutex<HashSet<String>>,
}

impl WarningSink {
    pub(super) fn new(host_services: Arc<OnceLock<Arc<dyn HostServices>>>) -> Self {
        Self {
            host_services,
            shown: Mutex::new(HashSet::new()),
        }
    }

    /// pi `notifyWarning` (`index.ts:1586-1592`): drop a message already shown this session, else
    /// remember it and push it to the host as a `warning` notification.
    ///
    /// \[CYRUP-DELTA] pi's guard is `!runtimeContext?.hasUI` — two conditions rolled into one,
    /// because pi's `ctx.ui.notify` is only reachable through a live context. Cyrup splits those:
    /// "is a host backend attached at all" is `host_services.get()`, which is the direct analog of
    /// pi's `runtimeContext != null` and is what is checked here. The `hasUI` half is NOT
    /// re-imposed: cyrup's [`HostServices::notify`] is already a fire-and-forget effect whose
    /// default implementation is a no-op and whose live implementation routes to whatever sink the
    /// active mode installed, so a headless host drops it on its own — and re-adding the check
    /// would suppress the warning in modes (e.g. RPC) that DO surface notifications.
    pub(super) fn notify(&self, message: &str) {
        let Some(services) = self.host_services.get() else {
            return;
        };
        if !guard(&self.shown).insert(message.to_string()) {
            return;
        }
        services.notify(message, NotifyKind::Warning);
    }

    /// pi `resetShownWarnings` (`index.ts:1582-1584`), called on session start / reload / shutdown.
    pub(super) fn reset(&self) {
        guard(&self.shown).clear();
    }
}

/// Build a [`PermissionManager`] whose `onWarning` is bound to `sink` — the analog of pi's
/// `createPermissionManagerForCwd(cwd, notifyWarning)` (`index.ts:1536-1550`), which likewise
/// threads the callback through EVERY construction site (`:1595`, `:2081`, `:2109-2110`). This is
/// the only way this crate builds a manager, so no construction site can silently drop policy-load
/// warnings again.
pub(super) fn manager_with_warnings(
    paths: ManagerPaths,
    sink: &Arc<WarningSink>,
) -> PermissionManager {
    let sink = Arc::clone(sink);
    PermissionManager::new(paths).with_on_warning(move |message| sink.notify(message))
}
