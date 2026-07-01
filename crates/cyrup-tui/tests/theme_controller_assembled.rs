//! Feature #4 — `ThemeController`, proven by an **assembled** `App` render. Production booted the TUI
//! DARK unconditionally, ignoring `settings.theme` + the terminal background (theme.rs audit #4). The
//! `ThemeController` (Pi `theme-controller.ts`) resolves the boot theme from `settings.theme` with a
//! terminal-bg/`ColorMode` fallback. Here we boot the whole `App` through the controller with
//! `settings.theme = "light"` (a non-default, non-dark theme) and assert the rendered foreground/accent
//! roles are the LIGHT theme's colors, not the hardwired dark palette.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::{App, ColorMode, TerminalTheme, ThemeController, UiTheme};
use ratatui::backend::TestBackend;
use ratatui::style::Color;

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
