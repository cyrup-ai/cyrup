//! TUI-143's footer half — the ` → <routed id> • <level>` suffix a VIRTUAL selection adds to the
//! footer's right cluster.
//!
//! Upstream is pi @ **v1.0.4**
//! `coding-agent/src/modes/interactive/components/footer.ts:240-245`:
//!
//! ```text
//! // A virtual model routes each request; show where the latest response went.
//! const routed = this.session.routedModel;
//! if (routed) {
//!     const level = routed.thinkingLevel ? ` • ${routed.thinkingLevel}` : "";
//!     rightSideWithoutProvider += ` → ${routed.model.id}${level}`;
//! }
//! ```
//!
//! and pinned upstream by `test/footer-width.test.ts:159-171`, whose session renders
//! `"auto • high → gpt-5.6-luna • medium"`.
//!
//! **Where the suffix is appended is load-bearing**, not cosmetic: upstream appends it to
//! `rightSideWithoutProvider`, i.e. BEFORE the `(provider)` prefix decision (`:248-255`) and before
//! the min-2-space width math (`:257-275`). Two of the tests below are about that position alone.
//!
//! **CORRECTION to TUI-143.** The row's Fix text says `footer.ts:107` makes `routedModel` choose the
//! model whose context window the footer shows. It does not: `:107` is one field of the
//! `sessionStats` memo KEY, and the window comes from `contextUsage?.contextWindow` at `:160`,
//! where `contextUsage = this.session.getContextUsage()` — whose model is `_limitsModel()`
//! (`agent-session.ts:629`, `:4223`). That is session-side and landed with
//! `AgentSession::limits_model`; the TUI needed no edit for it. This file therefore covers the
//! SUFFIX only.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use super::harness::*;
use crate::{App, RoutedModel, UiTheme};
use cyrup_core::{Content, ModelThinkingLevel, StopReason, Usage, VIRTUAL_MODEL_API};
use cyrup_provider::faux::{FauxConfig, FauxModelDefinition, FauxProvider};
use cyrup_provider::{
    Model, ModelRoute, ModelRouteError, ModelRouteRequest, ModelRouter, Provider,
    VirtualModelDefinition, VirtualModelRegistry, VirtualModelSpec,
};
use cyrup_session::{NewSessionOpts, SessionLayout, SessionManager};
use cyrup_session_svc::{
    AgentSession, AgentSessionEvent, SessionBuilder, SessionConfig, SessionTarget,
};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

fn footer_app(width: u16) -> App<TestBackend> {
    App::new(TestBackend::new(width, 6), UiTheme::dark()).unwrap()
}

fn routed(label: &str, level: Option<&str>) -> RoutedModel {
    RoutedModel {
        label: label.to_string(),
        thinking_level: level.map(str::to_string),
    }
}

/// The text of the last (bottom) row — the line-2 model/usage cluster.
fn last_row(app: &App<TestBackend>) -> String {
    row_text(app, app.terminal().backend().buffer().area.height - 1)
}

// --------------------------------------------------------------------- the suffix itself ----

/// The routed suffix names the physical model and the level that response was asked for, appended
/// after the selection's own ` • {level}`.
///
/// Mirrors upstream's own assertion (`test/footer-width.test.ts:170`).
///
/// RED-PROVE: delete the `if let Some(routed)` block from `StatusLine::right_cluster` — the
/// assertion fails with the arrow and everything after it absent.
#[test]
fn the_routed_suffix_names_the_physical_model_and_its_level() {
    let mut app = footer_app(120);
    app.status_mut().set_model("router/auto");
    app.status_mut().set_reasoning(true);
    app.status_mut().set_thinking_level("high");
    app.status_mut()
        .set_routed_model(Some(routed("openai/gpt-5.6-luna", Some("medium"))));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(
        text.contains("router/auto \u{2022} high \u{2192} openai/gpt-5.6-luna \u{2022} medium"),
        "routed suffix missing:\n{text}"
    );
}

/// A routed model with NO recorded level prints the arrow alone — upstream's ternary at
/// `footer.ts:243`. This is the shape cyrup renders for a response whose `thinkingLevel` the loop
/// never stamped (a session file written before the field existed).
///
/// RED-PROVE: make the level unconditional (`format!(" • {}", level.unwrap_or_default())`) — the
/// "nothing follows the arrow" assertion fails on the trailing ` • `.
#[test]
fn a_routed_model_without_a_level_prints_the_arrow_alone() {
    let mut app = footer_app(120);
    app.status_mut().set_model("router/auto");
    app.status_mut()
        .set_routed_model(Some(routed("faux/large", None)));
    app.draw().unwrap();
    let row = last_row(&app);
    assert!(
        row.contains("router/auto \u{2192} faux/large"),
        "routed suffix missing:\n[{row}]"
    );
    let after = row.split("faux/large").nth(1).unwrap_or("");
    assert!(
        !after.contains('\u{2022}'),
        "a bullet follows the arrow with no level:\n[{row}]"
    );
}

/// REGRESSION GUARD — **not a red proof**: it passes with the behaviour removed. A physical
/// selection shows no arrow, which is upstream's `routedModel === undefined` arm.
#[test]
fn a_physical_selection_shows_no_arrow() {
    let mut app = footer_app(120);
    app.status_mut().set_model("faux/large");
    app.status_mut().set_reasoning(true);
    app.status_mut().set_thinking_level("high");
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(!text.contains('\u{2192}'), "unexpected arrow:\n{text}");
}

// ------------------------------------------------------------------- where it is appended ----

/// REGRESSION GUARD — **not a red proof**, and worth saying why. It is upstream's standing
/// `footer-width.test.ts` invariant (every rendered line fits the width) applied to the routed
/// suffix, but it cannot FAIL under the realistic alternative: a span appended after
/// [`StatusLine::usage_line`]'s width math is clipped at the ratatui buffer edge rather than
/// widening the row, so a late append is invisible to a width assertion. The position behaviour is
/// pinned instead by `the_provider_prefix_yields_to_the_routed_suffix` below, which measures the
/// cluster the prefix decision is made against.
#[test]
fn the_routed_suffix_is_truncated_rather_than_overflowing() {
    let mut app = footer_app(40);
    app.status_mut()
        .set_model("some-very-long-provider/some-very-long-model-id");
    app.status_mut().set_reasoning(true);
    app.status_mut().set_thinking_level("xhigh");
    app.status_mut().set_routed_model(Some(routed(
        "another-long-provider/another-long-routed-model",
        Some("medium"),
    )));
    app.draw().unwrap();
    let buf = app.terminal().backend().buffer();
    for y in 0..buf.area.height {
        let row = row_text(&app, y);
        assert!(
            row.chars().count() <= 40,
            "row {y} is {} columns wide:\n[{row}]",
            row.chars().count()
        );
    }
}

/// At a width where `(provider) selection → routed` does not fit but `selection → routed` does, the
/// `(provider)` PREFIX is what yields — because the suffix is already part of the cluster the
/// prefix is measured against (`footer.ts:240-245` precedes `:248-255`).
///
/// This is the test that pins the suffix's POSITION. RED-PROVE: measure the prefix against the
/// cluster WITHOUT the suffix — `let with_provider = format!("({provider}) {}",
/// self.right_cluster_model())` in [`StatusLine::usage_line`] — and this test fails while every
/// other one stays green.
#[test]
fn the_provider_prefix_yields_to_the_routed_suffix() {
    // The left cluster is `0.0%/0` (6 columns) and the min gap is 2, so `aaa/bbb → ccc/ddd` (17)
    // needs 25 and `(faux) aaa/bbb → ccc/ddd` (24) needs 32. 28 fits the first and not the second.
    let mut app = footer_app(28);
    app.status_mut().set_model("aaa/bbb");
    app.status_mut().set_provider(Some("faux".to_string()));
    app.status_mut().set_provider_count(3);
    app.status_mut()
        .set_routed_model(Some(routed("ccc/ddd", None)));
    app.draw().unwrap();
    let row = last_row(&app);
    assert!(
        row.contains("aaa/bbb \u{2192} ccc/ddd"),
        "routed cluster missing:\n[{row}]"
    );
    assert!(
        !row.contains("(faux)"),
        "the provider prefix should have yielded:\n[{row}]"
    );
}

// ----------------------------------------------------------------- the real entry point ----

/// A router that always answers with the catalog row named `to`.
struct FixedRouter {
    catalog: Vec<Model>,
    to: &'static str,
}

#[async_trait::async_trait]
impl ModelRouter for FixedRouter {
    async fn route(&self, request: ModelRouteRequest<'_>) -> Result<ModelRoute, ModelRouteError> {
        let model = self
            .catalog
            .iter()
            .find(|m| m.id.as_str() == self.to)
            .cloned()
            .ok_or_else(|| ModelRouteError::new("no such model"))?;
        Ok(ModelRoute {
            model,
            thinking_level: request.thinking_level,
            state: None,
        })
    }
}

/// A resumed session whose branch selects `router/auto` and whose latest response came from
/// `faux/large` at `high` — the state `AgentSession::routed_model()` reads.
async fn routed_session() -> (TempDir, Arc<AgentSession>) {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    let sessions = tmp.path().join("sessions");
    for d in [&cwd, &agent_dir, &sessions] {
        std::fs::create_dir_all(d).unwrap();
    }

    // The branch, written through the real SessionManager: a virtual `model_change` followed by a
    // settled PHYSICAL response. Hand-built rather than prompted, so the test depends on the
    // restore + accessor path and on nothing else.
    let layout = SessionLayout::literal(sessions.clone(), cwd.clone());
    let mut m = SessionManager::create(&cwd, &layout, NewSessionOpts::default()).unwrap();
    m.append_model_change("router".into(), "auto".into())
        .unwrap();
    m.append_message(cyrup_core::Message::User {
        content: vec![Content::text("hi")],
        timestamp: 0,
    })
    .unwrap();
    m.append_message(cyrup_core::Message::Assistant(
        cyrup_core::AssistantMessage {
            content: vec![Content::text("ok")],
            provider: "faux".into(),
            model: "large".into(),
            api: "openai-completions".into(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            thinking_level: Some(ModelThinkingLevel::High),
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 0,
        },
    ))
    .unwrap();
    let path = m.session_file().unwrap().to_path_buf();
    drop(m);

    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        models: ["small", "large"]
            .iter()
            .map(|id| {
                let mut d = FauxModelDefinition::new(*id);
                d.reasoning = true;
                d.context_window = 200_000;
                d
            })
            .collect(),
        ..FauxConfig::default()
    }));
    let catalog = faux.models().to_vec();

    let registry = Arc::new(VirtualModelRegistry::new());
    registry
        .register(
            VirtualModelDefinition::new(
                VirtualModelSpec {
                    provider: "router".into(),
                    id: "auto".into(),
                    name: "Auto".to_string(),
                    thinking_levels: Some(vec![ModelThinkingLevel::Off, ModelThinkingLevel::High]),
                    context_window: None,
                    max_tokens: None,
                    input: None,
                },
                Arc::new(FixedRouter {
                    catalog,
                    to: "large",
                }),
            ),
            &cyrup_provider::NoCatalog,
        )
        .unwrap();

    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg.session_dir = Some(sessions);
    cfg.target = SessionTarget::Resume(path);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .virtual_models(registry)
        .build()
        .await
        .unwrap();
    (tmp, Arc::new(session))
}

/// **The only test that distinguishes "the field exists" from "the field is wired."** A session
/// event the predicate admits must push the live `routedModel` into the footer, with no test
/// touching `set_routed_model`.
///
/// RED-PROVE: remove the `refresh_routed_model(session)` call from the `usage_may_have_moved` block
/// in `App::ingest_session_event_owned` — the field stays `None`, no arrow is ever painted, and
/// this test is the only one in the file that fails.
#[tokio::test]
async fn a_response_from_a_routed_model_reaches_the_footer() {
    let (_tmp, session) = routed_session().await;
    // The selection really is the virtual model, else this test would be vacuous.
    let selected = session.model().expect("a selection");
    assert_eq!(selected.provider.as_str(), "router");
    assert_eq!(selected.model.as_str(), "auto");
    assert_eq!(
        selected.api.as_ref().map(cyrup_core::ApiId::as_str),
        Some(VIRTUAL_MODEL_API)
    );

    let mut app = footer_app(120);
    app.status_mut().set_model("router/auto");
    // No arrow before any event: the footer has not been told anything yet.
    app.draw().unwrap();
    assert!(!buf_text(&app).contains('\u{2192}'));

    app.ingest_session_event(
        &AgentSessionEvent::AgentEnd {
            messages: Vec::new(),
            will_retry: false,
        },
        &session,
    )
    .await;
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(
        text.contains("\u{2192} faux/large \u{2022} high"),
        "the routed model never reached the footer:\n{text}"
    );
}

/// The suffix CLEARS when the selection stops being virtual — `routedModel`'s first clause
/// (`agent-session.ts:1438`) answers `undefined`, and a push-fed footer must not keep a stale
/// arrow.
///
/// RED-PROVE: make `refresh_routed_model` early-return on `None` instead of pushing it — the arrow
/// survives the switch and the final assertion fails.
#[tokio::test]
async fn switching_to_a_physical_model_clears_the_arrow() {
    let (_tmp, session) = routed_session().await;
    let mut app = footer_app(120);
    app.status_mut().set_model("router/auto");
    app.ingest_session_event(
        &AgentSessionEvent::AgentEnd {
            messages: Vec::new(),
            will_retry: false,
        },
        &session,
    )
    .await;
    app.draw().unwrap();
    assert!(buf_text(&app).contains('\u{2192}'));

    session.set_model("faux/small").await.unwrap();
    app.status_mut().set_model("faux/small");
    app.ingest_session_event(
        &AgentSessionEvent::AgentEnd {
            messages: Vec::new(),
            will_retry: false,
        },
        &session,
    )
    .await;
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(
        !text.contains('\u{2192}'),
        "a stale routed suffix survived the switch:\n{text}"
    );
}
