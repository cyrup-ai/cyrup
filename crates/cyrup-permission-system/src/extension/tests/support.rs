//! Fixtures shared by two or more of the sibling test modules: the [`HostServices`] doubles, the
//! event/context builders, and the two env-lock wrappers every config-touching test must hold.
//!
//! These, and the tests across this directory that use them, are the Wave1b pi-parity regression
//! suite (dossier: `cyrup-permission-system/src/extension/`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::json;

use cyrup_ext::{HostCtx, HostCtxRich, HostEvent, HostServices, InitApi, NativeExtension, NotifyKind};

use crate::extension::{INSTALL_ENV_VAR, PermissionSystemExtension, guard};

/// A scripted [`HostServices`] whose ONLY override is `all_tool_names` — the full registry the
/// registry / unknown-tool gate checks against (pi `pi.getAllTools()`). Mirrors the identical
/// helper in `tests/layers_wired.rs`.
pub(super) struct FakeRegistry {
    pub(super) names: Vec<String>,
}
impl HostServices for FakeRegistry {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(self.names.clone())
    }
}

pub(super) fn write_file(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

pub(super) fn event_ctx(cwd: PathBuf) -> HostCtx {
    HostCtx::event(cyrup_ext::ExtMode::Print, false, cwd)
}

/// [`event_ctx`] with project trust GRANTED — pi `ctx.isProjectTrusted() === true`.
///
/// A hand-built [`HostCtx`] carries [`HostCtxRich::default()`], i.e. `is_project_trusted = false`,
/// so any test whose subject is PROJECT-scoped policy has to say so explicitly now that
/// [`PermissionSystemExtension::project_trusted`] withholds that scope from an untrusted project
/// (pi #644). In production the flag arrives from the `HostCtxSource` that
/// `ExtensionHost::load_native_with_services` attaches alongside the backend; these tests wire
/// `set_host_services` by hand and never attach one, so they supply it here.
pub(super) fn trusted_event_ctx(cwd: PathBuf) -> HostCtx {
    event_ctx(cwd).with_rich(HostCtxRich { is_project_trusted: true, ..HostCtxRich::default() })
}

pub(super) async fn init_ext(ext: &PermissionSystemExtension) {
    let mut api = InitApi::new();
    ext.init(&mut api).await.unwrap();
}

/// Drive `body` to completion with the crate-wide env lock held for the WHOLE test.
///
/// Any test that asserts on config it wrote into its own tempdir must take this lock.
/// `ExtensionConfig::load` resolves its path through `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH`
/// first ([`crate::ext_config::ExtensionConfig::resolve_config_path`]), and
/// `ext_config::tests::env_var_overrides_default_config_path` sets that variable PROCESS-WIDE
/// while it runs. A concurrent test then loads the OTHER test's fixture instead of its own and
/// fails on an assertion that has nothing to do with the code under test. Measured on this
/// binary: 8 failures in 300 runs before this guard, 0 in 300 after; and exporting the variable
/// by hand reproduces the same failure 100% of the time.
///
/// The lock is `crate::ext_config::env_lock` — the same one the mutator holds — and it is taken
/// in a SYNCHRONOUS frame around `block_on` rather than inside an `async` test body, so the
/// guard is never held across an `.await` point. That frame is
/// [`crate::ext_config::with_env_lock`]; this is the local spelling of it.
pub(super) fn with_config_env_lock<F: std::future::Future>(body: F) -> F::Output {
    crate::ext_config::with_env_lock(body)
}

pub(super) fn bash_call(call_id: &str) -> HostEvent {
    HostEvent::ToolCall {
        call_id: cyrup_core::ToolCallId::from(call_id),
        name: "bash".to_string(),
        input: json!({ "command": "echo hi" }),
    }
}

/// Run `body` with [`INSTALL_ENV_VAR`] guaranteed unset, restoring the ambient value after —
/// serialized against every other env-mutating test in the crate by the shared
/// [`crate::ext_config::env_lock`].
pub(super) fn without_install_env<T>(body: impl FnOnce() -> T) -> T {
    let _lock = crate::ext_config::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = std::env::var(INSTALL_ENV_VAR).ok();
    // SAFETY: serialized by `env_lock`, and restored below before the guard drops.
    unsafe { std::env::remove_var(INSTALL_ENV_VAR) };
    let out = body();
    // SAFETY: same scope/serialization; restores whatever the ambient shell had.
    unsafe {
        match previous {
            Some(v) => std::env::set_var(INSTALL_ENV_VAR, v),
            None => std::env::remove_var(INSTALL_ENV_VAR),
        }
    }
    out
}

/// A [`HostServices`] double that enumerates a registry (so the unknown-tool gate lets the call
/// through to the policy engine) AND records every `notify` the extension pushes at the host.
pub(super) struct NotifyRecorder {
    pub(super) names: Vec<String>,
    pub(super) notifications: Mutex<Vec<(String, NotifyKind)>>,
}

impl NotifyRecorder {
    pub(super) fn new() -> Self {
        Self { names: vec!["bash".to_string()], notifications: Mutex::new(Vec::new()) }
    }
    pub(super) fn warnings(&self) -> Vec<String> {
        guard(&self.notifications)
            .iter()
            .filter(|(_, kind)| *kind == NotifyKind::Warning)
            .map(|(message, _)| message.clone())
            .collect()
    }
}

impl HostServices for NotifyRecorder {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(self.names.clone())
    }
    fn notify(&self, message: &str, kind: NotifyKind) {
        guard(&self.notifications).push((message.to_string(), kind));
    }
}

/// Records `set_active_tools` and `set_status` so the cache's call COUNTS can be asserted, and
/// enumerates a fixed registry so `should_expose_tool` has something to filter.
pub(super) struct LifecycleRecorder {
    pub(super) names: Vec<String>,
    pub(super) active_tools: Mutex<Vec<Vec<String>>>,
    pub(super) statuses: Mutex<Vec<Option<String>>>,
}

impl LifecycleRecorder {
    pub(super) fn new() -> Self {
        Self {
            names: vec!["bash".to_string(), "read".to_string()],
            active_tools: Mutex::new(Vec::new()),
            statuses: Mutex::new(Vec::new()),
        }
    }
}

impl HostServices for LifecycleRecorder {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(self.names.clone())
    }
    fn set_active_tools(&self, tools: &[String]) {
        guard(&self.active_tools).push(tools.to_vec());
    }
    fn set_status(&self, _key: &str, value: Option<&str>) {
        guard(&self.statuses).push(value.map(str::to_string));
    }
}

pub(super) fn before_agent_start(prompt: &str) -> HostEvent {
    HostEvent::BeforeAgentStart {
        prompt: "hi".to_string(),
        images: json!([]),
        system_prompt: prompt.to_string(),
        options: json!({}),
        injected: Vec::new(),
    }
}

pub(super) fn ui_ctx(cwd: &Path) -> HostCtx {
    HostCtx::event(cyrup_ext::ExtMode::Tui, true, cwd.to_path_buf())
}

pub(super) fn headless_ctx(cwd: &Path) -> HostCtx {
    HostCtx::event(cyrup_ext::ExtMode::Tui, false, cwd.to_path_buf())
}
