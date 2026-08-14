//! TUI-016 / TUI-052 — a QUEUED message must be shown as queued, and only as queued.
//!
//! Both were measured live on 2026-08-13 (`docs/gap-analysis/REPRO-LOG.md`) against a real
//! streaming turn:
//!
//! * **TUI-016.** cyrup had no queue surface at all — the `{n} queued` footer segment had been
//!   deleted by a fidelity pass and `queue_update` discarded the message TEXTS — and, worse,
//!   `dispatch_submission` echoed every submission into the CHAT TRANSCRIPT as an ordinary user
//!   bubble before the session had decided whether to queue it. The user was shown the opposite of
//!   the truth. A 200-line scrollback search for `queue|steer|follow` returned only the two literal
//!   payloads that had been typed.
//! * **TUI-052.** Because that echo was never retracted, a queued message dequeued by Escape stayed
//!   in the transcript **forever** as a phantom user turn that was never sent and is not in the
//!   session JSONL.
//!
//! Upstream is `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts` @v0.83.0:
//! `updatePendingMessagesDisplay` (`:3974-3991`) renders `Steering: …` / `Follow-up: …` rows plus
//! the `↳ {key} to edit all queued messages` hint above the editor, and the user bubble is written
//! **only** from `case "message_start"` with `role === "user"` (`:2915-2918`) — never from the
//! submit handler, which clears the editor and calls `updatePendingMessagesDisplay()` (`:2827-2833`)
//! and writes nothing to the chat container.
//!
//! Every assertion below is on PAINTED CELLS, because that is the layer both defects lived in: the
//! source at `app.rs` looked correct and the count was being set on a `StatusBar` with no render
//! site.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_agent::AgentMessage;
use cyrup_core::Content;
use cyrup_session_svc::AgentSessionEvent;
use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{App, AppAction, InputEvent, UiTheme};
use ratatui::backend::TestBackend;

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 16), UiTheme::dark()).unwrap()
}

/// The whole rendered buffer as text.
fn screen(app: &App<TestBackend>) -> String {
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

fn key(c: char) -> InputEvent {
    InputEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}

fn enter() -> InputEvent {
    InputEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
}

/// Type `text` and press Enter, returning what the app asked the run loop to do.
fn submit(app: &mut App<TestBackend>, text: &str) -> AppAction {
    for c in text.chars() {
        app.handle_input(&key(c));
    }
    app.handle_input(&enter())
}

/// What the session emits once it has decided to hold the message (`_emitQueueUpdate`,
/// `agent-session.ts:1382`).
fn queued(app: &mut App<TestBackend>, steering: &[&str], follow_up: &[&str]) {
    app.ingest_event(&AgentSessionEvent::QueueUpdate {
        steering: steering.iter().map(|s| (*s).to_string()).collect(),
        follow_up: follow_up.iter().map(|s| (*s).to_string()).collect(),
    });
}

/// What the agent emits when the turn that actually carries the message begins
/// (`agent.rs:478`/`:513` → `interactive-mode.ts:2915-2918`).
fn dispatched(app: &mut App<TestBackend>, text: &str) {
    app.ingest_event(&AgentSessionEvent::MessageStart {
        message: AgentMessage::User {
            content: vec![Content::Text { text: text.to_string(), text_signature: None }],
            timestamp: None,
        },
    });
}

// ------------------------------------------------------------------------ TUI-016 ----

/// RED before this change: the screen contained `QUEUEDMSGONE` as an ordinary transcript bubble and
/// **zero** occurrences of `Steering:`, `Follow-up:` or the dequeue hint.
#[test]
fn a_queued_message_renders_as_a_pending_row_and_not_as_a_transcript_bubble() {
    let mut app = new_app();
    assert!(
        matches!(submit(&mut app, "QUEUEDMSGONE"), AppAction::Submit(_)),
        "the submission is handed to the run loop"
    );
    queued(&mut app, &["QUEUEDMSGONE"], &[]);
    app.draw().unwrap();
    let s = screen(&app);
    assert!(s.contains("Steering: QUEUEDMSGONE"), "the pending row must be on screen:\n{s}");
    assert_eq!(
        s.matches("QUEUEDMSGONE").count(),
        1,
        "exactly ONE occurrence — a transcript bubble alongside the row is the double-render \
         TUI-016's Fix calls out:\n{s}"
    );
}

#[test]
fn steering_follow_up_and_the_dequeue_hint_all_render_in_pi_s_order() {
    let mut app = new_app();
    queued(&mut app, &["ALPHA", "BETA"], &["GAMMA"]);
    app.draw().unwrap();
    let s = screen(&app);
    let at = |needle: &str| s.find(needle).unwrap_or_else(|| panic!("missing {needle:?}:\n{s}"));
    let alpha = at("Steering: ALPHA");
    let beta = at("Steering: BETA");
    let gamma = at("Follow-up: GAMMA");
    let hint = at("to edit all queued messages");
    assert!(alpha < beta && beta < gamma && gamma < hint, "order: {alpha} {beta} {gamma} {hint}");
    // `↳ ${dequeueHint} …` (`interactive-mode.ts:3988`) — the stock `app.message.dequeue` binding
    // is `alt+up` (`core/keybindings.ts:102-105`), title-cased by `keyDisplayText` and — on macOS
    // only — with `alt` rewritten to `option` (`keybinding-hints.ts:13`).
    let expected = if cfg!(target_os = "macos") {
        "↳ Option+Up to edit all queued messages"
    } else {
        "↳ Alt+Up to edit all queued messages"
    };
    assert!(s.contains(expected), "hint text (expected {expected:?}):\n{s}");
}

#[test]
fn the_pending_region_clears_when_the_queue_drains() {
    let mut app = new_app();
    queued(&mut app, &["ALPHA"], &[]);
    app.draw().unwrap();
    assert!(screen(&app).contains("Steering: ALPHA"));

    queued(&mut app, &[], &[]);
    app.draw().unwrap();
    let s = screen(&app);
    assert!(!s.contains("Steering:"), "the region must collapse:\n{s}");
    assert!(!s.contains("to edit all queued messages"), "and take the hint with it:\n{s}");
}

/// The hand-off: the row disappears and the bubble appears, driven by the two events the session
/// really emits. At no point are both on screen.
#[test]
fn the_bubble_appears_only_when_the_turn_that_carries_the_message_starts() {
    let mut app = new_app();
    submit(&mut app, "HELLO");
    queued(&mut app, &["HELLO"], &[]);
    app.draw().unwrap();
    assert_eq!(screen(&app).matches("HELLO").count(), 1, "queued: the pending row only");

    // The turn begins: the agent injects the steering message and the queue drains.
    queued(&mut app, &[], &[]);
    dispatched(&mut app, "HELLO");
    app.draw().unwrap();
    let s = screen(&app);
    assert!(!s.contains("Steering: HELLO"), "the pending row is gone:\n{s}");
    assert_eq!(s.matches("HELLO").count(), 1, "and the transcript bubble took its place:\n{s}");
}

/// A plain idle submission still reaches the transcript — via `message_start`, which is the only
/// writer now. Without this the fix would trade one defect for a worse one.
#[test]
fn an_idle_submission_still_produces_a_user_bubble() {
    let mut app = new_app();
    submit(&mut app, "PLAINMSG");
    app.draw().unwrap();
    assert!(
        !screen(&app).contains("PLAINMSG"),
        "nothing is written until the session says the turn started"
    );
    dispatched(&mut app, "PLAINMSG");
    app.draw().unwrap();
    assert!(screen(&app).contains("PLAINMSG"), "the bubble arrives with `message_start`");
}

// ------------------------------------------------------------------------ TUI-052 ----

/// RED before this change: `dispatch_submission` had already pushed `PHANTOM` into the transcript,
/// so dequeuing it with Escape left it on screen forever as a user turn that was never sent and is
/// not in the session JSONL.
#[test]
fn a_message_dequeued_by_escape_leaves_no_phantom_in_the_transcript() {
    let mut app = new_app();
    app.state_mut().status.set_streaming(true);
    submit(&mut app, "PHANTOM");
    queued(&mut app, &["PHANTOM"], &[]);
    app.draw().unwrap();
    assert_eq!(screen(&app).matches("PHANTOM").count(), 1, "the pending row");

    // Escape mid-turn → the run loop drains the queue and restores it to the editor
    // (`restoreQueuedMessagesToEditor`, `interactive-mode.ts:2636-2637`).
    assert_eq!(
        app.handle_input(&InputEvent::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))),
        AppAction::InterruptRestoreQueued
    );
    queued(&mut app, &[], &[]);
    app.restore_queued_to_editor(&["PHANTOM".to_string()]);
    app.draw().unwrap();

    let s = screen(&app);
    assert!(!s.contains("Steering: PHANTOM"), "the pending row is gone:\n{s}");
    assert_eq!(
        s.matches("PHANTOM").count(),
        1,
        "the ONLY remaining occurrence is the editor buffer it was restored into — a second one \
         would be the phantom transcript entry TUI-052 describes:\n{s}"
    );
}

/// TUI-031 — a message queued during a COMPACTION shows up in the same pending region, because
/// `getAllQueuedMessages` folds `compactionQueuedMessages` into the session's two queues
/// (`interactive-mode.ts:3942-3953` @v0.83.0).
///
/// RED at HEAD: `AppState` had no compaction queue at all (`rg 'compaction_queued|compactionQueued'
/// → zero`), so there was nothing to fold and nothing to render — the prompt was dispatched
/// immediately as a fresh turn against a context the compaction was mid-rewrite of.
#[tokio::test]
async fn a_message_queued_during_compaction_renders_in_the_pending_region() {
    let mut app = new_app();
    // The session's own queues are empty; only the compaction queue has anything in it.
    app.state_mut().compaction_queue.push(crate::CompactionQueued {
        text: "QUEUEDDURINGCOMPACTION".to_string(),
        follow_up: false,
    });
    app.ingest_event(&AgentSessionEvent::QueueUpdate {
        steering: Vec::new(),
        follow_up: Vec::new(),
    });
    app.draw().unwrap();
    let s = screen(&app);
    assert!(
        s.contains("Steering: QUEUEDDURINGCOMPACTION"),
        "the compaction queue must fold into the pending region:\n{s}"
    );
}

/// The follow-up mode lands in the follow-up half of the fold (`:3948-3951`).
#[tokio::test]
async fn a_follow_up_queued_during_compaction_uses_the_follow_up_label() {
    let mut app = new_app();
    app.state_mut()
        .compaction_queue
        .push(crate::CompactionQueued { text: "LATER".to_string(), follow_up: true });
    app.ingest_event(&AgentSessionEvent::QueueUpdate {
        steering: Vec::new(),
        follow_up: Vec::new(),
    });
    app.draw().unwrap();
    let s = screen(&app);
    assert!(s.contains("Follow-up: LATER"), "follow-up mode must use pi's label:\n{s}");
}
