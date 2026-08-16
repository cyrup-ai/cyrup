//! The TUI-092 F2 render cache must be invalidated on timer-driven repaints while
//! wall-clock-derived transcript content is live — or the live `!`/`!!` block's spinner glyph
//! (`BashExecution::render_lines` → `started.elapsed()`, bash.rs:204) and a running bash tool's
//! `Elapsed …` footer (`render_bash`, transcript.rs:2157) freeze at the values materialised by
//! the last content change.
//!
//! # What was broken
//!
//! The cache key is `(render_generation, width, theme.generation)`; `lines()` *also* reads
//! `Instant::now()` at those two sites. The run loop's spinner tick (gated on
//! `bash_running()`) and elapsed tick (gated on `has_running_elapsed_tool()`) exist precisely to
//! animate those figures, but their bodies only called `draw_synchronized()` — a guaranteed
//! cache hit — so a silent long-running `!` command or `bash` tool painted a frozen glyph and a
//! frozen `Elapsed` until the next content event.
//!
//! The fix adds `TranscriptView::bump_render_tick()` (exactly one generation bump) and calls it
//! from both arms **before** the draw, conditionally in the spinner arm so a quiet streaming
//! turn keeps its zero-materialisation tick.
//!
//! # Why this test reads the source
//!
//! The property is *inside the run loop's `select!` arms*, which own a terminal, a session and a
//! dozen channels; driving an 80 ms tick to observe a frozen glyph is timing-flaky under CI.
//! The guard is structural, the same shape as `run_loop_input_priority.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

/// `app.rs` and `transcript.rs` verbatim, at compile time.
const APP_SRC: &str = include_str!("../app.rs");
const TRANSCRIPT_SRC: &str = include_str!("../transcript.rs");

/// The body of one run-loop arm: from the arm's first line to the start of the next arm.
fn arm_body<'a>(src: &'a str, arm: &str, next_arm: &str) -> &'a str {
    let start = src
        .find(arm)
        .unwrap_or_else(|| panic!("run-loop arm `{arm}` not found — if the loop moved, move this guard with it"));
    let rest = &src[start..];
    let end = rest.find(next_arm).unwrap_or(rest.len());
    &rest[..end]
}

/// The spinner tick must invalidate the cache — but only while the transcript actually paints
/// wall-clock-derived content, so a quiet streaming turn (`indicator.is_active()` alone) keeps
/// its zero-materialisation tick (the F2 win).
#[test]
fn the_spinner_tick_bumps_when_time_derived_content_is_live() {
    let arm = arm_body(APP_SRC, "_ = spinner.tick()", "_ = dialog_countdown.tick()");
    assert!(
        arm.contains("bash_running()") && arm.contains("has_running_elapsed_tool()"),
        "the spinner arm's bump must be gated on live time-derived content \
         (`bash_running()` covers the `!` block's glyph, `has_running_elapsed_tool()` the \
         tool's `Elapsed` footer):\n{arm}"
    );
    let bump = arm
        .find("bump_render_tick()")
        .unwrap_or_else(|| panic!("the spinner arm must invalidate the render cache:\n{arm}"));
    let draw = arm
        .find("draw_synchronized()")
        .unwrap_or_else(|| panic!("the spinner arm must repaint:\n{arm}"));
    assert!(
        bump < draw,
        "the bump must precede the draw, so the very next `cached_render` misses once and the \
         frame re-materialises:\n{arm}"
    );
}

/// The elapsed tick is already gated on `has_running_elapsed_tool()`, so it invalidates
/// unconditionally — otherwise the `Elapsed …` footer it exists to advance stays frozen.
#[test]
fn the_elapsed_tick_bumps_before_repainting() {
    let arm = arm_body(APP_SRC, "_ = elapsed_tick.tick()", "_ = git_branch_poll.tick()");
    let bump = arm
        .find("bump_render_tick()")
        .unwrap_or_else(|| panic!("the elapsed arm must invalidate the render cache:\n{arm}"));
    let draw = arm
        .find("draw_synchronized()")
        .unwrap_or_else(|| panic!("the elapsed arm must repaint:\n{arm}"));
    assert!(
        bump < draw,
        "the bump must precede the draw, so the repaint shows a fresh `Elapsed` figure:\n{arm}"
    );
}

/// The tick mutator is exactly one generation bump, defined once — the bump discipline stays at
/// the public-mutator boundary (the census: 33 bump calls = 32 content mutators + this one).
#[test]
fn bump_render_tick_is_a_single_generation_bump_defined_once() {
    let needle = "pub fn bump_render_tick(&mut self) {\n        self.bump_render_generation();\n    }";
    assert_eq!(
        TRANSCRIPT_SRC.matches(needle).count(),
        1,
        "`bump_render_tick` must be defined exactly once, with a body of exactly one \
         `bump_render_generation()` call"
    );
}
