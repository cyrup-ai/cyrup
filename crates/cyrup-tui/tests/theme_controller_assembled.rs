//! Feature #4 — `ThemeController`, proven by an **assembled** `App` render. Production booted the TUI
//! DARK unconditionally, ignoring `settings.theme` + the terminal background (theme.rs audit #4). The
//! `ThemeController` (Pi `theme-controller.ts`) resolves the boot theme from `settings.theme` with a
//! terminal-bg/`ColorMode` fallback. Here we boot the whole `App` through the controller with
//! `settings.theme = "light"` (a non-default, non-dark theme) and assert the rendered foreground/accent
//! roles are the LIGHT theme's colors, not the hardwired dark palette.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_resources::theme::ThemeData;
use cyrup_tui::{App, ColorMode, TerminalTheme, ThemeController, UiTheme};
use ratatui::backend::TestBackend;
use ratatui::style::Color;

#[test]
fn structured_sub_themes_make_every_bg_and_thinking_field_addressable() {
    // Feature #3 — the flat role map was the root cause of the bg/thinking-border misses (an ad-hoc
    // `roles.get("…")` per field was easy to omit). The structured `BackgroundTheme`/`ThinkingTheme`
    // sub-structs project the map into typed, exhaustive fields.
    let data: ThemeData = serde_json::from_str(
        r##"{
            "name": "structured",
            "colors": {
                "text": "#ffffff",
                "selectedBg": "#101010",
                "userMessageBg": "#111111",
                "customMessageBg": "#121212",
                "toolPendingBg": "#131313",
                "toolSuccessBg": "#141414",
                "toolErrorBg": "#151515",
                "thinkingOff": "#202020",
                "thinkingMedium": "#81a2be",
                "thinkingXhigh": "#d183e8"
            }
        }"##,
    )
    .unwrap();
    let theme = UiTheme::from_theme_data(&data, 0);

    // Every background role is a named field, resolved from the theme (not a stray flat lookup).
    let bg = theme.backgrounds();
    assert_eq!(bg.selected, Some(Color::Rgb(0x10, 0x10, 0x10)));
    assert_eq!(bg.user_message, Some(Color::Rgb(0x11, 0x11, 0x11)));
    assert_eq!(bg.custom_message, Some(Color::Rgb(0x12, 0x12, 0x12)));
    assert_eq!(bg.tool_pending, Some(Color::Rgb(0x13, 0x13, 0x13)));
    assert_eq!(bg.tool_success, Some(Color::Rgb(0x14, 0x14, 0x14)));
    assert_eq!(bg.tool_error, Some(Color::Rgb(0x15, 0x15, 0x15)));

    // Every thinking-border level is a typed field; a defined level uses the theme, an omitted level
    // falls back to the spec dark hex (so the border is total, never a flat-map miss).
    let think = theme.thinking();
    assert_eq!(think.off, Color::Rgb(0x20, 0x20, 0x20), "defined level uses the theme value");
    assert_eq!(think.medium, Color::Rgb(0x81, 0xa2, 0xbe));
    assert_eq!(think.xhigh, Color::Rgb(0xd1, 0x83, 0xe8));
    assert_eq!(think.low, Color::Rgb(0x5f, 0x87, 0xaf), "omitted level uses the spec dark fallback");
    // PROV-002: this assembled theme predates the `max` rung and omits `thinkingMax`, so it must
    // reuse its OWN resolved `xhigh` color (Pi's `thinkingMax ?? thinkingXhigh`, theme.ts:329) —
    // not the spec dark hex, and not the neutral border.
    assert_eq!(think.max, think.xhigh, "a theme without thinkingMax falls back to xhigh");
    assert_eq!(theme.thinking_border_style("max").fg, Some(think.xhigh));

    // The structured fields drive the live accessors (one source of truth): the medium thinking rule
    // style resolves to the same field color.
    assert_eq!(theme.thinking_border_style("medium").fg, Some(think.medium));
    assert_eq!(theme.thinking_border_style("low").fg, Some(think.low));
}

/// Whether any cell in the assembled buffer carries `color` as its foreground.
fn any_fg(app: &App<TestBackend>, color: Color) -> bool {
    app.terminal().backend().buffer().content().iter().any(|c| c.fg == color)
}

#[test]
fn assembled_boot_honors_settings_theme_light_not_hardwired_dark() {
    // Boot the controller from `settings.theme = "light"` (truecolor so the role colors stay RGB and
    // are exactly comparable). Resolution: a bare name passes through → active theme "light".
    let controller = ThemeController::boot(Some("light"), ColorMode::TrueColor, TerminalTheme::Dark);
    assert_eq!(controller.active_name(), "light", "settings.theme=light not honored at boot");

    let light = UiTheme::builtin("light");
    let dark = UiTheme::builtin("dark");
    // Sanity: the two builtins genuinely differ (else the assertion below would be vacuous).
    assert_ne!(light.foreground, dark.foreground, "light/dark builtins must differ");

    // Boot the whole app from the controller's projected theme, seed a footer, render.
    let mut app = App::new(TestBackend::new(100, 30), controller.theme()).unwrap();
    app.status_mut().set_model("anthropic/claude-opus-4-6");
    app.status_mut().set_thinking_level("high");
    app.draw().unwrap();

    // The booted app carries the LIGHT theme, not dark.
    assert_eq!(app.state().theme.name, light.name, "app did not boot the light theme");
    // The LIGHT foreground reaches real cells; the DARK foreground never does — the boot is themed,
    // not hardwired dark (audit #4).
    let light_fg = light.foreground.expect("light theme has a foreground");
    let dark_fg = dark.foreground.expect("dark theme has a foreground");
    assert!(any_fg(&app, light_fg), "light-theme foreground did not reach the rendered buffer");
    assert!(
        !any_fg(&app, dark_fg),
        "dark-theme foreground leaked into a light-theme boot (hardwired dark, audit #4)"
    );
}

#[test]
fn theme_controller_resolves_auto_and_unset_against_terminal_background() {
    // `auto` (`light/dark`) picks the arm matching the terminal polarity (Pi `resolveThemeSetting`).
    let light_term =
        ThemeController::boot(Some("light/dark"), ColorMode::TrueColor, TerminalTheme::Light);
    assert_eq!(light_term.active_name(), "light", "auto setting on a light terminal → light");
    let dark_term =
        ThemeController::boot(Some("light/dark"), ColorMode::TrueColor, TerminalTheme::Dark);
    assert_eq!(dark_term.active_name(), "dark", "auto setting on a dark terminal → dark");

    // An unset setting falls back to the terminal polarity's theme name (never hardwired dark).
    let unset_light = ThemeController::boot(None, ColorMode::TrueColor, TerminalTheme::Light);
    assert_eq!(unset_light.active_name(), "light", "unset setting on a light terminal → light");
    let unset_dark = ThemeController::boot(None, ColorMode::TrueColor, TerminalTheme::Dark);
    assert_eq!(unset_dark.active_name(), "dark", "unset setting on a dark terminal → dark");
}

#[test]
fn live_theme_switch_reprojects_through_the_apps_color_mode() {
    // A `/theme` switch on a 256-color boot must keep indexed colors (the app re-projects on set_theme).
    let controller = ThemeController::boot(Some("dark"), ColorMode::Ansi256, TerminalTheme::Dark);
    let mut app = App::new(TestBackend::new(80, 24), controller.theme()).unwrap();
    assert_eq!(app.color_mode(), ColorMode::Ansi256);
    // Switch to the light theme (as `/theme` confirm does via `set_theme(UiTheme::builtin(..))`).
    app.set_theme(UiTheme::builtin("light"));
    app.draw().unwrap();
    let has_rgb = app
        .terminal()
        .backend()
        .buffer()
        .content()
        .iter()
        .any(|c| matches!(c.fg, Color::Rgb(_, _, _)) || matches!(c.bg, Color::Rgb(_, _, _)));
    assert!(!has_rgb, "a /theme switch leaked truecolor on a 256-color boot (set_theme re-projection)");
}

#[test]
fn hot_reload_theme_data_repaints_the_assembled_app() {
    // Feature #1 — hot-reload was DEAD in production (`theme_rx = None`, main.rs): the run loop's
    // `theme_changed` arm existed but the binary never fed it a watcher receiver. The binary now
    // spawns a `ThemeWatcher` on the active theme file and threads its `Receiver` in; when the file
    // changes the arm runs `set_theme(UiTheme::from_theme_data(&data, 0))`. This proves that exact
    // consumer application: a freshly-"watched" `ThemeData` with a distinct accent repaints the app.
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.status_mut().set_model("anthropic/claude-opus-4-6");
    // Put an accent-styled surface on screen: the status band's spinner glyph
    // (`status_indicator.rs:216-218`, Pi `status-indicator.ts:55-64`). Until E1 the only accent
    // foreground in an idle assembled frame was the editor's `› ` prompt glyph — a cyrup invention
    // `editor.ts:482-601` never emits — so this assertion was riding on the very thing E1 removes.
    app.state_mut().indicator.working();
    app.draw().unwrap();

    // A bright-magenta accent no dark/light builtin uses, so its presence is unambiguous.
    let accent = Color::Rgb(0xff, 0x00, 0xff);
    assert!(!any_fg(&app, accent), "sanity: the accent must not be present before the reload");

    // The `Arc<ThemeData>` shape the `ThemeWatcher` publishes on a file edit.
    let data: ThemeData = serde_json::from_str(
        r##"{ "name": "hot-magenta", "colors": { "text": "#ffffff", "accent": "#ff00ff" } }"##,
    )
    .unwrap();
    // Exactly what the run loop's `theme_changed` arm does with the received data.
    app.set_theme(UiTheme::from_theme_data(&data, 0));
    app.draw().unwrap();

    assert_eq!(app.state().theme.name, "hot-magenta", "hot-reloaded theme name not applied");
    assert!(any_fg(&app, accent), "hot-reloaded accent color did not reach the rendered buffer");
}
