//! SUBA-210 / SUBA-211 — an extension widget is live UI: a click on it reaches the extension that
//! published it, and the terminal size and its resizes reach the extension at all.
//!
//! Each test drives a real fullscreen `App` over a `TestBackend`, the production
//! `LiveHostServices` (the effect sink the run loop drains, the readbacks the app publishes) and a
//! real `ExtensionHost` with a native widget owner, through the whole path the run loop takes:
//! `App::handle_input` (pointer → `AppAction::ExtensionWidgetClick`) → `App::offer_widget_click`
//! (the host asks the owner) → the owner's `set_widget` → the `UiEffect` the run loop applies →
//! the next frame.
//!
//! Upstream: pi's widget is a mounted component (`pi-subagents/src/tui/render.ts:2911` @ad11b7ab)
//! whose `handleMouse` answers a left `click` at `y === 0` with no shift, alt or ctrl
//! (`:2938-2944`); pi delivers that click through the selection's release, because the widget
//! leaves the press alone (`tui-alt-screen.ts:938-962`, `:1338-1341`). See the module doc of
//! `app/pointer.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use cyrup_core::ExtensionId;
use cyrup_ext::host::{HostServices, WidgetPlacement};
use cyrup_ext::{
    ExtError, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent, InitApi, NativeExtension,
    TerminalSize, WidgetEvent, WidgetEventOutcome, WidgetPointer,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_resources::{ResourceRegistry, ResourceSet, builtin_themes};
use cyrup_session_svc::{LiveHostServices, UiEffect, UiRequest};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::component::InputEvent;
use crate::{App, AppAction, UiTheme};

const KEY: &str = "card";
const HEADER: &str = "CARD-HEADER";
const BODY: &str = "CARD-BODY row";
const FOLDED: &str = "CARD folded";

/// A widget owner that folds on a plain click on its header row and records every event.
struct FoldingCard {
    services: OnceLock<Arc<dyn HostServices>>,
    folded: AtomicBool,
    seen: Mutex<Vec<WidgetEvent>>,
}

impl FoldingCard {
    fn publish(&self) {
        let lines: Vec<String> = if self.folded.load(Ordering::Acquire) {
            vec![FOLDED.into()]
        } else {
            vec![HEADER.into(), BODY.into()]
        };
        self.services
            .get()
            .unwrap()
            .set_widget(KEY, Some(&lines), WidgetPlacement::AboveEditor);
    }

    fn seen(&self) -> Vec<WidgetEvent> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl NativeExtension for FoldingCard {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("folding-card")
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
    fn on_widget_event(&self, _key: &str, event: WidgetEvent) -> WidgetEventOutcome {
        self.seen.lock().unwrap().push(event);
        match event {
            WidgetEvent::Mouse(mouse) if mouse.is_plain_click_on_row(0) => {
                self.folded.fetch_xor(true, Ordering::AcqRel);
                self.publish();
                WidgetEventOutcome::Handled
            }
            WidgetEvent::Mouse(_) => WidgetEventOutcome::Unhandled,
            WidgetEvent::Resize(_) => {
                self.publish();
                WidgetEventOutcome::Handled
            }
        }
    }
}

struct Wired {
    app: App<TestBackend>,
    svc: Arc<LiveHostServices>,
    host: ExtensionHost,
    card: Arc<FoldingCard>,
    effects: tokio::sync::mpsc::UnboundedReceiver<UiEffect>,
    _ui: tokio::sync::mpsc::UnboundedReceiver<UiRequest>,
}

impl Wired {
    /// What the run loop's ui-effect arm does: apply every queued effect, then draw.
    fn drain_and_draw(&mut self) {
        while let Ok(effect) = self.effects.try_recv() {
            self.app.apply_ui_effect(effect);
        }
        self.app.draw().unwrap();
    }

    fn screen(&mut self) -> String {
        let alt = self.app.altscreen_for_test().expect("fullscreen is live");
        let buf = alt.backend_for_test().buffer().clone();
        let area = buf.area;
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn find(&mut self, needle: &str) -> (u16, u16) {
        let screen = self.screen();
        for (y, line) in screen.lines().enumerate() {
            if let Some(x) = line.find(needle) {
                return (line[..x].chars().count() as u16, y as u16);
            }
        }
        panic!("{needle:?} is not on screen:\n{screen}")
    }

    fn mouse(
        &mut self,
        kind: MouseEventKind,
        at: (u16, u16),
        modifiers: KeyModifiers,
    ) -> AppAction {
        self.app.handle_input(&InputEvent::Mouse(MouseEvent {
            kind,
            column: at.0,
            row: at.1,
            modifiers,
        }))
    }

    /// A press and a release on one cell, as the terminal reports a click.
    fn click(&mut self, at: (u16, u16), modifiers: KeyModifiers) -> AppAction {
        self.mouse(MouseEventKind::Down(MouseButton::Left), at, modifiers);
        self.mouse(MouseEventKind::Up(MouseButton::Left), at, modifiers)
    }

    /// The run loop's `AppAction::ExtensionWidgetClick` arm, then its ui-effect arm.
    async fn resolve(&mut self, action: AppAction) -> AppAction {
        let AppAction::ExtensionWidgetClick { key, mouse } = action else {
            panic!("the release was not offered to a widget: {action:?}");
        };
        let follow = self.app.offer_widget_click(&self.host, &key, mouse).await;
        self.drain_and_draw();
        follow
    }
}

async fn wired() -> Wired {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = Arc::new(LiveHostServices::new(
        provider,
        cyrup_tools::Backend::default().proc,
        std::env::temp_dir(),
    ));
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.state_mut().show_startup_hints = false;
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    let (ui_tx, ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    let (effect_tx, effects) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
    App::<TestBackend>::install_ui_sinks(&svc, ui_tx, effect_tx);
    let (switch_tx, _switch_rx) = tokio::sync::mpsc::unbounded_channel();
    app.install_extension_readbacks(
        &svc,
        Arc::new(ResourceRegistry {
            themes: ResourceSet::build(builtin_themes()),
            ..ResourceRegistry::default()
        }),
        switch_tx,
    );
    let host = ExtensionHost::new(HostConfig {
        mode: cyrup_ext::ExtMode::Tui,
        has_ui: true,
        cwd: std::env::temp_dir(),
    });
    let card = Arc::new(FoldingCard {
        services: OnceLock::new(),
        folded: AtomicBool::new(false),
        seen: Mutex::new(Vec::new()),
    });
    host.load_native_with_services(card.clone(), svc.clone() as Arc<dyn HostServices>)
        .await
        .unwrap();
    card.publish();
    let mut wired = Wired {
        app,
        svc,
        host,
        card,
        effects,
        _ui: ui_rx,
    };
    wired.drain_and_draw();
    wired
}

/// Verify 1 through the real TUI: a click on the widget's header row is offered to its owner as a
/// click on row 0 with no modifiers, the owner folds and republishes, and the next frame shows the
/// folded card; a second click unfolds it. The click copies nothing and leaves no selection.
///
/// RED at HEAD: `AppAction::ExtensionWidgetClick`, `App::offer_widget_click` and
/// `NativeExtension::on_widget_event` do not exist (does not compile). Named mutations, RED:
/// `complete_selection_click` not consulting `widget_at` (the release finishes as a plain
/// selection click and `resolve` panics: nothing was offered); `widget_at` counting rows from the
/// widget's SECOND line (the header click arrives as row 1 and the card does not fold).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_click_on_a_widget_header_folds_it_through_the_whole_path() {
    let mut w = wired().await;
    let at = w.find(HEADER);
    let action = w.click(at, KeyModifiers::NONE);
    match &action {
        AppAction::ExtensionWidgetClick { key, mouse } => {
            assert_eq!(key, KEY);
            assert!(
                matches!(
                    mouse.pointer,
                    WidgetPointer::Click {
                        row: 0,
                        count: 1,
                        ..
                    }
                ),
                "{mouse:?}"
            );
            assert!(!mouse.modifiers.any());
        }
        other => panic!("the release was not offered to the widget: {other:?}"),
    }
    let follow = w.resolve(action).await;
    assert!(
        !matches!(follow, AppAction::CopySelection(_)),
        "a click the widget took copies nothing: {follow:?}"
    );
    let screen = w.screen();
    assert!(
        screen.contains(FOLDED),
        "the next frame shows the fold:\n{screen}"
    );
    assert!(!screen.contains(HEADER), "{screen}");
    assert!(
        w.app
            .altscreen_for_test()
            .unwrap()
            .selection_text()
            .is_none(),
        "and leaves no selection"
    );

    let at = w.find(FOLDED);
    let action = w.click(at, KeyModifiers::NONE);
    w.resolve(action).await;
    let screen = w.screen();
    assert!(screen.contains(HEADER) && screen.contains(BODY), "{screen}");
}

/// Verify 1's negatives and Verify 2 through the real TUI: a click on another row, and a modified
/// click on the header, are offered — with their row and modifiers — and left alone, so nothing is
/// republished; a DRAG across the widget is never offered at all and copies its text, because the
/// press on a widget still starts a text selection.
///
/// The drag half is also a NON-REGRESSION GUARD (green before and after): it is the same contract
/// `dock_selection::widgets_and_queued_messages_are_selectable` pins, re-asserted here with a widget
/// owner that WOULD take a click, so a route that claimed the press would fail it.
///
/// RED at HEAD: does not compile. Named mutation, RED: `complete_selection_click` claiming the
/// widget's PRESS through `handle_dock_pointer` (the drag no longer copies).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_header_click_is_the_widgets_and_a_drag_still_selects() {
    let mut w = wired().await;

    let body = w.find(BODY);
    let action = w.click(body, KeyModifiers::NONE);
    assert!(
        matches!(
            &action,
            AppAction::ExtensionWidgetClick { mouse, .. }
                if matches!(mouse.pointer, WidgetPointer::Click { row: 1, .. })
        ),
        "{action:?}"
    );
    w.resolve(action).await;

    let header = w.find(HEADER);
    for modifiers in [KeyModifiers::ALT, KeyModifiers::CONTROL] {
        let action = w.click(header, modifiers);
        assert!(
            matches!(&action, AppAction::ExtensionWidgetClick { mouse, .. } if mouse.modifiers.any()),
            "{modifiers:?}: {action:?}"
        );
        w.resolve(action).await;
    }
    assert_eq!(w.card.seen().len(), 3, "all three clicks reached the owner");
    let screen = w.screen();
    assert!(
        screen.contains(HEADER) && !screen.contains(FOLDED),
        "none of them folded it:\n{screen}"
    );

    let before = w.card.seen().len();
    let (x, y) = w.find(BODY);
    w.mouse(
        MouseEventKind::Down(MouseButton::Left),
        (x, y),
        KeyModifiers::NONE,
    );
    w.mouse(
        MouseEventKind::Drag(MouseButton::Left),
        (x + 8, y),
        KeyModifiers::NONE,
    );
    let action = w.mouse(
        MouseEventKind::Up(MouseButton::Left),
        (x + 8, y),
        KeyModifiers::NONE,
    );
    assert_eq!(
        action,
        AppAction::CopySelection("CARD-BODY".into()),
        "a drag across the widget selects and copies its text"
    );
    assert_eq!(
        w.card.seen().len(),
        before,
        "and is never offered to the widget"
    );
}

/// SUBA-211 through the real TUI: the size an extension reads is the terminal the app measured,
/// published by the readbacks; a resize publishes the new size BEFORE the owner is told, and the
/// owner is told.
///
/// RED at HEAD: `HostServices::terminal_size` and `AppAction::ExtensionWidgetsResized` do not exist
/// (does not compile). Named mutations, RED: `publish_extension_readbacks` not publishing the size
/// (`terminal_size()` is `None`); the `InputEvent::Resize` arm not publishing it (the owner is
/// told, but the size it reads is still 80x24). The run loop's `ExtensionWidgetsResized` arm itself
/// (`app/run_action.rs`, one `terminal_resized` call) needs a live session and is not driven here;
/// the call it makes is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_terminal_size_and_its_resizes_reach_the_widgets_owner() {
    let mut w = wired().await;
    assert_eq!(
        w.svc.terminal_size(),
        Some(TerminalSize {
            columns: 80,
            rows: 24
        }),
        "the readbacks publish the measured terminal"
    );

    let action = w.app.handle_input(&InputEvent::Resize(100, 30));
    let size = TerminalSize {
        columns: 100,
        rows: 30,
    };
    assert_eq!(action, AppAction::ExtensionWidgetsResized(size));
    assert_eq!(
        w.svc.terminal_size(),
        Some(size),
        "published before the owner is told"
    );
    let AppAction::ExtensionWidgetsResized(resized) = action else {
        unreachable!()
    };
    w.host.terminal_resized(resized).await;
    assert_eq!(w.card.seen(), vec![WidgetEvent::Resize(size)]);
}
