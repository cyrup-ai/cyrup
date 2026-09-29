//! TUI-105 — the BOOT provider recount, rendered.
//!
//! pi's footer prepends `(provider)` only when more than one provider is available:
//! `if (this.footerData.getAvailableProviderCount() > 1 && state.model)`
//! (`packages/coding-agent/src/modes/interactive/components/footer.ts:192-193` @v0.87.1). That count
//! comes from `updateAvailableProviderCount` (`interactive-mode.ts:5092-5099`), which reads
//! `this.session.scopedModels.length > 0 ? scopedModels.map(s => s.model)
//! : this.session.modelRuntime.getAvailableSnapshot()` (`:5094-5096`) — the AUTH-FILTERED snapshot
//! across every configured provider. pi runs it during `init()` twice, via `rebindCurrentSession`
//! (`:1033` → `:2042`) and again at `:1051`, so the count is right on the FIRST frame.
//!
//! cyrup's boot seed used to tally `session.model_catalog()` instead — `provider.current().models()`
//! (`cyrup-session-svc/src/session/accessors.rs`), the CURRENT provider's catalog ONLY. After a dedup
//! that is always exactly 1, so the `> 1` gate (`cyrup-tui/src/status.rs:597`) could never fire at
//! boot and the `(provider)` prefix was unreachable on the first frame however many providers were
//! credentialed. `seed_model_footer` now calls `App::refresh_provider_count`, the same single
//! producer every later recount uses.
//!
//! These assert through the RENDERED BUFFER rather than the count field, because the count is a
//! means: what the row is about is whether the user sees the provider on the first frame.
//!
//! **No network.** The provider is the scripted faux double and the credentials are runtime-only.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup_config::AuthStore;
use cyrup_sdk::core::ProviderId;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use cyrup_tui::{App, UiTheme};
use ratatui::backend::TestBackend;

use crate::interactive::seed_model_footer;

/// Two real built-in providers to credential. Both ship embedded catalogs, so crediting them widens
/// `available_model_catalog()` — pi's `getAvailableSnapshot()` — without any network.
const P1: &str = "groq";
const P2: &str = "deepseek";

/// A session on the faux provider with `credentials` credentialed.
///
/// The faux provider is what makes the defect visible: `model_catalog()` spans exactly ONE provider
/// (`faux`) no matter how many credentials exist, so a tally over it can only ever be 1.
async fn session_with(credentials: &[&str]) -> (AgentSession, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let auth = Arc::new(AuthStore::at(agent_dir.join("auth.json")));
    for id in credentials {
        auth.set_runtime_api_key(ProviderId::from(*id), format!("sk-{id}-test"));
    }
    let mut config = SessionConfig::new(cwd, agent_dir);
    config.trust_override = Some(true);
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(provider, config)
        .auth(auth)
        .build()
        .await
        .unwrap();
    (session, tmp)
}

/// Seed the model half of the footer and render one frame, returning the whole screen as text.
fn seeded_frame(session: &AgentSession) -> String {
    let mut app = App::new(TestBackend::new(120, 24), UiTheme::dark()).unwrap();
    seed_model_footer(&mut app, session);
    app.draw().unwrap();
    let buffer = app.terminal().backend().buffer().clone();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// **The row's Impact.** Two credentialed providers ⇒ the first frame already carries the
/// `(provider)` prefix.
///
/// **Red without the change:** `seed_footer` tallied `session.model_catalog()`, which spans only the
/// installed faux provider, so `set_provider_count(1)` shut the `> 1` gate and the prefix never
/// rendered on the first frame.
#[tokio::test]
async fn boot_footer_shows_the_provider_prefix_when_several_are_available() {
    let (session, _tmp) = session_with(&[P1, P2]).await;

    // The precondition that makes this a REGRESSION test and not a tautology: the list the old boot
    // tally read spans exactly one provider, so it could only ever produce 1.
    let installed: std::collections::BTreeSet<String> = session
        .model_catalog()
        .iter()
        .map(|m| m.provider.as_str().to_string())
        .collect();
    assert_eq!(
        installed.len(),
        1,
        "the OLD tally's source spans one provider, which is the whole defect; got {installed:?}"
    );
    // …while the list pi reads spans more.
    let available: std::collections::BTreeSet<String> = session
        .available_model_catalog()
        .iter()
        .map(|m| m.provider.as_str().to_string())
        .collect();
    assert!(
        available.len() > 1,
        "crediting two providers must widen `getAvailableSnapshot()`; got {available:?}"
    );

    let model = session.model().expect("the faux session has a model");
    let expected = format!("({}) ", model.provider.as_str());
    let frame = seeded_frame(&session);
    assert!(
        frame.contains(&expected),
        "`footer.ts:192-193` prepends the provider once more than one is available; the first frame \
         must already show `{expected}`, got:\n{frame}"
    );
}

/// The other half of the same gate: pi prepends the provider only when the count is `> 1`, so a
/// single available provider must render NO prefix. Without this the test above would also pass an
/// implementation that always prefixes.
///
/// Driven through `scopedModels`, pi's first branch (`:5094-5095`), because it pins the count at one
/// deterministically — a machine with an ambient `GROQ_API_KEY` would otherwise credential extra
/// providers behind the test's back.
#[tokio::test]
async fn boot_footer_omits_the_provider_prefix_when_only_one_is_available() {
    let (session, _tmp) = session_with(&[P1, P2]).await;
    let model = session.model().expect("the faux session has a model");
    let resolved = session
        .model_catalog()
        .into_iter()
        .find(|m| m.id == model.model)
        .expect("the installed provider's own model resolves");
    // `session.scopedModels.length > 0 ? scopedModels.map(s => s.model) : …` (`:5094-5095`).
    session.set_scoped_models(vec![cyrup_session_svc::ScopedModel {
        model: resolved,
        thinking_level: None,
    }]);

    let frame = seeded_frame(&session);
    let unexpected = format!("({}) ", model.provider.as_str());
    assert!(
        !frame.contains(&unexpected),
        "one available provider ⇒ no prefix (`getAvailableProviderCount() > 1`, `footer.ts:192`); \
         got:\n{frame}"
    );
    // The model cell itself is still there — this is the gate closing, not the footer vanishing.
    assert!(
        frame.contains(model.model.as_str()),
        "the model cell still renders without the prefix, got:\n{frame}"
    );
}
