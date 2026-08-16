//! The TUI run loop's `select!` must poll the input arm ahead of every ticker arm.
//!
//! # What was broken (TUI-092 §2.5 / §5c)
//!
//! `App::run`'s `tokio::select!` declares `biased;`, so arms are polled in source order and the
//! FIRST ready one wins. At the bug's HEAD the input arm sat at position #7, below five ticker
//! arms — and the spinner ticker (`SPINNER_INTERVAL = 80 ms`, `status_indicator.rs:48`) is armed
//! for the whole of every streaming turn. The instant one `draw_synchronized` cost more than a
//! tick, the spinner arm was *always* ready when the loop came round, so the input arm was never
//! polled again: the loop kept drawing frames while the keyboard went progressively dead. No
//! `.await` had to hang for that to happen.
//!
//! The fix moved `maybe_in = input.next()` to position #2 — directly beneath
//! `_ = cancel.cancelled() => break` and above `_ = spinner.tick()`. The ordering rule is now
//! **cancel, then input, then everything else**, and the second half is as load-bearing as the
//! first. This test pins it so a future "tidy" cannot move the input arm back down among the
//! tickers.
//!
//! # Why this test reads the source
//!
//! The property is *ordering inside a macro*, and the loop it lives in owns a terminal, a session
//! and a dozen channels. Driving it to observe the starvation would need a frame that reliably
//! costs more than 80 ms under CI load — the exact non-determinism under test — so the guard is
//! structural, the same shape as `run_loop_cancel_bias.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

/// `app.rs` verbatim, at compile time.
const APP_SRC: &str = include_str!("../app.rs");

/// In the run loop's cancel-racing `select!`, the cancel arm is first, the input arm is second,
/// and every ticker arm sits below both.
#[test]
fn the_input_arm_outranks_every_ticker() {
    let mut checked = 0usize;
    for (offset, _) in APP_SRC.match_indices("tokio::select! {") {
        let body = &APP_SRC[offset..];
        // The arm list of one `select!`: everything up to the next one, which is enough context
        // to see all three arms of interest.
        let end = body[1..].find("tokio::select! {").map_or(body.len(), |i| i + 1);
        let block = &body[..end];
        if !block.contains("cancel.cancelled()") || !block.contains("input.next()") {
            continue;
        }
        checked += 1;
        let cancel_pos = block
            .find("_ = cancel.cancelled() => break")
            .unwrap_or_else(|| panic!("the cancel arm must be spelled `_ = cancel.cancelled() => break` at byte {offset} of app.rs"));
        let input_pos = block
            .find("maybe_in = input.next()")
            .unwrap_or_else(|| panic!("the input arm must be spelled `maybe_in = input.next()` at byte {offset} of app.rs"));
        assert!(
            cancel_pos < input_pos,
            "the cancel arm keeps position #1 (run_loop_cancel_bias.rs pins why); \
             found input before cancel in the select! at byte {offset} of app.rs",
        );
        for ticker in [
            "_ = spinner.tick()",
            "_ = dialog_countdown.tick()",
            "_ = progress_keepalive.tick()",
            "_ = elapsed_tick.tick()",
            "_ = git_branch_poll.tick()",
        ] {
            let Some(ticker_pos) = block.find(ticker) else {
                continue;
            };
            assert!(
                input_pos < ticker_pos,
                "TUI-092 §5c: the input arm must outrank every ticker — under `biased;` a ticker \
                 above it that re-arms faster than a frame costs starves input PERMANENTLY; \
                 found `{ticker}` above `maybe_in = input.next()` in the select! at byte \
                 {offset} of app.rs",
            );
        }
    }
    assert!(
        checked >= 1,
        "expected to find the run loop's select! racing both `cancel.cancelled()` and \
         `input.next()` in app.rs — if the loop moved, move this guard with it rather than \
         deleting it",
    );
}
