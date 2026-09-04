//! Fixtures shared by two or more of the sibling test modules: the [`HostServices`] doubles, the
//! event/context builders, and the runtime + env-pin wrappers the config-touching tests use.
//!
//! These, and the tests across this directory that use them, are the Wave1b pi-parity regression
//! suite (dossier: `cyrup-permission-system/src/extension/`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::json;

use cyrup_ext::{HostCtx, HostEvent, HostServices, InitApi, NativeExtension, NotifyKind};

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

pub(super) async fn init_ext(ext: &PermissionSystemExtension) {
    let mut api = InitApi::new();
    ext.init(&mut api).await.unwrap();
}

/// Drive `body` on a CURRENT-THREAD runtime, from a synchronous frame.
///
/// Formerly `with_config_env_lock`, which additionally held `ext_config::env_lock` so that
/// `ExtensionConfig::resolve_config_path`'s read of `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` could not
/// observe a sibling test's process-wide `set_var` of it. There is no longer a process-env mutation
/// anywhere in the crate for that lock to serialize: an override is pinned THREAD-LOCALLY through
/// [`crate::envx`], which no other test's thread can see. The runtime flavour is unchanged, and it
/// is load-bearing for the pins — a thread-local pin does not reach a multi-thread worker.
pub(super) fn block_on<F: std::future::Future>(body: F) -> F::Output {
    #[allow(clippy::unwrap_used)]
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(body)
}

pub(super) fn bash_call(call_id: &str) -> HostEvent {
    HostEvent::ToolCall {
        call_id: cyrup_core::ToolCallId::from(call_id),
        name: "bash".to_string(),
        input: json!({ "command": "echo hi" }),
    }
}

/// Run `body` with [`INSTALL_ENV_VAR`] pinned UNSET for this thread only.
///
/// The pin is a thread-local overlay in [`crate::envx`], not a process mutation: nothing another
/// test can observe changes, so no lock is taken and none is needed. The previous implementation
/// held `ext_config::env_lock` around an `unsafe { std::env::remove_var }`, which serialized the
/// crate's eight env WRITERS but not its sixteen unlocked READERS — and in edition 2024 a `getenv`
/// concurrent with `unsetenv` is undefined behaviour, which no writer-side lock can repair.
pub(super) fn without_install_env<T>(body: impl FnOnce() -> T) -> T {
    let _pin = crate::envx::pin(INSTALL_ENV_VAR, None);
    body()
}

/// A [`HostServices`] double that enumerates a registry (so the unknown-tool gate lets the call
/// through to the policy engine) AND records every `notify` the extension pushes at the host.
pub(super) struct NotifyRecorder {
    pub(super) names: Vec<String>,
    pub(super) notifications: Mutex<Vec<(String, NotifyKind)>>,
}

impl NotifyRecorder {
    pub(super) fn new() -> Self {
        Self {
            names: vec!["bash".to_string()],
            notifications: Mutex::new(Vec::new()),
        }
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
