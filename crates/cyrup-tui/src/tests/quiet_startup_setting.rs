//! The `/settings` "Quiet startup" row — pi's tri-state `quiet-startup` item
//! (`settings-selector.ts:554-560`, `:923-925` @v1.0.0): `currentValue: String(config.quietStartup)`,
//! `values: ["true", "header", "false"]`, description "Disable verbose printing at startup (header:
//! keep only the startup header)", and `onQuietStartupChange(newValue === "header" ? "header" :
//! newValue === "true")` on the way back.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

use crate::app::AppCommand;
use crate::settings_selector::{FIELD_SEP, SettingRow, SettingsSelector};
use crate::{App, SelectKeymap, Selector, SelectorOutcome, UiTheme};
use ratatui::backend::TestBackend;

fn quiet_row(stored: &str) -> SettingRow {
    let eff = cyrup_session_svc::EffectiveSettings::from_settings(
        cyrup_session_svc::Settings::parse(&format!("{{\"quietStartup\": {stored}}}")).unwrap(),
    );
    crate::app::settings_rows(
        &eff,
        "dark",
        &crate::keymap::Keymap::default(),
        "medium",
        false,
        &cyrup_session_svc::EnvVars::default(),
    )
    .into_iter()
    .find(|r| r.id == "quietStartup")
    .expect("the quietStartup row")
}

/// The row's label, description, cycle and displayed value for every stored spelling.
#[test]
fn the_row_shows_pis_label_description_values_and_current_value() {
    let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    for (stored, shown) in [
        ("true", "true"),
        ("false", "false"),
        ("\"header\"", "header"),
        // `getQuietStartup` reads anything else as `false`.
        ("\"bogus\"", "false"),
        ("null", "false"),
    ] {
        let row = quiet_row(stored);
        assert_eq!(row.label, "Quiet startup");
        assert_eq!(row.value, shown, "stored {stored}");
        assert_eq!(
            row.cycle,
            strings(&["true", "header", "false"]),
            "pi's order"
        );
        assert_eq!(
            row.description.as_deref(),
            Some("Disable verbose printing at startup (header: keep only the startup header)")
        );
        assert!(row.submenu.is_none());
    }
    // Absent key: the default is `false`.
    let eff = cyrup_session_svc::EffectiveSettings::default();
    assert_eq!(
        eff.quiet_startup(),
        cyrup_config::settings::QuietStartup::Off
    );
}

/// Enter walks pi's cycle: true -> header -> false -> true, each step emitting the
/// `id<FIELD_SEP>value` payload the persist arm consumes.
#[test]
fn enter_cycles_true_header_false_and_wraps() {
    let key = crate::crossterm::event::KeyEvent::new(
        crate::crossterm::event::KeyCode::Enter,
        crate::crossterm::event::KeyModifiers::NONE,
    );
    let keymap = SelectKeymap::default();
    let mut sel = SettingsSelector::new("Settings", vec![quiet_row("true")]);
    for next in ["header", "false", "true"] {
        assert_eq!(
            sel.handle(&key, &keymap),
            SelectorOutcome::Apply(format!("quietStartup{FIELD_SEP}{next}"))
        );
    }
}

async fn file_backed_session() -> (TempDir, Arc<cyrup_session_svc::AgentSession>) {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut cfg = SessionConfig::new(cwd.clone(), agent_dir.clone());
    cfg.trust_override = Some(true);
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = Arc::new(
        SessionBuilder::new(provider, cfg)
            .settings_store(Arc::new(cyrup_config::FileSettingsStore::new(
                agent_dir.join("settings.json"),
                cwd.join(".cyrup").join("settings.json"),
            )))
            .build()
            .await
            .unwrap(),
    );
    (tmp, session)
}

/// A row confirmed as `header` is persisted as the STRING, `true`/`false` as booleans, and the
/// effective view reads each back as the same tri-state value — the file a pi session would read.
#[tokio::test]
async fn applying_the_row_persists_the_string_header_and_booleans() {
    let (tmp, session) = file_backed_session().await;
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    let stored = || -> serde_json::Value {
        let text = std::fs::read_to_string(tmp.path().join("agent").join("settings.json")).unwrap();
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["quietStartup"].clone()
    };
    for (value, json, expected) in [
        (
            "header",
            serde_json::json!("header"),
            cyrup_config::settings::QuietStartup::Header,
        ),
        (
            "true",
            serde_json::json!(true),
            cyrup_config::settings::QuietStartup::On,
        ),
        (
            "false",
            serde_json::json!(false),
            cyrup_config::settings::QuietStartup::Off,
        ),
        // `newValue === "header" ? "header" : newValue === "true"` — any other text is `false`,
        // never persisted as a string pi's getter would have to discard.
        (
            "garbage",
            serde_json::json!(false),
            cyrup_config::settings::QuietStartup::Off,
        ),
    ] {
        app.execute_command(
            AppCommand::ApplySetting {
                id: "quietStartup".to_string(),
                value: value.to_string(),
            },
            &session,
            None,
        )
        .await;
        assert_eq!(stored(), json, "row value {value}");
        // What the next launch (or `/reload`) reads back from the file just written.
        let text = std::fs::read_to_string(tmp.path().join("agent").join("settings.json")).unwrap();
        let eff = cyrup_session_svc::EffectiveSettings::from_settings(
            cyrup_session_svc::Settings::parse(&text).unwrap(),
        );
        assert_eq!(eff.quiet_startup(), expected, "row value {value}");
    }
}
