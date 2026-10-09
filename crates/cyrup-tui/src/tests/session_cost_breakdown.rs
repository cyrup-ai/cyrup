//! SESS-065 — `/session` prints the per-model cost breakdown whenever it says something the total
//! does not, not only when there is more than one row.
//!
//! Upstream is pi @ **v1.0.4** `coding-agent/src/modes/interactive/interactive-mode.ts`:
//!
//! ```text
//! const model = this.session.model;
//! const selectedModelKey = `${model?.provider}/${model?.id}`;          // :6670-6671
//! …
//! // A single entry repeats the total, unless it names a model other than the selected one.
//! if (usageBreakdown.length > 1 || usageBreakdown[0]?.key !== selectedModelKey) {   // :6715
//! ```
//!
//! cyrup's guard was `if breakdown.len() > 1` alone, so a one-row breakdown was always suppressed —
//! including the row that says the money went to a model the user did not select. The case the
//! virtual-model feature makes routine (every response attributed to the PHYSICAL model a router
//! picked) is reachable without any virtual model at all: an OpenRouter-style `responseModel`, or a
//! session whose only cost is the `Tools/summaries` bucket.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use crate::{App, AppCommand, UiTheme};
use cyrup_core::{AssistantMessage, Content, Cost, Message, StopReason, Usage};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxConfig, FauxModelDefinition, FauxProvider};
use cyrup_session::{NewSessionOpts, SessionLayout, SessionManager};
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig, SessionTarget};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

fn usage(cost: f64) -> Usage {
    Usage {
        input: 100,
        output: 20,
        total_tokens: 120,
        cost: Cost {
            input: cost,
            total: cost,
            ..Cost::default()
        },
        ..Usage::default()
    }
}

fn answered(model: &str, usage: Usage) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![Content::text("ok")],
        provider: "faux".into(),
        model: model.into(),
        api: "openai-completions".into(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        thinking_level: None,
        diagnostics: None,
        usage,
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
        duration_ms: None,
    })
}

/// A resumed session with `build`'s branch and `--model faux/<selected>`.
async fn session_with(
    selected: &str,
    build: impl FnOnce(&mut SessionManager),
) -> (TempDir, Arc<AgentSession>) {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    let sessions = tmp.path().join("sessions");
    for d in [&cwd, &agent_dir, &sessions] {
        std::fs::create_dir_all(d).unwrap();
    }
    let layout = SessionLayout::literal(sessions.clone(), cwd.clone());
    let mut m = SessionManager::create(&cwd, &layout, NewSessionOpts::default()).unwrap();
    build(&mut m);
    let path = m.session_file().unwrap().to_path_buf();
    drop(m);

    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        models: ["small", "large"]
            .iter()
            .map(|id| FauxModelDefinition::new(*id))
            .collect(),
        ..FauxConfig::default()
    }));
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg.session_dir = Some(sessions);
    cfg.target = SessionTarget::Resume(path);
    cfg.model_pattern = Some(format!("faux/{selected}"));
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .build()
        .await
        .unwrap();
    (tmp, Arc::new(session))
}

async fn session_info_text(session: &Arc<AgentSession>) -> String {
    let mut app = App::new(TestBackend::new(120, 40), UiTheme::dark()).unwrap();
    app.execute_session_command(AppCommand::SessionInfo, session, None)
        .await;
    app.draw().unwrap();
    app.scrollback_text()
}

/// A ONE-row breakdown naming a model other than the selection is PRINTED.
///
/// RED-PROVE: restore the guard to `if breakdown.len() > 1` — the `faux/large` row is absent and
/// `/session` reports a total with no indication of where it went.
#[tokio::test]
async fn a_single_breakdown_row_naming_another_model_is_printed() {
    let (_tmp, session) = session_with("small", |m| {
        m.append_message(answered("large", usage(0.25))).unwrap();
    })
    .await;
    assert_eq!(
        session.model().unwrap().model.as_str(),
        "small",
        "the selection must differ from the model that answered"
    );
    let breakdown = session.usage_cost_breakdown().await;
    assert_eq!(
        breakdown.len(),
        1,
        "the fixture must be a ONE-row breakdown"
    );
    assert_eq!(breakdown[0].key, "faux/large");

    let text = session_info_text(&session).await;
    assert!(
        text.contains("faux/large"),
        "the one-row breakdown was suppressed:\n{text}"
    );
}

/// REGRESSION GUARD — **not a red proof**: a one-row breakdown that NAMES the selection stays
/// suppressed, because it only repeats the total. Upstream's own comment.
#[tokio::test]
async fn a_single_row_naming_the_selected_model_is_suppressed() {
    let (_tmp, session) = session_with("large", |m| {
        m.append_message(answered("large", usage(0.25))).unwrap();
    })
    .await;
    assert_eq!(session.model().unwrap().model.as_str(), "large");
    let text = session_info_text(&session).await;
    assert!(
        !text.contains("faux/large |"),
        "a row that repeats the total was printed:\n{text}"
    );
}

/// REGRESSION GUARD — **not a red proof**: it pins the `?.`-vs-`is_some_and` equivalence. An EMPTY
/// breakdown prints no rows under either spelling: pi's `usageBreakdown[0]?.key !==
/// selectedModelKey` is `true` and it then loops over an empty array, emitting nothing, where
/// cyrup's `first().is_some_and(..)` is `false` and skips the loop.
#[tokio::test]
async fn an_empty_breakdown_prints_no_rows() {
    let (_tmp, session) = session_with("small", |m| {
        m.append_message(answered("large", Usage::default()))
            .unwrap();
    })
    .await;
    assert!(session.usage_cost_breakdown().await.is_empty());
    let text = session_info_text(&session).await;
    assert!(
        !text.contains("faux/large |"),
        "a row appeared for a zero-cost session:\n{text}"
    );
}

/// Two rows still print, as they always did — the first disjunct is untouched.
#[tokio::test]
async fn a_multi_model_breakdown_still_prints_every_row() {
    let (_tmp, session) = session_with("small", |m| {
        m.append_message(answered("small", usage(0.10))).unwrap();
        m.append_message(answered("large", usage(0.25))).unwrap();
    })
    .await;
    let text = session_info_text(&session).await;
    assert!(text.contains("faux/small"), "{text}");
    assert!(text.contains("faux/large"), "{text}");
}
