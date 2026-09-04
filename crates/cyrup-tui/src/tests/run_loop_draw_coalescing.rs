//! The TUI-092 F3 draw coalescing: the run loop's three high-frequency arms must drain every
//! immediately-ready message and then draw ONCE — never one frame per message.
//!
//! # What was broken
//!
//! Every run-loop arm ended in its own `draw_synchronized()` after servicing exactly ONE message,
//! and now ends in exactly one `frames.request()` instead — the paint moved to the single
//! top-of-body site so that N arms firing inside one interval produce ONE frame (PERF-005 §3.1).
//! The token matched is the `frames.request` PREFIX, so it covers the input arm's
//! `request_immediate()` — pi's `requestImmediateRender()`, which preempts the throttle so a
//! keystroke is never delayed, but is still exactly ONE request.
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
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

use futures::{FutureExt, StreamExt};

/// `app.rs` verbatim, at compile time.
const APP_SRC: &str = include_str!("../app/run.rs");
/// The arm bodies the `select!` skeleton dispatches to (§7.2): the channel/timer handlers live in
/// `run_arms.rs`, the input/events drain handlers and the nested `AppAction` dispatch in
/// `run_action.rs`.
const ARMS_SRC: &str = include_str!("../app/run_arms.rs");
const ACTION_SRC: &str = include_str!("../app/run_action.rs");

/// The body of one run-loop arm: from the arm's first line to the start of the next arm. Both
/// anchors must resolve — a terminator that no longer matches (an arm renamed, a fn moved to
/// another file by a re-split) is a lost check, not a licence to read on.
fn arm_body<'a>(src: &'a str, arm: &str, next_arm: &str) -> &'a str {
    let start = src.find(arm).unwrap_or_else(|| {
        panic!("run-loop arm `{arm}` not found — if the loop moved, move this guard with it")
    });
    let rest = &src[start..];
    let end = rest.find(next_arm).unwrap_or_else(|| {
        panic!("terminator `{next_arm}` not found after `{arm}` — if the loop was re-split, re-anchor this guard rather than reading to EOF")
    });
    &rest[..end]
}

/// The body of the LAST arm handler in its file: from the arm's first line to end of file. Used
/// where there is genuinely no following anchor, so slice-to-EOF is stated here rather than
/// reached by an `arm_body` terminator quietly failing to match.
fn arm_body_to_end<'a>(src: &'a str, arm: &str) -> &'a str {
    let start = src.find(arm).unwrap_or_else(|| {
        panic!("run-loop arm `{arm}` not found — if the loop moved, move this guard with it")
    });
    &src[start..]
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
    // The exit is a two-hop chain in the §7.2 skeleton: `dispatch_run_action` maps
    // `AppAction::Quit` to `RunFlow::Break` (run_action.rs), and the select! arm maps
    // `RunFlow::Break` to `break 'run` (run.rs) — still no further draw, still mid-drain.
    // Terminator `swapped = session_swapped` lives in app/run.rs (APP_SRC), the next select! arm.
    let input = arm_body(
        APP_SRC,
        "maybe_in = input.next()",
        "swapped = session_swapped",
    );
    assert!(
        input.contains("RunFlow::Break => break 'run"),
        "a drained `Quit` must leave the run loop mid-drain with no further draw:\n{input}"
    );
    // Terminator `AppAction::Suspend` lives in app/run_action.rs (ACTION_SRC), the next match arm.
    let dispatch = arm_body(ACTION_SRC, "AppAction::Quit", "AppAction::Suspend");
    assert!(
        dispatch.contains("return Ok(RunFlow::Break)"),
        "`AppAction::Quit` must surface as `RunFlow::Break` for the arm's `break 'run`:\n{dispatch}"
    );
}

/// The events arm: every already-queued session event is folded BEFORE the arm's single frame —
/// 100 deltas in one wakeup cost 100 state folds and 1 frame, not 100 frames.
#[test]
fn the_events_arm_drains_every_ready_event_then_draws_once() {
    // `on_session_event` is the LAST fn in app/run_action.rs (ACTION_SRC) — there is no following
    // anchor in that file, so the slice deliberately runs to EOF.
    let arm = arm_body_to_end(ACTION_SRC, "fn on_session_event(");
    assert_eq!(
        arm.matches("frames.request").count(),
        1,
        "the events arm must request exactly one frame per wakeup:\n{arm}"
    );
    // ONE guard brackets the WHOLE drain — the reader thread's wedge detector keeps seeing a
    // single "events" span, not N. A closed stream can no longer enter this handler at all: the
    // `select!` arm's `Some(ev) = events.next()` pattern (`run.rs`) is refutable, so a `None` from
    // a dead subscription disables the branch instead of matching it — there is no closed-stream
    // early return to name any more.
    assert_eq!(
        arm.matches("ArmGuard::enter(\"events\")").count(),
        1,
        "exactly one events guard must bracket the whole drain:\n{arm}"
    );
    let guard = pos(arm, "ArmGuard::enter(\"events\")");
    let drain = pos(arm, "events.next().now_or_never()");
    let draw = pos(arm, "frames.request");
    assert!(
        guard < drain && drain < draw,
        "ONE guard, then the now_or_never drain, then the single draw:\n{arm}"
    );
    // TUI-092 F8 (landed): the ingest is the BY-VALUE one, so each dequeued event's payloads move
    // into the transcript instead of being cloned per event — and the two `matches!` booleans are
    // therefore read ahead of it, because the call consumes `ev`.
    let info = pos(
        arm,
        "let info_changed = matches!(ev, AgentSessionEvent::SessionInfoChanged { .. });",
    );
    let settled = pos(
        arm,
        "let settled = matches!(ev, AgentSessionEvent::AgentSettled);",
    );
    let ingest = pos(
        arm,
        "self.ingest_session_event_owned(ev, &ctx.session).await;",
    );
    assert!(
        info < ingest && settled < ingest,
        "the event-kind booleans must be computed BEFORE the ingest call:\n{arm}"
    );
    // A guest's shutdown is still honored the moment it is detected, mid-drain.
    assert!(
        pos(
            arm,
            "should_honor_extension_shutdown(&ctx.session, settled)"
        ) < pos(arm, "return Ok(RunFlow::ReturnOk);"),
        "the extension-shutdown return stays immediate:\n{arm}"
    );
}

/// The input arm: every queued key is dispatched BEFORE the arm's single frame — a 30-key
/// auto-repeat burst costs 30 dispatches and 1 frame, which is the property that bounds the
/// backlog's PROCESSING (the reader thread's channel stays unbounded by design).
#[test]
fn the_input_arm_drains_every_queued_key_then_draws_once() {
    // Terminator `fn on_session_event(` lives in app/run_action.rs (ACTION_SRC), the next fn.
    let arm = arm_body(ACTION_SRC, "fn on_input_event(", "fn on_session_event(");
    assert_eq!(
        arm.matches("frames.request").count(),
        1,
        "the input arm must request exactly one frame per wakeup:\n{arm}"
    );
    assert_eq!(
        arm.matches("ArmGuard::enter(\"input\")").count(),
        1,
        "exactly one input guard must bracket the whole drain:\n{arm}"
    );
    let guard = pos(arm, "ArmGuard::enter(\"input\")");
    let drain = pos(arm, "input.next().now_or_never()");
    let counted = pos(arm, "serviced += 1;");
    let draw = pos(arm, "frames.request");
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
    // Terminator `fn on_session_event(` lives in app/run_action.rs (ACTION_SRC), the next fn.
    let arm = arm_body(ACTION_SRC, "fn on_input_event(", "fn on_session_event(");
    let draw = pos(arm, "frames.request");
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
    // Terminator `fn on_overlay_ticked(` lives in app/run_arms.rs (ARMS_SRC), the next fn.
    let arm = arm_body(ARMS_SRC, "fn on_bash_msg(", "fn on_overlay_ticked(");
    assert_eq!(
        arm.matches("frames.request").count(),
        1,
        "the bash arm must request exactly one frame per wakeup:\n{arm}"
    );
    let drain = pos(arm, "rx.try_recv()");
    let draw = pos(arm, "frames.request");
    assert!(
        drain < draw,
        "the try_recv drain precedes the single draw:\n{arm}"
    );
    assert!(
        arm.contains("ctx.bash_rx = None;") && arm.contains("break;"),
        "the terminal `Done` still clears the receiver and ends the drain:\n{arm}"
    );
}

/// The coalescing invariant in one census: the three high-frequency arms draw once each; the
/// drain happens inside them, not by reordering arms or adding draws elsewhere.
#[test]
fn one_wakeup_one_frame_across_the_three_high_frequency_arms() {
    for (src, arm, next) in [
        // Each terminator lives in the same file as the arm body it follows: the next fn in
        // app/run_action.rs (ACTION_SRC) and app/run_arms.rs (ARMS_SRC) respectively.
        (
            ACTION_SRC,
            "fn on_input_event(",
            Some("fn on_session_event("),
        ),
        (ARMS_SRC, "fn on_bash_msg(", Some("fn on_overlay_ticked(")),
        // `on_session_event` is the last fn in app/run_action.rs — no following anchor, so this
        // one runs to EOF by design.
        (ACTION_SRC, "fn on_session_event(", None),
    ] {
        let body = next.map_or_else(|| arm_body_to_end(src, arm), |t| arm_body(src, arm, t));
        assert_eq!(
            body.matches("frames.request").count(),
            1,
            "arm `{arm}` must request exactly one frame per wakeup:\n{body}"
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
