//! Mode `2031`: the lifecycle value, the report's route out of the byte reader, and the writes the
//! terminal's start, stop and alternate-screen transitions owe it.
//!
//! The pure lifecycle is `tui.ts:917-929` (`start`), `:950-957` (`setTerminalColorScheme
//! Notifications`) and `:970-980` (`stop`) @v1.0.0; the report is `terminal-colors.ts:42`, `:85-91`
//! and `tui.ts:1048`, `:1168-1178`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

use std::sync::{Arc, Mutex};
use std::time::Instant;

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyCode};

use super::*;
use crate::AltScreen;
use crate::altscreen::captured_text;
use crate::input::reader::ByteDecoder;
use crate::theme::UiTheme;

fn heard() -> (Arc<Mutex<Vec<TerminalTheme>>>, Listener) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);
    (
        log,
        Arc::new(move |scheme| sink.lock().unwrap().push(scheme)),
    )
}

fn feed(decoder: &mut ByteDecoder, bytes: &[u8]) -> Vec<Event> {
    let mut out = Vec::new();
    decoder.feed(bytes, Instant::now(), &mut out);
    out
}

fn keys(events: &[Event]) -> Vec<KeyCode> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Key(k) => Some(k.code),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The lifecycle value
// ---------------------------------------------------------------------------------------------

/// `start()` writes the enable only when the notifications are wanted (`tui.ts:925-927`).
#[test]
fn start_writes_the_mode_only_when_it_is_wanted() {
    let mut n = Notifications::new();
    assert_eq!(n.start(), None, "nothing asked for it yet");
    assert_eq!(n.set_wanted(true), Some(ENABLE), "running and now wanted");
    let mut n = Notifications::new();
    n.set_wanted(true);
    assert_eq!(
        n.start(),
        Some(ENABLE),
        "wanted before the terminal started"
    );
}

/// `setTerminalColorSchemeNotifications` while stopped records the wish and writes nothing
/// (`tui.ts:954`); the next `start()` honours it.
#[test]
fn a_wish_made_while_stopped_waits_for_the_next_start() {
    let mut n = Notifications::new();
    n.start();
    n.stop();
    assert_eq!(n.set_wanted(true), None, "stopped: nothing is written");
    assert_eq!(n.start(), Some(ENABLE), "the resume turns it on");
}

/// `stop()` takes the mode down (`tui.ts:973-975`) but keeps the wish, so `ui.start()` after a
/// Ctrl+Z or `$EDITOR` brings it back.
#[test]
fn stop_and_start_bracket_a_suspend() {
    let mut n = Notifications::new();
    n.start();
    n.set_wanted(true);
    assert_eq!(n.stop(), Some(DISABLE));
    assert_eq!(n.stop(), None, "a second stop writes nothing");
    assert!(n.wanted(), "the wish survives the stop");
    assert_eq!(n.start(), Some(ENABLE));
    assert!(n.on_wire());
}

/// Switching the theme to a fixed one turns the mode off, and back to `system` on again.
#[test]
fn auto_sync_toggles_the_mode_while_running() {
    let mut n = Notifications::new();
    n.start();
    assert_eq!(n.set_wanted(true), Some(ENABLE));
    assert_eq!(n.set_wanted(true), None, "already on");
    assert_eq!(n.set_wanted(false), Some(DISABLE));
    assert_eq!(n.set_wanted(false), None, "already off");
}

/// A screen swap is `stop()` of the old TUI then `start()` of the new one: off, then on.
#[test]
fn a_screen_swap_cycles_the_mode_off_then_on() {
    let mut n = Notifications::new();
    n.start();
    n.set_wanted(true);
    assert_eq!(n.stop(), Some(DISABLE));
    assert_eq!(n.start(), Some(ENABLE));
}

// ---------------------------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------------------------

#[test]
fn only_a_framed_997_report_is_offered() {
    assert_eq!(offer_report(b"\x1b[?997;1n"), Some(TerminalTheme::Dark));
    assert_eq!(offer_report(b"\x1b[?997;2n"), Some(TerminalTheme::Light));
    assert_eq!(offer_report(b"\x1b[?997;3n"), None);
    assert_eq!(
        offer_report(b"\x1b[?62;22c"),
        None,
        "a DA1 reply is not one"
    );
    assert_eq!(offer_report(b"\x1b[A"), None);
}

/// pi's input handler consumes a report ahead of every input listener (`tui.ts:1048`): the reader
/// delivers it and it never becomes a key. FAILS without the `decode_frames` hook — the report was
/// swallowed as an anonymous reply and nobody heard it.
#[test]
fn the_reader_delivers_a_report_and_emits_no_keys() {
    let _guard = lock_for_test();
    reset_for_test();
    let (log, listener) = heard();
    set_listener(Some(listener));
    let mut decoder = ByteDecoder::new(std::time::Duration::from_millis(10));

    let events = feed(&mut decoder, b"\x1b[?997;2n");
    assert!(events.is_empty(), "a report is not typing: {events:?}");
    assert_eq!(*log.lock().unwrap(), vec![TerminalTheme::Light]);

    // Typing around a report is untouched, and the report between is still heard.
    let events = feed(&mut decoder, b"a\x1b[?997;1nb");
    assert_eq!(keys(&events), vec![KeyCode::Char('a'), KeyCode::Char('b')]);
    assert_eq!(
        *log.lock().unwrap(),
        vec![TerminalTheme::Light, TerminalTheme::Dark]
    );
    reset_for_test();
}

/// A back-to-back burst is one report whose scheme is the last frame's
/// (`terminal-colors.test.ts:118-119`), so the listener hears it once.
#[test]
fn a_burst_settles_on_its_last_scheme() {
    let _guard = lock_for_test();
    reset_for_test();
    let (log, listener) = heard();
    set_listener(Some(listener));
    let mut decoder = ByteDecoder::new(std::time::Duration::from_millis(10));

    let events = feed(&mut decoder, b"\x1b[?997;2n\x1b[?997;1n\x1b[?997;1n");
    assert!(events.is_empty());
    assert_eq!(*log.lock().unwrap(), vec![TerminalTheme::Dark]);
    reset_for_test();
}

/// With nobody listening a report is still consumed (pi consumes it whether or not a listener
/// exists), and a report split across reads is reassembled by the framer, not leaked.
#[test]
fn an_unheard_or_split_report_never_reaches_the_editor() {
    let _guard = lock_for_test();
    reset_for_test();
    let mut decoder = ByteDecoder::new(std::time::Duration::from_millis(10));
    assert!(feed(&mut decoder, b"\x1b[?997;2n").is_empty());

    let (log, listener) = heard();
    set_listener(Some(listener));
    assert!(feed(&mut decoder, b"\x1b[?99").is_empty());
    assert!(feed(&mut decoder, b"7;1n").is_empty());
    assert_eq!(*log.lock().unwrap(), vec![TerminalTheme::Dark]);
    reset_for_test();
}

// ---------------------------------------------------------------------------------------------
// The writes
// ---------------------------------------------------------------------------------------------

/// `terminal_started` / `terminal_stopped` write through the sink they are given.
#[cfg(unix)]
#[test]
fn the_terminal_start_and_stop_write_the_mode() {
    let _guard = lock_for_test();
    reset_for_test();
    with_state(|s| {
        s.start();
        s.set_wanted(true);
    });
    let mut out = Vec::new();
    terminal_stopped(&mut out);
    assert_eq!(out, DISABLE.as_bytes(), "restore/panic path takes it down");
    out.clear();
    terminal_stopped(&mut out);
    assert!(out.is_empty(), "idempotent");
    terminal_started(&mut out);
    assert_eq!(out, ENABLE.as_bytes(), "resume puts it back");
    reset_for_test();
}

/// Entering the alternate screen is pi's `stop()` of the old TUI then `start()` of the new one
/// (`tui.ts:972-975`, `:923-927`), and leaving it takes the mode down before the screen is left.
/// FAILS without the `TerminalSetup` hooks: no `2031` sequence is written around the screen.
#[cfg(unix)]
#[test]
fn the_alternate_screen_cycles_the_mode_around_enter_and_leave() {
    let _guard = lock_for_test();
    reset_for_test();
    with_state(|s| {
        s.start();
        s.set_wanted(true);
    });

    let (mut alt, captured) =
        AltScreen::for_test(TestBackend::new(20, 6), UiTheme::dark()).unwrap();
    let entered = captured_text(&captured);
    let off = entered.find(DISABLE).expect("the old TUI stops first");
    let screen = entered.find("\x1b[?1049h").expect("alt screen entered");
    let on = entered.find(ENABLE).expect("the new TUI starts");
    assert!(off < screen && screen < on, "off, screen, on: {entered:?}");

    alt.stop(true);
    let left = captured_text(&captured);
    let tail = &left[entered.len()..];
    let off = tail.find(DISABLE).expect("stop takes the mode down");
    let leave = tail.find("\x1b[?1049l").expect("alt screen left");
    assert!(
        off < leave,
        "the mode goes down before the screen: {tail:?}"
    );
    reset_for_test();
}

/// Not wanted means silent: the common path (a fixed theme) writes no `2031` at all.
#[test]
fn an_alternate_screen_without_the_wish_writes_nothing() {
    let _guard = lock_for_test();
    reset_for_test();
    with_state(|s| {
        s.start();
    });
    let (mut alt, captured) =
        AltScreen::for_test(TestBackend::new(20, 6), UiTheme::dark()).unwrap();
    alt.stop(true);
    assert!(!captured_text(&captured).contains("2031"));
    reset_for_test();
}
