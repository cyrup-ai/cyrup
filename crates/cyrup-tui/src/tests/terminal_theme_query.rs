//! TUI-004 / TUI-130 / TUI-133 — the theme follows what the terminal says its colours are.
//!
//! Pi v1.0.0 asks the terminal for its colours in one batch — OSC 10, OSC 11, sixteen OSC 4 and a
//! trailing DA1 (`tui/src/tui.ts:168-172`) — and feeds the answer to the theme controller
//! (`coding-agent/src/modes/interactive/theme/theme-controller.ts`): the `system` theme is generated
//! from it, `""` tokens take the terminal's defaults, and light/dark detection classifies the
//! background it reports. It no longer writes the detection back to `settings.json`.
//!
//! The tests drive the real `ThemeController::request_terminal_colors` with a scripted probe and
//! assert against the **assembled, rendered buffer** — the generated palette has to reach real
//! cells — plus the controller state that decides it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::cell::RefCell;
use std::time::Duration;

use cyrup_resources::color::Rgb;

use crate::{
    App, ColorMode, LateColors, NoTerminalProbe, TerminalColors, TerminalProbe, TerminalTheme,
    ThemeController, UiTheme, detect_color_fg_bg_theme, detect_terminal_theme,
};
use ratatui::backend::TestBackend;
use ratatui::style::Color;

const WHITE: Rgb = Rgb::new(0xfa, 0xfa, 0xfa);
const NEAR_BLACK: Rgb = Rgb::new(0x1e, 0x1e, 0x1e);

/// A terminal that answers exactly what the script says, recording that it was asked.
#[derive(Default)]
struct ScriptedProbe {
    colors: TerminalColors,
    asked: RefCell<usize>,
}

impl ScriptedProbe {
    fn background(rgb: Rgb) -> Self {
        ScriptedProbe {
            colors: TerminalColors {
                background: Some(rgb),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

impl TerminalProbe for ScriptedProbe {
    fn query_terminal_colors(
        &self,
        _timeout: Duration,
        _on_late_reply: Option<LateColors>,
    ) -> TerminalColors {
        *self.asked.borrow_mut() += 1;
        self.colors
    }
}

/// Whether any cell in the assembled buffer carries `color` as its foreground.
fn any_fg(app: &App<TestBackend>, color: Color) -> bool {
    app.terminal()
        .backend()
        .buffer()
        .content()
        .iter()
        .any(|c| c.fg == color)
}

fn boot(setting: Option<&str>, env: TerminalTheme) -> ThemeController {
    ThemeController::boot(setting, ColorMode::TrueColor, env)
}

// ---------------------------------------------------------------------------------------------
// The observable one: the generated theme reaches real cells, and follows the terminal.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_reported_light_background_repaints_the_assembled_app_in_the_generated_light_theme() {
    // No `settings.theme` at all: the theme is `system` (TUI-130), generated from whatever the
    // terminal reports. Truecolor is pinned so the generated colours stay RGB and comparable.
    let mut controller = boot(None, TerminalTheme::Dark);
    assert_eq!(controller.active_name(), "system");
    let mut app = App::new(TestBackend::new(100, 30), controller.theme()).unwrap();
    app.status_mut().set_model("anthropic/claude-opus-4-6");
    app.status_mut().set_thinking_level("high");
    app.draw().unwrap();
    // Before the terminal answers the theme is grayscale: no token has a colour of its own.
    assert!(controller.terminal_colors_pending());
    assert!(
        app.state()
            .theme
            .roles
            .values()
            .all(|color| *color == Color::Reset),
        "a pending system theme is grayscale"
    );

    let probe = ScriptedProbe::background(WHITE);
    let name = controller
        .request_terminal_colors(&probe, None)
        .expect("a reported background changes the theme");
    assert_eq!(name, "system", "the name to reload is still `system`");
    app.set_theme(controller.theme());
    app.draw().unwrap();

    assert!(
        !controller.terminal_colors_pending(),
        "the answer ends the grayscale"
    );
    assert_eq!(
        *probe.asked.borrow(),
        1,
        "the terminal must actually be queried"
    );
    let theme = &app.state().theme;
    assert_eq!(theme.name, "system");
    assert_eq!(theme.appearance(), cyrup_resources::Appearance::Light);
    // Generated for a light terminal: the body text is dark, and it reached rendered cells.
    let Some(Color::Rgb(r, g, b)) = theme.foreground else {
        panic!("a reported background with no foreground generates the body text colour");
    };
    assert!(
        u32::from(r) + u32::from(g) + u32::from(b) < 3 * 128,
        "text on a light terminal is dark, got {:?}",
        theme.foreground
    );
    assert!(
        any_fg(&app, Color::Rgb(r, g, b)),
        "the generated text colour never reached a rendered cell"
    );
    assert_eq!(controller.terminal_theme(), TerminalTheme::Light);
}

#[test]
fn a_reported_dark_background_generates_a_dark_theme_with_light_text() {
    let mut controller = boot(None, TerminalTheme::Light);
    let probe = ScriptedProbe::background(NEAR_BLACK);
    controller.request_terminal_colors(&probe, None).unwrap();
    let theme = controller.theme();
    assert_eq!(theme.appearance(), cyrup_resources::Appearance::Dark);
    let Some(Color::Rgb(r, g, b)) = theme.foreground else {
        panic!("body text is generated");
    };
    assert!(
        u32::from(r) + u32::from(g) + u32::from(b) > 3 * 128,
        "{r} {g} {b}"
    );
    assert_eq!(
        controller.terminal_theme(),
        TerminalTheme::Dark,
        "the reported background outranks the environment's guess"
    );
}

#[test]
fn a_terminal_that_reports_nothing_still_ends_the_grayscale_and_gets_ansi_indices() {
    let mut controller = boot(None, TerminalTheme::Dark);
    // Nothing reported on a controller that has never applied colours is still a change: pi's
    // `previous &&` guard needs a previous value, and there is none.
    assert_eq!(
        controller.request_terminal_colors(&NoTerminalProbe, None),
        Some("system".to_string())
    );
    assert!(!controller.terminal_colors_pending());
    let theme = controller.theme();
    assert_eq!(
        theme.accent,
        Some(Color::Indexed(5)),
        "violet is palette slot 5"
    );
    assert_eq!(
        theme.error,
        Some(Color::Indexed(1)),
        "red is palette slot 1"
    );
    // A second silence changes nothing, so nothing is re-rendered.
    assert_eq!(
        controller.request_terminal_colors(&NoTerminalProbe, None),
        None
    );
}

// ---------------------------------------------------------------------------------------------
// Which theme, per Pi's `resolveThemeName` / `applyFromSettings`.
// ---------------------------------------------------------------------------------------------

#[test]
fn with_no_theme_setting_the_theme_is_system_whatever_the_environment_says() {
    for env in [TerminalTheme::Dark, TerminalTheme::Light] {
        let controller = boot(None, env);
        assert_eq!(controller.active_name(), "system", "env polarity {env:?}");
        assert!(
            controller.auto_sync(),
            "the system theme follows the terminal's appearance (`setAutoSync`)"
        );
    }
    // A setting that resolves to nothing is the system theme too.
    assert_eq!(
        boot(Some("a/b/c"), TerminalTheme::Dark).active_name(),
        "system"
    );
}

#[test]
fn a_light_dark_pair_follows_the_polarity_of_the_reported_background() {
    let mut controller = boot(Some("light/dark"), TerminalTheme::Dark);
    assert_eq!(
        controller.active_name(),
        "dark",
        "no colours yet ⇒ the environment's guess"
    );
    assert!(controller.auto_sync(), "a pair arms colour-scheme sync");

    let probe = ScriptedProbe::background(WHITE);
    let name = controller.request_terminal_colors(&probe, None).unwrap();
    assert_eq!(name, "light");
    assert_eq!(controller.active_name(), "light");

    // The terminal flips to a dark background (a late reply, say): the pair follows.
    let name = controller
        .apply_terminal_colors(TerminalColors {
            background: Some(NEAR_BLACK),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(name, "dark");
    assert_eq!(controller.active_name(), "dark");
}

#[test]
fn an_explicit_setting_is_applied_verbatim_but_the_terminal_is_still_asked() {
    // v1.0.0 queries the terminal whatever the setting: its colours feed the `""` tokens and the
    // appearance of a theme that has none of its own.
    let mut controller = boot(Some("dark"), TerminalTheme::Light);
    assert_eq!(controller.active_name(), "dark");
    let probe = ScriptedProbe::background(WHITE);
    let reload = controller.request_terminal_colors(&probe, None);
    assert_eq!(
        reload.as_deref(),
        Some("dark"),
        "the name is untouched by the colours"
    );
    assert_eq!(
        controller.active_name(),
        "dark",
        "an explicit theme is not overridden by the probe"
    );
    assert_eq!(
        *probe.asked.borrow(),
        1,
        "the terminal is queried regardless"
    );
    assert!(
        !controller.auto_sync(),
        "an explicit built-in does not arm sync"
    );
    assert!(!controller.terminal_colors_pending());
}

// ---------------------------------------------------------------------------------------------
// The colours the controller remembers, per Pi's `applyTerminalColors`.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_query_that_times_out_keeps_the_colours_an_earlier_one_reported() {
    let mut controller = boot(None, TerminalTheme::Dark);
    controller.request_terminal_colors(&ScriptedProbe::background(WHITE), None);
    // A later query reports nothing (timed out): the background is kept, not erased.
    assert_eq!(
        controller.request_terminal_colors(&NoTerminalProbe, None),
        None
    );
    assert_eq!(controller.terminal_colors().background, Some(WHITE));
    assert_eq!(controller.terminal_theme(), TerminalTheme::Light);
}

#[test]
fn newly_reported_colours_are_merged_over_the_remembered_ones() {
    let mut controller = boot(None, TerminalTheme::Dark);
    controller.apply_terminal_colors(TerminalColors {
        background: Some(WHITE),
        ..Default::default()
    });
    controller
        .apply_terminal_colors(TerminalColors {
            foreground: Some(Rgb::new(0x20, 0x20, 0x20)),
            ..Default::default()
        })
        .expect("a new foreground is a change");
    let kept = controller.terminal_colors();
    assert_eq!(kept.background, Some(WHITE));
    assert_eq!(kept.foreground, Some(Rgb::new(0x20, 0x20, 0x20)));
}

// ---------------------------------------------------------------------------------------------
// Pi's `detectTerminalTheme` / `detectColorFgBgTheme`.
// ---------------------------------------------------------------------------------------------

/// Each expectation is `detectColorFgBgTheme` from pi's `theme.ts` @v1.0.0, run on the same value.
#[test]
fn colorfgbg_is_classified_by_the_background_index_as_pi_classifies_it() {
    use TerminalTheme::{Dark, Light};
    for (value, expected) in [
        ("0;15", Some(Light)),
        ("15;0", Some(Dark)),
        ("7;0", Some(Dark)),
        ("0;7", Some(Light)),
        // Bright black (Solarized Dark's background) is dark.
        ("0;8", Some(Dark)),
        ("0;9", Some(Light)),
        ("0;15;", None),
        ("default;default", None),
        ("", None),
        ("0;16", None),
        ("0;007", None),
        ("0; 7 ", Some(Light)),
        ("12", Some(Light)),
        ("0;default;15", Some(Light)),
        (";", None),
        ("0;-1", None),
        ("0;1x", None),
    ] {
        assert_eq!(detect_color_fg_bg_theme(value), expected, "{value:?}");
    }
}

#[test]
fn the_reported_background_outranks_the_scheme_report_and_the_environment() {
    let white = TerminalColors {
        background: Some(WHITE),
        ..Default::default()
    };
    // A white background is light even when the environment and a 2031 report say dark.
    assert_eq!(
        detect_terminal_theme(&white, Some(TerminalTheme::Dark), Some(TerminalTheme::Dark)),
        TerminalTheme::Light
    );
    // Without a background: the terminal's own report, then `COLORFGBG`, then dark.
    let nothing = TerminalColors::default();
    assert_eq!(
        detect_terminal_theme(
            &nothing,
            Some(TerminalTheme::Light),
            Some(TerminalTheme::Dark)
        ),
        TerminalTheme::Light
    );
    assert_eq!(
        detect_terminal_theme(&nothing, None, Some(TerminalTheme::Light)),
        TerminalTheme::Light
    );
    assert_eq!(
        detect_terminal_theme(&nothing, None, None),
        TerminalTheme::Dark
    );
}

// ---------------------------------------------------------------------------------------------
// The safety contract of hard constraint 5, asserted rather than asserted-about.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_silent_terminal_costs_nothing_and_changes_no_choice() {
    // The whole point of the timeout: a terminal that never answers must leave the theme choice
    // exactly where the setting left it, and must not stall the boot.
    let started = std::time::Instant::now();
    let mut controller = boot(Some("dark"), TerminalTheme::Dark);
    controller.request_terminal_colors(&NoTerminalProbe, None);
    assert_eq!(controller.active_name(), "dark");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "a silent terminal must not stall the boot"
    );
}

/// `UiTheme::builtin("system")` no longer degrades to dark: an unknown name is the system theme,
/// the `applyThemeName` fallback.
#[test]
fn an_unknown_theme_name_projects_the_system_theme_not_dark() {
    assert_eq!(UiTheme::builtin("does-not-exist").name, "system");
    assert!(UiTheme::builtin_named("does-not-exist").is_none());
    assert_eq!(UiTheme::builtin("system").name, "system");
}
