//! TUI-004 — boot-time light/dark detection must **ask the terminal**, not only read `COLORFGBG`.
//!
//! Pi queries the terminal twice before it settles a theme
//! (`coding-agent/src/modes/interactive/theme/theme.ts:768-801`,
//! `tui/src/tui.ts:1174-1220`, parsers in `tui/src/terminal-colors.ts`):
//!
//! * `detectTerminalBackgroundTheme` writes OSC 11 (`ESC ] 11 ; ? BEL`), classifies the reply's
//!   luminance, and only falls back to `COLORFGBG` in the `catch` (`:783-787`);
//! * `detectTerminalThemeForAuto` prefers the DSR `?996` color-scheme report
//!   (`CSI ? 997 ; 1|2 n`) for an `auto` (`light/dark`) setting (`:790-801`).
//!
//! cyrup shipped only the `COLORFGBG` half (`TerminalTheme::detect`). Since iTerm2, Ghostty,
//! Alacritty, WezTerm and Terminal.app do not set `COLORFGBG`, a light-background user was handed
//! the dark palette every single boot.
//!
//! The tests drive the real `ThemeController::sync_with_terminal` with a scripted probe and assert
//! against the **assembled, rendered buffer** — the light theme's foreground has to reach real
//! cells and the dark one must not — plus the parser-level contract that decides it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::cell::RefCell;
use std::time::Duration;

use cyrup_tui::{
    detect_terminal_background_theme, detect_terminal_theme_for_auto, App, ColorMode,
    DetectionConfidence, NoTerminalProbe, TerminalProbe, TerminalTheme, TerminalThemeSource,
    ThemeController, UiTheme,
};
use ratatui::backend::TestBackend;
use ratatui::style::Color;

const TIMEOUT: Duration = Duration::from_millis(100);

/// A terminal that answers exactly what the script says, recording that it was asked (Pi's
/// `TerminalBackgroundThemeDetector` / `TerminalAutoThemeDetector` interfaces, `theme.ts:703-709`).
#[derive(Default)]
struct ScriptedProbe {
    background: Option<(u8, u8, u8)>,
    color_scheme: Option<TerminalTheme>,
    asked: RefCell<Vec<&'static str>>,
}

impl ScriptedProbe {
    fn background(rgb: (u8, u8, u8)) -> Self {
        ScriptedProbe { background: Some(rgb), ..Default::default() }
    }
    fn color_scheme(scheme: TerminalTheme) -> Self {
        ScriptedProbe { color_scheme: Some(scheme), ..Default::default() }
    }
}

impl TerminalProbe for ScriptedProbe {
    fn query_background_color(&self, _timeout: Duration) -> Option<(u8, u8, u8)> {
        self.asked.borrow_mut().push("osc11");
        self.background
    }
    fn query_color_scheme(&self, _timeout: Duration) -> Option<TerminalTheme> {
        self.asked.borrow_mut().push("dsr996");
        self.color_scheme
    }
}

/// Whether any cell in the assembled buffer carries `color` as its foreground.
fn any_fg(app: &App<TestBackend>, color: Color) -> bool {
    app.terminal().backend().buffer().content().iter().any(|c| c.fg == color)
}

// ---------------------------------------------------------------------------------------------
// The observable one: a light terminal repaints the assembled app light.
// ---------------------------------------------------------------------------------------------

#[test]
fn an_osc11_light_background_repaints_the_assembled_app_light() {
    // No `settings.theme` and no `COLORFGBG` at all — exactly the iTerm2 / Ghostty / Alacritty
    // situation. Boot is therefore the `Fallback` dark guess. (Truecolor is pinned so the role
    // colors stay RGB and are exactly comparable, as in `theme_controller_assembled.rs`.)
    let mut controller = ThemeController::boot(None, ColorMode::TrueColor, TerminalTheme::Dark);
    let mut app = App::new(TestBackend::new(100, 30), controller.theme()).unwrap();
    app.status_mut().set_model("anthropic/claude-opus-4-6");
    app.status_mut().set_thinking_level("high");
    app.draw().unwrap();

    let light = UiTheme::builtin("light");
    let dark = UiTheme::builtin("dark");
    assert_ne!(light.foreground, dark.foreground, "light/dark builtins must differ");
    let light_fg = light.foreground.expect("light theme has a foreground");
    let dark_fg = dark.foreground.expect("dark theme has a foreground");
    assert!(any_fg(&app, dark_fg), "pre-probe boot is the dark fallback");

    // ...until the terminal is asked and answers `#fafafa` — a light background.
    let probe = ScriptedProbe::background((0xfa, 0xfa, 0xfa));
    let theme = controller
        .sync_with_terminal(&probe, TIMEOUT, "")
        .expect("a light reply must change the theme");
    app.set_theme(theme);
    app.draw().unwrap();

    assert_eq!(controller.active_name(), "light");
    assert_eq!(app.state().theme.name, light.name, "app did not adopt the queried light theme");
    assert!(any_fg(&app, light_fg), "the light foreground never reached a rendered cell");
    assert!(
        !any_fg(&app, dark_fg),
        "dark palette still painted after a light OSC 11 reply (COLORFGBG-only detection)"
    );
    assert_eq!(probe.asked.borrow().as_slice(), ["osc11"], "the terminal must actually be queried");
}

#[test]
fn an_osc11_dark_background_leaves_the_dark_boot_alone() {
    let mut controller = ThemeController::boot_from_env(None);
    // `#1e1e1e` is dark: same conclusion the fallback reached, so no repaint is requested.
    let probe = ScriptedProbe::background((0x1e, 0x1e, 0x1e));
    assert!(controller.sync_with_terminal(&probe, TIMEOUT, "").is_none());
    assert_eq!(controller.active_name(), "dark");
}

// ---------------------------------------------------------------------------------------------
// Precedence, per Pi's three `applyFromSettings` branches.
// ---------------------------------------------------------------------------------------------

#[test]
fn the_terminal_reply_outranks_colorfgbg() {
    // `COLORFGBG=15;0` says the background is palette index 0 (black). The terminal itself says
    // white. Pi asks first and only falls back on failure (`theme.ts:773-787`), so light wins.
    let mut controller = ThemeController::boot_from_env(None);
    let probe = ScriptedProbe::background((0xff, 0xff, 0xff));
    assert!(controller.sync_with_terminal(&probe, TIMEOUT, "15;0").is_some());
    assert_eq!(controller.active_name(), "light");

    // And with no reply, `COLORFGBG` is still honoured: index 15 is white ⇒ light.
    let mut controller = ThemeController::boot_from_env(None);
    assert!(controller.sync_with_terminal(&NoTerminalProbe, TIMEOUT, "0;15").is_some());
    assert_eq!(controller.active_name(), "light");
}

#[test]
fn an_auto_setting_prefers_the_color_scheme_report() {
    // `detectTerminalThemeForAuto` (`theme.ts:790-801`) tries DSR `?996` FIRST; a terminal that
    // reports `light` picks the light arm without any OSC 11 round trip.
    let mut controller = ThemeController::boot_from_env(Some("light/dark"));
    assert_eq!(controller.active_name(), "dark", "no hint ⇒ the dark arm at boot");
    let probe = ScriptedProbe::color_scheme(TerminalTheme::Light);
    assert!(controller.sync_with_terminal(&probe, TIMEOUT, "").is_some());
    assert_eq!(controller.active_name(), "light", "auto setting follows the ?997 report");
    assert!(controller.auto_sync(), "an auto setting arms Pi's color-scheme sync");
    assert_eq!(probe.asked.borrow().as_slice(), ["dsr996"], "OSC 11 is not needed once ?997 answers");
}

#[test]
fn an_auto_setting_falls_through_to_osc11_when_dsr_is_unsupported() {
    let mut controller = ThemeController::boot_from_env(Some("light/dark"));
    // `color_scheme: None` = the terminal ignored `CSI ? 996 n`, as most do.
    let probe = ScriptedProbe::background((0xf5, 0xf5, 0xf5));
    assert!(controller.sync_with_terminal(&probe, TIMEOUT, "").is_some());
    assert_eq!(controller.active_name(), "light");
    assert_eq!(probe.asked.borrow().as_slice(), ["dsr996", "osc11"], "DSR first, then OSC 11");
}

#[test]
fn an_explicit_setting_is_never_second_guessed() {
    // Pi's middle branch (`theme-controller.ts:46-49`) applies the name and returns — no query.
    let mut controller = ThemeController::boot_from_env(Some("dark"));
    let probe = ScriptedProbe::background((0xff, 0xff, 0xff));
    assert!(controller.sync_with_terminal(&probe, TIMEOUT, "").is_none());
    assert_eq!(controller.active_name(), "dark", "an explicit theme is not overridden by the probe");
    assert!(probe.asked.borrow().is_empty(), "the terminal must not be queried at all");
    assert!(!controller.auto_sync());
}

// ---------------------------------------------------------------------------------------------
// Persistence + provenance (Pi only writes a HIGH-confidence detection back to settings).
// ---------------------------------------------------------------------------------------------

#[test]
fn only_a_high_confidence_detection_is_offered_for_persistence() {
    let mut queried = ThemeController::boot_from_env(None);
    queried.sync_with_terminal(&ScriptedProbe::background((0xff, 0xff, 0xff)), TIMEOUT, "");
    assert_eq!(queried.theme_to_persist(), Some("light"), "an OSC 11 answer is high confidence");

    // Nothing answered and no `COLORFGBG` ⇒ Pi's low-confidence `fallback`; never written to disk.
    let mut guessed = ThemeController::boot_from_env(None);
    guessed.sync_with_terminal(&NoTerminalProbe, TIMEOUT, "");
    assert_eq!(guessed.theme_to_persist(), None, "a fallback guess must not be persisted");
}

#[test]
fn detection_records_pis_source_and_confidence() {
    let queried =
        detect_terminal_background_theme(&ScriptedProbe::background((0x28, 0x28, 0x28)), TIMEOUT, "");
    assert_eq!(queried.theme, TerminalTheme::Dark);
    assert_eq!(queried.source, TerminalThemeSource::TerminalBackground);
    assert_eq!(queried.confidence, DetectionConfidence::High);
    assert_eq!(queried.detail, "OSC 11 background rgb(40, 40, 40)");

    let from_env = detect_terminal_background_theme(&NoTerminalProbe, TIMEOUT, "0;15");
    assert_eq!(from_env.theme, TerminalTheme::Light);
    assert_eq!(from_env.source, TerminalThemeSource::ColorFgBg);
    assert_eq!(from_env.detail, "background color index 15");

    let nothing = detect_terminal_background_theme(&NoTerminalProbe, TIMEOUT, "");
    assert_eq!(nothing.theme, TerminalTheme::Dark);
    assert_eq!(nothing.source, TerminalThemeSource::Fallback);
    assert_eq!(nothing.confidence, DetectionConfidence::Low);

    // The auto path returns the polarity only (Pi `detectTerminalThemeForAuto`).
    assert_eq!(
        detect_terminal_theme_for_auto(&ScriptedProbe::color_scheme(TerminalTheme::Light), TIMEOUT, ""),
        TerminalTheme::Light
    );
}

// ---------------------------------------------------------------------------------------------
// The safety contract of hard constraint 5, asserted rather than asserted-about.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_silent_terminal_costs_nothing_and_changes_nothing() {
    // The whole point of the timeout: a terminal that never answers must leave detection exactly
    // where `COLORFGBG` left it, and must not stall the boot.
    let started = std::time::Instant::now();
    let mut controller = ThemeController::boot_from_env(None);
    assert!(controller.sync_with_terminal(&NoTerminalProbe, Duration::from_secs(5), "").is_none());
    assert_eq!(controller.active_name(), "dark");
    assert!(started.elapsed() < Duration::from_secs(1), "a silent terminal must not stall the boot");
}
