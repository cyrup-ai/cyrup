//! TUI-030 — the four working-indicator `ExtensionUIContext` verbs must reach live TUI state.
//!
//! `setWorkingMessage` (`core/extensions/types.ts:151` @v0.83.0), `setWorkingVisible` (`:154`),
//! `setWorkingIndicator` (`:164`) and `setHiddenThinkingLabel` (`:167`) were a WHOLE-CLUSTER gap,
//! not four forgotten lines: `UiEffect` had no variant for any of them, so `LiveHostServices` kept
//! the `HostServices` trait's empty default bodies and every call an extension made returned
//! successfully having changed nothing — no error, no log line, in every mode. Because
//! `cyrup-ext/src/host/live.rs` forwards the guest's `ui.*` imports to those same trait methods,
//! WASM guests and native extensions were equally dead.
//!
//! Pi binds all four to real interactive state in `createExtensionUIContext`
//! (`modes/interactive/interactive-mode.ts:2377-2385`) and resets all four in `resetExtensionUI`
//! (`:2210-2218`); only the headless modes get `noOpUIContext` (`core/extensions/runner.ts:242-245`,
//! four `() => {}` bodies) and pi's RPC mode declines all four explicitly
//! (`modes/rpc/rpc-mode.ts:179-193`, "requires TUI loader access").
//!
//! Every pi line number in this file is **@v0.84.2**, the checked-out upstream — `types.ts`'s
//! `:151`/`:154`/`:164`/`:167` happen to be identical at @v0.83.0, but the `interactive-mode.ts`,
//! `rpc-mode.ts` and `runner.ts` ones are not (`setWorkingVisible` alone is `:1877` at @v0.83.0 and
//! `:2091` here).
//!
//! Every test here drives the PRODUCTION seam end to end — a real [`LiveHostServices`], the real
//! `App::install_ui_sinks` the run loop calls, and the real `App::apply_ui_effect` its drain arm
//! calls — rather than constructing a `UiEffect` by hand. That is deliberate: a test that built the
//! variants itself would pass against the unfixed tree, because the break was in the four
//! `HostServices` overrides that never existed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use cyrup_ext::host::HostServices;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AgentSessionEvent, LiveHostServices, UiEffect, UiRequest};
use ratatui::backend::TestBackend;

use crate::{App, SPINNER_FRAMES, UiTheme};

/// A backend plus the effect channel the interactive run loop drains, wired exactly as
/// `App::install_ui_sinks` wires them in production.
struct Wired {
    svc: Arc<LiveHostServices>,
    effects: tokio::sync::mpsc::UnboundedReceiver<UiEffect>,
    app: App<TestBackend>,
    /// Kept alive so the request/reply sink is not reported closed; unused by these tests.
    _ui_rx: tokio::sync::mpsc::UnboundedReceiver<UiRequest>,
}

fn wired() -> Wired {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = Arc::new(LiveHostServices::new(
        provider,
        cyrup_tools::Backend::default().proc,
        std::env::temp_dir(),
    ));
    let (ui_tx, _ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    let (effect_tx, effects) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
    App::<TestBackend>::install_ui_sinks(&svc, ui_tx, effect_tx);
    Wired {
        svc,
        effects,
        app: App::new(TestBackend::new(70, 16), UiTheme::dark()).unwrap(),
        _ui_rx,
    }
}

impl Wired {
    /// Drain the effect channel into the app, as the run loop's `ui_effect_rx` arm does.
    fn pump(&mut self) {
        while let Ok(effect) = self.effects.try_recv() {
            self.app.apply_ui_effect(effect);
        }
    }

    /// The painted screen.
    fn screen(&mut self) -> String {
        self.app.draw().unwrap();
        let buf = self.app.terminal().backend().buffer();
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
}

/// **`setWorkingMessage`.** Pi: `this.workingMessage = message` and, when the working band is live,
/// `activeStatusIndicator.setMessage(message ?? this.defaultWorkingMessage)`
/// (`interactive-mode.ts:2377-2382`).
///
/// PRE-FIX: `LiveHostServices::set_working_message` did not exist, so the trait's empty default ran,
/// nothing reached the effect channel, and the band still read `Working...` — the first
/// `assert!(after.contains("indexing 412 files"))` fails.
#[test]
fn a_working_message_replaces_the_band_copy_live_and_none_restores_the_default() {
    let mut w = wired();
    w.app.ingest_event(&AgentSessionEvent::AgentStart);
    assert!(
        w.screen().contains("Working..."),
        "fixture: the default band is up"
    );

    w.svc.set_working_message(Some("indexing 412 files"));
    w.pump();
    let after = w.screen();
    assert!(
        after.contains("indexing 412 files"),
        "the extension's copy must replace it:\n{after}"
    );
    assert!(
        !after.contains("Working..."),
        "…and the default must be gone:\n{after}"
    );

    // Upstream's no-argument call restores `defaultWorkingMessage` (`?? this.defaultWorkingMessage`).
    w.svc.set_working_message(None);
    w.pump();
    let restored = w.screen();
    assert!(
        restored.contains("Working..."),
        "`None` restores the default:\n{restored}"
    );
    assert!(
        !restored.contains("indexing 412 files"),
        "…and drops the override:\n{restored}"
    );
}

/// **`setWorkingMessage` persists across turns.** Upstream seeds every new `WorkingStatusIndicator`
/// from `this.workingMessage` (`interactive-mode.ts:3116-3120`), so the override survives the turn
/// it was set in — an extension sets it once, not once per turn.
///
/// PRE-FIX: fails for the same reason as the test above — the override never arrived at all.
#[test]
fn a_working_message_survives_the_turn_it_was_set_in() {
    let mut w = wired();
    w.svc.set_working_message(Some("waiting on the build"));
    w.pump();
    // Set while IDLE: nothing is on screen yet, and the next turn must pick it up.
    w.app.ingest_event(&AgentSessionEvent::AgentStart);
    assert!(
        w.screen().contains("waiting on the build"),
        "seeded into the next turn's band"
    );

    w.app.ingest_event(&AgentSessionEvent::AgentEnd {
        messages: vec![],
        will_retry: false,
    });
    w.app.ingest_event(&AgentSessionEvent::AgentStart);
    let second = w.screen();
    assert!(
        second.contains("waiting on the build"),
        "and into the one after that:\n{second}"
    );
}

/// **`setWorkingVisible`.** Pi: `false` ⇒ `clearStatusIndicator("working")`; a later `agent_start`
/// takes the `else { this.clearStatusIndicator() }` branch instead of mounting a loader
/// (`interactive-mode.ts:2091-2108`, `:3114-3124`).
///
/// PRE-FIX: `set_working_visible` was the trait's empty default, so the band stayed up and the
/// `assert!(!hidden.contains("Working..."))` fails.
#[test]
fn working_visible_false_takes_the_band_down_and_keeps_it_down_next_turn() {
    let mut w = wired();
    w.app.ingest_event(&AgentSessionEvent::AgentStart);
    assert!(w.screen().contains("Working..."), "fixture: the band is up");

    w.svc.set_working_visible(false);
    w.pump();
    let hidden = w.screen();
    assert!(
        !hidden.contains("Working..."),
        "the band must come down at once:\n{hidden}"
    );

    // The flag is session state, not a one-shot: the NEXT turn must not resurrect the band.
    w.app.ingest_event(&AgentSessionEvent::AgentEnd {
        messages: vec![],
        will_retry: false,
    });
    w.app.ingest_event(&AgentSessionEvent::AgentStart);
    let next = w.screen();
    assert!(
        !next.contains("Working..."),
        "…and stay down for the next turn:\n{next}"
    );

    // Re-showing mid-turn mounts it again (`if (isStreaming && kind !== "working")`, `:2099`).
    w.svc.set_working_visible(true);
    w.pump();
    let back = w.screen();
    assert!(
        back.contains("Working..."),
        "`true` mid-stream brings it back:\n{back}"
    );
}

/// MIRROR (the kind filter): hiding the working row must not blank a RETRY countdown, because
/// upstream's `clearStatusIndicator("working")` returns early when the live band is a different kind
/// (`interactive-mode.ts:2079-2081`). This stays green through a revert of the whole fix — it is the
/// guard against over-clearing, not the fix's own proof.
#[test]
fn working_visible_false_leaves_a_retry_band_alone() {
    let mut w = wired();
    w.app.ingest_event(&AgentSessionEvent::AutoRetryStart {
        attempt: 1,
        max_attempts: 3,
        delay_ms: 30_000,
        error_message: "429".to_string(),
    });
    assert!(
        w.screen().contains("Retrying (1/3)"),
        "fixture: a retry band is live"
    );

    w.svc.set_working_visible(false);
    w.pump();
    let after = w.screen();
    assert!(
        after.contains("Retrying (1/3)"),
        "the retry countdown must survive:\n{after}"
    );
}

/// **`setWorkingIndicator`.** Pi's `Loader.setIndicator` swaps the frame list and the interval
/// (`pi/packages/tui/src/components/loader.ts:64-69`), and `frames: []` suppresses the glyph
/// entirely — `updateDisplay` emits the `"{frame} "` prefix only when the frame is non-empty
/// (`:86`).
///
/// PRE-FIX: `set_working_indicator` was the trait's empty default, so the Braille spinner kept
/// drawing and `assert!(custom.contains("<*>"))` fails.
#[test]
fn a_custom_working_indicator_replaces_the_frames_and_empty_frames_hide_the_glyph() {
    let mut w = wired();
    w.app.ingest_event(&AgentSessionEvent::AgentStart);
    assert!(
        SPINNER_FRAMES.iter().any(|f| w.screen().contains(f)),
        "fixture: the built-in Braille spinner is drawing"
    );

    // A single frame never animates upstream (`restartAnimation`'s `frames.length <= 1` early
    // return, `loader.ts:74-76`), so it is stable to assert on without a clock.
    w.svc
        .set_working_indicator(Some(&serde_json::json!({"frames": ["<*>"]})));
    w.pump();
    let custom = w.screen();
    assert!(
        custom.contains("<*>"),
        "the extension's frame must draw:\n{custom}"
    );
    assert!(
        !SPINNER_FRAMES.iter().any(|f| custom.contains(f)),
        "…and the built-in Braille frames must be gone:\n{custom}"
    );
    assert!(
        custom.contains("Working..."),
        "the MESSAGE is independent of the glyph:\n{custom}"
    );

    // `frames: []` — no glyph at all, message intact.
    w.svc
        .set_working_indicator(Some(&serde_json::json!({"frames": []})));
    w.pump();
    let bare = w.screen();
    assert!(!bare.contains("<*>"), "the previous frame is gone:\n{bare}");
    assert!(
        !SPINNER_FRAMES.iter().any(|f| bare.contains(f)),
        "`frames: []` draws no spinner at all:\n{bare}"
    );
    assert!(
        bare.contains("Working..."),
        "…but the band and its message stay:\n{bare}"
    );

    // `None` restores the built-in animated spinner (`indicator?.frames !== undefined ? … :
    // DEFAULT_FRAMES`, `loader.ts:66`).
    w.svc.set_working_indicator(None);
    w.pump();
    let restored = w.screen();
    assert!(
        SPINNER_FRAMES.iter().any(|f| restored.contains(f)),
        "`None` restores the default spinner:\n{restored}"
    );
}

/// **`setHiddenThinkingLabel`.** Pi assigns `label ?? this.defaultHiddenThinkingLabel` and
/// re-broadcasts it to every mounted assistant component plus the streaming one
/// (`interactive-mode.ts:2118-2129`); the label is what `hideThinkingBlock` renders in place of the
/// reasoning body (`components/assistant-message.ts:139-143`).
///
/// PRE-FIX: `set_hidden_thinking_label` was the trait's empty default and the TUI's label was the
/// hard-coded `HIDDEN_THINKING_LABEL` constant, so the placeholder still read `Thinking...` and
/// `assert!(after.contains("[reasoning withheld]"))` fails.
#[test]
fn a_hidden_thinking_label_replaces_the_collapsed_placeholder_retroactively() {
    let mut w = wired();
    w.app.state_mut().transcript.set_hide_thinking_block(true);
    w.app
        .state_mut()
        .transcript
        .push_thinking_delta("chain of thought");
    assert!(
        w.screen().contains("Thinking..."),
        "fixture: the default placeholder is up"
    );

    // Set AFTER the block is already on screen: upstream re-labels what is already mounted.
    w.svc
        .set_hidden_thinking_label(Some("[reasoning withheld]"));
    w.pump();
    let after = w.screen();
    assert!(
        after.contains("[reasoning withheld]"),
        "the override must apply:\n{after}"
    );
    assert!(
        !after.contains("Thinking..."),
        "…and replace the default:\n{after}"
    );
    assert!(
        !after.contains("chain of thought"),
        "the body stays collapsed either way:\n{after}"
    );

    w.svc.set_hidden_thinking_label(None);
    w.pump();
    let restored = w.screen();
    assert!(
        restored.contains("Thinking..."),
        "`None` restores the default:\n{restored}"
    );
}

/// **`resetExtensionUI`.** Pi resets all four in one block on a session swap /extension reload:
/// `workingMessage = undefined; workingVisible = true; setWorkingIndicator(); … ;
/// setHiddenThinkingLabel()` (`interactive-mode.ts:2210-2218`).
///
/// PRE-FIX: the four could not be SET in the first place, so the fixture asserts below
/// ("non-vacuity") fail before the resets are ever reached — the state this test is about did not
/// exist.
#[test]
fn a_session_swap_restores_every_working_default() {
    let mut w = wired();
    w.svc
        .set_working_message(Some("owned by the outgoing session"));
    w.svc
        .set_working_indicator(Some(&serde_json::json!({"frames": ["ZQX"]})));
    w.svc.set_hidden_thinking_label(Some("[withheld]"));
    w.svc.set_working_visible(false);
    w.pump();

    // Non-vacuity: every override must actually be IN FORCE before the swap.
    w.app.state_mut().transcript.set_hide_thinking_block(true);
    w.app.state_mut().transcript.push_thinking_delta("secret");
    w.app.ingest_event(&AgentSessionEvent::AgentStart);
    let before = w.screen();
    assert!(
        !before.contains("Working..."),
        "pre-swap: the band is hidden:\n{before}"
    );
    assert!(
        before.contains("[withheld]"),
        "pre-swap: the label is overridden:\n{before}"
    );

    w.app.rebind_session();

    // The transcript is rebuilt by the swap, so re-seed the reasoning block to read the label back.
    w.app.state_mut().transcript.set_hide_thinking_block(true);
    w.app.state_mut().transcript.push_thinking_delta("secret");
    w.app.ingest_event(&AgentSessionEvent::AgentStart);
    let after = w.screen();
    assert!(
        after.contains("Working..."),
        "visibility and message reset:\n{after}"
    );
    assert!(!after.contains("ZQX"), "the custom frame is gone:\n{after}");
    assert!(
        SPINNER_FRAMES.iter().any(|f| after.contains(f)),
        "the built-in spinner is back:\n{after}"
    );
    assert!(
        after.contains("Thinking..."),
        "the hidden-thinking label reset:\n{after}"
    );
    assert!(
        !after.contains("[withheld]"),
        "…and the override is gone:\n{after}"
    );
}
