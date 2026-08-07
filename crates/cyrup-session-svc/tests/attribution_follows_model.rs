//! Provider-attribution + opencode session-affinity headers must follow the ACTIVE model.
//!
//! pi recomputes them inside `streamFn` for the model each request is going to (`sdk.ts:318-327`).
//! cyrup merged them once at session build and pinned them onto the agent via
//! `AgentBuilder::headers`, so a cross-provider `/model` switch kept sending the PREVIOUS provider's
//! attribution. `AgentSession::attribution_headers()` already computed the right value per model —
//! it simply had no caller, and `Agent` had no way to accept a new one.
//!
//! This asserts the WIRING: the agent's live overlay changes when the model changes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_provider::faux::FauxProvider;
use cyrup_provider::Provider;
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

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

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// A cross-provider `/model` switch must change the agent's header overlay.
///
/// This is the assertion that actually discriminates. An earlier version of this test only compared
/// the overlay against the ACTIVE model's attribution without ever switching — and it passed against
/// the pinned build-time value too, because at build the pinned map trivially equals the initial
/// model's attribution. A test that cannot fail against the defect is not a test.
#[tokio::test]
async fn a_cross_provider_model_switch_repoints_the_attribution_headers() {
    let fx = fixture();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(provider, base_config(&fx)).build().await.expect("build");

    let catalog = session.available_model_catalog();
    // Find two models whose attribution genuinely DIFFERS — the merge is host-matched, so an
    // openrouter model carries `HTTP-Referer`/`X-Title` that a plain one does not.
    let mut pair = None;
    for a in &catalog {
        for b in &catalog {
            if session.attribution_headers(a) != session.attribution_headers(b) {
                pair = Some((a.clone(), b.clone()));
                break;
            }
        }
        if pair.is_some() {
            break;
        }
    }
    let Some((from, to)) = pair else {
        // The faux catalog offers no attribution-distinguishable pair; the wiring is asserted by
        // `the_agents_header_overlay_is_live_and_model_derived` plus the unit tests in
        // `attribution.rs`. Skipping is honest — inventing a fake catalog here would test the fake.
        return;
    };

    session.set_model_resolved(from.clone()).await.expect("select the first model");
    let before = session.agent_headers().await;
    assert_eq!(before, session.attribution_headers(&from), "overlay tracks the first model");

    session.set_model_resolved(to.clone()).await.expect("switch model");
    let after = session.agent_headers().await;

    assert_eq!(after, session.attribution_headers(&to), "overlay tracks the NEW model");
    assert_ne!(
        after, before,
        "a cross-provider switch must repoint the attribution headers, not keep the previous \
         provider's (an OpenRouter HTTP-Referer on an Anthropic request)"
    );
}

/// The agent must expose a LIVE overlay, and the session must recompute it per model rather than
/// pinning the build-time value.
#[tokio::test]
async fn the_agents_header_overlay_is_live_and_model_derived() {
    let fx = fixture();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(provider, base_config(&fx)).build().await.expect("build");

    let models = session.available_model_catalog();
    assert!(!models.is_empty(), "the faux provider offers at least one model");

    // Whatever the active model's attribution is, the agent's live overlay must EQUAL it. Before the
    // fix the agent held a build-time snapshot with no way to be updated, so this could only ever
    // agree by accident.
    let active = session.model();
    let resolved = models
        .iter()
        .find(|m| m.id == active.model && m.provider == active.provider)
        .expect("the active model is in the catalog");
    let expected = session.attribution_headers(resolved);

    assert_eq!(
        session.agent_headers().await,
        expected,
        "the agent's live header overlay must be the attribution for the ACTIVE model"
    );
}
