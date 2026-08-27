//! EXT-006 END TO END FOR THE RUNTIME IT WAS WRITTEN FOR — a LIVE WASM guest's renderer must reach
//! the SCREEN, not just the host facade.
//!
//! Pi's renderer returns an in-process `pi-tui` `Component` that `CustomMessageComponent.rebuild()`
//! adds as a child (`modes/interactive/components/custom-message.ts:66-81`, resolved at
//! `interactive-mode.ts:3326`); nothing is ever stringified. A WASM guest cannot hand back an
//! object, so cyrup's wire analog is that component tree SERIALIZED — the shape both WIT world
//! copies document above `render-call` and the shape `cyrup_ext_sdk::widget` builds.
//!
//! The gap this test closes: the routing was live (`cyrup-ext/tests/wasm_renderer_routing.rs`) and
//! the TUI consulted it, but the TUI FLATTENER only special-cased `Value::String` and
//! pretty-printed everything else — so a guest following cyrup's own SDK example drew a raw JSON
//! blob where Pi draws the component. Every other renderer test used a NATIVE extension returning a
//! bare string, so the widget path had no end-to-end coverage at all.
//!
//! This test therefore uses the REAL demo component (`cyrup-ext-sdk/src/example/renderers.rs`,
//! whose
//! `DemoToolRenderer` returns a multi-node `container`) and asserts on TERMINAL CELLS.
//!
//! MIGRATION NOTES (mechanical only; no assertion changed):
//!
//! * The file-level `#![cfg(feature = "wasm-host")]` it carried in `crates/cyrup-tui/tests/` is
//!   DELETED rather than re-spelled. In `cyrup-tui` that name resolved to *cyrup-tui's* feature;
//!   here it would resolve to `cyrup-it`'s own `wasm-host`, which is default-OFF — so keeping the
//!   line would make `cargo test -p cyrup-it --features it` compile this file away and report a
//!   silent pass, the exact invisible-skip the `it` gate exists to prevent. The host is available
//!   unconditionally because `cyrup-ext` enables `wasm-host` in its OWN `default`
//!   (`crates/cyrup-ext/Cargo.toml`), which is how `cyrup-it` dev-depends on it. Restore a gate
//!   here the day `cyrup-ext` stops defaulting it on — and gate the `[[test]]` target too, so the
//!   skip is visible.
//! * `mod fixture;` (`crates/cyrup-tui/tests/fixture/mod.rs`) is gone: its `component()` was one of
//!   the 24 nested `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` invocations, and the
//!   component now comes from `build.rs` via `support::bins::component()`. Its two-line `cfg()`
//!   had exactly one caller and is inlined below.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::ToolCallId;
use cyrup_ext::{DenyServices, ExtMode, ExtensionHost, HostConfig};
use cyrup_session_svc::AgentSessionEvent;
use cyrup_tui::{App, UiTheme};
use ratatui::backend::TestBackend;
use serde_json::json;

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(100, 24), UiTheme::dark()).unwrap()
}

fn buffer_text(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// Inlined from the deleted `crates/cyrup-tui/tests/fixture/mod.rs` (its only caller was here).
fn cfg() -> HostConfig {
    HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: PathBuf::from(".") }
}

async fn host_with_demo_guest() -> Arc<ExtensionHost> {
    let bytes =
        std::fs::read(crate::support::bins::component()).expect("read fixture component bytes");
    let host = ExtensionHost::with_wasm(cfg()).expect("host with wasm runtime");
    host.load_wasm("demo".into(), &bytes, Arc::new(DenyServices)).await.expect("load + init");
    Arc::new(host)
}

/// The demo guest's `demo_echo` tool renderer returns `container[text, text]`. Both rows must land
/// on screen as ROWS — and no JSON punctuation may.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wasm_guests_widget_tree_draws_on_screen_as_rows() {
    let host = host_with_demo_guest().await;
    let mut app = app();

    let start = AgentSessionEvent::ToolExecutionStart {
        tool_call_id: ToolCallId::from("call-g1"),
        tool_name: "demo_echo".into(),
        args: json!({ "text": "hi" }),
    };
    app.ingest_event_with_extensions(&start, &host).await;
    app.draw().unwrap();

    let live = buffer_text(&app);
    assert!(
        live.contains("guest-rendered echo call"),
        "the GUEST's first widget row is on screen:\n{live}"
    );
    assert!(
        live.contains("(drawn by the demo extension)"),
        "the GUEST's SECOND widget row is on screen too — the container stacked its children, \
         which a single-`Text` return could not have proved:\n{live}"
    );
    // The regression. Before the flattener understood the vocabulary, this frame held
    // `{ "children": [ { "text": "guest-rendered echo call: …", "widget": "text" } ], … }`.
    assert!(
        !live.contains("\"widget\"") && !live.contains("\"children\""),
        "the serialized tree was FLATTENED, not dumped into the transcript as JSON:\n{live}"
    );

    let end = AgentSessionEvent::ToolExecutionEnd {
        tool_call_id: ToolCallId::from("call-g1"),
        tool_name: "demo_echo".into(),
        is_error: false,
        result: json!({ "content": [{ "type": "text", "text": "echo: hi" }] }),
    };
    app.ingest_event_with_extensions(&end, &host).await;
    app.draw().unwrap();

    let seen = format!("{}\n{}", buffer_text(&app), app.scrollback_text());
    assert!(
        seen.contains("guest-rendered echo result"),
        "the GUEST's result-side widget drew too:\n{seen}"
    );
    assert!(!seen.contains("\"widget\""), "and not as JSON:\n{seen}");
}

/// The custom-MESSAGE surface of the same guest (`registerMessageRenderer("demo", …)`), which
/// reaches the transcript through a different entry than the tool row above.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wasm_guests_message_renderer_draws_on_screen() {
    let host = host_with_demo_guest().await;
    let mut app = app();

    let ev = AgentSessionEvent::MessageEnd {
        message: cyrup_agent::AgentMessage::Custom {
            kind: "demo".into(),
            payload: json!("plain fallback body"),
            timestamp: None,
        },
    };
    app.ingest_event_with_extensions(&ev, &host).await;
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(sb.contains("demo call:"), "the GUEST's message renderer drew the block:\n{sb}");
    assert!(!sb.contains("\"widget\""), "flattened, not dumped as JSON:\n{sb}");
    // The guest's renderer REPLACES the default framing (Pi's `CustomMessageComponent.rebuild()`
    // returns early once a custom renderer produced a component, `custom-message.ts:66-81`). This
    // guest echoes the whole message JSON, so the *payload* text legitimately appears inside its
    // own output — the thing that must be gone is the default `[demo]` LABEL block.
    assert!(
        !sb.contains("[demo]"),
        "the default `[label] body` framing did not also draw:\n{sb}"
    );
}
