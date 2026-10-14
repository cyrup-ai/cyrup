//! SUBA-210 / SUBA-211 — the GUEST tier of the live-widget seam, against the real
//! `wasm32-wasip2` demo component: the guest publishes a widget, the host routes a click on its
//! header row back across the new `events.handle-widget-event` export, and the guest folds the
//! widget and republishes it through `ui.set-widget`; a resize reaches it the same way, and the
//! `ui.get-terminal-size` import reports the terminal the host measured.
//!
//! This is the half a native stand-in cannot prove: `cyrup-ext`'s `tests::widget_events` and the
//! subagents card drive NATIVE owners, which never cross the export or the import that the
//! `cyrup:ext@0.20 -> @0.21` bump is for. A guest built against 0.20 would fail to link here,
//! which is what `check_world` turns into a typed `ExtError::WorldVersion`
//! (`cyrup-ext/src/tests/widget_events.rs`).
//!
//! Upstream: pi's widget is a mounted component (`pi-subagents/src/tui/render.ts:2911`
//! @ad11b7ab) whose `handleMouse` folds on a left click at `y === 0` with no shift, alt or ctrl
//! (`:2938-2944`), and whose render reads `process.stdout.rows`/`columns` (`:2536-2547`).

use crate::fixture;

use cyrup_core::CancelToken;
use cyrup_ext::{
    HostServices, PointerModifiers, TerminalSize, WidgetEvent, WidgetEventOutcome, WidgetMouse,
    WidgetPointer,
};
use cyrup_ext_sdk::example::widget::{WIDGET_COMMAND, WIDGET_FOLDED, WIDGET_HEADER, WIDGET_KEY};
use std::sync::{Arc, Mutex};

/// A backend that reports a terminal, as the TUI's mirror does in production. Everything else is
/// the trait's deny default, so the guest's widget writes land on the guest-side recorder.
struct Terminal(Mutex<Option<TerminalSize>>);

impl HostServices for Terminal {
    fn terminal_size(&self) -> Option<TerminalSize> {
        *self.0.lock().unwrap()
    }
}

fn click(row: u16, modifiers: PointerModifiers) -> WidgetEvent {
    WidgetEvent::Mouse(WidgetMouse {
        pointer: WidgetPointer::Click {
            column: 1,
            row,
            count: 1,
        },
        modifiers,
    })
}

/// THE fold round trip, across the WIT boundary both ways.
///
/// RED at HEAD: `events.handle-widget-event` and `ui.get-terminal-size` do not exist in the world,
/// so neither this file nor the demo's widget compiles. Named mutations, each RED: the guest
/// export body (`cyrup_ext_sdk::guest::handle_widget_event`) answering `Unhandled` without calling
/// the handler (the card never folds); the `ui.set-widget` import not recording the guest as the
/// key's owner (`LiveExtension`'s `set_widget`; the host finds no owner); the host's
/// `get_terminal_size` import answering `None` (the card reads `terminal unknown`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_widget_folds_on_a_header_click_and_relays_out_on_a_resize() {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = cyrup_ext::ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    let backend = Arc::new(Terminal(Mutex::new(Some(TerminalSize {
        columns: 100,
        rows: 20,
    }))));
    let ext = host
        .load_wasm("demo".into(), &bytes, backend.clone())
        .await
        .expect("load + init the live wasm extension");
    let card = || {
        ext.guest()
            .widgets()
            .into_iter()
            .rfind(|w| w.key == WIDGET_KEY)
            .and_then(|w| w.lines)
            .expect("the guest published its card")
    };

    ext.execute_command(WIDGET_COMMAND, "", &CancelToken::new())
        .await
        .expect("the guest's /demo-widget ran");
    assert_eq!(
        card(),
        vec![WIDGET_HEADER.to_string(), "terminal 100x20".to_string()],
        "the guest read the measured terminal through `ui.get-terminal-size`"
    );
    assert_eq!(
        host.widget_owner(WIDGET_KEY),
        Some(cyrup_core::ExtensionId::from("demo")),
        "`ui.set-widget` recorded the guest as the key's owner"
    );

    // A modified click and a click on another row are not the card's: nothing is republished.
    let published = ext.guest().widgets().len();
    let shift = PointerModifiers {
        shift: true,
        ..PointerModifiers::default()
    };
    assert_eq!(
        host.widget_event(WIDGET_KEY, click(0, shift)).await,
        WidgetEventOutcome::Unhandled
    );
    assert_eq!(
        host.widget_event(WIDGET_KEY, click(1, PointerModifiers::default()))
            .await,
        WidgetEventOutcome::Unhandled
    );
    assert_eq!(ext.guest().widgets().len(), published);

    // The plain header click folds it, and a second unfolds it.
    assert_eq!(
        host.widget_event(WIDGET_KEY, click(0, PointerModifiers::default()))
            .await,
        WidgetEventOutcome::Handled
    );
    assert_eq!(card(), vec![WIDGET_FOLDED.to_string()]);
    assert_eq!(
        host.widget_event(WIDGET_KEY, click(0, PointerModifiers::default()))
            .await,
        WidgetEventOutcome::Handled
    );
    assert_eq!(card()[0], WIDGET_HEADER);

    // A resize reaches the guest with the new size, and it republishes against it.
    *backend.0.lock().unwrap() = Some(TerminalSize {
        columns: 132,
        rows: 41,
    });
    host.terminal_resized(TerminalSize {
        columns: 132,
        rows: 41,
    })
    .await;
    assert_eq!(
        card(),
        vec![WIDGET_HEADER.to_string(), "terminal 132x41".to_string()]
    );
}
