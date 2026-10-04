//! The wheel **accelerator** — cyrup's port of pi's `WheelScrollAccelerator`
//! (`packages/tui/src/wheel-scroll.ts` @v1.0.0, `f1927c2d5`), which turns one wheel event into the
//! number of document rows it moves. TUI-136.
//!
//! # What it decides
//! With a fixed `fullscreenWheelScrollLines` every event moves that many rows, whatever the timing.
//! With `"auto"` the answer depends on whether the terminal already accelerated the wheel itself:
//!
//! * **It did** ([`terminal_accelerates_wheel`]) — local macOS terminals receive deltas the OS has
//!   accelerated and emit one event per line, so every event is one row.
//! * **It did not** — the count follows event velocity. Events closer together than
//!   [`BURST_GAP_MS`] belong to one physical notch (Ghostty emits them ~4 ms apart) or come from a
//!   high-resolution source and move one row each; a pause longer than [`GESTURE_GAP_MS`] or a
//!   change of direction ends the gesture; within a gesture the count is
//!   `min(MAX_AUTO_LINES, max(1, REFERENCE_GAP_MS / averageGap))` plus the fraction carried from
//!   the previous event, floored. Notches 100 ms apart move 1 row, 50 ms apart 2, 20 ms apart 5.
//!
//! # What it does not decide
//! The Alt multiplier is applied by the caller AFTER this (`tui-alt-screen.ts:698-703`), so Alt
//! always means "five times whatever the accelerator said" — see [`super::wheel`].
//!
//! # Time
//! pi reads `performance.now()` inside `handleInput`; this takes an [`Instant`] argument so the
//! arithmetic is driven by whoever owns the clock, and a test supplies synthetic gaps without
//! sleeping.

use std::time::Instant;

use cyrup_config::settings::WheelScrollLines;

/// Several events closer than this belong to one physical notch or come from a high-resolution
/// source: they move one row each and do not accelerate (`BURST_GAP_MS`).
const BURST_GAP_MS: f64 = 5.0;
/// A pause longer than this ends a scroll gesture (`GESTURE_GAP_MS`).
const GESTURE_GAP_MS: f64 = 200.0;
/// The average event gap that maps to one row per event; faster events scale up proportionally
/// (`REFERENCE_GAP_MS`).
const REFERENCE_GAP_MS: f64 = 100.0;
/// The most rows an accelerated event can move (`MAX_AUTO_LINES`).
const MAX_AUTO_LINES: f64 = 6.0;

/// Which way a wheel notch points — pi's `direction: -1 | 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Direction {
    /// Towards the start of the document (`-1`).
    Up,
    /// Towards the end of the document (`1`).
    Down,
}

/// Whether the terminal pre-accelerates wheel input — pi's `terminalAcceleratesWheel()`
/// (`wheel-scroll.ts:16-25`): macOS, and none of `SSH_CONNECTION`, `SSH_CLIENT`, `SSH_TTY` set.
///
/// `process.platform === "darwin"` is `cfg!(target_os = "macos")` here: a compile-time fact of the
/// binary, which is what `process.platform` is for a Node process too. The three variables count
/// as set when present at all, even empty, as `env.X === undefined` does.
pub(super) fn terminal_accelerates_wheel() -> bool {
    accelerates_wheel(cfg!(target_os = "macos"), |name| {
        std::env::var_os(name).is_some()
    })
}

/// The body of [`terminal_accelerates_wheel`] with its two inputs explicit, so each of the four
/// ways it can answer is reachable from a test without mutating the process environment (which
/// this crate's `#![forbid(unsafe_code)]` rules out).
fn accelerates_wheel(is_macos: bool, is_set: impl Fn(&str) -> bool) -> bool {
    is_macos
        && !["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
            .into_iter()
            .any(is_set)
}

/// Milliseconds from `earlier` to `later`, via whole nanoseconds so a gap that is a whole number of
/// milliseconds converts exactly (`as_secs_f64() * 1000.0` would turn 20 ms into
/// `20.000000000000004` and floor `100 / 20` to 4).
fn elapsed_ms(earlier: Instant, later: Instant) -> f64 {
    // A gap this module acts on is at most `GESTURE_GAP_MS`; beyond `u64` nanoseconds (~584 years)
    // the saturated value is still "a long pause".
    let nanos =
        u64::try_from(later.saturating_duration_since(earlier).as_nanos()).unwrap_or(u64::MAX);
    nanos as f64 / 1_000_000.0
}

/// Converts wheel events into row counts — pi's `WheelScrollAccelerator`.
#[derive(Debug)]
pub(super) struct WheelAccelerator {
    /// The `fullscreenWheelScrollLines` in force.
    lines: WheelScrollLines,
    /// Whether `"auto"` may speed up. pi's `accelerate`, which defaults to
    /// `!terminalAcceleratesWheel()`.
    accelerate: bool,
    /// When and which way the previous accelerated event came — pi's `lastTime` and
    /// `lastDirection`, whose initial `-Infinity` / `0` ("no previous event") is `None`.
    last: Option<(Instant, Direction)>,
    /// The running average of the gap inside this gesture — `averageGap`.
    average_gap_ms: Option<f64>,
    /// The fraction of a row owed to the next event — `carry`.
    carry: f64,
}

impl WheelAccelerator {
    /// An accelerator for `lines`, accelerating when `accelerate` — pi's `constructor(lines,
    /// accelerate)`.
    pub(super) fn new(lines: WheelScrollLines, accelerate: bool) -> Self {
        Self {
            lines,
            accelerate,
            last: None,
            average_gap_ms: None,
            carry: 0.0,
        }
    }

    /// Replace the setting and forget the gesture — pi's `setLines` (`:50-53`), which resets.
    pub(super) fn set_lines(&mut self, lines: WheelScrollLines) {
        self.lines = lines;
        self.last = None;
        self.average_gap_ms = None;
        self.carry = 0.0;
    }

    /// The positive row count for a wheel event in `direction` at `now` — pi's `next(direction,
    /// now)` (`:56-77`).
    pub(super) fn next(&mut self, direction: Direction, now: Instant) -> i32 {
        if let WheelScrollLines::Lines(fixed) = self.lines {
            return i32::from(fixed.get());
        }
        if !self.accelerate {
            return 1;
        }
        // `gap = now - lastTime; sameGesture = direction === lastDirection && gap <= GESTURE_GAP_MS`
        // — with `lastTime` updated before the test, so even a reset event starts the next gap.
        let previous = self.last.replace((now, direction));
        let gap_ms = previous
            .filter(|&(_, last_direction)| last_direction == direction)
            .map(|(at, _)| elapsed_ms(at, now))
            .filter(|gap| *gap <= GESTURE_GAP_MS);
        let Some(gap) = gap_ms else {
            self.average_gap_ms = None;
            self.carry = 0.0;
            return 1;
        };
        if gap < BURST_GAP_MS {
            return 1;
        }
        let average = self.average_gap_ms.map_or(gap, |avg| (avg + gap) / 2.0);
        self.average_gap_ms = Some(average);
        let lines = (REFERENCE_GAP_MS / average).clamp(1.0, MAX_AUTO_LINES) + self.carry;
        let whole = lines.floor();
        self.carry = lines - whole;
        // `lines` is at most `MAX_AUTO_LINES + carry < 7`, so the conversion is exact.
        whole as i32
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use std::time::Duration;

    use cyrup_config::settings::WheelLineCount;

    use super::*;

    /// `scroll(accelerator, times, direction)` (`test/wheel-scroll.test.ts:5-7`): one `next` per
    /// timestamp, the timestamps being milliseconds from a fixed origin.
    fn scroll(
        accelerator: &mut WheelAccelerator,
        origin: Instant,
        times_ms: &[u64],
        direction: Direction,
    ) -> Vec<i32> {
        times_ms
            .iter()
            .map(|ms| accelerator.next(direction, origin + Duration::from_millis(*ms)))
            .collect()
    }

    fn fixed(n: u8) -> WheelScrollLines {
        WheelScrollLines::Lines(WheelLineCount::from_number(f64::from(n)).unwrap())
    }

    /// `uses fixed line counts regardless of timing` (`:11-15`).
    #[test]
    fn fixed_counts_ignore_timing() {
        let t0 = Instant::now();
        let mut acc = WheelAccelerator::new(fixed(3), true);
        assert_eq!(
            scroll(&mut acc, t0, &[0, 10, 20, 1000], Direction::Down),
            [3, 3, 3, 3]
        );
        // pi's `setLines(0.5)` floors to a one-row minimum; the clamp is the setting's.
        acc.set_lines(WheelScrollLines::from_number(0.5));
        assert_eq!(
            acc.next(Direction::Down, t0 + Duration::from_millis(2000)),
            1
        );
    }

    /// `keeps one line per event in auto mode when the terminal already accelerates` (`:17-20`).
    #[test]
    fn auto_is_one_row_when_the_terminal_accelerates() {
        let t0 = Instant::now();
        let mut acc = WheelAccelerator::new(WheelScrollLines::Auto, false);
        assert_eq!(
            scroll(&mut acc, t0, &[0, 10, 20, 30], Direction::Down),
            [1, 1, 1, 1]
        );
    }

    /// `scales auto mode with wheel velocity` (`:22-27`): 100 ms gaps move 1 row, 50 ms 2, 20 ms 5,
    /// and 10 ms hits the 6-row cap. The first event of every gesture is 1.
    #[test]
    fn auto_scales_with_velocity() {
        let t0 = Instant::now();
        let mut acc = WheelAccelerator::new(WheelScrollLines::Auto, true);
        assert_eq!(
            scroll(&mut acc, t0, &[0, 150, 300, 450], Direction::Down),
            [1, 1, 1, 1]
        );
        assert_eq!(
            scroll(&mut acc, t0, &[1000, 1050, 1100, 1150], Direction::Down),
            [1, 2, 2, 2]
        );
        assert_eq!(
            scroll(&mut acc, t0, &[2000, 2020, 2040, 2060], Direction::Down),
            [1, 5, 5, 5]
        );
        assert_eq!(
            scroll(&mut acc, t0, &[3000, 3010, 3020, 3030], Direction::Down),
            [1, 6, 6, 6]
        );
    }

    /// `does not accelerate bursts of events for a single notch` (`:29-32`): events 3 ms apart are
    /// one physical notch.
    #[test]
    fn bursts_move_one_row_each() {
        let t0 = Instant::now();
        let mut acc = WheelAccelerator::new(WheelScrollLines::Auto, true);
        assert_eq!(
            scroll(&mut acc, t0, &[0, 3, 6, 9], Direction::Down),
            [1, 1, 1, 1]
        );
    }

    /// `resets acceleration on direction changes and pauses` (`:34-39`): a reversal restarts the
    /// gesture at 1, and a pause longer than 200 ms does too.
    #[test]
    fn direction_changes_and_pauses_reset_the_gesture() {
        let t0 = Instant::now();
        let mut acc = WheelAccelerator::new(WheelScrollLines::Auto, true);
        assert_eq!(
            scroll(&mut acc, t0, &[0, 20, 40], Direction::Down),
            [1, 5, 5]
        );
        assert_eq!(acc.next(Direction::Up, t0 + Duration::from_millis(60)), 1);
        assert_eq!(
            scroll(&mut acc, t0, &[500, 520], Direction::Down),
            [1, 5],
            "a 440 ms pause ended the gesture"
        );
    }

    /// A pause alone — same direction throughout — ends a gesture once it exceeds 200 ms: the
    /// event after a 380 ms silence is a first event again (1), and the gesture then rebuilds from
    /// there (`GESTURE_GAP_MS`). pi's own reset case also reverses direction, so it cannot tell a
    /// pause from a reversal.
    #[test]
    fn a_pause_ends_the_gesture_without_a_direction_change() {
        let t0 = Instant::now();
        let mut acc = WheelAccelerator::new(WheelScrollLines::Auto, true);
        assert_eq!(
            scroll(&mut acc, t0, &[0, 20, 400, 420], Direction::Down),
            [1, 5, 1, 5]
        );
    }

    /// `carries fractional lines between events` (`:41-44`): 40 ms gaps are 2.5 rows, so the half
    /// row owed is paid on the following event.
    #[test]
    fn fractional_rows_carry_between_events() {
        let t0 = Instant::now();
        let mut acc = WheelAccelerator::new(WheelScrollLines::Auto, true);
        assert_eq!(
            scroll(&mut acc, t0, &[0, 40, 80, 120, 160], Direction::Down),
            [1, 2, 3, 2, 3]
        );
    }

    /// `setLines` resets the gesture (`wheel-scroll.ts:50-53`): the event after a setting change is
    /// a first event again, however close it is to the one before.
    #[test]
    fn set_lines_forgets_the_gesture() {
        let t0 = Instant::now();
        let mut acc = WheelAccelerator::new(WheelScrollLines::Auto, true);
        assert_eq!(scroll(&mut acc, t0, &[0, 20], Direction::Down), [1, 5]);
        acc.set_lines(WheelScrollLines::Auto);
        assert_eq!(scroll(&mut acc, t0, &[40], Direction::Down), [1]);
    }

    /// `terminalAcceleratesWheel` (`wheel-scroll.ts:16-25`): darwin AND none of the three SSH
    /// variables.
    #[test]
    fn only_a_local_macos_terminal_accelerates() {
        let none = |_: &str| false;
        assert!(accelerates_wheel(true, none));
        assert!(!accelerates_wheel(false, none), "not macOS");
        for var in ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"] {
            assert!(
                !accelerates_wheel(true, |name| name == var),
                "{var} marks an SSH session"
            );
        }
    }
}
