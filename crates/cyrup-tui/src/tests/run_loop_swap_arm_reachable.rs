//! The TUI run loop's `select!` must both (a) let a closed agent-event stream DISABLE its arm
//! instead of matching it forever, and (b) poll the session-swap arm ahead of that events arm and
//! every ticker.
//!
//! # What was broken
//!
//! `App::run`'s events arm bound `maybe_ev = events.next()` — an IRREFUTABLE pattern, so a `None`
//! (a closed stream) matched it just as readily as `Some(ev)`. `Fanout::invalidate`
//! (`cyrup-session-svc/src/subscriber.rs:89-93`) drops every sender on a session-swap, so the old
//! subscription's stream goes permanently `Ready(None)` the instant a replacement lands — and
//! under `biased;` a permanently-ready arm wins every poll and starves everything below it. The
//! swap arm (`swapped = session_swapped`) sat LAST, so it was never reached: no re-subscribe, no
//! `rebind_session()`, the loop stayed bound to the DISPOSED session (a `/new` prints its receipt
//! and the TUI is up but dead, one worker hot-spinning at 100%). This is the third instance of the
//! same hazard class `run_loop_input_priority.rs` pins for the input arm.
//!
//! The fix is two changes, checked together because either one alone is insufficient: (1) the
//! events arm's pattern must be refutable (`Some(ev) = events.next()`), so a closed stream
//! disables the branch instead of matching it, and (2) the swap arm must sit ABOVE every arm that
//! can become permanently ready — the tickers *and* the events arm — so a swap is serviced even
//! while (1) is not yet exploited.
//!
//! # Why this test reads the source
//!
//! Same reason as `run_loop_input_priority.rs`: the property is *ordering and pattern shape inside
//! a macro*, in a loop that owns a terminal, a session and a dozen channels — and
//! `runtime_swap.rs`'s coverage calls `app.rebind_session()` BY HAND (`:92`, `:137`), never through
//! `App::run` (which is implemented only for `App<InlineBackend<Stdout>>`), so the `select!` arm
//! ordering itself has zero behavioural coverage. The guard is structural, the same shape as
//! `run_loop_input_priority.rs` and `run_loop_draw_coalescing.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

/// `run.rs` verbatim, at compile time.
const APP_SRC: &str = include_str!("../app/run.rs");

/// In the run loop's cancel-racing `select!`: the events arm is refutable, and the swap arm
/// outranks it and every ticker.
#[test]
fn the_swap_arm_outranks_the_events_arm_and_every_ticker_and_the_events_pattern_is_refutable() {
    let mut checked = 0usize;
    for (offset, _) in APP_SRC.match_indices("tokio::select! {") {
        let body = &APP_SRC[offset..];
        let end = body[1..].find("tokio::select! {").map_or(body.len(), |i| i + 1);
        let block = &body[..end];
        if !block.contains("cancel.cancelled()") || !block.contains("events.next()") {
            continue;
        }
        checked += 1;

        // (1) The events arm must be refutable: `Some(ev) = events.next()`, never the irrefutable
        // `maybe_ev = events.next()` a `None` from a closed stream would match forever.
        assert!(
            block.contains("Some(ev) = events.next()"),
            "the events arm must be spelled `Some(ev) = events.next()` (refutable) in the \
             select! at byte {offset} of run.rs — a `None` from a closed stream (every session \
             swap ends the old subscription, subscriber.rs:89-93) must DISABLE this branch, not \
             match it and stay ready forever",
        );
        assert!(
            !block.contains("maybe_ev = events.next() =>"),
            "found the OLD irrefutable `maybe_ev = events.next() =>` arm spelling in the select! \
             at byte {offset} of run.rs — a `None` matches an irrefutable binding, which keeps a \
             closed stream's arm permanently ready and starves every arm below it under `biased;` \
             (the `/new`-freezes-the-TUI bug)",
        );

        // (2) The swap arm must outrank the events arm and every ticker.
        let swap_pos = block.find("swapped = session_swapped").unwrap_or_else(|| {
            panic!("the swap arm must be spelled `swapped = session_swapped` in the select! at byte {offset} of run.rs")
        });
        let events_pos = block.find("Some(ev) = events.next()").unwrap_or_else(|| {
            panic!("expected a `Some(ev) = events.next()` arm in the select! at byte {offset} of run.rs")
        });
        assert!(
            swap_pos < events_pos,
            "the swap arm must outrank the events arm — a closed events stream (every session \
             swap) must be re-subscribed by the swap arm before the events arm is polled again; \
             found `events.next()` above `swapped = session_swapped` in the select! at byte \
             {offset} of run.rs",
        );
        for ticker in [
            "_ = ctx.spinner.tick()",
            "_ = dialog_countdown.tick()",
            "_ = progress_keepalive.tick()",
            "_ = elapsed_tick.tick()",
            "_ = git_branch_poll.tick()",
        ] {
            let ticker_pos = block.find(ticker).unwrap_or_else(|| {
                panic!("ticker `{ticker}` not found in the run-loop select! — if the arm was renamed, rename it here rather than losing the check")
            });
            assert!(
                swap_pos < ticker_pos,
                "the swap arm must outrank every ticker — under `biased;` a ticker above it that \
                 re-arms every poll would starve the rebind permanently; found `{ticker}` above \
                 `swapped = session_swapped` in the select! at byte {offset} of run.rs",
            );
        }

        // The input arm still outranks the swap arm (run_loop_input_priority.rs owns the
        // cancel/input half of this ordering; this just confirms the swap arm's own position
        // relative to it, since `on_session_swapped`'s reconcile call from the input arm depends
        // on the swap arm being reachable directly below it).
        let input_pos = block.find("maybe_in = input.next()").unwrap_or_else(|| {
            panic!("expected an `input.next()` arm in the select! at byte {offset} of run.rs")
        });
        assert!(
            input_pos < swap_pos,
            "the input arm must precede the swap arm; found `swapped = session_swapped` above \
             `maybe_in = input.next()` in the select! at byte {offset} of run.rs",
        );
    }
    assert!(
        checked >= 1,
        "expected to find the run loop's select! racing both `cancel.cancelled()` and \
         `events.next()` in run.rs — if the loop moved, move this guard with it rather than \
         deleting it",
    );
}
