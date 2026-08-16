//! The TUI-092 F3 draw coalescing: the run loop's three high-frequency arms must drain every
//! immediately-ready message and then draw ONCE — never one frame per message.
//!
//! # What was broken
//!
//! Every run-loop arm ended in its own `draw_synchronized()` after servicing exactly ONE message,
//! so frames/s was proportional to event rate: a model streaming 100+ `TextDelta`s/s demanded
//! 100+ full frames/s (each paying F2's triple materialisation), every output chunk of a live `!`
//! run cost a frame, and key auto-repeat (30–60/s) against a slow frame was an unbounded
//! one-frame-per-key backlog the loop could never catch up on once frame-cost × key-rate > 1 —
//! the backlog half of the phase-4 lockup.
//!
//! The fix drains each arm before its single draw: the two boxed `EventStream` arms
//! (`maybe_in = input.next()`, `maybe_ev = events.next()`) drain via
//! `futures::FutureExt::now_or_never` on `.next()` — cancel-safe on tokio mpsc, so the dropped
//! one-shot `Next` future loses nothing — and the concrete bash `UnboundedReceiver` drains via
//! `try_recv`. One `select!` wakeup then costs N state folds and ONE frame.
//!
//! # Why this test reads the source
//!
//! The property is *inside the run loop's `select!` arms*, which own a terminal, a session and a
//! dozen channels; driving the loop to count frames per wakeup needs a frame that reliably costs
//! more than an event-arrival interval under CI load — the exact non-determinism under test — so
//! the guard is structural, the same shape as `run_loop_input_priority.rs` and
//! `render_cache_tick.rs`. The one behavioural half — the cancel-safety the drain idiom stands
//! on — is deterministic and is exercised directly on the real `EventStream` type below.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use futures::{FutureExt, StreamExt};

/// `app.rs` verbatim, at compile time.
const APP_SRC: &str = include_str!("../app.rs");

/// The body of one run-loop arm: from the arm's first line to the start of the next arm.
fn arm_body<'a>(src: &'a str, arm: &str, next_arm: &str) -> &'a str {
    let start = src
        .find(arm)
        .unwrap_or_else(|| panic!("run-loop arm `{arm}` not found — if the loop moved, move this guard with it"));
    let rest = &src[start..];
    let end = rest.find(next_arm).unwrap_or(rest.len());
    &rest[..end]
}

/// Position of `needle` inside `haystack`, or panic with the arm body for context.
fn pos(haystack: &str, needle: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not found in:\n{haystack}"))
}

/// The drained `Quit` exits the **run** loop from inside the drain loop, which needs the loop
/// label — a plain `break` there would target the drain loop instead and the quit would draw one
/// more frame and keep running.
#[test]
fn the_run_loop_is_labelled_so_a_drained_quit_can_exit_it() {
    assert_eq!(
        APP_SRC.matches("'run: loop {").count(),
        1,
        "the run loop must carry exactly one `'run` label"
    );
    let input = arm_body(APP_SRC, "maybe_in = input.next() => {", "_ = spinner.tick()");
    assert!(
        input.contains("AppAction::Quit => break 'run"),
        "a drained `Quit` must leave the run loop mid-drain with no further draw:\n{input}"
    );
}

/// The events arm: every already-queued session event is folded BEFORE the arm's single frame —
/// 100 deltas in one wakeup cost 100 state folds and 1 frame, not 100 frames.
#[test]
fn the_events_arm_drains_every_ready_event_then_draws_once() {
    let arm = arm_body(APP_SRC, "maybe_ev = events.next() => {", "ok = theme_changed");
    assert_eq!(
        arm.matches("draw_synchronized()").count(),
        1,
        "the events arm must draw exactly once per wakeup:\n{arm}"
    );
    // One guard brackets the WHOLE drain — the reader thread's wedge detector keeps seeing a
    // single "events" span, not N — and a closed stream (`continue`) is not named to it.
    assert_eq!(
        arm.matches("ArmGuard::enter(\"events\")").count(),
        1,
        "exactly one events guard must bracket the whole drain:\n{arm}"
    );
    let first = pos(arm, "let Some(first) = maybe_ev else { continue };");
    let guard = pos(arm, "ArmGuard::enter(\"events\")");
    let drain = pos(arm, "events.next().now_or_never()");
    let draw = pos(arm, "draw_synchronized()");
    assert!(
        first < guard && guard < drain && drain < draw,
        "let-else, then ONE guard, then the now_or_never drain, then the single draw:\n{arm}"
    );
    // F8 readiness: the two `matches!` booleans are computed ahead of the by-ref ingest call so
    // the by-value swap (which moves `ev`) stays a one-line change.
    let info = pos(arm, "let info_changed = matches!(ev, AgentSessionEvent::SessionInfoChanged { .. });");
    let settled = pos(arm, "let settled = matches!(ev, AgentSessionEvent::AgentSettled);");
    let ingest = pos(arm, "self.ingest_session_event(&ev, &session).await;");
    assert!(
        info < ingest && settled < ingest,
        "the event-kind booleans must be computed BEFORE the ingest call:\n{arm}"
    );
    // A guest's shutdown is still honored the moment it is detected, mid-drain.
    assert!(
        pos(arm, "should_honor_extension_shutdown(&session, settled)") < pos(arm, "return Ok(());"),
        "the extension-shutdown return stays immediate:\n{arm}"
    );
}

/// The input arm: every queued key is dispatched BEFORE the arm's single frame — a 30-key
/// auto-repeat burst costs 30 dispatches and 1 frame, which is the property that bounds the
/// backlog's PROCESSING (the reader thread's channel stays unbounded by design).
#[test]
fn the_input_arm_drains_every_queued_key_then_draws_once() {
    let arm = arm_body(APP_SRC, "maybe_in = input.next() => {", "_ = spinner.tick()");
    assert_eq!(
        arm.matches("draw_synchronized()").count(),
        1,
        "the input arm must draw exactly once per wakeup:\n{arm}"
    );
    assert_eq!(
        arm.matches("ArmGuard::enter(\"input\")").count(),
        1,
        "exactly one input guard must bracket the whole drain:\n{arm}"
    );
    let guard = pos(arm, "ArmGuard::enter(\"input\")");
    let drain = pos(arm, "input.next().now_or_never()");
    let counted = pos(arm, "serviced += 1;");
    let draw = pos(arm, "draw_synchronized()");
    assert!(
        guard < drain && drain < counted && counted < draw,
        "ONE guard, then the now_or_never drain, then per-event dispatch counting, then the \
         single draw:\n{arm}"
    );
}

/// The liveness beacon keeps both invariants under coalescing: it fires once per SERVICED event
/// (a drained chord burst advances the counter by what it serviced — the wedge detector's "is it
/// servicing input" question), and all of the marks land AFTER the single draw ("a frame the user
/// never sees is not service").
#[test]
fn the_input_arm_marks_each_serviced_key_after_the_single_draw() {
    let arm = arm_body(APP_SRC, "maybe_in = input.next() => {", "_ = spinner.tick()");
    let draw = pos(arm, "draw_synchronized()");
    let marks_loop = pos(arm, "for _ in 0..serviced {");
    let mark = pos(arm, "mark_input_serviced();");
    assert!(
        draw < marks_loop && marks_loop < mark,
        "the per-event marks must all land AFTER the arm's single draw:\n{arm}"
    );
}

/// The bash arm: a chatty `!` run's queued chunks cost one frame. The receiver is the concrete
/// `UnboundedReceiver` in scope, so the drain is the synchronous `try_recv` — no future
/// constructed, no waker consulted — and the terminal `Done` still ends the drain and clears the
/// receiver.
#[test]
fn the_bash_arm_drains_with_try_recv_then_draws_once() {
    let arm = arm_body(APP_SRC, "Some(msg) = bash_next => {", "() = overlay_ticked");
    assert_eq!(
        arm.matches("draw_synchronized()").count(),
        1,
        "the bash arm must draw exactly once per wakeup:\n{arm}"
    );
    let drain = pos(arm, "rx.try_recv()");
    let draw = pos(arm, "draw_synchronized()");
    assert!(drain < draw, "the try_recv drain precedes the single draw:\n{arm}");
    assert!(
        arm.contains("bash_rx = None;") && arm.contains("break;"),
        "the terminal `Done` still clears the receiver and ends the drain:\n{arm}"
    );
}

/// The coalescing invariant in one census: the three high-frequency arms draw once each; the
/// drain happens inside them, not by reordering arms or adding draws elsewhere.
#[test]
fn one_wakeup_one_frame_across_the_three_high_frequency_arms() {
    for (arm, next) in [
        ("maybe_in = input.next() => {", "_ = spinner.tick()"),
        ("Some(msg) = bash_next => {", "() = overlay_ticked"),
        ("maybe_ev = events.next() => {", "ok = theme_changed"),
    ] {
        let body = arm_body(APP_SRC, arm, next);
        assert_eq!(
            body.matches("draw_synchronized()").count(),
            1,
            "arm `{arm}` must draw exactly once per wakeup:\n{body}"
        );
    }
}

/// The cancel-safety the drain idiom stands on, exercised on the run loop's real stream type:
/// `now_or_never` polls the one-shot `Next` future once with a no-op waker and drops it, and
/// tokio's mpsc `recv` is cancel-safe — so the pending poll that ENDS a drain loses nothing, and
/// everything queued at drain time comes out in FIFO order. If a future refactor swapped the
/// drain to a non-cancel-safe construct, the final awaited `next()` would miss the late send.
#[tokio::test]
async fn a_pending_drain_poll_loses_no_queued_message() {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
    let mut stream: cyrup_core::EventStream<u64> =
        Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx));
    for i in 0..64 {
        tx.send(i).unwrap();
    }
    // The run loop's drain spelling, verbatim.
    let mut pending = std::collections::VecDeque::new();
    while let Some(Some(ev)) = stream.next().now_or_never() {
        pending.push_back(ev);
    }
    assert_eq!(
        pending.iter().copied().collect::<Vec<_>>(),
        (0..64).collect::<Vec<_>>(),
        "every already-queued message drains, in FIFO order"
    );
    // The poll that ended the drain came back pending and its `Next` future was dropped; a send
    // landing AFTER that drop must still be delivered to the next awaited poll.
    tx.send(64).unwrap();
    assert_eq!(stream.next().await, Some(64));
}
