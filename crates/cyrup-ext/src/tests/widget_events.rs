//! SUBA-210 / SUBA-211 — the live-widget seam: a widget's pointer and resize events reach the
//! extension that published it, and the terminal size reaches an extension at all.
//!
//! **Upstream.** pi's extension widget is a mounted COMPONENT (`ctx.ui.setWidget(key, factory)`),
//! so the TUI routes `handleMouse` to it and re-renders it on a resize;
//! `pi-subagents`' async-jobs card (`buildWidgetComponent`, `src/tui/render.ts:2911` @ad11b7ab) is
//! the consumer: `handleMouse` folds it (`:2938-2944`) and its render reads
//! `process.stdout.rows`/`columns` (`:2536-2547`). cyrup's `set_widget` carries LINES, so the host
//! records who published each key and routes the events back ([`crate::widget`]).
//!
//! Every test here is NEW API and cannot compile at the pre-change HEAD; each names the mutation of
//! the new code that turns it red.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::{Arc, Mutex, OnceLock};

use crate::host::{HostServices, RecordingServices, WidgetPlacement};
use crate::{
    ExtError, ExtMode, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent, InitApi,
    NativeExtension, PointerModifiers, TerminalSize, WidgetEvent, WidgetEventOutcome, WidgetMouse,
    WidgetPointer,
};
use cyrup_core::ExtensionId;

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

fn click(row: u16) -> WidgetEvent {
    WidgetEvent::Mouse(WidgetMouse {
        pointer: WidgetPointer::Click {
            column: 2,
            row,
            count: 1,
        },
        modifiers: PointerModifiers::default(),
    })
}

/// A native that publishes widgets through the services handle the host gave it, and records
/// every widget event it is handed.
struct CardOwner {
    id: ExtensionId,
    services: OnceLock<Arc<dyn HostServices>>,
    seen: Arc<Mutex<Vec<(String, WidgetEvent)>>>,
    answer: WidgetEventOutcome,
}

impl CardOwner {
    fn new(id: &str, answer: WidgetEventOutcome) -> Arc<Self> {
        Arc::new(Self {
            id: ExtensionId::from(id),
            services: OnceLock::new(),
            seen: Arc::new(Mutex::new(Vec::new())),
            answer,
        })
    }

    fn publish(&self, key: &str, lines: Option<&[String]>) {
        self.services
            .get()
            .expect("the host bound a services handle")
            .set_widget(key, lines, WidgetPlacement::AboveEditor);
    }

    fn seen(&self) -> Vec<(String, WidgetEvent)> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl NativeExtension for CardOwner {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, _api: &mut InitApi) -> Result<(), ExtError> {
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        let _ = self.services.set(services);
    }
    fn on_widget_event(&self, key: &str, event: WidgetEvent) -> WidgetEventOutcome {
        self.seen.lock().unwrap().push((key.to_string(), event));
        self.answer
    }
}

/// A native whose widget handler panics.
struct PanickingOwner(OnceLock<Arc<dyn HostServices>>);

#[async_trait::async_trait]
impl NativeExtension for PanickingOwner {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("panicky")
    }
    async fn init(&self, _api: &mut InitApi) -> Result<(), ExtError> {
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        let _ = self.0.set(services);
    }
    fn on_widget_event(&self, _key: &str, _event: WidgetEvent) -> WidgetEventOutcome {
        panic!("a broken widget handler");
    }
}

fn lines(text: &str) -> Vec<String> {
    vec![text.to_string()]
}

async fn host_with(owners: &[Arc<CardOwner>]) -> ExtensionHost {
    let host = ExtensionHost::new(cfg());
    let backend: Arc<dyn HostServices> = Arc::new(RecordingServices::new(
        crate::host::CannedResponses::default(),
    ));
    for owner in owners {
        host.load_native_with_services(owner.clone(), backend.clone())
            .await
            .unwrap();
    }
    host
}

/// The event reaches the extension that published the key, unchanged, and its answer comes back.
/// A key nobody published answers `Unhandled` (the click stays the text selection's).
///
/// RED mutation: `CallerScopedServices::set_widget` not recording the owner (no owner is found,
/// `Unhandled`, and the extension sees nothing).
#[tokio::test]
async fn a_widget_event_reaches_the_extension_that_published_the_key() {
    let a = CardOwner::new("a", WidgetEventOutcome::Handled);
    let host = host_with(std::slice::from_ref(&a)).await;
    a.publish("card", Some(&lines("card")));

    assert_eq!(host.widget_owner("card"), Some(ExtensionId::from("a")));
    assert_eq!(
        host.widget_event("card", click(0)).await,
        WidgetEventOutcome::Handled
    );
    assert_eq!(a.seen(), vec![("card".to_string(), click(0))]);

    assert_eq!(
        host.widget_event("nobody-published-this", click(0)).await,
        WidgetEventOutcome::Unhandled
    );
    assert_eq!(a.seen().len(), 1, "an unowned key reaches no extension");
}

/// pi's widget map is one shared map whose last write wins the key, and a removal unmounts it: the
/// last publisher answers, and after a removal nobody does.
///
/// RED mutation: `ExtensionRegistry::record_widget` ignoring removals (the removed widget's last
/// owner still answers).
#[tokio::test]
async fn the_last_publisher_owns_the_key_and_a_removal_forgets_it() {
    let a = CardOwner::new("a", WidgetEventOutcome::Handled);
    let b = CardOwner::new("b", WidgetEventOutcome::Handled);
    let host = host_with(&[a.clone(), b.clone()]).await;
    a.publish("shared", Some(&lines("a's")));
    b.publish("shared", Some(&lines("b's")));
    host.widget_event("shared", click(0)).await;
    assert!(
        a.seen().is_empty(),
        "the earlier publisher no longer owns the key"
    );
    assert_eq!(b.seen().len(), 1);

    b.publish("shared", None);
    assert_eq!(host.widget_owner("shared"), None);
    assert_eq!(
        host.widget_event("shared", click(0)).await,
        WidgetEventOutcome::Unhandled
    );
    assert_eq!(b.seen().len(), 1, "a removed widget is told nothing");
    assert!(a.seen().is_empty());
}

/// A purged extension answers for no widget it published, while a sibling's widget still routes.
///
/// RED mutation: the `widget_owner` line removed from `ExtensionRegistry::purge_in`.
#[tokio::test]
async fn a_purged_extension_answers_for_no_widget() {
    let a = CardOwner::new("a", WidgetEventOutcome::Handled);
    let b = CardOwner::new("b", WidgetEventOutcome::Handled);
    let host = host_with(&[a.clone(), b.clone()]).await;
    a.publish("a-card", Some(&lines("a")));
    b.publish("b-card", Some(&lines("b")));
    host.registry()
        .purge_owner(&ExtensionId::from("a"))
        .unwrap();
    assert_eq!(host.widget_owner("a-card"), None);
    assert_eq!(host.widget_owner("b-card"), Some(ExtensionId::from("b")));
}

/// A resize reaches every mounted widget's owner, once per widget it owns, with the new size.
///
/// RED mutation: `ExtensionHost::terminal_resized` with an empty body (nobody is told).
#[tokio::test]
async fn a_resize_reaches_each_mounted_widget_once() {
    let a = CardOwner::new("a", WidgetEventOutcome::Handled);
    let b = CardOwner::new("b", WidgetEventOutcome::Unhandled);
    let host = host_with(&[a.clone(), b.clone()]).await;
    a.publish("a1", Some(&lines("one")));
    a.publish("a2", Some(&lines("two")));
    b.publish("b1", Some(&lines("three")));
    a.publish("gone", Some(&lines("gone")));
    a.publish("gone", None);

    let size = TerminalSize {
        columns: 100,
        rows: 33,
    };
    host.terminal_resized(size).await;
    assert_eq!(
        a.seen(),
        vec![
            ("a1".to_string(), WidgetEvent::Resize(size)),
            ("a2".to_string(), WidgetEvent::Resize(size)),
        ]
    );
    assert_eq!(
        b.seen(),
        vec![("b1".to_string(), WidgetEvent::Resize(size))]
    );
}

/// A panicking widget handler is contained and answers `Unhandled`: the click stays the
/// selection's and the host keeps routing.
///
/// RED mutation: the `catch_unwind` in `ExtensionHost::widget_event` removed (the test panics).
#[tokio::test]
async fn a_panicking_widget_handler_is_contained() {
    let host = ExtensionHost::new(cfg());
    let owner = Arc::new(PanickingOwner(OnceLock::new()));
    host.load_native_with_services(
        owner.clone(),
        Arc::new(RecordingServices::new(
            crate::host::CannedResponses::default(),
        )),
    )
    .await
    .unwrap();
    owner
        .0
        .get()
        .unwrap()
        .set_widget("boom", Some(&lines("x")), WidgetPlacement::AboveEditor);
    assert_eq!(
        host.widget_event("boom", click(0)).await,
        WidgetEventOutcome::Unhandled
    );
}

/// The size a native reads is the backend's, through the caller-scoped handle; the default
/// backend answers `None` (no terminal attached), which is pi's cue for its `|| 30` / `|| 120`.
///
/// RED mutation: `CallerScopedServices::terminal_size` not forwarded (the trait default `None`).
#[tokio::test]
async fn the_terminal_size_reaches_a_native_through_its_handle() {
    struct Sized;
    impl HostServices for Sized {
        fn terminal_size(&self) -> Option<TerminalSize> {
            Some(TerminalSize {
                columns: 91,
                rows: 23,
            })
        }
    }
    let a = CardOwner::new("a", WidgetEventOutcome::Handled);
    let host = ExtensionHost::new(cfg());
    host.load_native_with_services(a.clone(), Arc::new(Sized))
        .await
        .unwrap();
    assert_eq!(
        a.services.get().unwrap().terminal_size(),
        Some(TerminalSize {
            columns: 91,
            rows: 23
        })
    );
    assert_eq!(crate::DenyServices.terminal_size(), None);
}

/// The world bump: `events.handle-widget-event` is an EXPORT ADDITION, so a guest built against
/// `cyrup:ext@0.20` exports nothing under that name. The gate must refuse it with the TYPED
/// `ExtError::WorldVersion` before instantiation.
///
/// RED mutation: `HOST_WORLD` left at `cyrup:ext@0.20` (the 0.20 manifest passes the gate).
#[test]
fn a_guest_built_against_0_20_is_refused_with_a_typed_error() {
    let m = crate::ExtensionManifest {
        id: "pre-suba-210".into(),
        version: "1.0.0".into(),
        world: "cyrup:ext@0.20".into(),
        entry: None,
        capabilities: Default::default(),
    };
    let err = m.check_world(crate::HOST_WORLD).unwrap_err();
    assert!(
        matches!(&err, ExtError::WorldVersion { found, required }
            if found == "cyrup:ext@0.20" && required == crate::HOST_WORLD),
        "{err:?}"
    );
    assert_eq!(crate::HOST_WORLD, "cyrup:ext@0.21");
}

/// The same refusal through the PRODUCTION discover-and-load path, against a real component built
/// against the 0.20 world (the base WAT fixture with its `handle-widget-event` export removed and
/// its interfaces at `@0.20.0`): the load error is the typed world mismatch, not a wasmtime link
/// error. And the reason the gate matters: loaded with no gate at all, the same component does not
/// link against this host.
///
/// RED mutation: `check_world` removed from `ExtensionHost::load_discovered` (the error becomes
/// the opaque instantiation failure).
#[tokio::test]
async fn a_0_20_component_is_refused_by_the_load_path_before_wasmtime_sees_it() {
    let base = include_str!("startup_timings_guest.wat");
    let old = base.replace("@0.21.0", "@0.20.0").replace(
        "    (export \"handle-widget-event\" (func $handle-widget-event))\n",
        "",
    );
    assert_ne!(old, base, "the fixture changed");
    let bytes = wat::parse_str(&old).expect("the 0.20-shaped component parses");

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let cwd = std::env::temp_dir().join(format!("cyrup-ext-world-0-20-{nanos}"));
    let dir = cwd.join(".cyrup").join("extensions").join("old");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("extension.json"),
        r#"{ "id": "old", "version": "1.0.0", "world": "cyrup:ext@0.20" }"#,
    )
    .unwrap();
    std::fs::write(dir.join("old.wasm"), &bytes).unwrap();

    let host = ExtensionHost::with_wasm(cfg()).unwrap();
    let services: Arc<dyn HostServices> = Arc::new(RecordingServices::new(
        crate::host::CannedResponses::default(),
    ));
    let result = host
        .discover_and_load(
            &crate::loader::DiscoveryRoots {
                project_cwd: Some(cwd.clone()),
                agent_dir: None,
                configured: vec![],
                disabled: Vec::new(),
            },
            true,
            services.clone(),
        )
        .await;
    assert!(result.loaded.is_empty(), "{:?}", result.loaded);
    let typed = ExtError::WorldVersion {
        found: "cyrup:ext@0.20".into(),
        required: crate::HOST_WORLD.into(),
    }
    .to_string();
    assert!(
        result.errors.iter().any(|e| e.error == typed),
        "the typed world mismatch, not a link error: {:?}",
        result.errors
    );

    let ungated = host.load_wasm("old-ungated".into(), &bytes, services).await;
    assert!(
        ungated.is_err(),
        "with no gate, a 0.20 component does not link against a 0.21 host"
    );
    let _ = std::fs::remove_dir_all(&cwd);
}
