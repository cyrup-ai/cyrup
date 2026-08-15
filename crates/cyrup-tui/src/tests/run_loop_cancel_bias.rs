//! The TUI run loop's `select!` must declare `biased;` so the cancel arm wins every tie.
//!
//! # What was broken
//!
//! `App::run`'s `tokio::select!` races the app cancel token against terminal input, the agent event
//! stream, five tickers and eight channels with **no `biased;`**. `tokio::select!` without it polls
//! its ready arms in a RANDOM order, so an iteration in which teardown was requested *and* another
//! arm was simultaneously ready could service the other arm: one more consumed keystroke, one more
//! drawn frame, one more applied agent event after `cancel` fired. It terminates quickly in
//! expectation, but nothing in the code bounds how much runs after cancellation, and shutdown
//! ordering is the one thing the token exists to define. The codebase already treats this as
//! mandatory elsewhere — `cyrup-tools/src/lock.rs:178` calls `biased;` "REQUIRED, not a
//! micro-optimisation", and every `select!` in `cyrup-ext/src/host/live.rs` carries it.
//!
//! # Why this test reads the source
//!
//! The property is *ordering inside a macro*, and the loop it lives in is a 700-line `async fn`
//! that owns a terminal, a session, a spawned tmux probe and eight channels. Driving it to observe
//! the tie would be a coin-flip assertion — the exact non-determinism under test — so the guard is
//! structural, the same shape as `cyrup-session-svc/src/tests/compaction_tokens_after.rs` and
//! `cyrup-ext-subagents/src/extension.rs`'s own `include_str!` checks. It fails on a `select!` that
//! races `cancel.cancelled()` without `biased;`, whoever writes it and whenever.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

/// `app.rs` verbatim, at compile time.
const APP_SRC: &str = include_str!("../app.rs");

/// Every `tokio::select!` in `app.rs` that races the app cancel token declares `biased;` as its
/// first statement (comments in between are fine — the check ignores them).
#[test]
fn every_cancel_racing_select_in_the_run_loop_is_biased() {
    let mut checked = 0usize;
    for (offset, _) in APP_SRC.match_indices("tokio::select! {") {
        let body = &APP_SRC[offset..];
        // The arm list of one `select!`: everything up to the next one, which is enough context to
        // see both its first statement and whether it races the token.
        let end = body[1..].find("tokio::select! {").map_or(body.len(), |i| i + 1);
        let block = &body[..end];
        if !block.contains("cancel.cancelled()") {
            continue;
        }
        checked += 1;
        let first_stmt = block
            .lines()
            .skip(1)
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("//"))
            .unwrap_or("");
        assert_eq!(
            first_stmt, "biased;",
            "a select! racing `cancel.cancelled()` must poll the cancel arm first; \
             found `{first_stmt}` at byte {offset} of app.rs",
        );
    }
    assert!(
        checked >= 1,
        "expected to find the run loop's cancel-racing select! in app.rs — if the loop moved, move \
         this guard with it rather than deleting it",
    );
}

/// …and the cancel arm is still the FIRST arm, since `biased;` polls in written order: putting a
/// work arm above it would reinstate the defect while keeping the keyword.
#[test]
fn the_cancel_arm_is_the_first_arm() {
    for (offset, _) in APP_SRC.match_indices("tokio::select! {") {
        let body = &APP_SRC[offset..];
        let end = body[1..].find("tokio::select! {").map_or(body.len(), |i| i + 1);
        let block = &body[..end];
        if !block.contains("cancel.cancelled()") {
            continue;
        }
        let first_arm = block
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//") && *l != "biased;")
            .map(str::to_string)
            .next()
            .unwrap_or_default();
        assert!(
            first_arm.contains("cancel.cancelled()"),
            "the cancel arm must be written first under `biased;`; found `{first_arm}`",
        );
    }
}
