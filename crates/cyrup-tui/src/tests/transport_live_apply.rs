//! The `/settings` "Transport" row must reach the RUNNING agent, and it must offer all four of Pi's
//! transports.
//!
//! Pi's handler does two things (`coding-agent/src/modes/interactive/interactive-mode.ts:4213-4216`):
//!
//! ```ts
//! onTransportChange: (transport) => {
//!     this.settingsManager.setTransport(transport);   // persist
//!     this.session.agent.transport = transport;       // apply to the live agent
//! },
//! ```
//!
//! `agent.transport` is a mutable public field (`agent/src/agent.ts:204`, seeded at `:228`) that the
//! loop config is rebuilt from at every run start (`createLoopConfig`, `agent.ts:442`), so the very
//! next request streams with the chosen transport.
//!
//! cyrup only did the persist half. `AgentBuilder::transport` was called once, from the
//! Settings→Agent block in `cyrup-session-svc/src/builder.rs`, and its value was frozen in the
//! agent's `GenerationConfig` for the life of the process — cycling the row wrote JSON that nothing
//! re-read until restart. The row's choice set was also short one value: cyrup offered
//! `["auto","websocket","sse"]` against Pi's `["sse","websocket","websocket-cached","auto"]`
//! (`settings-selector.ts:505-510`), so `websocket-cached` — which the settings parser and
//! `parse_transport` both accept — was unreachable from the UI.
//!
//! The observable is `StreamOptions.transport` as the provider is actually called with: the same
//! struct an embedder-supplied `StreamFn` (`ProxyStreamFn`, which forwards it onto the proxy wire
//! body — `cyrup-agent/src/proxy.rs:580`, Pi `proxy.ts:110`) and every wire API read from.
//! `FauxProvider` is a real `Provider`, so these turns take the production stream path offline.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider, FauxResponseStep};
use cyrup_provider::{Provider, Transport};
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig, Settings};
use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{App, AppAction, AppCommand, InputEvent, SelectorKind, UiTheme};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

/// Every `StreamOptions.transport` the provider was called with, in call order.
type Seen = Arc<Mutex<Vec<Option<Transport>>>>;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(100, 40), UiTheme::dark()).unwrap()
}

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn buf_text(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// A real faux-provider-backed session whose provider records `StreamOptions.transport` on every
/// call. `turns` scripted steps are queued so a run can never fall through to an unscripted reply —
/// the tests assert on the recorded count, so a dry queue can never look like a pass.
async fn session_recording_transport(
    fx: &Fixture,
    cli: Settings,
    turns: usize,
) -> (Arc<AgentSession>, Seen) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let faux = Arc::new(FauxProvider::new());
    let steps: Vec<FauxResponseStep> = (0..turns)
        .map(|_| {
            let sink = seen.clone();
            FauxResponseStep::factory(move |_ctx, opts, _s, _m| {
                sink.lock().unwrap().push(opts.transport);
                faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)
            })
        })
        .collect();
    faux.set_response_steps(steps);
    let provider: Arc<dyn Provider> = faux;
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    let session =
        SessionBuilder::new(provider, cfg).cli_settings(cli).build().await.unwrap();
    (Arc::new(session), seen)
}

/// One completed turn through the real prompt path.
async fn turn(session: &Arc<AgentSession>, prompt: &str) {
    let _ = session.prompt(prompt).await.unwrap();
    session.wait_for_idle().await;
}

/// The transport recorded for turn `n` (0-based), with a message that names the real failure mode
/// (an unscripted/dry queue) rather than panicking on an index.
fn nth(seen: &Seen, n: usize) -> Option<Transport> {
    let got = seen.lock().unwrap().clone();
    assert!(
        got.len() > n,
        "expected at least {} provider call(s), the run only made {}: {got:?}",
        n + 1,
        got.len()
    );
    got[n]
}

// ---------------------------------------------------------------- the live-apply wire -----------

/// The core of the gap: cycling `/settings` → Transport must change what the NEXT request streams
/// with, on the already-running agent.
///
/// Before the fix the second turn still carried the build-time `Transport::Auto` because nothing
/// could mutate the agent's frozen `GenerationConfig`.
#[tokio::test]
async fn applying_the_transport_row_changes_the_next_request() {
    let fx = fixture();
    let (session, seen) = session_recording_transport(&fx, Settings::new(), 2).await;
    let mut app = app();

    // Baseline: an unconfigured session streams on Pi's `"auto"` default.
    turn(&session, "first").await;
    assert_eq!(
        nth(&seen, 0),
        Some(Transport::Auto),
        "baseline: an unset `transport` must stream as `auto`"
    );

    // The real user path — the command `/settings` Enter-cycling emits.
    app.execute_command(
        AppCommand::ApplySetting { id: "transport".to_string(), value: "sse".to_string() },
        &session,
        None,
    )
    .await;

    turn(&session, "second").await;
    assert_eq!(
        nth(&seen, 1),
        Some(Transport::Sse),
        "cycling the Transport row must reach the LIVE agent, so the next request streams as `sse`"
    );
}

/// `websocket-cached` — the value the row could not previously select — must survive the same wire
/// intact, distinctly from plain `websocket`.
#[tokio::test]
async fn websocket_cached_reaches_the_provider_distinctly() {
    let fx = fixture();
    let (session, seen) = session_recording_transport(&fx, Settings::new(), 2).await;
    let mut app = app();

    app.execute_command(
        AppCommand::ApplySetting {
            id: "transport".to_string(),
            value: "websocket-cached".to_string(),
        },
        &session,
        None,
    )
    .await;
    turn(&session, "first").await;
    assert_eq!(nth(&seen, 0), Some(Transport::WebsocketCached));

    app.execute_command(
        AppCommand::ApplySetting { id: "transport".to_string(), value: "websocket".to_string() },
        &session,
        None,
    )
    .await;
    turn(&session, "second").await;
    assert_eq!(
        nth(&seen, 1),
        Some(Transport::Websocket),
        "`websocket-cached` and `websocket` must not collapse into the same value"
    );
}

/// MIRROR CASE — deliberately green with or without the live-apply wire.
///
/// A `transport` configured BEFORE the session is built already reached the provider at HEAD (the
/// `builder.rs` Settings→Agent block). Keeping this assertion in the file proves the recording
/// harness, the faux step queue and the `StreamOptions.transport` observable all work, so a failure
/// in the tests above is the missing live wire and not a broken fixture.
#[tokio::test]
async fn mirror_build_time_transport_still_reaches_the_provider() {
    let fx = fixture();
    let (session, seen) =
        session_recording_transport(&fx, Settings::parse(r#"{"transport":"sse"}"#).unwrap(), 1)
            .await;
    turn(&session, "first").await;
    assert_eq!(
        nth(&seen, 0),
        Some(Transport::Sse),
        "a settings-configured transport must still be honoured at build time"
    );
}

// ------------------------------------------------------------- the real `/settings` row ---------

/// Drive the ACTUAL `/settings` selector — the rows built from the live effective settings, not a
/// hand-made list — and Enter-cycle the Transport row through a full lap.
///
/// Asserts two things at once: the row emits `ApplySetting { id: "transport", .. }` (the command the
/// live wire above hangs off), and its cycle set is Pi's full four-value union — `websocket-cached`
/// was missing before, so a user could never select it.
#[tokio::test]
async fn the_settings_row_cycles_through_all_four_pi_transports() {
    let fx = fixture();
    let (session, _seen) = session_recording_transport(&fx, Settings::new(), 1).await;
    let mut app = app();

    app.execute_command(AppCommand::OpenSelector(SelectorKind::Settings), &session, None).await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Settings));

    // Walk down to the Transport row by its RENDERED highlight (`→ ` prefix, select_list.rs:217)
    // rather than a hardcoded index, so reordering or adding settings rows cannot break this.
    let mut found = false;
    for _ in 0..256 {
        app.draw().unwrap();
        if buf_text(&app).lines().any(|l| l.trim_end().starts_with("→ Transport")) {
            found = true;
            break;
        }
        let _ = app.handle_input(&key(KeyCode::Down));
    }
    assert!(found, "the /settings grid never highlighted a `Transport` row:\n{}", buf_text(&app));

    // A full lap: four Enters must visit four distinct values and return to the start.
    let mut values: Vec<String> = Vec::new();
    for _ in 0..4 {
        match app.handle_input(&key(KeyCode::Enter)) {
            AppAction::Command(AppCommand::ApplySetting { id, value }) => {
                assert_eq!(id, "transport", "the highlighted row emitted the wrong setting id");
                // Feed it through the same handler the run loop uses, so this is the whole path.
                app.execute_command(
                    AppCommand::ApplySetting { id, value: value.clone() },
                    &session,
                    None,
                )
                .await;
                values.push(value);
            }
            other => panic!("Enter on the Transport row did not apply a setting: {other:?}"),
        }
    }

    let mut sorted = values.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted,
        vec![
            "auto".to_string(),
            "sse".to_string(),
            "websocket".to_string(),
            "websocket-cached".to_string()
        ],
        "the Transport row must cycle Pi's four `TransportSetting` values, saw {values:?}"
    );
}
