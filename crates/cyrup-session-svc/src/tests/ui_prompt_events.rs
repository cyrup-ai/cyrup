//! EXT-075 — an extension's blocking UI prompt emits `ui_prompt_start` / `ui_prompt_end` to every
//! other extension.
//!
//! pi `ExtensionRunner.setUIContext` wraps the one `ExtensionUIContext` every extension shares
//! (`core/extensions/runner.ts:522-537` @v0.87.1), so each of `select`/`confirm`/`input`/`editor`/
//! `custom` runs through `withUIPrompt` (`:539-566`): a runner-wide depth counter, `ui_prompt_start`
//! on the outermost call only, `ui_prompt_end` once it settles, carrying the OUTER prompt's
//! `kind`/`title`. Without a UI the runner keeps `noOpUIContext` unwrapped (`:523`) and emits
//! nothing.
//!
//! cyrup's shared context is the session's `LiveHostServices`, and a native extension prompts
//! through it directly. These tests drive the backend the builder hands a native
//! (`NativeExtension::set_host_services`) and observe the events at a second native subscriber —
//! before EXT-075 neither event existed, so nothing was ever delivered.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{SessionBuilder, SessionConfig};
use cyrup_core::ExtensionId;
use cyrup_ext::{
    DialogOptions, EventKind, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi,
    NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use serde_json::json;
use tempfile::TempDir;

/// Records every prompt event it is delivered, and keeps the backend the builder bound into it so
/// the test can prompt exactly as a native's own handler would.
#[derive(Default)]
struct Watcher {
    events: Mutex<Vec<String>>,
    services: Mutex<Option<Arc<dyn HostServices>>>,
}

#[async_trait::async_trait]
impl NativeExtension for Watcher {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("ui-prompt-watcher")
    }
    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        *self.services.lock().unwrap() = Some(services);
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::UiPromptStart, EventKind::UiPromptEnd]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        let line = match ev {
            HostEvent::UiPromptStart { kind, title } => format!("start {kind} {title:?}"),
            HostEvent::UiPromptEnd { kind, title } => format!("end {kind} {title:?}"),
            other => format!("unexpected {:?}", other.kind()),
        };
        self.events.lock().unwrap().push(line);
        HookOutcome::Noop
    }
}

struct Harness {
    _tmp: TempDir,
    watcher: Arc<Watcher>,
    // Holds the extension host (and so the dispatcher the tracker delivers through) alive.
    _session: crate::AgentSession,
}

async fn harness(app_mode: cyrup_config::AppMode) -> Harness {
    let tmp = TempDir::new().unwrap();
    let cwd: PathBuf = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg.app_mode = app_mode;
    let watcher = Arc::new(Watcher::default());
    let session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .with_native_extension(watcher.clone() as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("build");
    Harness {
        _tmp: tmp,
        watcher,
        _session: session,
    }
}

impl Harness {
    fn services(&self) -> Arc<dyn HostServices> {
        self.watcher
            .services
            .lock()
            .unwrap()
            .clone()
            .expect("bound")
    }

    /// Run a blocking prompt call the way a native's handler does — synchronously, on a runtime
    /// thread — then wait for `want` events to be delivered.
    async fn prompt(&self, call: impl FnOnce(&dyn HostServices) + Send + 'static, want: usize) {
        let services = self.services();
        tokio::task::spawn_blocking(move || call(services.as_ref()))
            .await
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while self.watcher.events.lock().unwrap().len() < want {
            assert!(
                tokio::time::Instant::now() < deadline,
                "only {:?} delivered",
                self.watcher.events.lock().unwrap()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn events(&self) -> Vec<String> {
        self.watcher.events.lock().unwrap().clone()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_blocking_prompt_kind_emits_one_start_then_one_end() {
    let h = harness(cyrup_config::AppMode::Interactive).await;
    h.prompt(
        |s| {
            s.select("Pick one", &json!(["a", "b"]), &DialogOptions::default());
        },
        2,
    )
    .await;
    h.prompt(
        |s| {
            s.confirm("Sure?", "really", &DialogOptions::default());
        },
        4,
    )
    .await;
    h.prompt(
        |s| {
            s.input("Name", None, &DialogOptions::default());
        },
        6,
    )
    .await;
    h.prompt(
        |s| {
            s.editor("Edit", "draft");
        },
        8,
    )
    .await;
    assert_eq!(
        h.events(),
        vec![
            r#"start select Some("Pick one")"#,
            r#"end select Some("Pick one")"#,
            r#"start confirm Some("Sure?")"#,
            r#"end confirm Some("Sure?")"#,
            r#"start input Some("Name")"#,
            r#"end input Some("Name")"#,
            r#"start editor Some("Edit")"#,
            r#"end editor Some("Edit")"#,
        ]
    );
}

/// pi's depth rule: `custom` reaches the overlay route, which is itself a wrapped prompt — one pair,
/// carrying `custom` and no title (`withUIPrompt("custom", undefined, …)`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_nested_prompt_emits_one_pair_for_the_outer_call() {
    let h = harness(cyrup_config::AppMode::Interactive).await;
    h.prompt(
        |s| {
            s.custom(&json!({"title": "Menu", "options": ["x"]}));
        },
        2,
    )
    .await;
    // Nothing else is coming: give a stray inner pair the chance to land before asserting.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.events(), vec!["start custom None", "end custom None"]);
}

/// pi keeps `noOpUIContext` unwrapped without a UI (`runner.ts:523`): print mode emits nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_ui_no_prompt_event_is_emitted() {
    let h = harness(cyrup_config::AppMode::Print).await;
    h.prompt(
        |s| {
            s.select("Pick one", &json!(["a"]), &DialogOptions::default());
        },
        0,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(h.events().is_empty(), "{:?}", h.events());
}
