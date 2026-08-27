//! The TUI-092 F2 render cache must be invalidated on timer-driven repaints while
//! wall-clock-derived transcript content is live — or the live `!`/`!!` block's spinner glyph
//! (`BashExecution::render_lines` → `started.elapsed()`, bash.rs:226-233) and a running bash tool's
//! `Elapsed …` footer (`render_bash`, transcript/tool_builtin.rs:214) freeze at the values
//! materialised by the last content change.
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
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

/// `app.rs` and `transcript/cache.rs` verbatim, at compile time.
const APP_SRC: &str = include_str!("../app/run.rs");
const ARMS_SRC: &str = include_str!("../app/run_arms.rs");
const TRANSCRIPT_SRC: &str = include_str!("../transcript/cache.rs");

/// The body of one run-loop arm: from the arm's first line to the start of the next arm. Both
/// anchors must resolve — a terminator that no longer matches (an arm renamed, a fn moved to
/// another file by a re-split) is a lost check, not a licence to read on.
fn arm_body<'a>(src: &'a str, arm: &str, next_arm: &str) -> &'a str {
    let start = src
        .find(arm)
        .unwrap_or_else(|| panic!("run-loop arm `{arm}` not found — if the loop moved, move this guard with it"));
    let rest = &src[start..];
    let end = rest.find(next_arm).unwrap_or_else(|| {
        panic!("terminator `{next_arm}` not found after `{arm}` — if the loop was re-split, re-anchor this guard rather than reading to EOF")
    });
    &rest[..end]
}

/// The spinner tick must invalidate the cache — but only while the transcript actually paints
/// wall-clock-derived content, so a quiet streaming turn (`indicator.is_active()` alone) keeps
/// its zero-materialisation tick (the F2 win).
#[test]
fn the_spinner_tick_bumps_when_time_derived_content_is_live() {
    // Terminator `_ = dialog_countdown.tick()` lives in app/run.rs (APP_SRC), the next select! arm.
    let arm = arm_body(APP_SRC, "_ = ctx.spinner.tick()", "_ = dialog_countdown.tick()");
    assert!(
        arm.contains("bash_running()"),
        "the spinner arm's guard must cover the `!` block's live glyph (`bash_running()`):\n{arm}"
    );
    // …and the handler additionally gates the bump on the tool's `Elapsed` footer
    // (`has_running_elapsed_tool()`), inside `on_spinner_tick` (run_arms.rs).
    // Terminator `fn on_dialog_countdown_tick(` lives in app/run_arms.rs (ARMS_SRC), the next fn.
    assert!(
        arm_body(ARMS_SRC, "fn on_spinner_tick(", "fn on_dialog_countdown_tick(")
            .contains("has_running_elapsed_tool()"),
        "the spinner bump must also be gated on the tool's live `Elapsed` footer"
    );
    // …and the arm's handler (`on_spinner_tick`, run_arms.rs) owns the bump-then-repaint body.
    // Terminator `fn on_dialog_countdown_tick(` lives in app/run_arms.rs (ARMS_SRC), the next fn.
    let body = arm_body(ARMS_SRC, "fn on_spinner_tick(", "fn on_dialog_countdown_tick(");
    let bump = body
        .find("bump_render_tick()")
        .unwrap_or_else(|| panic!("the spinner arm must invalidate the render cache:\n{body}"));
    let draw = body
        .find("draw_synchronized()")
        .unwrap_or_else(|| panic!("the spinner arm must repaint:\n{body}"));
    assert!(
        bump < draw,
        "the bump must precede the draw, so the very next `cached_render` misses once and the \
         frame re-materialises:\n{body}"
    );
}

/// The elapsed tick is already gated on `has_running_elapsed_tool()`, so it invalidates
/// unconditionally — otherwise the `Elapsed …` footer it exists to advance stays frozen.
#[test]
fn the_elapsed_tick_bumps_before_repainting() {
    // Terminator `_ = git_branch_poll.tick()` lives in app/run.rs (APP_SRC), the next select! arm.
    let arm = arm_body(APP_SRC, "_ = elapsed_tick.tick()", "_ = git_branch_poll.tick()");
    assert!(
        arm.contains("has_running_elapsed_tool()"),
        "the elapsed arm stays gated on a live `Elapsed` footer:\n{arm}"
    );
    // The arm's handler (`on_elapsed_tick`, run_arms.rs) owns the bump-then-repaint body.
    // Terminator `fn on_git_branch_poll(` lives in app/run_arms.rs (ARMS_SRC), the next fn.
    let body = arm_body(ARMS_SRC, "fn on_elapsed_tick(", "fn on_git_branch_poll(");
    let bump = body
        .find("bump_render_tick()")
        .unwrap_or_else(|| panic!("the elapsed arm must invalidate the render cache:\n{body}"));
    let draw = body
        .find("draw_synchronized()")
        .unwrap_or_else(|| panic!("the elapsed arm must repaint:\n{body}"));
    assert!(
        bump < draw,
        "the bump must precede the draw, so the repaint shows a fresh `Elapsed` figure:\n{body}"
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
