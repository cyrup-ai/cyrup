//! CFG-090 — the `terminal.{images,trueColor,hyperlinks}` settings reach capability detection.
//!
//! Pi `getTerminalCapabilityOverrides()` (`core/settings-manager.ts:1195-1203` @v0.87.1) feeds
//! `setCapabilityOverrides` (`packages/tui/src/terminal-image.ts:175-186`) at startup
//! (`main.ts:853`, `cli/startup-ui.ts:84`), in the interactive constructor (`:566`) and on every
//! `applyRuntimeSettings()` (`interactive-mode.ts:1993`); `getCapabilities()` spreads them over the
//! detection (`terminal-image.ts:160-169`) and `createTheme` reads `trueColor` from it
//! (`theme.ts:529`). Every assertion pins BOTH values of a field, so none depends on the ambient
//! terminal the tests happen to run under.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

use super::harness::caps_lock;
use crate::theme::ColorMode;
use crate::{App, CapabilityOverrides, ImageProtocol, UiTheme};
use cyrup_config::settings::{TerminalCapabilityOverrides, TerminalImagesOverride};
use ratatui::backend::TestBackend;

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(100, 24), UiTheme::dark()).unwrap()
}

fn overrides(
    images: Option<TerminalImagesOverride>,
    true_color: Option<bool>,
    hyperlinks: Option<bool>,
) -> TerminalCapabilityOverrides {
    TerminalCapabilityOverrides {
        images,
        true_color,
        hyperlinks,
    }
}

/// Leave the process-wide overrides and cache as the next test expects to find them.
fn clear() {
    crate::set_capability_overrides(CapabilityOverrides::default());
    crate::reset_capabilities_cache();
}

#[test]
fn settings_overrides_are_spread_over_the_detected_capabilities() {
    let _guard = caps_lock();
    for (images, hyperlinks, true_color) in [
        (TerminalImagesOverride::Kitty, true, true),
        (TerminalImagesOverride::Disabled, false, false),
        (TerminalImagesOverride::Iterm2, true, false),
    ] {
        crate::set_capability_overrides(CapabilityOverrides::from_settings(overrides(
            Some(images),
            Some(true_color),
            Some(hyperlinks),
        )));
        let caps = crate::cached_capabilities();
        let expected = match images {
            TerminalImagesOverride::Kitty => Some(ImageProtocol::Kitty),
            TerminalImagesOverride::Iterm2 => Some(ImageProtocol::Iterm2),
            TerminalImagesOverride::Disabled => None,
        };
        assert_eq!(caps.images, expected);
        assert_eq!(caps.hyperlinks, hyperlinks);
        assert_eq!(caps.true_color, true_color);
        assert_eq!(ColorMode::detect(), ColorMode::from_true_color(true_color));
    }
    clear();
}

/// Pi's `setCapabilityOverrides` early-returns on an unchanged set and otherwise drops the cache —
/// so a pinned record survives the former and not the latter.
#[test]
fn an_unchanged_set_is_a_no_op_and_a_changed_one_drops_the_cache() {
    let _guard = caps_lock();
    clear();
    let pinned = crate::TerminalCapabilities {
        images: Some(ImageProtocol::Kitty),
        true_color: true,
        hyperlinks: true,
    };
    crate::set_capabilities(pinned);
    assert!(!crate::set_capability_overrides(
        CapabilityOverrides::default()
    ));
    assert_eq!(crate::cached_capabilities(), pinned);

    let off = CapabilityOverrides::from_settings(overrides(
        Some(TerminalImagesOverride::Disabled),
        None,
        Some(false),
    ));
    assert!(crate::set_capability_overrides(off));
    let caps = crate::cached_capabilities();
    assert_eq!(caps.images, None);
    assert!(!caps.hyperlinks);
    assert!(!crate::set_capability_overrides(off));
    clear();
}

/// The startup path: the launcher sets the overrides, then `App::detect_image_support` publishes
/// what it detects UNDER them to the renderer and the transcript's OSC-8 gate.
#[test]
fn detect_image_support_publishes_the_overridden_capabilities() {
    let _guard = caps_lock();
    for hyperlinks in [true, false] {
        clear();
        crate::set_capability_overrides(CapabilityOverrides::from_settings(overrides(
            Some(TerminalImagesOverride::Disabled),
            None,
            Some(hyperlinks),
        )));
        let mut app = app();
        app.detect_image_support();
        let state = app.state();
        assert_eq!(state.capabilities.images, None);
        assert!(!state.image_renderer.is_graphical());
        assert_eq!(state.capabilities.hyperlinks, hyperlinks);
        assert_eq!(state.transcript.hyperlinks(), hyperlinks);
        assert_eq!(crate::hyperlinks_supported(), hyperlinks);
    }
    clear();
}

/// The rebind / `/reload` path (pi `applyRuntimeSettings`): a changed set re-derives the published
/// capabilities and the colour depth the theme projects through; an unchanged one leaves them.
#[test]
fn a_rebind_with_changed_settings_republishes_capabilities_and_colour_depth() {
    let _guard = caps_lock();
    clear();
    let mut app = app();
    app.detect_image_support();

    app.apply_terminal_capability_overrides(overrides(None, Some(true), Some(true)));
    assert!(app.state().transcript.hyperlinks());
    assert_eq!(app.state().color_mode, ColorMode::TrueColor);

    app.apply_terminal_capability_overrides(overrides(
        Some(TerminalImagesOverride::Disabled),
        Some(false),
        Some(false),
    ));
    assert!(!app.state().transcript.hyperlinks());
    assert!(!app.state().capabilities.hyperlinks);
    assert_eq!(app.state().capabilities.images, None);
    assert_eq!(app.state().color_mode, ColorMode::Ansi256);
    assert!(!crate::hyperlinks_supported());
    clear();
}

/// The `session_swapped` arm (every rebind and `/reload`) re-applies the overrides, and does so
/// BEFORE the theme is re-applied — pi's `rebindCurrentSession` runs `applyRuntimeSettings()`
/// (`interactive-mode.ts:2025` @v0.87.1) ahead of `themeController.applyFromSettings()` (`:578`),
/// so the re-applied theme projects through the new `trueColor`. Same source-order device as
/// `theme_reapply_on_reload::the_session_swap_arm_reapplies_the_theme_after_the_registry_and_before_the_replay`.
#[test]
fn the_session_swap_arm_reapplies_the_overrides_before_the_theme() {
    const ARMS_SRC: &str = include_str!("../app/run_arms.rs");
    let offset = ARMS_SRC
        .find("pub(crate) async fn on_session_swapped")
        .expect("run_arms.rs must still define `on_session_swapped`");
    let arm = &ARMS_SRC[offset..];
    let arm = &arm[..arm
        .find("pub(crate) fn drain_over_budget_arm")
        .unwrap_or(arm.len())];
    let overrides = arm
        .find("self.apply_terminal_capability_overrides(")
        .expect("the swap arm must re-apply `terminal.*` capability overrides");
    let theme = arm
        .find("self.reapply_theme_from_settings(")
        .expect("the swap arm must re-apply the theme");
    assert!(overrides < theme);
}
