//! SEAM-131 / EXT-085 — the two `/`-command surfaces of prompt-cache warming: the `/session`
//! `Cache Warming` rows and the `/settings` `Cache warming` row.
//!
//! Upstream is pi @ **v1.0.4**:
//!
//! ```text
//! // interactive-mode.ts:6702-6709 — ABOVE the `if (stats.cost > 0 || …)` guard, which opens at :6711
//! info += `\n${theme.bold("Cache Warming")}\n`;
//! info += `${theme.fg("dim", "Mode:")} ${cacheWarmingMode}\n`;
//! info += `${theme.fg("dim", "Status:")} ${cacheWarmingStatus ? formatCacheWarmingStatus(cacheWarmingStatus)
//!                                                             : "Inactive (cache warming unavailable)"}\n`;
//! const decision = cacheWarmingStatus?.decision;
//! if (decision?.economicsAvailable) {
//!     info += `${theme.fg("dim", "Cache miss penalty:")} $${decision.missCost.toFixed(3)}\n`;
//!     info += `${theme.fg("dim", "Refresh cost:")} $${decision.warmCost.toFixed(3)}\n`;
//! }
//!
//! // interactive-mode.ts:4965-4968 — the row's apply goes through the SESSION, not the settings manager
//! onCacheWarmingModeChange: (mode) => { this.session.setCacheWarmingMode(mode); … }
//! ```
//!
//! The `/settings` row's live half is the gap class `transport_live_apply.rs` documents: the warmer
//! re-reads its mode only at its own checkpoints, so a persist-only row would leave an armed
//! refresh timer standing after a switch to `off`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use super::harness::*;
use crate::crossterm::event::KeyCode;
use crate::{App, AppAction, AppCommand, SelectorKind, UiTheme};
use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxConfig, FauxModelDefinition, FauxProvider};
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

/// A session on a faux model with a 300 s short-tier prompt-cache lifetime, so the warmer is
/// eligible and `/session` has something to report.
///
/// `global` is written as the GLOBAL settings document, not a CLI or project layer: `cacheWarming`
/// is global-only (CFG-093 — upstream's `getCacheWarmingMode` reads
/// `this.globalSettings.cacheWarming`), so a lower layer is deliberately ignored and a test that
/// used one would pass for the wrong reason.
async fn session(global: &str) -> (TempDir, Arc<AgentSession>) {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    for d in [&cwd, &agent_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(agent_dir.join("settings.json"), global).unwrap();
    let mut def = FauxModelDefinition::new("warmable");
    def.prompt_cache = Some(cyrup_provider::ModelPromptCache {
        short: Some(300),
        long: Some(3_600),
    });
    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        models: vec![def],
        ..FauxConfig::default()
    }));
    let mut cfg = SessionConfig::new(cwd.clone(), agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let s = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        // The store must be installed explicitly, as `fullscreen_settings.rs` does: without it
        // the builder reads no settings document at all and a global-only key could never arrive.
        .settings_store(Arc::new(cyrup_config::FileSettingsStore::new(
            agent_dir.join("settings.json"),
            cwd.join(".cyrup").join("settings.json"),
        )))
        .build()
        .await
        .unwrap();
    (tmp, Arc::new(s))
}

async fn session_info_text(session: &Arc<AgentSession>) -> String {
    let mut app = App::new(TestBackend::new(120, 40), UiTheme::dark()).unwrap();
    app.execute_session_command(AppCommand::SessionInfo, session, None)
        .await;
    app.draw().unwrap();
    app.scrollback_text()
}

/// The RENDERED `warming mode` row. `/session` is pushed as a markdown block and drawn as a grid,
/// so every assertion here reads the rendered cell rather than the markdown source — which is also
/// the only thing that proves a user can actually see it. (An earlier draft of this feature
/// appended its rows after the table's closing newline and the renderer folded all of them into
/// the previous row's value cell; a source-only assertion passed throughout.)
fn mode_row(text: &str) -> String {
    text.lines()
        .find(|l| l.contains("warming mode"))
        .unwrap_or_else(|| panic!("no `warming mode` row rendered:\n{text}"))
        .to_string()
}

// ------------------------------------------------------------------- `/session` -----------------

/// The mode and status rows are present on a session that has spent NOTHING — upstream puts the
/// whole section above the `stats.cost > 0 || cacheWaste.missedTokens > 0` guard, and that is the
/// case that matters: a user whose warming is doing nothing wants `/session` to say why.
///
/// RED-PROVE: move the `warming` interpolation inside the `if stats.cost > 0.0 || …` guard — this
/// session's cost is 0, so both rows vanish and this test fails.
#[tokio::test]
async fn session_shows_the_warming_mode_and_status_on_a_zero_cost_session() {
    let (_tmp, s) = session("{}").await;
    let text = session_info_text(&s).await;
    assert!(
        mode_row(&text).contains("streaming"),
        "pi's default mode must be shown:\n{text}"
    );
    // No request has been sent yet, so the warmer is in upstream's own "nothing started" state.
    assert!(
        text.contains("Inactive (waiting for first request)"),
        "a warmer that exists but has not seen a request reports pi's own literal, NOT the \
         `unavailable` one — the two mean different things and `/session` must not conflate \
         them:\n{text}"
    );
    assert_eq!(
        s.cache_warming_mode(),
        Some(cyrup_config::CacheWarmingMode::Streaming)
    );
}

/// The configured mode reaches the row (not a hardcoded default).
///
/// RED-PROVE: hardcode `"streaming"` in place of the `warming_mode` interpolation — the row reads
/// `streaming` against a global document that says `idle`, and this test fails.
#[tokio::test]
async fn session_shows_a_configured_mode() {
    let (_tmp, s) = session(r#"{"cacheWarming":"idle"}"#).await;
    let text = session_info_text(&s).await;
    assert!(
        mode_row(&text).contains("idle"),
        "the row must read the EFFECTIVE setting:\n{text}"
    );
}

/// With `off`, the status row carries upstream's own `cache warming disabled` reason — which is
/// how a user confirms the setting took, and the only `/session` output that distinguishes "turned
/// off" from "nothing to warm".
#[tokio::test]
async fn session_reports_the_disabled_mode_in_the_status_row() {
    let (_tmp, s) = session(r#"{"cacheWarming":"off"}"#).await;
    let text = session_info_text(&s).await;
    assert!(mode_row(&text).contains("off"), "mode row:\n{text}");
    assert!(
        text.contains("cache warming disabled"),
        "the status row must report pi's own stop reason:\n{text}"
    );
}

/// The two price rows are gated on `decision?.economicsAvailable`. A session that has sent no
/// request has no decision at all, so neither row may appear — without the guard they would read
/// `$0.000` and advertise a costing that never happened.
///
/// RED-PROVE: replace the `and_then(|s| s.decision).filter(|d| d.economics_available)` chain with
/// an unconditional default decision — both rows appear as `$0.000` and this test fails.
#[tokio::test]
async fn session_omits_the_economics_rows_without_a_decision() {
    let (_tmp, s) = session("{}").await;
    let text = session_info_text(&s).await;
    assert!(
        !text.contains("miss penalty") && !text.contains("refresh cost"),
        "no decision has been made, so neither price row may be printed:\n{text}"
    );
}

// ------------------------------------------------------------------ `/settings` -----------------

/// The REAL `/settings` grid must carry a `Cache warming` row that cycles pi's three
/// `CACHE_WARMING_MODES` values and emits `ApplySetting { id: "cacheWarming", .. }` — the command
/// the live half below hangs off.
///
/// RED-PROVE: delete the `SettingRow::choice("cacheWarming", …)` from `settings_rows.rs` — the row
/// is never highlighted and this test fails on the "never highlighted" assertion. That is the
/// state of the tree before this change: the setting existed and was reachable only by hand-editing
/// the global settings file.
#[tokio::test]
async fn the_settings_row_cycles_pis_three_modes() {
    let (_tmp, s) = session("{}").await;
    let mut app = App::new(TestBackend::new(100, 40), UiTheme::dark()).unwrap();
    app.execute_command(AppCommand::OpenSelector(SelectorKind::Settings), &s, None)
        .await;

    // Walk down by the RENDERED highlight (`→ `), not a hardcoded index, so reordering the rows
    // cannot break this.
    let mut found = false;
    for _ in 0..256 {
        app.draw().unwrap();
        if buf_text(&app)
            .lines()
            .any(|l| l.trim_end().starts_with("→ Cache warming"))
        {
            found = true;
            break;
        }
        let _ = app.handle_input(&key(KeyCode::Down));
    }
    assert!(
        found,
        "the /settings grid never highlighted a `Cache warming` row:\n{}",
        buf_text(&app)
    );

    let mut values: Vec<String> = Vec::new();
    for _ in 0..3 {
        match app.handle_input(&key(KeyCode::Enter)) {
            AppAction::Command(AppCommand::ApplySetting { id, value }) => {
                assert_eq!(id, "cacheWarming", "wrong setting id");
                app.execute_command(
                    AppCommand::ApplySetting {
                        id,
                        value: value.clone(),
                    },
                    &s,
                    None,
                )
                .await;
                values.push(value);
            }
            other => panic!("Enter on the Cache warming row did not apply a setting: {other:?}"),
        }
    }
    values.sort();
    values.dedup();
    assert_eq!(
        values,
        vec![
            "idle".to_string(),
            "off".to_string(),
            "streaming".to_string()
        ],
        "the row must cycle pi's three `CACHE_WARMING_MODES` values"
    );
}

/// The LIVE half: cycling the row must reach the RUNNING warmer, not only the settings file. The
/// observable is the warmer's own mode — what `validate_run` and `mode_stop_reason` read — so a
/// persist-only row would leave an armed timer standing under `off`.
///
/// RED-PROVE: delete the `if id == "cacheWarming" { … }` block from `execute_misc.rs` — the
/// warmer's mode stays `Streaming` after the apply and this test fails, which is the same gap
/// `transport_live_apply.rs` closed for the transport row.
#[tokio::test]
async fn applying_the_row_reaches_the_live_warmer() {
    let (_tmp, s) = session("{}").await;
    let mut app = App::new(TestBackend::new(100, 40), UiTheme::dark()).unwrap();
    assert_eq!(
        s.cache_warming_mode(),
        Some(cyrup_config::CacheWarmingMode::Streaming),
        "baseline: pi's default"
    );

    app.execute_command(
        AppCommand::ApplySetting {
            id: "cacheWarming".to_string(),
            value: "off".to_string(),
        },
        &s,
        None,
    )
    .await;
    assert_eq!(
        s.cache_warming_mode(),
        Some(cyrup_config::CacheWarmingMode::Off),
        "the row must reach the LIVE warmer: `on_mode_changed` is what disarms an armed refresh"
    );
    // And `/session` now reports the new mode, so the two surfaces cannot disagree.
    assert!(mode_row(&session_info_text(&s).await).contains("off"));

    app.execute_command(
        AppCommand::ApplySetting {
            id: "cacheWarming".to_string(),
            value: "idle".to_string(),
        },
        &s,
        None,
    )
    .await;
    assert_eq!(
        s.cache_warming_mode(),
        Some(cyrup_config::CacheWarmingMode::Idle),
        "and back out of `off` again"
    );
}

/// An unknown value must not reach the warmer. The row can only emit the three, but
/// `ApplySetting` is a public command anything can carry a string on, and
/// `CacheWarmingMode::parse` answering `None` is what keeps a bad value from silently degrading
/// the LIVE mode to `streaming` while the settings document says otherwise.
#[tokio::test]
async fn an_unknown_value_leaves_the_live_mode_alone() {
    let (_tmp, s) = session(r#"{"cacheWarming":"idle"}"#).await;
    let mut app = App::new(TestBackend::new(100, 40), UiTheme::dark()).unwrap();
    app.execute_command(
        AppCommand::ApplySetting {
            id: "cacheWarming".to_string(),
            value: "aggressive".to_string(),
        },
        &s,
        None,
    )
    .await;
    assert_eq!(
        s.cache_warming_mode(),
        Some(cyrup_config::CacheWarmingMode::Idle),
        "an unparseable value is not a mode change"
    );
}

// --------------------------------------------- end-to-end harness -------------------------------

/// Everything the end-to-end tests need to hold on to. `_tmp` keeps the session directory alive.
struct WarmingE2e {
    _tmp: TempDir,
    session: Arc<AgentSession>,
    faux: Arc<FauxProvider>,
    session_file: std::path::PathBuf,
    /// Releases the held-open second turn.
    gate: Arc<tokio::sync::Notify>,
    /// The `StreamOptions` the WARM request carried, captured by its scripted step.
    warm_opts: Arc<std::sync::Mutex<Option<cyrup_provider::StreamOptions>>>,
}

impl WarmingE2e {
    fn release(&self) {
        self.gate.notify_waiters();
    }

    fn warm_options(&self) -> cyrup_provider::StreamOptions {
        self.warm_opts
            .lock()
            .expect("lock")
            .clone()
            .expect("the warm step must have run")
    }
}

/// A session shaped like a direct-Anthropic one: a 300 s short-tier prompt-cache lifetime (what
/// `apply_prompt_cache_metadata` stamps on every `anthropic` + `anthropic-messages` catalog row)
/// and prices steep enough that a few hundred prompt tokens clear upstream's `$0.05` floor. The
/// prices are 100x a real Anthropic row so the test needs a sentence rather than a novel as its
/// prompt; the ARITHMETIC they feed is pi's own and is pinned separately in
/// `cyrup-session-svc`'s `seam131_savings_threshold_is_inclusive`.
async fn warming_e2e(global: &str) -> WarmingE2e {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    for d in [&cwd, &agent_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(agent_dir.join("settings.json"), global).unwrap();

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
    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        models: vec![def],
        ..FauxConfig::default()
    }));

    let gate = Arc::new(tokio::sync::Notify::new());
    let warm_opts = Arc::new(std::sync::Mutex::new(None));
    {
        let held = gate.clone();
        let captured = warm_opts.clone();
        faux.set_response_steps(vec![
            // Turn one: ordinary, and the only reason it exists is to leave an assistant entry
            // with usage for `last_prompt_tokens` to price.
            cyrup_provider::faux::faux_assistant_message(
                vec![cyrup_provider::faux::faux_text("the first answer")],
                StopReason::Stop,
            )
            .into(),
            // Turn two: resolves only when the test releases it, so the turn OUTLASTS the TTL.
            cyrup_provider::faux::FauxResponseStep::async_factory(move |_c, _o, _s, _m| {
                let held = held.clone();
                async move {
                    held.notified().await;
                    cyrup_provider::faux::faux_assistant_message(
                        vec![cyrup_provider::faux::faux_text("the second answer")],
                        StopReason::Stop,
                    )
                }
            }),
            // The warm. A factory rather than a static message so its request options can be
            // inspected: `maxTokens: 1` / `maxRetries: 0` is what makes a warm cheap, and a warm
            // that quietly replayed the original `maxTokens` would be a full-price request.
            cyrup_provider::faux::FauxResponseStep::factory(move |_c, opts, _s, _m| {
                if let Ok(mut g) = captured.lock() {
                    *g = Some(opts.clone());
                }
                cyrup_provider::faux::faux_assistant_message(
                    vec![cyrup_provider::faux::faux_text("")],
                    StopReason::Stop,
                )
            }),
        ]);
    }

    let mut cfg = SessionConfig::new(cwd.clone(), agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, cfg)
        .settings_store(Arc::new(cyrup_config::FileSettingsStore::new(
            agent_dir.join("settings.json"),
            cwd.join(".cyrup").join("settings.json"),
        )))
        .build()
        .await
        .unwrap()
        .into_shared();
    let session_file = session
        .session_file()
        .await
        .expect("a persisted session must have a file");

    WarmingE2e {
        _tmp: tmp,
        session,
        faux,
        session_file,
        gate,
        warm_opts,
    }
}

/// Yield until `cond` holds, then once more so whatever satisfied it can park. Panics rather than
/// returning quietly on exhaustion — a silent timeout here would turn a real failure into a
/// confusing assertion further down.
async fn settle_until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..4_096 {
        if cond() {
            tokio::task::yield_now().await;
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("the session never reached the expected state: {what}");
}

async fn settle() {
    for _ in 0..4_096 {
        tokio::task::yield_now().await;
    }
}

/// The `usage` entries actually written to the session file. Read off DISK, not from the in-memory
/// manager: "it becomes a `usage` entry in the session file" is what the guide promises and what a
/// resumed session depends on.
fn usage_entries_on_disk(path: &std::path::Path) -> Vec<serde_json::Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("usage"))
        .collect()
}

// ------------------------------------------------- the whole feature, end to end ----------------

/// SEAM-131 — THE success test, as one test: a builder-wired session on a direct-Anthropic-shaped
/// model, `cacheWarming` at its default `streaming`, a turn that outlasts ~90% of the 5-minute
/// cache TTL. Exactly ONE cheap warm request goes out, it is persisted as a `usage` session entry
/// ON DISK, and `/session` reports the decision.
///
/// Everything here was already proved in four separate places — the `ProviderSwap` seam firing only
/// for the live session request, the state machine against a stub stream and host, `append_usage`
/// plus the stats/breakdown/transcript surfaces, and the `/session` rows from a scripted status.
/// What was NOT asserted anywhere is the COMPOSITION, which is the only thing a user experiences:
/// a real `SessionBuilder`, the real agent stamping `session_id` on its request, the real catalog
/// lifetime, a real clock crossing 270 s, the real persist. A hand audit of that chain holds, but a
/// hand audit is not a test, and a single field drifting (the `session_id` the guard compares)
/// would silently disable the whole feature with every one of those four tests still green.
///
/// **The clock is virtual** (`start_paused`); nothing here sleeps for real time.
///
/// **Turn one is not scenery.** `last_prompt_tokens` walks the branch backwards for the newest
/// ASSISTANT entry, so on a first turn it is 0, `economics_available` is false and the run stops
/// with `cache economics unavailable` whatever else is true. The warm under test is the one on
/// turn TWO.
///
/// **Red-proved four ways**, each applied alone and reverted:
/// 1. `maybe_start_warming`'s `opts.session_id.as_ref() != Some(session_id)` guard inverted to `==`
///    — the seam never fires, nothing is sent, no `usage` entry, `/session` shows
///    `Inactive (waiting for first request)`.
/// 2. `prompt_cache` dropped from the model definition — `prompt_cache_ttl_ms` is `None`,
///    `start` stops with `cache lifetime unavailable` and nothing is sent.
/// 3. `cacheWarming` set to `off` in the global settings document — nothing is sent (this is the
///    second half of the success test and is asserted below as its own case).
/// 4. `append_usage` neutered to a no-op — the request still goes out but no entry reaches the
///    session file, so the spend would be invisible.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_turn_that_outlasts_the_cache_ttl_warms_once_and_session_reports_it() {
    let h = warming_e2e("{}").await;

    // --- turn one: completes normally, leaving an assistant entry the warmer can price ---------
    let _s1 = h
        .session
        .prompt(cyrup_session_svc::UserInput::text(
            "the first turn, long enough to be worth caching",
            cyrup_session_svc::InputSource::Sdk,
        ))
        .await
        .expect("first prompt accepted");
    h.session.wait_for_idle().await;
    assert_eq!(h.faux.call_count(), 1, "one real provider turn so far");

    // --- turn two: held open past the warm delay ------------------------------------------------
    let _s2 = h
        .session
        .prompt(cyrup_session_svc::UserInput::text(
            "the second turn, which is going to take a while",
            cyrup_session_svc::InputSource::Sdk,
        ))
        .await
        .expect("second prompt accepted");
    // Let the request reach the provider (and therefore the warming seam) before moving the clock.
    settle_until("the second turn reaches the provider", || {
        h.faux.call_count() == 2
    })
    .await;
    assert!(
        h.session.is_run_active(),
        "the turn must still be in flight — the whole point is a turn that OUTLASTS the TTL"
    );

    // 270_001 ms: one millisecond past `getCacheWarmingDelayMs(300_000)` = 270_000.
    tokio::time::advance(Duration::from_millis(270_001)).await;
    settle_until("the warm request reaches the provider", || {
        h.faux.call_count() >= 3
    })
    .await;

    // ONE warm, not a storm: the warmer re-arms for the NEXT TTL rather than retrying.
    assert_eq!(
        h.faux.call_count(),
        3,
        "exactly one warm request on top of the two real turns"
    );
    let warm_opts = h.warm_options();
    assert_eq!(
        warm_opts.max_tokens,
        Some(1),
        "the warm is upstream's `maxTokens: 1` replay — a cache read plus one output token"
    );
    assert_eq!(
        warm_opts.max_retries,
        Some(0),
        "and `maxRetries: 0`: best-effort, never retried"
    );

    // --- the persisted entry, read back off DISK ------------------------------------------------
    settle_until("the usage entry is persisted", || {
        usage_entries_on_disk(&h.session_file).len() == 1
    })
    .await;
    let persisted = usage_entries_on_disk(&h.session_file);
    assert_eq!(
        persisted.len(),
        1,
        "exactly one `usage` entry on disk: {persisted:?}"
    );
    let entry = &persisted[0];
    assert_eq!(
        entry.get("kind").and_then(|k| k.as_str()),
        Some("cache_warm"),
        "pi's `appendUsage(\"cache_warm\", …)` kind, which is what gates the transcript notice \
         and the replay walk: {entry:?}"
    );
    assert!(
        entry.get("usage").is_some() && entry.get("model").is_some(),
        "the entry must carry the warm's own usage and the responding model: {entry:?}"
    );

    // --- `/session` ----------------------------------------------------------------------------
    let text = session_info_text(&h.session).await;
    assert!(
        mode_row(&text).contains("streaming"),
        "the mode row must show the default this session really ran on:\n{text}"
    );
    assert!(
        text.contains("Decision in") || text.contains("Decision now") || text.contains("Warming"),
        "the status row must report the LIVE decision, not an inactive warmer:\n{text}"
    );
    for row in ["miss penalty", "refresh cost"] {
        assert!(
            text.contains(row),
            "the economics rows appear once a decision has real numbers ({row}):\n{text}"
        );
    }

    h.release();
    h.session.wait_for_idle().await;
}

/// The other half of the success test: with `cacheWarming: "off"` the identical session sends
/// NOTHING, however long the turn runs.
///
/// **Red-proved** by deleting the `CacheWarmingMode::Off` arm from `CacheWarmer::start`: the warm
/// went out and the `usage` entry appeared, i.e. the setting would have been decorative.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn with_cache_warming_off_the_same_turn_sends_nothing() {
    let h = warming_e2e(r#"{"cacheWarming":"off"}"#).await;
    assert_eq!(
        h.session.cache_warming_mode(),
        Some(cyrup_config::CacheWarmingMode::Off)
    );

    let _s1 = h
        .session
        .prompt(cyrup_session_svc::UserInput::text(
            "the first turn, long enough to be worth caching",
            cyrup_session_svc::InputSource::Sdk,
        ))
        .await
        .expect("first prompt accepted");
    h.session.wait_for_idle().await;
    let _s2 = h
        .session
        .prompt(cyrup_session_svc::UserInput::text(
            "the second turn, which is going to take a while",
            cyrup_session_svc::InputSource::Sdk,
        ))
        .await
        .expect("second prompt accepted");
    settle_until("the second turn reaches the provider", || {
        h.faux.call_count() == 2
    })
    .await;

    // 1 ms past the 270 s warm delay and well inside the 15 s refresh deadline that follows it.
    //
    // The size of this jump is the whole test. It was `1_200_000` at first, and that made the test
    // VACUOUS: a single advance that large moves the clock past `refresh_deadline_at`
    // (270 000 + (300 000 - 270 000)/2 = 285 000) before the timer task is ever polled, so the
    // refresh stops itself with `"cache refresh deadline missed"` and sends nothing whatever the
    // mode is. Proved by mutation — with the `Off` arms deleted from both `start` and
    // `mode_stop_reason` the test still passed. Landing inside the deadline is what makes `off`
    // the only thing standing between this session and a warm request.
    tokio::time::advance(Duration::from_millis(270_001)).await;
    settle().await;

    assert_eq!(
        h.faux.call_count(),
        2,
        "the two real turns and nothing else — `off` means nothing is ever sent"
    );
    assert!(
        usage_entries_on_disk(&h.session_file).is_empty(),
        "and nothing was persisted"
    );
    let text = session_info_text(&h.session).await;
    assert!(
        text.contains("cache warming disabled"),
        "`/session` must say WHY, in upstream's own words:\n{text}"
    );

    h.release();
    h.session.wait_for_idle().await;
}
