//! TUI-S01 — the interactive TUI must install the fire-and-forget [`UiEffectSink`], not just the
//! request/reply [`UiSink`].
//!
//! Pi's interactive mode passes a REAL `uiContext` whose mutators land on live TUI state
//! (`pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2223-2268`); only headless
//! modes get `noOpUIContext` (`core/extensions/runner.ts:230-265`). Cyrup's `LiveHostServices`
//! encodes that policy as "no sink ⇒ drop" (`emit_ui_effect`), and the interactive TUI never
//! installed the effect sink — so every `notify`/`setStatus`/`setTitle`/`setEditorText`/
//! `pasteToEditor`/`setToolsExpanded`/`setWidget`/`setHeader`/`setFooter` an extension made was
//! silently discarded in the DEFAULT mode while working fine over RPC.
//!
//! These tests drive the production seams: the real `LiveHostServices`, the real
//! `App::install_ui_sinks` that `App::run` and its session-swap arm call, and the real
//! `App::apply_ui_effect` the run loop's drain arm calls.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use crate::{App, UiTheme};
use cyrup_ext::host::HostServices;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{LiveHostServices, NotifyKind, UiEffect, UiRequest};
use ratatui::backend::TestBackend;

fn services() -> Arc<LiveHostServices> {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    Arc::new(LiveHostServices::new(
        provider,
        cyrup_tools::Backend::default().proc,
        std::env::temp_dir(),
    ))
}

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap()
}

/// Characterizes the drop the TUI used to sit on top of: with no effect sink attached,
/// `LiveHostServices` discards every fire-and-forget `ui.*` call. This is correct for headless
/// (print/json) — it is Pi's `noOpUIContext` — and it is exactly what interactive inherited.
#[test]
fn without_a_sink_every_fire_and_forget_ui_call_is_discarded() {
    let svc = services();
    let (_ui_tx, _ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    let (effect_tx, mut effect_rx) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
    // Deliberately NOT installed.
    drop(effect_tx);

    svc.notify("hello", NotifyKind::Info);
    svc.set_status("ext", Some("busy"));
    svc.set_title("t");
    svc.set_editor_text("x", false);
    svc.set_tools_expanded(true);

    assert!(
        effect_rx.try_recv().is_err(),
        "nothing can arrive without an installed sink"
    );
}

/// The wiring itself: `App::install_ui_sinks` is the one call `App::run` (and its session-swap arm)
/// makes, and after it every one of the eight fire-and-forget capabilities reaches the run loop.
#[test]
fn install_ui_sinks_delivers_all_eight_fire_and_forget_capabilities() {
    let svc = services();
    let (ui_tx, _ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    let (effect_tx, mut effect_rx) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
    App::<TestBackend>::install_ui_sinks(&svc, ui_tx, effect_tx);

    svc.notify("note", NotifyKind::Warning);
    svc.set_status("ext", Some("busy"));
    // SEAM-011: pi's `RpcExtensionUIRequest` pins widgetKey/widgetLines/widgetPlacement
    // (`modes/rpc/rpc-types.ts:264-271` @v0.83.0) — not a cyrup-invented `widget` blob.
    svc.set_widget(
        "w",
        Some(&["a".to_string()]),
        cyrup_ext::host::services::WidgetPlacement::default(),
    );
    svc.set_header("H");
    svc.set_footer("F");
    svc.set_title("cyrup — repo");
    svc.set_editor_text("typed", false);
    svc.set_tools_expanded(true);

    let mut got = Vec::new();
    while let Ok(effect) = effect_rx.try_recv() {
        got.push(effect);
    }
    assert_eq!(
        got,
        vec![
            UiEffect::Notify {
                message: "note".into(),
                kind: NotifyKind::Warning
            },
            UiEffect::SetStatus {
                key: "ext".into(),
                text: Some("busy".into())
            },
            UiEffect::SetWidget {
                widget: serde_json::json!({
                    "key": "w", "lines": ["a"], "placement": "aboveEditor",
                }),
            },
            UiEffect::SetHeader {
                content: "H".into()
            },
            UiEffect::SetFooter {
                content: "F".into()
            },
            UiEffect::SetTitle {
                title: "cyrup — repo".into()
            },
            UiEffect::SetEditorText {
                text: "typed".into(),
                is_paste: false
            },
            UiEffect::SetToolsExpanded { expanded: true },
        ],
        "these eight ui.* mutators must reach the interactive run loop, in call order. TUI-030 \
         added four more (`setWorkingMessage`/`setWorkingVisible`/`setWorkingIndicator`/\
         `setHiddenThinkingLabel`); they are covered by `crate::tests::extension_working_indicator`, \
         which drives them through this same sink to a painted band"
    );
}

/// End-to-end through the seam the run loop uses: an extension calls `ui.notify(...)`, the effect
/// travels the installed sink, the run loop's drain arm applies it, and the user SEES it in the
/// transcript — Pi `showExtensionNotify` (`interactive-mode.ts:2518-2526`).
#[test]
fn notify_reaches_the_transcript_with_pi_severity_routing() {
    let svc = services();
    let (ui_tx, _ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    let (effect_tx, mut effect_rx) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
    App::<TestBackend>::install_ui_sinks(&svc, ui_tx, effect_tx);

    svc.notify("indexing finished", NotifyKind::Info);
    svc.notify("stale cache", NotifyKind::Warning);
    svc.notify("token refresh failed", NotifyKind::Error);

    let mut app = app();
    while let Ok(effect) = effect_rx.try_recv() {
        app.apply_ui_effect(effect);
    }

    let text = rendered(&mut app);
    assert!(
        text.contains("indexing finished"),
        "info notify must show: {text}"
    );
    // Pi's `showWarning`/`showError` prefix the copy (`interactive-mode.ts:3950-3960`).
    assert!(
        text.contains("Warning: stale cache"),
        "warning notify must show: {text}"
    );
    assert!(
        text.contains("Error: token refresh failed"),
        "error notify must show: {text}"
    );
}

/// `setStatus` reaches the footer's extension-status line (Pi `setExtensionStatus` →
/// `footerDataProvider.setExtensionStatus`, `interactive-mode.ts:1920-1923`), and `text: None`
/// clears the key (`footer.ts:233`).
#[test]
fn set_status_reaches_the_footer_and_clears() {
    let svc = services();
    let (ui_tx, _ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    let (effect_tx, mut effect_rx) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
    App::<TestBackend>::install_ui_sinks(&svc, ui_tx, effect_tx);

    let mut app = app();
    svc.set_status("lsp", Some("rust-analyzer: indexing"));
    drain(&mut app, &mut effect_rx);
    assert_eq!(
        app.state().status.extension_status_text(),
        "rust-analyzer: indexing"
    );
    assert!(
        rendered(&mut app).contains("rust-analyzer: indexing"),
        "must reach the rendered footer"
    );

    svc.set_status("lsp", None);
    drain(&mut app, &mut effect_rx);
    assert_eq!(
        app.state().status.extension_status_text(),
        "",
        "None must clear the key"
    );
}

/// `setEditorText` and `pasteToEditor`: Pi routes the first through `editor.setText` and the second
/// through the editor's bracketed-paste input path (`interactive-mode.ts:2240-2241`), so a paste is
/// sanitized exactly like a real terminal paste.
#[test]
fn set_editor_text_and_paste_reach_the_editor() {
    let svc = services();
    let (ui_tx, _ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    let (effect_tx, mut effect_rx) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
    App::<TestBackend>::install_ui_sinks(&svc, ui_tx, effect_tx);

    let mut app = app();
    svc.set_editor_text("fix the flaky test", false);
    drain(&mut app, &mut effect_rx);
    assert_eq!(app.state().editor.text(), "fix the flaky test");

    // The paste variant appends at the cursor rather than replacing the buffer.
    svc.set_editor_text(" now", true);
    drain(&mut app, &mut effect_rx);
    assert_eq!(app.state().editor.text(), "fix the flaky test now");
}

/// `setToolsExpanded` flips the transcript's expansion state and echoes Pi's exact status copy —
/// including the no-op early-out when the value is unchanged (`interactive-mode.ts:3887-3903`).
#[test]
fn set_tools_expanded_flips_the_transcript_and_echoes_pi_status() {
    let svc = services();
    let (ui_tx, _ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    let (effect_tx, mut effect_rx) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
    App::<TestBackend>::install_ui_sinks(&svc, ui_tx, effect_tx);

    let mut app = app();
    assert!(
        !app.state().transcript.tool_expanded,
        "collapsed by default"
    );

    svc.set_tools_expanded(true);
    drain(&mut app, &mut effect_rx);
    assert!(app.state().transcript.tool_expanded);
    assert!(rendered(&mut app).contains("Tool output: expanded"));

    // A repeat is a no-op: no second status line (Pi's `if (expanded === …) return`).
    let before = rendered(&mut app).matches("Tool output: expanded").count();
    svc.set_tools_expanded(true);
    drain(&mut app, &mut effect_rx);
    assert_eq!(
        rendered(&mut app).matches("Tool output: expanded").count(),
        before
    );
}

/// `setTitle`/`setWidget`/`setHeader`/`setFooter` now ARRIVE and are retained. TUI-014 (widgets
/// stored but never rendered) does NOT close as a consequence of this wiring: cyrup's TUI has no
/// extension chrome slot, so the payload lands in state and stops there. This test pins that
/// honestly so the remaining half is not mistaken for done.
#[test]
fn title_widget_header_footer_arrive_and_are_rendered() {
    let svc = services();
    let (ui_tx, _ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    let (effect_tx, mut effect_rx) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
    App::<TestBackend>::install_ui_sinks(&svc, ui_tx, effect_tx);

    let mut app = app();
    svc.set_title("cyrup — my-repo");
    svc.set_widget(
        "todo",
        Some(&["WIDGET-A".to_string(), "WIDGET-B".to_string()]),
        cyrup_ext::host::services::WidgetPlacement::default(),
    );
    svc.set_header("HEADER-LINE");
    svc.set_footer("FOOTER-LINE");
    drain(&mut app, &mut effect_rx);

    assert_eq!(
        app.state().terminal_title.as_deref(),
        Some("cyrup — my-repo")
    );
    assert_eq!(app.state().extension_widgets.len(), 1);
    assert_eq!(app.state().extension_widgets[0].key, "todo");
    assert_eq!(app.state().extension_header.as_deref(), Some("HEADER-LINE"));
    assert_eq!(app.state().extension_footer.as_deref(), Some("FOOTER-LINE"));

    // TUI-014 / TUI-033 — they are DRAWN now. Before the fix all three were stored in fields no
    // render function read: `rg 'extension_widget|extension_header|extension_footer'` returned only
    // the assignment, the reset and these test assertions, and this test asserted the ABSENCE of
    // the strings on screen. Upstream mounts them as real components — `setExtensionWidget`
    // (`interactive-mode.ts:1920-1960`), `setExtensionHeader` (`:2262-2290`) and
    // `setExtensionFooter` (`:2235-2257`).
    let text = rendered(&mut app);
    assert!(
        text.contains("HEADER-LINE"),
        "the extension header must paint:\n{text}"
    );
    assert!(
        text.contains("FOOTER-LINE"),
        "the extension footer must paint:\n{text}"
    );
    assert!(
        text.contains("WIDGET-A"),
        "the extension widget must paint:\n{text}"
    );
    assert!(
        text.contains("WIDGET-B"),
        "every widget row must paint:\n{text}"
    );
}

/// The footer is REPLACED, not overlaid, and clearing it restores the built-in — Pi's
/// `setExtensionFooter` clears `footerContainer` and adds the extension component, and re-adds
/// `this.footer` when the factory is undefined (`interactive-mode.ts:2245-2254`). TUI-033.
#[tokio::test]
async fn an_extension_footer_swaps_the_built_in_one_out_and_back() {
    let mut app = app();
    app.state_mut().status.set_model("BUILTINMODEL");
    let before = rendered(&mut app);
    assert!(
        before.contains("BUILTINMODEL"),
        "the built-in footer paints first:\n{before}"
    );

    app.apply_ui_effect(UiEffect::SetFooter {
        content: "EXTFOOTER".to_string(),
    });
    let swapped = rendered(&mut app);
    assert!(
        swapped.contains("EXTFOOTER"),
        "the extension footer paints:\n{swapped}"
    );
    assert!(
        !swapped.contains("BUILTINMODEL"),
        "the built-in must be swapped OUT:\n{swapped}"
    );

    app.apply_ui_effect(UiEffect::SetFooter {
        content: String::new(),
    });
    let restored = rendered(&mut app);
    assert!(
        restored.contains("BUILTINMODEL"),
        "clearing restores the built-in:\n{restored}"
    );
    assert!(
        !restored.contains("EXTFOOTER"),
        "the extension footer is gone:\n{restored}"
    );
}

/// A second `setWidget` under the same key REPLACES rather than appends — Pi removes the key from
/// both placement maps before re-inserting (`interactive-mode.ts:1926-1933`, `:1958`). TUI-014.
#[tokio::test]
async fn a_second_widget_with_the_same_key_replaces_the_first() {
    let mut app = app();
    app.apply_ui_effect(UiEffect::SetWidget {
        widget: serde_json::json!({"key": "todo", "lines": ["FIRST"], "placement": "aboveEditor"}),
    });
    app.apply_ui_effect(UiEffect::SetWidget {
        widget: serde_json::json!({"key": "todo", "lines": ["SECOND"], "placement": "aboveEditor"}),
    });
    assert_eq!(
        app.state().extension_widgets.len(),
        1,
        "same key must not append"
    );
    let text = rendered(&mut app);
    assert!(
        text.contains("SECOND") && !text.contains("FIRST"),
        "replaced, not appended:\n{text}"
    );
}

/// `content === undefined` REMOVES the widget (`interactive-mode.ts:1935-1938`). TUI-014.
#[tokio::test]
async fn a_widget_with_no_content_is_removed() {
    let mut app = app();
    app.apply_ui_effect(UiEffect::SetWidget {
        widget: serde_json::json!({"key": "todo", "lines": ["GONE"], "placement": "aboveEditor"}),
    });
    assert_eq!(
        app.state().extension_widgets.len(),
        1,
        "mounted before the removal"
    );
    // `lines: null` is what `LiveHostServices::set_widget` emits for pi's `content: undefined`
    // (`host_services.rs:730-737`); an absent key reads the same.
    app.apply_ui_effect(UiEffect::SetWidget {
        widget: serde_json::json!({"key": "todo", "lines": null, "placement": "aboveEditor"}),
    });
    assert!(
        app.state().extension_widgets.is_empty(),
        "an empty content list removes the widget"
    );
    let text = rendered(&mut app);
    assert!(
        !text.contains("GONE"),
        "the removed widget must not paint:\n{text}"
    );
}

/// `MAX_WIDGET_LINES = 10` with pi's own truncation row (`interactive-mode.ts:1945-1950`, `:2008`).
#[test]
fn a_widget_longer_than_ten_lines_is_truncated_with_pis_marker() {
    let content: Vec<String> = (0..15).map(|i| format!("row{i}")).collect();
    let w = crate::ExtensionWidget::from_json(&serde_json::json!({
        "key": "big",
        "lines": content,
        "placement": "aboveEditor",
    }));
    assert_eq!(w.lines.len(), 11, "10 rows plus the marker");
    assert_eq!(w.lines[10], crate::ExtensionWidget::TRUNCATED);
    assert_eq!(w.lines[9], "row9");
}

/// `options.placement === "belowEditor"` routes to the other container (`:1925`, `:1957`).
#[test]
fn placement_below_editor_is_recovered_from_the_payload() {
    let below = crate::ExtensionWidget::from_json(&serde_json::json!({
        "key": "k", "lines": ["x"], "placement": "belowEditor",
    }));
    assert!(below.below);
    let above = crate::ExtensionWidget::from_json(&serde_json::json!({
        "key": "k", "lines": ["x"], "placement": "aboveEditor",
    }));
    assert!(!above.below, "the default is `aboveEditor`");
    // A carrier with no placement at all still reads as pi's documented default
    // (`extensions/types.ts:107-110` @v0.83.0, `interactive-mode.ts:1925`).
    let implicit =
        crate::ExtensionWidget::from_json(&serde_json::json!({"key": "k", "lines": ["x"]}));
    assert!(!implicit.below, "an absent placement is `aboveEditor`");
}

fn drain(app: &mut App<TestBackend>, rx: &mut tokio::sync::mpsc::UnboundedReceiver<UiEffect>) {
    while let Ok(effect) = rx.try_recv() {
        app.apply_ui_effect(effect);
    }
}

/// The rendered frame as plain text — the only thing the user actually sees.
fn rendered(app: &mut App<TestBackend>) -> String {
    app.draw().unwrap();
    let mut out = app.scrollback_text();
    out.push('\n');
    let buf = app.terminal().backend().buffer().clone();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}
