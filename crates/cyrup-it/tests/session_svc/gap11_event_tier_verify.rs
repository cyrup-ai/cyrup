//! GAP-11 INDEPENDENT VERIFICATION (reviewer-authored, not the implementer's).
//!
//! Drives a REAL `wasm32-wasip2` guest whose `on_message_end` EVENT handler calls BOTH `set_model`
//! AND `set_thinking_level`, inside an ASSEMBLED `AgentSession`, and OBSERVES the change take effect
//! on the SUBSEQUENT turn — matching Pi (which allows both from any handler, loader.ts:342-354, and
//! they take effect). Pre-fix, cyrup rejected event-tier control ops at `require_command_tier`
//! (live.rs, R-08-008): `set_model` was silently dropped and `set_thinking_level` returned a deadlock
//! `Err`. This test proves the fix: the ops are QUEUED and applied at the store-free turn-boundary
//! drain, no deadlock / no wasm-store re-entrancy panic, and the command tier still applies.
//!
//! Observation seams (no facade shortcuts):
//!   * the OUTGOING request model — recorded per turn via the faux provider's `on_response` hook,
//!     which fires with the resolved `request_model` before each stream (faux.rs `stream`),
//!   * the session model — `session.model()`,
//!   * the thinking level — `session.thinking_level()` (reads the live agent snapshot).
// The original `#![cfg(feature = "wasm-host")]` is deliberately GONE. It named
// cyrup-session-svc's own feature, which that crate enables in its `default` — so it was
// always true here. Re-spelled in cyrup-it it would name THIS crate's `wasm-host`, which
// `--features it` does not enable, and every test below would SILENTLY not compile in.
// See the `[[test]]` note in crates/cyrup-it/Cargo.toml.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::{ExtensionId, ModelThinkingLevel, StopReason};
use cyrup_provider::faux::{
    faux_assistant_message, faux_text, FauxConfig, FauxModelDefinition, FauxProvider,
    FauxResponseMeta,
};
use cyrup_provider::{Model, Provider};
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

// MIGRATION (docs/TEST-ARCHITECTURE.md §3.4): this file used to carry its own `fixture_component()`
// that shelled out to `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` into the SHARED, fixed
// `std::env::temp_dir()/cyrup-session-svc-fixture-target` — one of ten byte-identical copies that
// serialized on each other's cargo build lock and never cleaned up. `cyrup-it`'s `build.rs` now
// builds the component ONCE for the whole suite and exports its path; `CYRUP_EXT_FIXTURE_COMPONENT`
// still overrides it, at that one place instead of ten.
use crate::support::bins;

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
    cfg.no_extensions = true;
    cfg.model_pattern = Some("faux-1".to_string());
    cfg
}

/// A faux provider with TWO REASONING-capable models (so `set_thinking_level("high")` does not clamp
/// to `off`) and an `on_response` hook that records the resolved request model of every turn — the
/// direct observation of which model the OUTGOING request used.
fn recording_two_model_faux(recorded: Arc<Mutex<Vec<String>>>) -> Arc<FauxProvider> {
    let reasoning = |id: &str| {
        let mut d = FauxModelDefinition::new(id);
        d.reasoning = true;
        d
    };
    let rec = recorded.clone();
    let cfg = FauxConfig {
        models: vec![reasoning("faux-1"), reasoning("faux-2")],
        on_response: Some(Arc::new(move |_meta: &FauxResponseMeta, model: &Model| {
            rec.lock().unwrap_or_else(|e| e.into_inner()).push(model.id.as_str().to_string());
        })),
        ..FauxConfig::default()
    };
    let faux = Arc::new(FauxProvider::with_config(cfg));
    // Enough plain "ok" turns for the two real prompts (the /commands short-circuit — no stream).
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
    ]);
    faux
}

/// THE verification: an event-tier `set_model` + `set_thinking_level` (from `on_message_end`) take
/// effect on the SUBSEQUENT turn, with no deadlock; the command tier still applies.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn event_tier_set_model_and_thinking_take_effect_on_next_turn() {
    let bytes = bins::component_bytes();

    let fx = fixture();
    let recorded: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn Provider> = recording_two_model_faux(recorded.clone());
    // BIND the session (`into_shared`) — production always does (runtime.rs:107). Only a bound
    // session spawns the post-run driver (`drive_run`) that hosts the GAP-11 event-tier turn-boundary
    // drain; an unbound by-value session runs the prompt without it.
    let session = SessionBuilder::new(provider, base_config(&fx)).build().await.unwrap().into_shared();

    // Load the REAL guest COMPONENT through the session's own host (arch-08 §5.6 seam).
    let ext = session
        .load_wasm_extension(
            ExtensionId::from("demo"),
            &bytes,
            // EXT-059: the grant is now explicit. `host_granted()` is the TOTAL grant these
            // fixtures previously got implicitly from `load_wasm_extension`'s `load_wasm` call.
            &cyrup_ext::Capabilities::host_granted(),
        )
        .await
        .expect("load + init the live wasm extension");

    // BASELINE: session starts on faux-1, thinking at the shipped default.
    // CFG-056 (commit c06bb0c, 2026-08-14) changed the unset-`defaultThinkingLevel` fallback from
    // the enum's `Off` zero to `cyrup_config::DEFAULT_THINKING_LEVEL` = `Medium`, matching pi's
    // `getDefaultThinkingLevel() ?? DEFAULT_THINKING_LEVEL` (sdk.ts:230,:235; core/defaults.ts:3).
    // This fixture writes no `settings.json`, so it lands on that fallback; `faux-1` is
    // reasoning-capable, so the clamp leaves `Medium` intact. `Off` here was the PRE-CFG-056
    // baseline — the file predates that commit and cyrup-it did not compile in between, so it was
    // never re-run. What the assertion is FOR is unchanged: pin a known starting level distinct
    // from the `high` the event handler sets below, so the switch is observable.
    assert_eq!(session.model().expect("session must have a resolved model").model.as_str(), "faux-1", "starts on faux-1");
    assert_eq!(
        session.thinking_level().await,
        ModelThinkingLevel::Medium,
        "starts at the DEFAULT_THINKING_LEVEL baseline (CFG-056), not the enum zero"
    );

    // ---- TURN 1: drive a real turn whose user message fires on_message_end("gap11switch"),
    //      which (event tier) calls set_model("faux/faux-2") + set_thinking_level("high"). ----
    let _ = session.prompt("gap11switch").await.unwrap();
    session.wait_for_idle().await;

    // The event handler ran across the wasm boundary (both calls issued). set_model's WIT import is
    // void (fire-and-forget), so it is observed by EFFECT below; set_thinking_level returns a result
    // and the guest observed Ok — pre-fix it returned an honest deadlock Err → notify "err".
    let notes = ext.guest().notifications();
    assert!(
        notes.iter().any(|n| n.contains("gap11: set_model called from message_end")),
        "event-tier set_model must be reached across the wasm boundary; notes: {notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.contains("gap11: set_thinking_level ok from message_end")),
        "event-tier set_thinking_level must surface Ok to the guest; notes: {notes:?}"
    );
    assert!(
        !notes.iter().any(|n| n.contains("err from message_end")),
        "NO event-tier rejection/deadlock error may surface; notes: {notes:?}"
    );

    // (1)+(2) The event-tier ops TOOK EFFECT: session model switched, thinking level switched.
    assert_eq!(
        session.model().expect("session must have a resolved model").model.as_str(),
        "faux-2",
        "event-tier set_model took effect on the session model"
    );
    assert_eq!(
        session.thinking_level().await,
        ModelThinkingLevel::High,
        "event-tier set_thinking_level took effect on the live agent"
    );

    // ---- TURN 2 (SUBSEQUENT): the next turn's OUTGOING request must use the NEW model. ----
    let _ = session.prompt("second turn").await.unwrap();
    session.wait_for_idle().await;

    // The recorded per-turn request models prove turn 1 used the ORIGINAL model and turn 2 (driven
    // AFTER the event-tier switch) used the NEW model — the crisp "next turn uses the new model".
    let models = recorded.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(
        models,
        vec!["faux-1".to_string(), "faux-2".to_string()],
        "turn 1 outgoing request used faux-1; the SUBSEQUENT turn's request used the new faux-2"
    );

    // (3) No deadlock / no hang / no wasm-store re-entrancy panic. This is the CRUX: applying the
    //     event-tier set_thinking_level at the drain RE-EMITS thinking_level_select back INTO the
    //     single-instance wasm store (the guest subscribes via on_thinking_level_select). That is the
    //     exact re-entry the old command-tier gate rejected to avoid a deadlock. The guest's handler
    //     notified — so the re-emit reached the guest as a fresh top-level call at a STORE-FREE point,
    //     with no deadlock/hang (the test would never reach here otherwise) and no re-entrancy panic.
    assert!(
        ext.guest()
            .notifications()
            .iter()
            .any(|n| n.contains("tls re-emit reached guest: high")),
        "the thinking_level_select re-emit must re-enter the wasm store safely at the drain; \
         notifications: {:?}",
        ext.guest().notifications()
    );

    // (4) COMMAND-tier path still applies (the gate was only removed for these two ops).
    //     Command-tier set_thinking_level via /thinkdemo:
    let _ = session.prompt("/thinkdemo low").await.unwrap();
    session.wait_for_idle().await;
    assert_eq!(
        session.thinking_level().await,
        ModelThinkingLevel::Low,
        "command-tier set_thinking_level still applies"
    );
    //     Command-tier set_model via /gap11setmodel:
    let _ = session.prompt("/gap11setmodel faux/faux-1").await.unwrap();
    session.wait_for_idle().await;
    assert_eq!(
        session.model().expect("session must have a resolved model").model.as_str(),
        "faux-1",
        "command-tier set_model still applies"
    );

    // The /commands short-circuited (no stream) — the recorded request models are unchanged.
    let models_after = recorded.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(models_after, models, "slash commands did not issue provider requests");
}
