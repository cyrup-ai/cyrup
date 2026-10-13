//! SEAM-164 (interactive half) — what an extension sets in `session_start` reaches the screen.
//!
//! A `session_start` handler that sets a status, a widget, the terminal title, the editor text or
//! the working message does it once. The interactive host used to announce the session in
//! `main.rs`, before `App::run` attached the effect drain, so every one of those calls was dropped
//! (the drain-less host keeps notices alone) and the footer never showed the status. pi binds the
//! mode's `uiContext` before `bindExtensions` emits `session_start` (`interactive-mode.ts:1971-1975`
//! @f1b2e77f5).
//!
//! Driven end to end: a real session with a real native extension, announced through
//! [`App::announce_with_effects`] — the call both `App::announce_session` (the composition root's)
//! and `App::run` make — then drained into [`App::apply_ui_effect`] as the run loop's drain arm
//! does, and painted.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use cyrup_core::ExtensionId;
use cyrup_ext::host::{HostServices, WidgetPlacement};
use cyrup_ext::{EventKind, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AgentSessionEvent, SessionBuilder, SessionConfig, UiEffect};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

use crate::{App, UiTheme};

/// Sets five kinds of interface state in `session_start`, and nothing else.
struct StateAtStart {
    services: OnceLock<Arc<dyn HostServices>>,
}

#[async_trait::async_trait]
impl NativeExtension for StateAtStart {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("state-at-start")
    }
    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        let _ = self.services.set(services);
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::SessionStart]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let (HostEvent::SessionStart { .. }, Some(s)) = (ev, self.services.get()) {
            s.set_status("ext", Some("ready at start"));
            s.set_widget(
                "w",
                Some(&["widget at start".to_string()]),
                WidgetPlacement::default(),
            );
            s.set_title("titled at start");
            s.set_editor_text("drafted at start", false);
            s.set_working_message(Some("warming at start"));
        }
        HookOutcome::Noop
    }
}

fn screen(app: &mut App<TestBackend>) -> String {
    app.draw().unwrap();
    let buf = app.terminal().backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(c) = buf.cell((x, y)) {
                out.push_str(c.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// RED at the base: the announce ran before the drain was attached, so none of the five arrived —
/// the footer had no `ready at start`, the editor was empty, the title unset.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_start_state_reaches_the_tui() {
    let tmp = TempDir::new().unwrap();
    let cwd: PathBuf = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(provider, cfg)
        .with_native_extension(Arc::new(StateAtStart {
            services: OnceLock::new(),
        }) as Arc<dyn NativeExtension>)
        .build()
        .await
        .unwrap();

    let (effect_tx, mut effects) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
    App::<TestBackend>::announce_with_effects(&session, effect_tx).await;

    let mut app = App::new(TestBackend::new(100, 24), UiTheme::dark()).unwrap();
    while let Ok(effect) = effects.try_recv() {
        app.apply_ui_effect(effect);
    }
    // The working message shows on the next run's band.
    app.ingest_event(&AgentSessionEvent::AgentStart);
    app.ingest_event(&AgentSessionEvent::TurnStart);
    let painted = screen(&mut app);

    assert!(
        painted.contains("ready at start"),
        "the footer shows the status:\n{painted}"
    );
    assert!(
        painted.contains("widget at start"),
        "the widget is drawn:\n{painted}"
    );
    assert!(
        painted.contains("drafted at start"),
        "the editor holds the text:\n{painted}"
    );
    assert!(
        painted.contains("warming at start"),
        "the working band carries the message:\n{painted}"
    );
    assert_eq!(
        app.state().terminal_title.as_deref(),
        Some("titled at start"),
        "the title is set (a `TestBackend` app writes no OSC 0)"
    );
}
