//! The record rules of [`super::ConsolePump`], on their own.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event, KeyCode};

use super::*;

fn key(key_down: bool, unit: u16) -> ConsoleRecord {
    ConsoleRecord::Key {
        key_down,
        unit,
        repeat: 1,
    }
}

fn run(records: &[ConsoleRecord]) -> Vec<Event> {
    let mut pump = ConsolePump::new(Duration::from_millis(10));
    let mut out = Vec::new();
    pump.push(records, Instant::now(), &|| Ok((120, 40)), &mut out);
    out
}

fn chars(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Key(k) => match k.code {
                KeyCode::Char(c) => Some(c),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Only key-down records carry input: the console sends a key-up record with the same character
/// for every key, and reading it too would double every keystroke.
#[test]
fn key_up_records_are_not_input() {
    assert_eq!(
        chars(&run(&[
            key(true, u16::from(b'a')),
            key(false, u16::from(b'a'))
        ])),
        "a"
    );
}

/// A record with no character (a bare modifier, a key the console could not translate) is not
/// input either.
#[test]
fn a_key_without_a_character_is_not_input() {
    assert!(run(&[key(true, 0), key(false, 0)]).is_empty());
}

/// `wRepeatCount` repeats the character.
#[test]
fn the_repeat_count_repeats_the_character() {
    let held = ConsoleRecord::Key {
        key_down: true,
        unit: u16::from(b'z'),
        repeat: 3,
    };
    assert_eq!(chars(&run(&[held])), "zzz");
}

/// A character outside the BMP arrives as two records, its surrogate halves; they make one
/// character. An unpaired half is U+FFFD, as a lossy UTF-16 decode gives.
#[test]
fn surrogate_halves_make_one_character() {
    let crab: Vec<u16> = "🦀".encode_utf16().collect();
    assert_eq!(chars(&run(&[key(true, crab[0]), key(true, crab[1])])), "🦀");
    assert_eq!(
        chars(&run(&[key(true, crab[0]), key(true, u16::from(b'a'))])),
        "\u{FFFD}a"
    );
}

/// The VT bytes of a key decode as that key — the same rules as the unix reader — and a resize
/// falls where it came between them.
#[test]
fn escape_sequences_decode_and_a_resize_keeps_its_place() {
    let mut records: Vec<ConsoleRecord> =
        b"\x1b[A".iter().map(|&b| key(true, u16::from(b))).collect();
    records.push(ConsoleRecord::Resize);
    records.push(key(true, u16::from(b'q')));
    let events = run(&records);
    assert!(
        matches!(events[0], Event::Key(k) if k.code == KeyCode::Up),
        "{events:?}"
    );
    assert_eq!(events[1], Event::Resize(120, 40));
    assert!(matches!(events[2], Event::Key(k) if k.code == KeyCode::Char('q')));
    assert_eq!(events.len(), 3);
}
