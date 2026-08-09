//! Headless render tests against ratatui's `TestBackend` (R-10-010 / R-ARCH-TUI-010).
//!
//! Renders the app with a synthetic transcript + status to a virtual buffer and asserts the
//! expected text / themed cells appear: user text, assistant text, a streaming delta, the status
//! line, event-driven status updates, and theme colors reaching the rendered cells.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use cyrup_session_svc::AgentSessionEvent;
use cyrup_tui::{App, UiTheme};
use ratatui::backend::TestBackend;
use ratatui::style::Color;

/// Flatten the test backend's cell grid into rows of text for `contains` assertions.
fn buf_text(app: &App<TestBackend>) -> String {
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

/// Flatten only the **live region** — the bottom `viewport_height` rows the app repaints each frame
/// (ADR-0001 #1). Committed history scrolls *above* this band into native scrollback, so this is what
/// "is in the viewport" means once the inline region is content-sized (audit #1).
fn live_region_text(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let h = app.viewport_height().min(area.height);
    let start = area.height - h;
    let mut out = String::new();
    for y in start..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// True if any non-blank cell carries the given foreground color.
fn has_fg(app: &App<TestBackend>, color: Color) -> bool {
    let buf = app.terminal().backend().buffer();
    buf.content().iter().any(|cell| cell.fg == color && cell.symbol() != " ")
}

#[test]
fn committed_entries_move_to_scrollback_and_only_active_turn_renders() {
    // R-ARCH-TUI-003: finalized entries flow to native scrollback (insert_before); the inline
    // viewport renders only the active streaming turn + editor + status.
    let mut app = App::new(TestBackend::new(50, 16), UiTheme::dark()).unwrap();
    app.transcript_mut().push_user("hello world");
    app.transcript_mut().commit_assistant(Some("hi there".to_string()));
    // A second, still-streaming assistant turn arrives as deltas (the active region).
    app.transcript_mut().push_assistant_delta("strea");
    app.transcript_mut().push_assistant_delta("ming…");
    app.draw().unwrap();

    // (a) Committed user + assistant text reached the scrollback accumulator (what insert_before got).
    let sb = app.scrollback_text();
    // X1: no `you: ` / `assistant: ` labels — `user-message.ts:38-58` and
    // `assistant-message.ts:104-114` render the body alone.
    assert!(sb.contains("hello world"), "scrollback missing user line:\n{sb}");
    assert!(sb.contains("hi there"), "scrollback missing assistant line:\n{sb}");
    assert!(!sb.contains("you:") && !sb.contains("assistant:"), "invented role label:\n{sb}");

    // (b) The live region no longer contains the committed text (it scrolled into native scrollback
    //     above the inline band, audit #1), but DOES show the active streaming turn + editor + footer.
    let text = live_region_text(&app);
    assert!(!text.contains("hello world"), "committed user leaked into live region:\n{text}");
    assert!(!text.contains("hi there"), "committed assistant leaked into live region:\n{text}");
    assert!(text.contains("streaming…"), "active streaming turn missing from live region:\n{text}");
    // The footer right cluster is the model (here unset → Pi's "no-model", footer.ts:170), never an
    // invented streaming/idle word; the active turn renders inline with NO box/title (footer.ts:84-93).
    assert!(text.contains("no-model"), "footer model cluster missing from viewport:\n{text}");
    assert!(!text.contains("conversation"), "streaming partial must not be boxed:\n{text}");
}

#[test]
fn footer_right_cluster_is_model_not_streaming_word() {
    // gap 22: Pi's footer shows the model on the right, never a `streaming`/`idle` state word — that
    // is the separate status band (spec/tui/01 §6). `streaming` here drives only the (unbuilt) band.
    let mut app = App::new(TestBackend::new(80, 12), UiTheme::dark()).unwrap();
    app.status_mut().set_model("anthropic/claude");
    app.status_mut().set_streaming(true);
    app.draw().unwrap();

    let text = buf_text(&app);
    assert!(text.contains("anthropic/claude"), "missing model:\n{text}");
    assert!(!text.contains("streaming"), "footer invented a streaming word:\n{text}");
    assert!(!text.contains("idle"), "footer invented an idle word:\n{text}");
}

#[test]
fn footer_renders_usage_cluster_and_location() {
    // Pi footer (footer.ts:116-228): line 1 = cwd (branch) • name; line 2 = usage cluster + model.
    use cyrup_core::{Cost, Usage};
    let mut app = App::new(TestBackend::new(100, 12), UiTheme::dark()).unwrap();
    app.status_mut().set_model("anthropic/claude-opus-4-8");
    app.status_mut().set_cwd("~/src/cyrup");
    app.status_mut().set_branch(Some("david/cyrup".to_string()));
    app.status_mut().set_session_name(Some("my-session".to_string()));
    app.status_mut().add_usage(&Usage {
        input: 12_300,
        output: 4_100,
        cache_read: 88_000,
        cache_write: 2_100,
        total_tokens: 106_500,
        cost: Cost { total: 0.214, ..Cost::default() },
        ..Usage::default()
    });
    app.status_mut().set_context(0.412, 200_000, true);
    app.draw().unwrap();

    let text = buf_text(&app);
    // formatTokens thresholds (footer.ts:22-29): 12300 ≥10k → "12k"; 4100 <10k → "4.1k"; 88k; 2.1k.
    assert!(text.contains("↑12k"), "input tokens cluster missing:\n{text}");
    assert!(text.contains("↓4.1k"), "output tokens cluster missing:\n{text}");
    assert!(text.contains("R88k"), "cache-read cluster missing:\n{text}");
    assert!(text.contains("$0.214"), "cost segment missing:\n{text}");
    assert!(text.contains("41.2%/200k (auto)"), "context segment missing:\n{text}");
    // Location line.
    assert!(text.contains("~/src/cyrup (david/cyrup) • my-session"), "location line missing:\n{text}");
    assert!(text.contains("anthropic/claude-opus-4-8"), "model missing:\n{text}");
}

#[test]
fn streaming_text_deltas_render_in_viewport_via_events() {
    // gap 1: StreamEvent::TextDelta drives the live token-by-token render.
    use cyrup_agent::AgentMessage;
    use cyrup_core::{AssistantMessage, Content, ProviderId, StopReason, Usage};
    use cyrup_provider::StreamEvent;
    use cyrup_session_svc::AgentSessionEvent;

    // A minimal partial assistant message to ride along with each delta event.
    let partial =
        AssistantMessage::errored(ProviderId::from("anthropic"), "claude", None, StopReason::Stop, "");

    let mut app = App::new(TestBackend::new(60, 14), UiTheme::dark()).unwrap();
    app.ingest_event(&AgentSessionEvent::AgentStart);
    for delta in ["The ", "live ", "stream"] {
        app.ingest_event(&AgentSessionEvent::MessageUpdate {
            message: AgentMessage::user_text(""),
            assistant_message_event: Box::new(StreamEvent::TextDelta {
                content_index: 0,
                delta: delta.to_string(),
                partial: partial.clone(),
            }),
        });
    }
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("The live stream"), "streamed deltas not in viewport:\n{text}");
    // Still streaming → not yet committed to scrollback.
    assert!(
        !app.scrollback_text().contains("The live stream"),
        "in-flight stream leaked to scrollback"
    );

    // A terminal `Done` commits the authoritative message and clears the active region.
    let mut final_msg = partial.clone();
    final_msg.content = vec![Content::text("The live stream is done")];
    final_msg.usage = Usage { total_tokens: 42, ..Usage::default() };
    app.ingest_event(&AgentSessionEvent::MessageUpdate {
        message: AgentMessage::user_text(""),
        assistant_message_event: Box::new(StreamEvent::terminal(final_msg)),
    });
    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: vec![], will_retry: false });
    app.draw().unwrap();
    assert!(
        app.scrollback_text().contains("The live stream is done"),
        "terminal message not committed to scrollback:\n{}",
        app.scrollback_text()
    );
    // X1: dropping `assistant: ` from the needle above only proves the BODY is there — the negative
    // is what proves the label is gone. `assistant-message.ts:104-114` is the `Markdown` child alone.
    assert!(
        !app.scrollback_text().contains("assistant:"),
        "invented assistant label in scrollback:\n{}",
        app.scrollback_text()
    );
}

#[test]
fn ingest_events_drive_status_and_transcript() {
    let mut app = App::new(TestBackend::new(60, 14), UiTheme::dark()).unwrap();
    app.ingest_event(&AgentSessionEvent::AgentStart);
    app.ingest_event(&AgentSessionEvent::ModelChanged {
        provider: "openai".to_string(),
        model: "gpt".to_string(),
    });
    app.ingest_event(&AgentSessionEvent::QueueUpdate {
        steering: vec!["one".to_string()],
        follow_up: vec!["two".to_string(), "three".to_string()],
    });
    app.draw().unwrap();

    let text = buf_text(&app);
    assert!(text.contains("openai/gpt"), "model not reflected:\n{text}");
    // The queue depth is TRACKED but must NOT reach the footer: `statsParts` (v0.84.1
    // `footer.ts:129-164`) is exactly `↑ ↓ R W CH% $cost`, the context segment and `xp` — there is no
    // queue segment upstream under any name. The extra segment pushed the right-aligned model name
    // over at narrow widths. pi surfaces queued messages in the transcript instead.
    assert_eq!(app.state().status.queued, 3, "queue depth not tracked");
    assert!(!text.contains("queued"), "pi's footer has no queue segment:\n{text}");
    // The model-change notification is a committed entry: it lives in scrollback, not the live region.
    let sb = app.scrollback_text();
    assert!(sb.contains("model → openai/gpt"), "model change not logged to scrollback:\n{sb}");
    assert!(
        !live_region_text(&app).contains("model → openai/gpt"),
        "status entry leaked into live region:\n{}",
        live_region_text(&app)
    );

    // AgentEnd clears the streaming flag; the footer keeps showing the model (no idle word).
    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: vec![], will_retry: false });
    app.draw().unwrap();
    let after = buf_text(&app);
    assert!(after.contains("openai/gpt"), "footer model lost after agent_end:\n{after}");
    assert!(!after.contains("idle"), "footer invented an idle word after agent_end:\n{after}");
}

#[test]
fn theme_colors_reach_rendered_cells() {
    // The accent-styled live-region surface is the status band's spinner glyph
    // (`status_indicator.rs:216-218`, Pi `status-indicator.ts:55-64` → `loader.ts`), which is
    // `accent` for every kind except `Retry`. Driving the indicator (rather than a streaming turn)
    // is deliberate: the comment this test used to carry claimed the accent came from "the active
    // streaming label", but that label was DELETED as X1 (`user-message.ts:38-58` renders the body
    // only) and the assertion has been surviving since on the editor's `› ` prompt glyph — a cyrup
    // invention pi's `editor.ts:482-601` never emits, removed here as E1. An assertion whose only
    // live subject is the thing under removal proves nothing about themes.
    let accent = Color::Rgb(0x8a, 0xbe, 0xb7);
    let mut app = App::new(TestBackend::new(40, 12), UiTheme::dark()).unwrap();
    app.state_mut().indicator.working();
    app.transcript_mut().push_assistant_delta("colored");
    app.draw().unwrap();
    assert!(has_fg(&app, accent), "dark accent color did not reach any cell");

    // A different theme yields a different accent on the cells (Pi light `accent` = teal #5a8080).
    let light_accent = Color::Rgb(0x5a, 0x80, 0x80);
    let mut light = App::new(TestBackend::new(40, 12), UiTheme::light()).unwrap();
    light.state_mut().indicator.working();
    light.transcript_mut().push_assistant_delta("colored");
    light.draw().unwrap();
    assert!(has_fg(&light, light_accent), "light accent color did not reach any cell");
}

#[test]
fn finalized_turn_via_events_flows_to_scrollback_and_clears_viewport() {
    // Drive a full turn through the neutral push API + AgentSessionEvents, exactly as the run loop
    // does, and prove the committed turn lands in scrollback while the viewport empties.
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    app.ingest_event(&AgentSessionEvent::AgentStart);
    app.transcript_mut().push_user("question?");
    app.transcript_mut().push_assistant_delta("ans");
    app.transcript_mut().push_assistant_delta("wer");

    // Mid-turn: the user line has committed to scrollback; the assistant is still active (live region).
    app.draw().unwrap();
    let mid = live_region_text(&app);
    assert!(mid.contains("answer"), "active streaming turn missing from live region mid-turn:\n{mid}");
    assert!(!mid.contains("question?"), "committed user leaked into live region:\n{mid}");
    assert!(app.scrollback_text().contains("question?"), "user not flushed to scrollback");
    // X1: the needle lost its `you: ` prefix, so assert the prefix is actually absent rather than
    // just unasserted (`user-message.ts:38-58` has no role label).
    assert!(
        !app.scrollback_text().contains("you:"),
        "invented `you: ` label in scrollback:\n{}",
        app.scrollback_text()
    );

    // AgentEnd finalizes the streaming assistant turn.
    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: vec![], will_retry: false });
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(sb.contains("question?"), "user missing from scrollback:\n{sb}");
    assert!(sb.contains("answer"), "finalized assistant missing from scrollback:\n{sb}");
    assert!(!sb.contains("you:"), "invented `you: ` label in scrollback:\n{sb}");
    assert!(!sb.contains("assistant:"), "invented assistant label in scrollback:\n{sb}");

    let view = live_region_text(&app);
    assert!(!view.contains("answer"), "finalized assistant still in live region:\n{view}");
    // The footer persists (model cluster present); no invented idle word (gap 22).
    assert!(view.contains("no-model"), "footer missing from viewport after finalize:\n{view}");
    assert!(!view.contains("idle"), "footer invented an idle word after finalize:\n{view}");
}

#[test]
fn committed_entries_flush_exactly_once() {
    // A flushed entry must not reappear in scrollback on subsequent draws (insert_before once).
    let mut app = App::new(TestBackend::new(50, 12), UiTheme::dark()).unwrap();
    app.transcript_mut().push_user("only once");
    app.draw().unwrap();
    app.draw().unwrap();
    app.draw().unwrap();

    let occurrences = app.scrollback_text().matches("only once").count();
    assert_eq!(occurrences, 1, "committed entry flushed more than once");
    // X1: the needle used to be `you: only once`; keep the label assertion alive as a negative.
    assert!(
        !app.scrollback_text().contains("you:"),
        "invented `you: ` label in scrollback:\n{}",
        app.scrollback_text()
    );
    assert!(app.state().transcript.pending().is_empty(), "pending buffer not drained");
}

/// Render only the footer region (last 2 rows) of a width×3 backend and return its text + the buffer.
fn footer_app(width: u16) -> App<TestBackend> {
    App::new(TestBackend::new(width, 6), UiTheme::dark()).unwrap()
}

/// The text of the last (bottom) row — the line-2 model/usage cluster.
fn last_row(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let y = area.height.saturating_sub(1);
    let mut s = String::new();
    for x in 0..area.width {
        if let Some(cell) = buf.cell((x, y)) {
            s.push_str(cell.symbol());
        }
    }
    s
}

#[test]
fn footer_right_cluster_shows_thinking_level_when_model_reasons() {
    // footer.ts:184-189: reasoning model → `{model} • {level}`, or `{model} • thinking off` at off.
    let mut app = footer_app(80);
    app.status_mut().set_model("claude-opus-4-8");
    app.status_mut().set_reasoning(true);
    app.status_mut().set_thinking_level("high");
    app.draw().unwrap();
    assert!(buf_text(&app).contains("claude-opus-4-8 • high"), "thinking level missing:\n{}", buf_text(&app));

    app.status_mut().set_thinking_level("off");
    app.draw().unwrap();
    assert!(buf_text(&app).contains("claude-opus-4-8 • thinking off"), "thinking-off missing:\n{}", buf_text(&app));

    // A non-reasoning model shows the bare id, no thinking suffix (footer.ts:184).
    app.status_mut().set_reasoning(false);
    app.draw().unwrap();
    let t = buf_text(&app);
    assert!(t.contains("claude-opus-4-8"), "model missing:\n{t}");
    assert!(!t.contains("thinking"), "thinking shown for non-reasoning model:\n{t}");
}

#[test]
fn footer_right_aligns_model_cluster_with_padding() {
    // footer.ts:204-208: the model cluster is right-aligned by padding to the full width.
    let mut app = footer_app(40);
    app.status_mut().set_model("gpt-4o");
    app.draw().unwrap();
    let row = last_row(&app);
    assert!(row.ends_with("gpt-4o"), "model not flush-right:\n[{row}]");
    // With no left cluster the right side fills from the left edge with padding before it (min 2 gap).
    assert_eq!(visible_len(&row), 40, "row not padded to full width:\n[{row}]");
}

#[test]
fn footer_cache_hit_uses_latest_turn_full_prompt_denominator() {
    // footer.ts:102-105: latest cache-hit = cacheRead / (input + cacheRead + cacheWrite).
    use cyrup_core::Usage;
    let mut app = footer_app(120);
    app.status_mut().add_usage(&Usage {
        input: 12_300,
        cache_read: 88_000,
        cache_write: 2_100,
        ..Usage::default()
    });
    app.draw().unwrap();
    // 88000 / (12300+88000+2100) = 85.9375 → "85.9%" (NOT the old 88000/(88000+12300)=87.7%).
    let t = buf_text(&app);
    assert!(t.contains("CH85.9%"), "cache-hit denominator wrong (expected latest-turn full prompt):\n{t}");
}

#[test]
fn footer_usage_is_cumulative_across_turns() {
    // footer.ts:86-100: token totals sum across every assistant turn.
    use cyrup_core::{Cost, Usage};
    let mut app = footer_app(120);
    app.status_mut().add_usage(&Usage {
        input: 1_000,
        output: 500,
        cost: Cost { total: 0.10, ..Cost::default() },
        ..Usage::default()
    });
    app.status_mut().add_usage(&Usage {
        input: 1_000,
        output: 1_500,
        cost: Cost { total: 0.05, ..Cost::default() },
        ..Usage::default()
    });
    app.draw().unwrap();
    let t = buf_text(&app);
    assert!(t.contains("↑2.0k"), "input not summed across turns:\n{t}");
    assert!(t.contains("↓2.0k"), "output not summed across turns:\n{t}");
    assert!(t.contains("$0.150"), "cost not summed across turns:\n{t}");
}

#[test]
fn footer_shows_subscription_provider_and_experimental_markers() {
    // footer.ts:142-145 (sub), :191-199 (provider) prefix, :163-165 (xp) marker.
    use cyrup_core::{Cost, Usage};
    let mut app = footer_app(120);
    app.status_mut().set_model("claude-opus-4-8");
    app.status_mut().set_provider(Some("anthropic".to_string()));
    app.status_mut().set_provider_count(3);
    app.status_mut().set_using_subscription(true);
    app.status_mut().set_experimental(true);
    app.status_mut().add_usage(&Usage { cost: Cost { total: 0.0, ..Cost::default() }, ..Usage::default() });
    app.draw().unwrap();
    let t = buf_text(&app);
    assert!(t.contains("$0.000 (sub)"), "subscription marker missing:\n{t}");
    assert!(t.contains("xp"), "experimental marker missing:\n{t}");
    assert!(t.contains("(anthropic) claude-opus-4-8"), "provider prefix missing:\n{t}");
}

#[test]
fn footer_truncates_overlong_left_cluster_with_ellipsis() {
    // footer.ts:175-178 / spec/tui/01 §8: an overlong left cluster is right-truncated with `...`.
    use cyrup_core::Usage;
    let mut app = footer_app(12);
    app.status_mut().add_usage(&Usage {
        input: 1_234_567,
        output: 2_345_678,
        cache_read: 3_456_789,
        cache_write: 4_567_890,
        ..Usage::default()
    });
    app.status_mut().set_model("some-really-long-model-id");
    app.draw().unwrap();
    let row = last_row(&app);
    assert!(row.contains("..."), "overlong left cluster not truncated with ellipsis:\n[{row}]");
    assert!(visible_len(&row) <= 12, "truncated row exceeds width:\n[{row}]");
}

/// Visible length of a rendered row (chars; the test footer text is all single-width).
fn visible_len(s: &str) -> usize {
    s.chars().count()
}

#[test]
fn builtin_themes_resolve_known_colors() {
    let dark = UiTheme::builtin("dark");
    assert_eq!(dark.name, "dark");
    // Pi dark `accent` token resolves through `vars.accent` to #8abeb7; `text` to #d4d4d4.
    assert_eq!(dark.accent, Some(Color::Rgb(0x8a, 0xbe, 0xb7)));
    assert_eq!(dark.foreground, Some(Color::Rgb(0xd4, 0xd4, 0xd4)));
    let light = UiTheme::builtin("light");
    assert_eq!(light.name, "light");
    // Pi light `accent` token resolves through `vars.teal` to #5a8080.
    assert_eq!(light.accent, Some(Color::Rgb(0x5a, 0x80, 0x80)));
    // Unknown names fall back to the dark palette (never panics).
    let fallback = UiTheme::builtin("does-not-exist");
    assert_eq!(fallback.accent, dark.accent);
}
