//! TUI-134 — a boot with no `theme` setting leaves `settings.json` untouched.
//!
//! pi v1.0.0 deleted the persist-the-detection path: with nothing configured the controller
//! resolves the generated `system` theme from what the terminal reports at every start, and
//! `applyFromSettings` no longer calls `settingsManager.setTheme(detection.theme)`. cyrup used to
//! write `"theme": "dark"` or `"theme": "light"` into the file on the first interactive start, which
//! pinned the user to a fixed palette for good.
//!
//! The seam is [`crate::interactive::settle_terminal_theme`], the single function the interactive
//! boot runs between the raw-mode switch and the first paint. The terminal here answers a light
//! background, so a regression that persisted the detection would have something to write.
//!
//! **No network.** The provider is the scripted faux double.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use cyrup_config::FileSettingsStore;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_resources::color::Rgb;
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use cyrup_tui::{
    App, LateColors, SYSTEM_THEME_NAME, TerminalColors, TerminalProbe, ThemeController, UiTheme,
};
use ratatui::backend::TestBackend;

use crate::interactive::settle_terminal_theme;

/// A terminal that reports a white background and nothing else.
struct LightTerminal;

impl TerminalProbe for LightTerminal {
    fn query_terminal_colors(
        &self,
        _timeout: Duration,
        _on_late_reply: Option<LateColors>,
    ) -> TerminalColors {
        TerminalColors {
            background: Some(Rgb::new(0xfa, 0xfa, 0xfa)),
            ..TerminalColors::default()
        }
    }
}

/// Deliberately not what a serialiser would emit (odd indentation, a trailing newline, key order),
/// so ANY rewrite of the file changes its bytes.
const SETTINGS_BYTES: &str =
    "{\n      \"quietStartup\": true,\n  \"defaultThinkingLevel\": \"low\"\n}\n\n";

/// A session whose settings are the FILE at `<agent>/settings.json`, holding [`SETTINGS_BYTES`].
async fn session_on_file_settings() -> (AgentSession, std::path::PathBuf, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let global = agent_dir.join("settings.json");
    std::fs::write(&global, SETTINGS_BYTES).unwrap();
    let store = Arc::new(FileSettingsStore::new(
        global.clone(),
        cwd.join(".cyrup").join("settings.json"),
    ));
    let mut config = SessionConfig::new(cwd, agent_dir);
    config.trust_override = Some(true);
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(provider, config)
        .settings_store(store)
        .build()
        .await
        .unwrap();
    (session, global, tmp)
}

/// **The row's Impact.** Boot with no theme setting, terminal answering: the theme is `system` and
/// the settings file is byte-identical afterwards.
///
/// **Red without the change:** the boot wrote the detected theme into `settings.theme` between the
/// colour query and the first paint, so the file gained `"theme": "light"` (or `"dark"`).
#[tokio::test]
async fn a_boot_with_no_theme_setting_leaves_settings_json_byte_identical() {
    let (session, global, _tmp) = session_on_file_settings().await;
    assert_eq!(
        session.services().settings.effective().theme_setting(),
        None,
        "the precondition is a file with no `theme` key"
    );

    let mut app = App::new(TestBackend::new(100, 24), UiTheme::dark()).unwrap();
    let mut controller = ThemeController::boot_from_env_with_initial(None, None);
    settle_terminal_theme(&mut app, &mut controller, &LightTerminal, &session, None);

    assert_eq!(
        controller.active_name(),
        SYSTEM_THEME_NAME,
        "no setting resolves to the generated system theme"
    );
    assert_eq!(
        app.theme_controller().map(ThemeController::active_name),
        Some(SYSTEM_THEME_NAME),
        "the app holds the settled controller"
    );
    assert_eq!(
        std::fs::read(&global).unwrap(),
        SETTINGS_BYTES.as_bytes(),
        "pi v1.0.0 never writes the detected theme back"
    );
}
