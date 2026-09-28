//! SEAM-119 — `--use-theme <name[/name]>`, the one-run theme override (pi v0.84.4).
//!
//! pi hands the flag to `InteractiveThemeController` as `initialThemeSetting`
//! (`main.ts:944` @v0.87.1), which seeds `currentThemeSetting` (`theme-controller.ts:42`). Every
//! later resolution reads `currentThemeSetting ?? settingsManager.getThemeSetting()` (`:44`,
//! `:57`), so the override outlives a session rebind or `/reload` without ever being written to
//! settings, and it is retired only when the user switches theme in-app — `setThemeName` and
//! `setThemeSetting` both overwrite `currentThemeSetting` (`:88-99`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use cyrup_resources::{ResourceRegistry, ResourceSet, builtin_themes};
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

use crate::{
    App, AppCommand, ColorMode, NoTerminalProbe, TerminalTheme, ThemeApply, ThemeController,
};

fn registry() -> ResourceRegistry {
    ResourceRegistry {
        themes: ResourceSet::build(builtin_themes()),
        ..ResourceRegistry::default()
    }
}

/// The app as `run_interactive` boots it: `settings.theme` plus the `--use-theme` value.
fn booted(initial: Option<&str>, setting: Option<&str>) -> App<TestBackend> {
    let controller = ThemeController::boot_with_initial(
        initial,
        setting,
        ColorMode::TrueColor,
        TerminalTheme::Dark,
    );
    let mut app = App::new(TestBackend::new(80, 24), controller.theme()).unwrap();
    app.set_theme_controller(controller);
    app
}

fn active(app: &App<TestBackend>) -> String {
    app.theme_controller().unwrap().active_name().to_string()
}

/// The constructor: `resolveThemeSetting(initialThemeSetting ?? settings.theme, terminal)`.
#[test]
fn the_override_wins_the_boot_over_settings_theme() {
    let app = booted(Some("light"), Some("dark"));
    assert_eq!(active(&app), "light");
}

/// `name[/name]`: an auto pair resolves against the terminal polarity exactly as the setting does.
#[test]
fn an_auto_pair_override_resolves_against_the_terminal() {
    let app = booted(Some("light/dark"), Some("light"));
    assert_eq!(active(&app), "dark", "the terminal is dark");
}

/// `applyFromSettings` on a session rebind or `/reload` prefers the in-memory setting: the
/// override survives, even though the re-read `settings.theme` names another theme.
#[test]
fn the_override_survives_a_reapply_from_settings() {
    let mut app = booted(Some("light"), Some("dark"));
    let outcome = app.reapply_theme_from_settings(Some("dark"), &registry());
    assert!(
        matches!(&outcome, ThemeApply::Loaded(name) if name == "light"),
        "{outcome:?}"
    );
    assert_eq!(active(&app), "light");
}

/// MIRROR: with no override the re-read setting is honoured, as it always was.
#[test]
fn without_an_override_a_reapply_follows_settings() {
    let mut app = booted(None, Some("dark"));
    let outcome = app.reapply_theme_from_settings(Some("light"), &registry());
    assert!(
        matches!(&outcome, ThemeApply::Loaded(name) if name == "light"),
        "{outcome:?}"
    );
}

/// An explicit setting never probes and never asks to persist a detection, so a run started with
/// the override leaves `settings.theme` untouched even when the user has none.
#[test]
fn the_override_is_never_offered_for_persistence() {
    let mut controller = ThemeController::boot_with_initial(
        Some("light"),
        None,
        ColorMode::TrueColor,
        TerminalTheme::Dark,
    );
    let _ = controller.sync_with_terminal(&NoTerminalProbe, std::time::Duration::ZERO, "");
    assert_eq!(controller.active_name(), "light");
    assert_eq!(controller.theme_to_persist(), None);
}

async fn session() -> (TempDir, Arc<AgentSession>) {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    let provider: Arc<dyn cyrup_provider::Provider> =
        Arc::new(cyrup_provider::faux::FauxProvider::new());
    let session = SessionBuilder::new(provider, cfg).build().await.unwrap();
    (tmp, Arc::new(session))
}

/// An in-app switch (the `/settings` confirm and an extension's `setTheme` both reach the persist
/// arm) replaces the in-memory setting, so the NEXT reapply follows the user's choice rather than
/// reverting to the `--use-theme` value.
#[tokio::test]
async fn an_in_app_switch_retires_the_override() {
    let (_tmp, session) = session().await;
    let mut app = booted(Some("light"), None);
    app.execute_command(
        AppCommand::ApplySetting {
            id: "theme".to_string(),
            value: "dark".to_string(),
        },
        &session,
        None,
    )
    .await;
    assert_eq!(
        app.theme_controller().unwrap().current_setting(),
        Some("dark")
    );
    let outcome = app.reapply_theme_from_settings(Some("dark"), &registry());
    assert!(
        matches!(&outcome, ThemeApply::Loaded(name) if name == "dark"),
        "{outcome:?}"
    );
}
