//! The TUI-092 F2 render cache: key handling, the bump discipline, and the one invariant that
//! makes it safe — **a key hit is byte-identical to a fresh `lines()` compute on every frame**.
//! These tests live inside the module (not `src/tests/`) precisely so they can read and poison
//! the private `render_cache` / `render_generation` fields: the poison-sentinel pattern proves
//! a hit serves the STORED entry (no recompute), which no black-box observation can distinguish.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::transcript::*;
use crate::transcript::layout::wrap_all_owned;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// The impossible height planted into the cache to prove a hit serves the stored entry.
const POISON_HEIGHT: usize = 4242;

fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
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

/// Poison the cache with rows nothing could have produced, so a later read proves the cache was
/// consulted rather than recomputed. `POISON_HEIGHT` rows, so `content_height` reports it.
fn poison(view: &mut TranscriptView) {
    view.render_cache.rows =
        std::sync::Arc::new(vec![Line::from("POISON"); POISON_HEIGHT]);
}

/// The core invariant, asserted after every step of a simulated turn: the cached entry equals a
/// fresh `lines()` wrapped at the same key.
///
/// Re-anchored for PERF-005 §3.0: the cache now holds already-WRAPPED rows, so the oracle is
/// `wrap_all_owned` — cyrup's own `wrap_line`, the rows that actually get blitted — and not
/// `wrapped_height`'s ratatui `Paragraph::line_count`, which was a PREDICTION of what a
/// `.wrap()`ing `Paragraph` would paint. There is no `.wrap()` any more, so `rows.len()` is not an
/// estimate of the height, it IS the height.
fn assert_fresh(view: &mut TranscriptView, theme: &UiTheme, step: &str) {
    let fresh = view.lines(80, theme);
    let fresh_rows = wrap_all_owned(fresh, 80);
    let cache = view.cached_render(80, theme);
    assert_eq!(cache.rows.as_slice(), fresh_rows.as_slice(), "stale cached rows after {step}");
    assert_eq!(cache.rows.len(), fresh_rows.len(), "stale cached height after {step}");
    assert_eq!(
        view.content_height(80, theme),
        fresh_rows.len(),
        "stale content_height after {step}"
    );
}

#[test]
fn a_default_view_starts_unprimed() {
    let view = TranscriptView::new();
    assert_eq!(view.render_generation, 0);
    assert_eq!(view.render_cache.generation, 0);
    assert_eq!(view.render_cache.width, 0);
    assert!(view.render_cache.rows.is_empty());
}

#[test]
fn the_first_content_height_populates_the_cache() {
    let mut view = TranscriptView::new();
    let theme = UiTheme::dark();
    view.push_assistant_delta("hello **world**");
    let h = view.content_height(80, &theme);
    let fresh = view.lines(80, &theme);
    let fresh_rows = wrap_all_owned(fresh, 80);
    assert_eq!(h, fresh_rows.len());
    assert_eq!(view.render_cache.rows.as_slice(), fresh_rows.as_slice());
    assert_eq!(view.render_cache.width, 80);
    assert_eq!(view.render_cache.generation, view.render_generation);
    assert_eq!(view.render_cache.theme_generation, theme.generation);
}

#[test]
fn a_key_hit_serves_the_cached_entry_without_recomputing() {
    let mut view = TranscriptView::new();
    let theme = UiTheme::dark();
    view.push_assistant_delta("real content");
    let _ = view.content_height(80, &theme);
    poison(&mut view);
    assert_eq!(
        view.content_height(80, &theme),
        POISON_HEIGHT,
        "same (generation, width, theme.generation) must hit and serve the stored entry"
    );
}

#[test]
fn a_content_bump_misses_and_recomputes() {
    let mut view = TranscriptView::new();
    let theme = UiTheme::dark();
    view.push_assistant_delta("real content");
    let _ = view.content_height(80, &theme);
    poison(&mut view);
    let generation = view.render_generation;
    view.push_assistant_delta(" more");
    assert!(
        view.render_generation > generation,
        "a bump-list mutator must advance the generation"
    );
    let fresh = view.lines(80, &theme);
    assert_eq!(
        view.content_height(80, &theme),
        wrap_all_owned(fresh, 80).len(),
        "the frame after a bump must re-materialise, not serve the poisoned entry"
    );
}

#[test]
fn a_width_change_misses_and_recomputes() {
    let mut view = TranscriptView::new();
    let theme = UiTheme::dark();
    view.push_assistant_delta("real content");
    let _ = view.content_height(80, &theme);
    poison(&mut view);
    let fresh = view.lines(81, &theme);
    assert_eq!(view.content_height(81, &theme), wrap_all_owned(fresh, 81).len());
    assert_eq!(view.render_cache.width, 81);
}

#[test]
fn a_theme_generation_change_misses_and_recomputes() {
    let mut view = TranscriptView::new();
    let theme = UiTheme::dark();
    view.push_assistant_delta("real content");
    let _ = view.content_height(80, &theme);
    poison(&mut view);
    let mut theme2 = theme.clone();
    theme2.generation = theme.generation.wrapping_add(1);
    let fresh = view.lines(80, &theme);
    assert_eq!(view.content_height(80, &theme2), wrap_all_owned(fresh, 80).len());
    assert_eq!(view.render_cache.theme_generation, theme2.generation);
}

/// The bump census: every public mutator that changes what `lines()` emits must advance the
/// generation. Each case is `(name, setup, measured call)` — the setup (unmeasured) puts the
/// view in the state the measured call needs (a live bash block, a running tool, …), so the
/// assertion pins the MEASURED call's own bump. Delegating pairs (`push_tool_start` →
/// `push_tool_start_rendered`, `bash_complete_simple` → `bash_complete`, `push_tool_end` →
/// `push_tool_end_rendered`) advance the generation more than once, so the assertion is `>`.
#[test]
fn every_bump_list_mutator_advances_the_generation() {
    type Step = (&'static str, fn(&mut TranscriptView), fn(&mut TranscriptView));
    fn noop(_: &mut TranscriptView) {}
    fn live_bash(v: &mut TranscriptView) {
        v.start_bash("ls", false, None, None);
    }
    fn live_tool(v: &mut TranscriptView) {
        v.push_tool_start("read", serde_json::json!({}));
    }
    let steps: Vec<Step> = vec![
        ("set_output_pad", noop, |v| v.set_output_pad(2)),
        ("set_show_images", noop, |v| v.set_show_images(false)),
        ("set_graphical_images", noop, |v| v.set_graphical_images(false)),
        ("set_image_width_cells", noop, |v| v.set_image_width_cells(30)),
        ("set_expand_hint", noop, |v| v.set_expand_hint(Some("ctrl+x".to_string()))),
        ("set_cwd", noop, |v| v.set_cwd(Some(std::path::PathBuf::from("/tmp")))),
        ("start_bash", noop, |v| v.start_bash("ls", false, None, None)),
        ("bash_append", live_bash, |v| v.bash_append("chunk")),
        ("bash_complete", live_bash, |v| v.bash_complete(Some(0), false, false, None)),
        ("bash_complete_simple", live_bash, |v| v.bash_complete_simple(Some(0), false)),
        ("toggle_bash_expanded", live_bash, |v| {
            v.toggle_bash_expanded();
        }),
        ("set_bash_expanded", live_bash, |v| v.set_bash_expanded(true)),
        ("commit_bash", live_bash, |v| v.commit_bash()),
        ("push_bash_execution", noop, |v| {
            v.push_bash_execution("ls", false, "out", Some(0), false, false, None);
        }),
        ("push_user", noop, |v| v.push_user("hi")),
        ("push_assistant_delta", noop, |v| v.push_assistant_delta("a")),
        ("commit_assistant", |v| v.push_assistant_delta("a"), |v| {
            v.commit_assistant(None);
        }),
        ("discard_streaming", |v| v.push_assistant_delta("a"), |v| v.discard_streaming()),
        ("push_thinking_delta", noop, |v| v.push_thinking_delta("t")),
        ("commit_thinking", |v| v.push_thinking_delta("t"), |v| {
            v.commit_thinking(None);
        }),
        ("set_hide_thinking_block", noop, |v| v.set_hide_thinking_block(true)),
        ("set_hidden_thinking_label", noop, |v| {
            v.set_hidden_thinking_label(Some("L".to_string()));
        }),
        ("push_tool_start", noop, |v| v.push_tool_start("read", serde_json::json!({}))),
        ("push_tool_start_rendered", noop, |v| {
            v.push_tool_start_rendered("read", None, serde_json::json!({}), None);
        }),
        ("push_tool_update", live_tool, |v| {
            v.push_tool_update(None, Some(serde_json::json!({ "content": [] })));
        }),
        ("set_edit_preview", live_tool, |v| v.set_edit_preview(None, Ok("diff".to_string()))),
        ("push_tool_end", noop, |v| v.push_tool_end("read", false, None)),
        ("push_tool_end_rendered", noop, |v| {
            v.push_tool_end_rendered("read", None, false, None, None);
        }),
        ("commit_tools", live_tool, |v| v.commit_tools()),
        ("commit_finished_leading_tools", |v| v.push_tool_end("read", false, None), |v| {
            v.commit_finished_leading_tools();
        }),
        ("toggle_tool_expanded", noop, |v| {
            v.toggle_tool_expanded();
        }),
        ("set_tool_expanded", noop, |v| {
            v.set_tool_expanded(true);
        }),
    ];
    assert_eq!(steps.len(), 32, "the bump census is 32 mutators — update both together");
    for (name, setup, call) in steps {
        let mut view = TranscriptView::new();
        setup(&mut view);
        let generation = view.render_generation;
        call(&mut view);
        assert!(
            view.render_generation > generation,
            "{name} must bump the render generation"
        );
    }
}

/// The exemption census: scroll-only and pending-only methods must NOT bump — a spurious bump
/// costs one recompute per call, and these fire on paths that must stay free (every
/// PageUp/PageDown, every status line).
#[test]
fn exempt_methods_never_advance_the_generation() {
    type Step = (&'static str, fn(&mut TranscriptView), fn(&mut TranscriptView));
    fn noop(_: &mut TranscriptView) {}
    let steps: Vec<Step> = vec![
        ("page_up", noop, |v| v.page_up(1)),
        ("page_down", |v| v.page_up(2), |v| v.page_down(1)),
        ("drain_committed", |v| v.push_user("q"), |v| {
            v.drain_committed();
        }),
        ("push_status", noop, |v| v.push_status("s")),
        ("push_loaded_resources", noop, |v| {
            v.push_loaded_resources(vec![crate::startup::StartupLine::default()]);
        }),
        ("push_error", noop, |v| v.push_error("e")),
        ("push_warning", noop, |v| v.push_warning("w")),
        ("push_block", noop, |v| v.push_block("t", "m")),
        ("push_package_updates", noop, |v| v.push_package_updates(&["pkg".to_string()])),
        ("push_skill_invocation", noop, |v| v.push_skill_invocation("n", "c")),
        ("push_custom_message", noop, |v| v.push_custom_message("l", "b")),
        ("push_custom_message_rendered", noop, |v| {
            v.push_custom_message_rendered("l", "b", Rendered::None);
        }),
        ("push_branch_summary", noop, |v| v.push_branch_summary("s")),
        ("push_compaction_summary", noop, |v| v.push_compaction_summary(7, "s")),
    ];
    assert_eq!(steps.len(), 14, "the exemption census is 14 methods — update both together");
    for (name, setup, call) in steps {
        let mut view = TranscriptView::new();
        setup(&mut view);
        let generation = view.render_generation;
        call(&mut view);
        assert_eq!(
            view.render_generation, generation,
            "{name} is exempt and must NOT bump the render generation"
        );
    }
    // Non-vacuous: the pending-pushers really did push (drain collects them all).
    let mut view = TranscriptView::new();
    view.push_status("s");
    view.push_loaded_resources(vec![crate::startup::StartupLine::default()]);
    view.push_error("e");
    view.push_warning("w");
    view.push_block("t", "m");
    view.push_package_updates(&["pkg".to_string()]);
    view.push_skill_invocation("n", "c");
    view.push_custom_message("l", "b");
    view.push_custom_message_rendered("l", "b", Rendered::None);
    view.push_branch_summary("s");
    view.push_compaction_summary(7, "s");
    assert_eq!(view.drain_committed().len(), 11, "every pending-pusher landed one entry");
}

#[test]
fn bump_render_tick_advances_once_and_invalidates() {
    let mut view = TranscriptView::new();
    let theme = UiTheme::dark();
    view.push_assistant_delta("real content");
    let _ = view.content_height(80, &theme);
    poison(&mut view);
    let generation = view.render_generation;
    view.bump_render_tick();
    assert_eq!(
        view.render_generation,
        generation + 1,
        "a timer tick advances the generation by exactly one"
    );
    let fresh = view.lines(80, &theme);
    assert_eq!(
        view.content_height(80, &theme),
        wrap_all_owned(fresh, 80).len(),
        "the frame after a tick bump must re-materialise (the wall-clock inputs live in lines())"
    );
}

#[test]
fn the_cache_never_serves_stale_lines_across_a_turn() {
    let mut view = TranscriptView::new();
    let theme = UiTheme::dark();
    assert_fresh(&mut view, &theme, "fresh view");
    view.push_user("read main.rs please");
    assert_fresh(&mut view, &theme, "push_user");
    view.push_thinking_delta("let me ");
    assert_fresh(&mut view, &theme, "push_thinking_delta");
    view.push_thinking_delta("think");
    assert_fresh(&mut view, &theme, "more thinking");
    view.push_assistant_delta("partial **mark");
    assert_fresh(&mut view, &theme, "push_assistant_delta");
    view.push_assistant_delta("down** text");
    assert_fresh(&mut view, &theme, "more streaming");
    view.push_tool_start("read", serde_json::json!({ "file_path": "main.rs" }));
    assert_fresh(&mut view, &theme, "push_tool_start");
    view.push_tool_update(None, Some(serde_json::json!({ "content": [] })));
    assert_fresh(&mut view, &theme, "push_tool_update");
    view.set_edit_preview(None, Ok("@@ diff".to_string()));
    assert_fresh(&mut view, &theme, "set_edit_preview");
    view.push_tool_end("read", false, Some(serde_json::json!({ "content": [] })));
    assert_fresh(&mut view, &theme, "push_tool_end");
    view.commit_finished_leading_tools();
    assert_fresh(&mut view, &theme, "commit_finished_leading_tools");
    view.start_bash("ls -la", false, None, None);
    assert_fresh(&mut view, &theme, "start_bash");
    view.bash_append("total 4");
    assert_fresh(&mut view, &theme, "bash_append");
    view.bash_complete_simple(Some(0), false);
    assert_fresh(&mut view, &theme, "bash_complete_simple");
    view.commit_bash();
    assert_fresh(&mut view, &theme, "commit_bash");
    view.commit_assistant(None);
    assert_fresh(&mut view, &theme, "commit_assistant");
    view.commit_thinking(None);
    assert_fresh(&mut view, &theme, "commit_thinking");
    view.commit_tools();
    assert_fresh(&mut view, &theme, "commit_tools");
    view.discard_streaming();
    assert_fresh(&mut view, &theme, "discard_streaming");
    view.toggle_tool_expanded();
    assert_fresh(&mut view, &theme, "toggle_tool_expanded");
    view.set_output_pad(0);
    assert_fresh(&mut view, &theme, "set_output_pad");
    // Exempt paths change nothing `lines()` emits, so the invariant holds across them too.
    view.page_up(2);
    assert_fresh(&mut view, &theme, "page_up (exempt)");
    view.page_down(1);
    assert_fresh(&mut view, &theme, "page_down (exempt)");
    view.push_status("a status line");
    assert_fresh(&mut view, &theme, "push_status (exempt)");
    view.bump_render_tick();
    assert_fresh(&mut view, &theme, "bump_render_tick");
}

#[test]
fn render_paints_the_cached_lines_not_a_recompute() {
    let mut view = TranscriptView::new();
    let theme = UiTheme::dark();
    view.push_assistant_delta("real-streamed-text");
    let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();
    // Prime the cache at the paint width, then poison it: the paint must show the SENTINEL,
    // proving `Component::render` reads the cache instead of re-running `lines()`.
    let _ = view.content_height(40, &theme);
    view.render_cache.rows = std::sync::Arc::new(vec![Line::from("SENTINEL-LINE")]);
    terminal
        .draw(|frame| {
            let area = frame.area();
            view.render(frame, area, &theme);
        })
        .unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("SENTINEL-LINE"), "paint must come from the cache:\n{text}");
    assert!(
        !text.contains("real-streamed-text"),
        "a cache hit must not recompute the markdown:\n{text}"
    );
    // A content bump invalidates: the next paint re-materialises the real stream.
    view.push_assistant_delta("-tail");
    terminal
        .draw(|frame| {
            let area = frame.area();
            view.render(frame, area, &theme);
        })
        .unwrap();
    let text = buffer_text(&terminal);
    assert!(
        text.contains("real-streamed-text-tail"),
        "the post-bump paint must re-materialise:\n{text}"
    );
}

/// PERF-005 §5.5.19 — the go/no-go measurement for §3.3.
///
/// The spec gates the dedicated render thread on a number rather than on momentum: with §3.0a,
/// §3.0b and §3.0 landed, if a frame at the largest reachable transcript no longer outruns the
/// 80 ms spinner tick (`status_indicator.rs`), §3.3 is not justified by CPU and the remaining case
/// rests on tty blocking alone.
///
/// `#[ignore]` because it is a measurement, not an assertion — run it deliberately:
/// `cargo test -p cyrup-tui --lib go_no_go -- --ignored --nocapture --test-threads=1`
/// and read it in `--release`, where the numbers mean something.
#[test]
#[ignore = "measurement, not an assertion — see PERF-005 §5.5.19"]
fn go_no_go_frame_cost_at_the_largest_reachable_transcript() {
    use std::time::Instant;

    let theme = UiTheme::dark();
    let code: String = (0..2_000)
        .map(|i| format!("fn f{i}(x: u32) -> u32 {{ x + {i} }}\n"))
        .collect();
    let body: String = (0..2_000).map(|i| format!("line {i} of a read result\n")).collect();

    // A turn that holds BOTH terms the cliff was made of: 2,000 lines of streaming code in a
    // fence, and a settled 2,000-line `read` whose body is collapsed to ten rows.
    let mut view = TranscriptView::new();
    view.push_tool_start("read", serde_json::json!({ "file_path": "/tmp/big.rs" }));
    view.push_tool_end(
        "read",
        false,
        Some(serde_json::json!({ "content": body, "file_path": "/tmp/big.rs" })),
    );
    view.push_assistant_delta(&format!("Here is the code:\n\n```rust\n{code}"));

    // The MISS path is the streaming path: `push_assistant_delta` bumps the generation, so every
    // frame of a streaming turn re-materialises. Measure that, not the hit.
    let frames = 20usize;
    let mut each = Vec::with_capacity(frames);
    for i in 0..frames {
        view.push_assistant_delta(&format!("fn extra{i}() {{}}\n"));
        let t = Instant::now();
        let h = view.content_height(120, &theme);
        each.push(t.elapsed());
        std::hint::black_box(h);
    }
    let first = each.first().copied().unwrap_or_default();
    let steady: Vec<_> = each.iter().skip(1).copied().collect();
    let worst_steady = steady.iter().copied().max().unwrap_or_default();
    let mean_steady = steady.iter().sum::<std::time::Duration>()
        / u32::try_from(steady.len().max(1)).unwrap_or(1);
    let tick = std::time::Duration::from_millis(80);
    println!(
        "PERF-005 §5.5.19 go/no-go — 2,000-line fence + collapsed 2,000-line read, width 120\n  \
         first frame (cold caches): {first:?}\n  \
         steady-state mean        : {mean_steady:?}\n  \
         steady-state worst       : {worst_steady:?}\n  \
         spinner tick (the cliff) : {tick:?}\n  \
         all frames: {each:?}\n  \
         verdict: {}",
        if worst_steady < tick {
            "steady state is UNDER the tick — the cliff is not reachable while streaming"
        } else {
            "steady state is OVER the tick — the cliff is still reachable"
        }
    );
}
