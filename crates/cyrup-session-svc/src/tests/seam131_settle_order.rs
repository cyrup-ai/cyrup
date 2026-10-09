//! SEAM-131 — the SETTLE ORDER, driven through the real `emit_agent_settled` rather than through
//! `CacheWarmer::on_agent_settled` directly.
//!
//! **Upstream.** `_emitAgentSettled` (`core/agent-session.ts:1066-1075` @v1.0.4) is:
//!
//! ```text
//! private async _emitAgentSettled(): Promise<void> {
//!     this._cacheWarmer?.onAgentSettled();            // <- FIRST statement
//!     this._isAgentRunActive = false;
//!     this._isEmittingAgentSettled = true;
//!     try {
//!         await this._extensionRunner.emit({ type: "agent_settled" });
//!         this._emit({ type: "agent_settled" });
//!     } finally { … }
//! ```
//!
//! The warmer settles BEFORE anything is awaited. That ordering is load-bearing and it costs money
//! to get wrong: the extension dispatch awaits guest wasm handlers, each up to its invocation
//! budget, and a refresh timer coming due inside that window still reads mode=streaming /
//! phase=streaming — so `validate_run` admits it and a paid warm request goes out that upstream had
//! already stopped with `"agent run settled"`. In `idle` mode the same window prices the refresh at
//! a continuation probability of 1.0 instead of 0.15 and so clears the `$0.05` floor it should have
//! failed.
//!
//! **Why this file exists at all.** Every assertion in `cache_warmer_state.rs` calls
//! `warmer.on_agent_settled()` itself, so the whole settle PATH — the only thing that decides the
//! order — was untested, and the inversion was invisible to a green suite. This is the same failure
//! shape the gap-analysis README warns about: a 14 000-test suite that still missed a virtual model
//! being handed to a real provider.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::{InputSource, SessionBuilder, SessionConfig, UserInput};
use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::{
    EventKind, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxConfig, FauxModelDefinition, FauxProvider, faux_assistant_message, faux_text,
};
use tempfile::TempDir;

/// An `agent_settled` handler that does two things no `CacheWarmer`-level test can: it reads the
/// warmer's status from INSIDE the dispatch window, and it pushes the virtual clock past the warm
/// delay while still holding that window open. A timer the settle failed to disarm fires here.
struct SettleObserver {
    session: Mutex<Option<Arc<crate::AgentSession>>>,
    settles: Arc<AtomicUsize>,
    /// The warmer's stop reason as seen from inside the dispatch, per settle.
    seen: Arc<Mutex<Vec<Option<String>>>>,
    /// Whether a refresh driven from inside the dispatch window found a live run, per settle.
    refreshed: Arc<Mutex<Vec<bool>>>,
}

#[async_trait::async_trait]
impl NativeExtension for SettleObserver {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("seam-131-settle-observer")
    }

    fn set_host_services(&self, _services: Arc<dyn HostServices>) {}

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::AgentSettled]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if !matches!(ev, HostEvent::AgentSettled { .. }) {
            return HookOutcome::Noop;
        }
        self.settles.fetch_add(1, Ordering::SeqCst);
        let session = self.session.lock().ok().and_then(|g| g.clone());
        let Some(session) = session else {
            return HookOutcome::Noop;
        };
        let reason = session.cache_warming_status().await.and_then(|s| s.reason);
        if let Ok(mut g) = self.seen.lock() {
            g.push(reason);
        }
        // A refresh coming due INSIDE the dispatch window, driven inline rather than left to the
        // armed timer. This is upstream's own test seam (`internal.refresh(internal.run)`,
        // `test/cache-warmer.test.ts:186`) and it is deliberately not a clock jump: an
        // `advance` large enough to be sure the timer task gets polled also moves the clock past
        // `refresh_deadline_at` (270 000 + (300 000 - 270 000)/2 = 285 000 ms), so the refresh
        // that fires stops itself with `"cache refresh deadline missed"` and sends nothing
        // whatever the settle did. Two earlier drafts of this test passed against the very bug it
        // exists for, for exactly that reason. Driving the refresh directly removes the timing
        // question: if the settle left a run armed, a paid warm request goes out here.
        let refreshed = match session.cache_warmer_for_test() {
            Some(warmer) => warmer.refresh_active_run_for_test().await,
            None => false,
        };
        if let Ok(mut g) = self.refreshed.lock() {
            g.push(refreshed);
        }
        HookOutcome::Noop
    }
}

/// A faux model shaped like a direct-Anthropic row: a 300 s short-tier prompt-cache lifetime and
/// prices steep enough that a few hundred prompt tokens clear the `$0.05` floor, so a warm that
/// the settle failed to stop really would be SENT rather than declined on its economics. (With a
/// zero-cost model `economics_available` is false and the run stops on its own, which would make
/// this test pass for the wrong reason.)
fn warmable_model() -> FauxModelDefinition {
    let mut def = FauxModelDefinition::new("warmable");
    def.prompt_cache = Some(cyrup_provider::ModelPromptCache {
        short: Some(300),
        long: Some(3_600),
    });
    def.cost = cyrup_provider::ModelCost {
        input: 62.5,
        output: 2_500.0,
        cache_read: 50.0,
        cache_write: 625.0,
        tiers: None,
    };
    def
}

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    for d in [&cwd, &agent_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

/// THE proof. Two real turns on a warmable model, with an `agent_settled` handler that holds the
/// dispatch window open across the whole warm delay. In `streaming` mode — the default — the
/// warmer must already be stopped with `"agent run settled"` by the time that handler runs, and
/// NOTHING may be sent inside the window.
///
/// **RED with the pre-fix order**, measured: moving `warmer.on_agent_settled()` back below the
/// `dispatch_notify` and `fanout_emit` awaits in `session/run.rs::emit_agent_settled` fails the
/// FIRST assertion below with `left: 4, right: 2` — both settles found a live armed run and each
/// sent a paid warm request that upstream had already stopped. The probe assertion fails with it
/// (`[None, None]` instead of two `Some("agent run settled")`), and `usage` entries are persisted
/// for warms that should never have happened.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn seam131_the_warmer_settles_before_the_agent_settled_dispatch() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        models: vec![warmable_model()],
        ..FauxConfig::default()
    }));
    // Two scripted turns. A THIRD is deliberately not scripted: a warm request that escaped the
    // settle would show up as a third call, which is the money assertion.
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second")], StopReason::Stop),
    ]);

    let settles = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let refreshed = Arc::new(Mutex::new(Vec::new()));
    let ext = Arc::new(SettleObserver {
        session: Mutex::new(None),
        settles: settles.clone(),
        seen: seen.clone(),
        refreshed: refreshed.clone(),
    });

    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, cfg)
        .with_native_extension(ext.clone() as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("build")
        .into_shared();
    if let Ok(mut g) = ext.session.lock() {
        *g = Some(session.clone());
    }

    // Turn one exists to give the warmer something to price: `last_prompt_tokens` walks the branch
    // for the newest ASSISTANT entry, so on a first turn it is 0, economics are unavailable and
    // the run would stop on its own however the settle is ordered.
    for text in ["tell me about the first thing", "and now the second thing"] {
        let _stream = session
            .prompt(UserInput::text(text, InputSource::Sdk))
            .await
            .expect("prompt accepted");
        session.wait_for_idle().await;
    }

    assert_eq!(settles.load(Ordering::SeqCst), 2, "one settle per run");
    // The MONEY assertion first, because it is the consequence: a warm request upstream never
    // sends is a real charge on the user's account.
    assert_eq!(
        faux.call_count(),
        2,
        "two real turns and NOT a third: the warm timer that came due inside the dispatch window \
         had already been disarmed by the settle"
    );
    assert_eq!(
        refreshed.lock().unwrap().clone(),
        vec![false, false],
        "a refresh driven from inside the dispatch window must find NO live run — the settle \
         disarmed it before the first await"
    );
    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![
            Some("agent run settled".to_string()),
            Some("agent run settled".to_string())
        ],
        "an extension handler must never observe an armed warming run: pi settles the warmer as \
         the FIRST statement of `_emitAgentSettled`, before it awaits the runner"
    );
    let warms = session
        .entries_json()
        .await
        .into_iter()
        .filter(|e| e.get("type").and_then(|t| t.as_str()) == Some("usage"))
        .count();
    assert_eq!(
        warms, 0,
        "nothing was warmed, so no `usage` entry may have been persisted"
    );
}
