//! The `Model scope:` startup line, driven through the real boot helper over a real session.
//!
//! pi prints it from `init()` with `console.log`, before its TUI mounts (`interactive-mode.ts:940-953`
//! @v1.0.0), gated on `session.scopedModels.length > 0 && shouldShowStartupDetails()`. The helper
//! under test, [`crate::interactive::announce_model_scope`], reads the scoped set and
//! `quietStartup` off the session and the cycle key off the user's `keybindings.json`, and writes
//! to a plain stream that `run_interactive` points at stdout ahead of `App::into_stdout`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup_config::FileSettingsStore;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_sdk::core::{ModelId, ModelThinkingLevel};
use cyrup_session_svc::{AgentSession, ScopedModel, SessionBuilder, SessionConfig};
use cyrup_tui::UiTheme;

use crate::interactive::announce_model_scope;

/// A faux session whose `<agent>/settings.json` holds `settings`.
async fn session_with_settings(settings: &str) -> (AgentSession, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let global = agent_dir.join("settings.json");
    std::fs::write(&global, settings).unwrap();
    let store = Arc::new(FileSettingsStore::new(
        global,
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
    (session, tmp)
}

fn scope(session: &AgentSession, entries: &[(&str, Option<ModelThinkingLevel>)]) {
    session.set_scoped_models(
        entries
            .iter()
            .map(|(id, level)| {
                let mut model = FauxProvider::new().model().clone();
                model.id = ModelId::from(*id);
                ScopedModel {
                    model,
                    thinking_level: *level,
                }
            })
            .collect(),
    );
}

fn announced(session: &AgentSession, verbose: bool, keybindings: Option<&str>) -> (bool, String) {
    let mut out = Vec::new();
    let wrote =
        announce_model_scope(&mut out, session, verbose, keybindings, &UiTheme::dark()).unwrap();
    (wrote, String::from_utf8(out).unwrap())
}

#[tokio::test]
async fn a_scoped_session_announces_its_models_and_the_cycle_key() {
    let (session, _tmp) = session_with_settings("{}").await;
    scope(
        &session,
        &[("alpha", Some(ModelThinkingLevel::High)), ("beta", None)],
    );
    let (wrote, out) = announced(&session, false, None);
    assert!(wrote);
    assert!(
        out.contains("Model scope: alpha:high, beta") && out.contains(" (Ctrl+P to cycle)"),
        "{out:?}"
    );
    assert!(out.ends_with('\n'), "{out:?}");
}

#[tokio::test]
async fn an_unscoped_session_announces_nothing() {
    let (session, _tmp) = session_with_settings("{}").await;
    assert_eq!(announced(&session, true, None), (false, String::new()));
}

#[tokio::test]
async fn quiet_startup_true_and_header_hide_it_and_verbose_restores_it() {
    for (stored, hidden) in [("false", false), ("true", true), ("\"header\"", true)] {
        let (session, _tmp) =
            session_with_settings(&format!("{{\"quietStartup\": {stored}}}")).await;
        scope(&session, &[("alpha", None)]);
        let (wrote, out) = announced(&session, false, None);
        assert_eq!(wrote, !hidden, "quietStartup {stored}: {out:?}");
        assert_eq!(out.is_empty(), hidden, "quietStartup {stored}: {out:?}");
        // `--verbose` wins over every value.
        let (wrote, out) = announced(&session, true, None);
        assert!(
            wrote && out.contains("Model scope: alpha"),
            "{stored}: {out:?}"
        );
    }
}

#[tokio::test]
async fn the_hint_names_the_users_keybindings_json_binding() {
    let (session, _tmp) = session_with_settings("{}").await;
    scope(&session, &[("alpha", None)]);
    let (_, out) = announced(
        &session,
        false,
        Some(r#"{"app.model.cycleForward": "ctrl+g"}"#),
    );
    assert!(out.contains(" (Ctrl+G to cycle)"), "{out:?}");
    assert!(!out.contains("Ctrl+P"), "{out:?}");
}
