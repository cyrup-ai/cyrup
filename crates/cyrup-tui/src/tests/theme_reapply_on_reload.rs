//! TUI-004 (the `/reload` half) — a session swap must re-resolve and re-apply the render theme.
//!
//! pi pairs `setRegisteredThemes(this.session.resourceLoader.getThemes().themes)` with
//! `await this.themeController.applyFromSettings()` at both of its call sites — the
//! `setRebindSession` hook (`modes/interactive/interactive-mode.ts:1977` then `:578` @v0.84.4) and
//! `handleReloadCommand` (`:5985` then `:5987`) — so `/reload` picks up a `settings.theme` the user
//! edited, a custom theme file rewritten under an unchanged name, and a theme an extension newly
//! registered. cyrup's swap arm re-read `outputPad`, `hideThinking`, `showImages`,
//! `imageWidthCells` and `editorPaddingX` and never touched the theme, which was resolved once at
//! boot in the composition root's stack frame and then unreachable.
//!
//! The MODE-`2031` half of TUI-004 is deliberately not ported and is not tested here: crossterm
//! surfaces no event for the unsolicited `CSI ? 997 ; N n` notification, so enabling it would feed
//! the push into `event::read()` as stray keystrokes. That reasoning lives on
//! [`crate::ThemeController::auto_sync`].

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    // The call-site guard below slices the run-loop source, exactly as
    // `startup_resources_panel.rs` and `run_loop_swap_arm_reachable.rs` do.
    clippy::string_slice
)]

use cyrup_resources::theme::ThemeData;
use cyrup_resources::{
    ResourceKey, ResourceOrigin, ResourceRegistry, ResourceScope, ResourceSet, Theme,
    builtin_themes,
};
use ratatui::backend::TestBackend;
use ratatui::style::Color;

use crate::{App, ColorMode, TerminalTheme, ThemeController, UiTheme};

/// A file-backed custom theme, the shape `cyrup-resources`' discovery produces for a `themes/*.json`
/// the user authored. Only the two roles the assertions read are declared; `from_theme_data` leaves
/// every undeclared role inheriting, which is the production behaviour for a partial theme.
fn custom_theme(name: &str, accent: &str) -> Theme {
    let data: ThemeData = serde_json::from_str(&format!(
        r##"{{"name": "{name}", "colors": {{"text": "#ffffff", "accent": "{accent}"}}}}"##
    ))
    .unwrap();
    Theme {
        key: ResourceKey::normalize(name),
        data,
        origin_path: Some(std::path::PathBuf::from(format!("/themes/{name}.json"))),
        scope: ResourceScope::Global,
        origin: ResourceOrigin::Builtin,
    }
}

/// A swapped-in session's discovered themes: the compiled-in built-ins (which real discovery always
/// seeds the candidate list with) plus whatever this reload found.
fn registry(extra: Vec<Theme>) -> ResourceRegistry {
    let mut all = builtin_themes();
    all.extend(extra);
    ResourceRegistry {
        themes: ResourceSet::build(all),
        ..ResourceRegistry::default()
    }
}

/// An app booted the way the composition root boots it: a `ThemeController` resolved from
/// `settings.theme` at startup, handed over so the run loop can re-run `applyFromSettings`.
fn booted(setting: Option<&str>, terminal: TerminalTheme) -> App<TestBackend> {
    let controller = ThemeController::boot(setting, ColorMode::TrueColor, terminal);
    let mut app = App::new(TestBackend::new(80, 24), controller.theme()).unwrap();
    app.set_theme_controller(controller);
    app
}

// ---------------------------------------------------------------------------
// `ThemeController::apply_from_settings` — pi `theme-controller.ts:57-81` @v0.84.4
// ---------------------------------------------------------------------------

/// Branch 2 (`:68-72`): an explicit setting is applied verbatim and turns auto-sync off.
#[test]
fn apply_from_settings_takes_an_explicit_name_verbatim_and_disables_auto_sync() {
    let mut controller = ThemeController::boot(
        Some("light/dark"),
        ColorMode::TrueColor,
        TerminalTheme::Dark,
    );
    assert!(
        controller.auto_sync(),
        "an auto pair boots with auto-sync armed"
    );

    let name = controller.apply_from_settings(Some("solarized"));

    assert_eq!(name, "solarized", "pi `applyThemeName(themeSetting, true)`");
    assert_eq!(controller.active_name(), "solarized");
    assert!(
        !controller.auto_sync(),
        "pi calls `setAutoSync(false)` before the explicit branch (`theme-controller.ts:68`)"
    );
}

/// Branch 1 (`:61-66`): an `auto` pair resolves against the terminal polarity and re-arms auto-sync.
/// The polarity is the one detected at boot — see the [CYRUP-DELTA] on `apply_from_settings`.
#[test]
fn apply_from_settings_resolves_an_auto_pair_against_the_boot_polarity() {
    let mut dark_term = ThemeController::boot(Some("x"), ColorMode::TrueColor, TerminalTheme::Dark);
    assert_eq!(dark_term.apply_from_settings(Some("light/dark")), "dark");
    assert!(dark_term.auto_sync(), "pi `setAutoSync(true)` (`:63`)");

    let mut light_term =
        ThemeController::boot(Some("x"), ColorMode::TrueColor, TerminalTheme::Light);
    assert_eq!(light_term.apply_from_settings(Some("light/dark")), "light");
}

/// Branch 3 (`:74-80`) minus the probe: an unset setting falls back to the polarity's own theme
/// name, which is the value pi's `detectTerminalBackgroundTheme` would have re-answered.
#[test]
fn apply_from_settings_falls_back_to_the_terminal_polarity_when_the_setting_is_cleared() {
    let mut controller = ThemeController::boot(
        Some("solarized"),
        ColorMode::TrueColor,
        TerminalTheme::Light,
    );
    assert_eq!(controller.apply_from_settings(None), "light");
    assert!(!controller.auto_sync(), "pi `setAutoSync(false)` (`:68`)");
}

/// Every apply bumps the generation, which is what invalidates the render caches — pi's
/// `applyThemeName` → `notifyChanged` → `ui.invalidate()` (`:126-139`) is likewise unconditional.
#[test]
fn apply_from_settings_bumps_the_generation_even_when_the_name_is_unchanged() {
    let mut controller =
        ThemeController::boot(Some("dark"), ColorMode::TrueColor, TerminalTheme::Dark);
    let before = controller.theme().generation;
    controller.apply_from_settings(Some("dark"));
    assert!(
        controller.theme().generation > before,
        "an unchanged NAME is not an unchanged THEME — the file it names may have been rewritten"
    );
}

// ---------------------------------------------------------------------------
// `App::reapply_theme_from_settings` — the swap arm's half
// ---------------------------------------------------------------------------

/// The core of the item: `/reload` after editing `settings.theme` repaints.
///
/// RED before the fix — `App` held no controller and the swap arm had no theme call site at all, so
/// the app stayed on its boot theme for the life of the process.
#[test]
fn reload_repaints_when_settings_theme_changed_on_disk() {
    let mut app = booted(Some("dark"), TerminalTheme::Dark);
    let dark = UiTheme::builtin("dark");
    let light = UiTheme::builtin("light");
    assert_ne!(
        dark.foreground, light.foreground,
        "the two built-ins must differ or the assertion below is vacuous"
    );
    assert_eq!(app.state().theme.foreground, dark.foreground);

    app.reapply_theme_from_settings(Some("light"), &registry(Vec::new()));

    assert_eq!(
        app.state().theme.foreground,
        light.foreground,
        "pi re-reads `settings.theme` and re-applies it on every rebind \
         (`interactive-mode.ts:578`, `:5987` @v0.84.4)"
    );
}

/// The second half of the item's Impact: a **custom** theme file the reload discovered has to be
/// loadable by name. `UiTheme::builtin` alone silently degrades an unknown name to dark, so the
/// resolution has to go through the swapped-in session's resource set.
#[test]
fn reload_repaints_from_a_custom_theme_the_reloaded_session_discovered() {
    let mut app = booted(Some("dark"), TerminalTheme::Dark);

    app.reapply_theme_from_settings(
        Some("midnight"),
        &registry(vec![custom_theme("midnight", "#010203")]),
    );

    assert_eq!(
        app.state().theme.accent,
        Some(Color::Rgb(0x01, 0x02, 0x03)),
        "a file-backed custom theme must resolve out of the session's discovered set, not \
         degrade to the built-in dark fallback"
    );
}

/// The case a change-gated implementation gets wrong, and the one `/reload` exists for: the user
/// edited the theme FILE, not the setting. pi's `applyThemeName` re-runs `setTheme(name)`
/// unconditionally (`theme-controller.ts:70`, `:126-135`), which re-reads the theme.
///
/// RED before the fix for the same reason as the test above; RED again against any implementation
/// that returns early when the resolved name is unchanged.
#[test]
fn reload_repaints_when_the_theme_file_changed_under_an_unchanged_name() {
    let mut app = booted(Some("midnight"), TerminalTheme::Dark);
    app.reapply_theme_from_settings(
        Some("midnight"),
        &registry(vec![custom_theme("midnight", "#010203")]),
    );
    assert_eq!(app.state().theme.accent, Some(Color::Rgb(0x01, 0x02, 0x03)));

    // The user edits `themes/midnight.json` and runs `/reload`: same name, new content.
    app.reapply_theme_from_settings(
        Some("midnight"),
        &registry(vec![custom_theme("midnight", "#0a0b0c")]),
    );

    assert_eq!(
        app.state().theme.accent,
        Some(Color::Rgb(0x0a, 0x0b, 0x0c)),
        "an unchanged theme NAME must still re-read the theme — this is the `/reload` case"
    );
}

/// A name that resolves to nothing degrades to dark, which is pi's `applyThemeName` failure path
/// (`activeThemeName = "dark"`, `theme-controller.ts:126-135`) — never a panic, never a stale theme
/// silently kept.
#[test]
fn reload_falls_back_to_dark_when_the_named_theme_no_longer_resolves() {
    let mut app = booted(Some("midnight"), TerminalTheme::Dark);
    app.reapply_theme_from_settings(
        Some("midnight"),
        &registry(vec![custom_theme("midnight", "#010203")]),
    );
    assert_eq!(
        app.state().theme.accent,
        Some(Color::Rgb(0x01, 0x02, 0x03)),
        "the custom theme must be live before the deletion can be observed"
    );

    // The user deleted `themes/midnight.json` and reloaded; `settings.theme` still names it.
    app.reapply_theme_from_settings(Some("midnight"), &registry(Vec::new()));

    assert_eq!(
        app.state().theme.foreground,
        UiTheme::builtin("dark").foreground,
        "pi falls back to the dark theme when the named theme fails to load"
    );
}

/// The re-projection is the app's, not the controller's: a 256-color session must keep indexed
/// colors after a reload, exactly as it does after a live `/theme` switch.
#[test]
fn reload_reprojects_through_the_apps_color_mode() {
    let controller = ThemeController::boot(Some("dark"), ColorMode::Ansi256, TerminalTheme::Dark);
    let mut app = App::new(TestBackend::new(80, 24), controller.theme()).unwrap();
    app.set_theme_controller(controller);
    assert_eq!(app.color_mode(), ColorMode::Ansi256);

    app.reapply_theme_from_settings(
        Some("midnight"),
        &registry(vec![custom_theme("midnight", "#010203")]),
    );

    let expected = UiTheme::from_theme_data(&custom_theme("midnight", "#010203").data, 0)
        .with_color_mode(ColorMode::Ansi256)
        .accent;
    assert!(
        matches!(expected, Some(Color::Indexed(_))),
        "the fixture must project to an indexed color or this assertion is vacuous"
    );
    assert_eq!(
        app.state().theme.accent,
        expected,
        "a 256-color session must be handed the RELOADED theme's role, quantized — not a \
         truecolor value and not the boot theme's"
    );
}

/// An app whose launcher never booted a controller has no `settings.theme` to answer from and keeps
/// the theme it was constructed with — the pre-TUI-004 behaviour, which every harness relies on.
#[test]
fn an_app_with_no_controller_keeps_the_theme_it_was_constructed_with() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::light()).unwrap();

    app.reapply_theme_from_settings(Some("dark"), &registry(Vec::new()));

    assert_eq!(
        app.state().theme.foreground,
        UiTheme::light().foreground,
        "no controller ⇒ no re-resolution"
    );
}

// ---------------------------------------------------------------------------
// The call site
// ---------------------------------------------------------------------------

/// The swap arm must actually make the call, and in pi's order.
///
/// Read from the source for the reason
/// `startup_resources_panel.rs::the_session_swap_arm_pushes_the_panel_after_the_shortcuts_and_before_the_replay`
/// gives: nothing in this crate can construct a `RunCtx` (it owns a runtime, an event stream and
/// nine channels), so the arm's internal ordering has no other coverage. Two orderings are
/// load-bearing:
///
/// * AFTER `install_extension_readbacks` — that is cyrup's `setRegisteredThemes`, and upstream pairs
///   the two in that order at both call sites (`interactive-mode.ts:1977`→`:578`, `:5985`→`:5987`
///   @v0.84.4); re-applying first would resolve the name against the OUTGOING session's themes.
/// * BEFORE the replay — the replay materialises transcript rows against the live theme, so a
///   re-theme after it would leave the restored conversation on the old palette.
#[test]
fn the_session_swap_arm_reapplies_the_theme_after_the_registry_and_before_the_replay() {
    const ARMS_SRC: &str = include_str!("../app/run_arms.rs");
    let offset = ARMS_SRC
        .find("pub(crate) async fn on_session_swapped")
        .expect("run_arms.rs must still define `on_session_swapped`");
    let body = &ARMS_SRC[offset..];
    let end = body
        .find("pub(crate) fn drain_over_budget_arm")
        .unwrap_or(body.len());
    let arm = &body[..end];

    let reapply = arm
        .find("self.reapply_theme_from_settings(")
        .unwrap_or_else(|| {
            panic!(
                "the `session_swapped` arm never re-applies the theme — `/reload` cannot pick up an \
                 edited `settings.theme` or an edited theme file (pi `applyFromSettings` at \
                 interactive-mode.ts:578 and :5987 @v0.84.4)"
            )
        });
    let readbacks = arm
        .find("self.install_extension_readbacks(")
        .unwrap_or_else(|| panic!("the `session_swapped` arm must rebuild the theme registry"));
    let replay = arm
        .find(".replay_items()")
        .unwrap_or_else(|| panic!("the `session_swapped` arm must still replay the conversation"));

    assert!(
        readbacks < reapply,
        "the theme registry must be rebuilt from the swapped-in session BEFORE the theme is \
         re-resolved against it — pi orders `setRegisteredThemes` then `applyFromSettings` at both \
         call sites (interactive-mode.ts:1977→:578, :5985→:5987 @v0.84.4)"
    );
    assert!(
        reapply < replay,
        "the theme must be re-applied BEFORE the replay materialises the restored conversation"
    );
}
