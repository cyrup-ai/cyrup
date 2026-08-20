//! The **OSC 9;4 terminal progress indicator** — the port of Pi's `ProcessTerminal.setProgress`
//! (`pi/packages/tui/src/terminal.ts:11-13` for the sequences, `:509-523` for the emitter, `:407-409`
//! for the shutdown clear) together with the five `interactive-mode.ts` call sites that drive it.
//!
//! # What was missing
//!
//! Cyrup already had the *switch*: `terminal.showTerminalProgress` is read by
//! [`cyrup_config::EffectiveSettings::show_terminal_progress`] and the `/settings` selector shows a
//! "Terminal progress" toggle row wired to it (`app.rs`, the `SettingRow::toggle` for
//! `"terminal.showTerminalProgress"`). Nothing was behind it: no code in the workspace ever wrote
//! `ESC ] 9 ; 4`, so a user could turn the row on and observe precisely nothing. This module is the
//! mechanism that row was always describing.
//!
//! # What Pi emits, and when
//!
//! `ProcessTerminal` (`terminal.ts:509-523`):
//!
//! ```text
//! setProgress(true)  -> write "\x1b]9;4;3\x07"          (OSC 9;4;3 = indeterminate)
//!                       and arm a 1000 ms setInterval that re-writes the SAME sequence
//! setProgress(false) -> clear the interval, write "\x1b]9;4;0\x07"   (OSC 9;4;0 = clear)
//! stop()             -> if the interval was armed, write the clear sequence  (`:407-409`)
//! ```
//!
//! The keepalive is not decoration. OSC 9;4;3 is ConEmu's protocol as adopted by Windows Terminal,
//! WezTerm and others, and several implementations expire an indeterminate state that is not
//! refreshed; a long turn would otherwise lose its taskbar pulse partway through.
//!
//! `interactive-mode.ts` drives it from exactly five places, each gated on the setting
//! (`getShowTerminalProgress()`):
//!
//! | site | line (v0.83.0) | call |
//! |---|---|---|
//! | `case "agent_start"` | `:2865-2867` | `setProgress(true)` |
//! | `case "agent_end"` | `:3057-3059` | `setProgress(false)` |
//! | `case "compaction_start"` | `:3076-3078` | `setProgress(true)` |
//! | `case "compaction_end"` | `:3090-3092` | `setProgress(false)` |
//! | `stop()` | `:6041-6043` | `setProgress(false)` |
//!
//! Note what is NOT in that list: `agent_settled`, `turn_start`/`turn_end`, tool execution, and the
//! auto-retry events. "Progress" here means *the agent is doing something the user is waiting on*,
//! which begins at `agent_start` and ends at `agent_end` — a retry backoff sits inside that window
//! and needs no separate signal.
//!
//! # Version note
//!
//! v0.83.0 spells the clear sequence `"\x1b]9;4;0;\x07"` with a trailing `;`. v0.84.1 — the version
//! this port targets — drops it (`terminal.ts:13`, the whole of that commit's change to this file
//! besides the unrelated Windows shift-enter rename). [`TERMINAL_PROGRESS_CLEAR_SEQUENCE`] is the
//! v0.84.1 spelling; the stray `;` was a malformed parameter that some terminals reject outright.
//!
//! # The split between the two halves here
//!
//! [`TerminalProgress`] is the `interactive-mode` half: it holds the setting and decides *whether*
//! a transition writes anything. [`write_terminal_progress`] is the `ProcessTerminal` half: it does
//! the write and maintains [`progress_is_armed`], a process-global mirror of Pi's
//! `progressInterval` field. The global exists because the shutdown clear has to work from
//! [`crate::panic_hook::restore_terminal_best_effort`], which runs from a `std::panic` hook and has
//! no `&App` to consult — and under the release profile's `panic = "abort"` it is the only code that
//! still runs at all. A progress indicator that outlives the process that set it is a taskbar the
//! user cannot clear without restarting their terminal, so this is the case that most needs
//! covering.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// OSC 9;4;3 — set indeterminate progress. Pi `TERMINAL_PROGRESS_ACTIVE_SEQUENCE`
/// (`tui/src/terminal.ts:12`), byte-identical at v0.83.0 and v0.84.1.
pub const TERMINAL_PROGRESS_ACTIVE_SEQUENCE: &str = "\x1b]9;4;3\x07";

/// OSC 9;4;0 — clear progress. Pi `TERMINAL_PROGRESS_CLEAR_SEQUENCE` (`tui/src/terminal.ts:13`) in
/// its **v0.84.1** spelling; v0.83.0 had a trailing `;` inside the OSC body that upstream removed.
pub const TERMINAL_PROGRESS_CLEAR_SEQUENCE: &str = "\x1b]9;4;0\x07";

/// Pi `TERMINAL_PROGRESS_KEEPALIVE_MS` (`tui/src/terminal.ts:11`) — the `setInterval` period that
/// re-writes [`TERMINAL_PROGRESS_ACTIVE_SEQUENCE`] for as long as progress is armed.
pub const TERMINAL_PROGRESS_KEEPALIVE: Duration = Duration::from_millis(1000);

/// Process-global mirror of Pi's `ProcessTerminal.progressInterval` — "the terminal currently has an
/// active progress indicator that we put there".
///
/// Written only by [`write_terminal_progress`], so it can never disagree with what was actually sent
/// to the terminal. Read by [`progress_is_armed`], whose only caller is the exit/crash restore path.
static PROGRESS_ARMED: AtomicBool = AtomicBool::new(false);

/// Whether an active progress sequence has been written and not yet cleared — Pi's
/// `if (this.progressInterval)` guard in `ProcessTerminal.stop()` (`terminal.ts:407-409`).
///
/// The exit path uses this so an ordinary session that never armed progress does not emit a stray
/// OSC on every quit.
pub fn progress_is_armed() -> bool {
    PROGRESS_ARMED.load(Ordering::Relaxed)
}

/// Write the OSC 9;4 progress sequence — Pi `ProcessTerminal.setProgress`
/// (`tui/src/terminal.ts:509-523`).
///
/// `active` selects [`TERMINAL_PROGRESS_ACTIVE_SEQUENCE`] or [`TERMINAL_PROGRESS_CLEAR_SEQUENCE`]
/// and updates [`PROGRESS_ARMED`] to match. Both sequences are fixed constants with no interpolated
/// payload, so unlike [`crate::app::write_terminal_title`] there is nothing here to sanitize.
///
/// Flushed immediately: the indicator is the point, and a buffered stdout would hold it until the
/// next frame — or, on the clear written from a crash path, discard it entirely.
pub fn write_terminal_progress(active: bool) {
    use std::io::Write;
    let seq = if active {
        TERMINAL_PROGRESS_ACTIVE_SEQUENCE
    } else {
        TERMINAL_PROGRESS_CLEAR_SEQUENCE
    };
    let mut out = std::io::stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
    PROGRESS_ARMED.store(active, Ordering::Relaxed);
}

/// The `interactive-mode` half of the indicator: the `terminal.showTerminalProgress` gate plus the
/// armed/idle bit the keepalive ticker is driven from.
///
/// Held on [`crate::AppState`] and mutated by the session-event fold; the run loop is what turns a
/// transition into an actual write, exactly as it does for the OSC 0 window title.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalProgress {
    /// `terminal.showTerminalProgress`. Pi has no field for this — it calls
    /// `this.settingsManager.getShowTerminalProgress()` afresh at each of the five call sites, so
    /// the setting is live and a `/settings` flip takes effect on the very next transition. Caching
    /// it here reproduces that only because [`Self::set_enabled`] is wired to the `ApplySetting`
    /// command; see the `terminal.showTerminalProgress` arm in `App::execute_command`.
    enabled: bool,
    /// Whether this session considers progress to be running. Distinct from [`PROGRESS_ARMED`],
    /// which tracks what the *terminal* was last told: a Ctrl+Z suspend clears the terminal's
    /// indicator (Pi does the same — `handleCtrlZ` → `ui.stop()` → `terminal.stop()`) while this
    /// stays set, and the next keepalive tick re-arms the terminal on resume.
    active: bool,
    /// The transition recorded but not yet written to the terminal, drained by
    /// [`Self::take_pending`].
    ///
    /// Pi has no equivalent because `ui.terminal.setProgress` writes to stdout synchronously from
    /// inside the event handler. Cyrup's session-event fold ([`crate::App::ingest_event_rendered_owned`])
    /// is a pure state transition that the run loop turns into terminal output one step later — the
    /// same split [`crate::AppState::terminal_title`] uses — so the transition has to be parked
    /// somewhere in between. Draining is what guarantees a write happens once per transition rather
    /// than once per frame.
    pending: Option<bool>,
}

impl TerminalProgress {
    /// Seed the setting without producing a transition — used when a session binds and the
    /// effective settings are first read.
    pub fn with_enabled(enabled: bool) -> Self {
        Self { enabled, active: false, pending: None }
    }

    /// Whether `terminal.showTerminalProgress` is on.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Whether this session believes a progress indicator is running — the condition the keepalive
    /// ticker is gated on.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// A `/settings` flip of `terminal.showTerminalProgress`. Returns `Some(false)` when the row was
    /// turned OFF while progress was running, meaning the caller must write a clear.
    ///
    /// `[CYRUP-DELTA]`: Pi emits nothing here. Because its gate is re-read at every call site, a
    /// user who turns the row off mid-turn makes Pi's own `agent_end` clear unreachable, and the
    /// indicator stays lit until `ProcessTerminal.stop()` runs at exit. Clearing on the disabling
    /// edge is a strict subset of that eventual cleanup — the same [`TERMINAL_PROGRESS_CLEAR_SEQUENCE`]
    /// Pi would have sent, sent sooner — and it is the behaviour the row's own label promises: after
    /// turning "Terminal progress" off there is no terminal progress.
    pub fn set_enabled(&mut self, enabled: bool) -> Option<bool> {
        let was_running = self.enabled && self.active;
        self.enabled = enabled;
        if !enabled && was_running {
            self.active = false;
            self.pending = Some(false);
            return Some(false);
        }
        None
    }

    /// A progress transition at one of Pi's four event call sites — `if
    /// (this.settingsManager.getShowTerminalProgress()) this.ui.terminal.setProgress(active)`.
    ///
    /// Returns the value to hand [`write_terminal_progress`], or `None` when the setting is off and
    /// Pi would not have called `setProgress` at all.
    ///
    /// Deliberately NOT deduplicated against [`Self::active`]: Pi re-writes the active sequence on
    /// every `setProgress(true)` and the clear on every `setProgress(false)`, and a `compaction_end`
    /// nested inside a still-streaming turn is one of the shapes that depends on it.
    pub fn set(&mut self, active: bool) -> Option<bool> {
        if !self.enabled {
            return None;
        }
        self.active = active;
        self.pending = Some(active);
        Some(active)
    }

    /// Drain the transition recorded by [`Self::set`] / [`Self::set_enabled`], if any. The run loop
    /// hands the result to [`write_terminal_progress`].
    pub fn take_pending(&mut self) -> Option<bool> {
        self.pending.take()
    }

    /// The exit clear — Pi's `stop()` (`interactive-mode.ts:6041-6043`) plus
    /// `ProcessTerminal.stop()`'s own guarded clear (`terminal.ts:407-409`).
    ///
    /// Returns `true` when a clear must be written. The condition is the *terminal's* armed state
    /// ([`progress_is_armed`]), not the setting: Pi's `ProcessTerminal.stop()` clears whenever its
    /// interval is live regardless of what the interactive mode's gate now says, which is what stops
    /// a setting flipped off mid-turn from stranding a lit indicator past process exit.
    pub fn shutdown(&mut self) -> bool {
        self.active = false;
        self.pending = None;
        progress_is_armed()
    }

    /// The keepalive tick body — Pi's `setInterval(() => write(ACTIVE), 1000)`
    /// (`terminal.ts:514-516`). `true` when the active sequence should be re-sent.
    pub fn keepalive(&self) -> bool {
        self.enabled && self.active
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

    use super::*;

    /// The wire bytes, against `pi/packages/tui/src/terminal.ts:12-13` read at v0.84.1. A terminal
    /// parses these positionally, so this pins the payload rather than a formatting choice —
    /// including the absence of the trailing `;` that v0.83.0 had and v0.84.1 removed.
    #[test]
    fn sequences_match_pi_byte_for_byte() {
        assert_eq!(TERMINAL_PROGRESS_ACTIVE_SEQUENCE.as_bytes(), b"\x1b]9;4;3\x07");
        assert_eq!(TERMINAL_PROGRESS_CLEAR_SEQUENCE.as_bytes(), b"\x1b]9;4;0\x07");
        assert_eq!(TERMINAL_PROGRESS_KEEPALIVE, Duration::from_millis(1000));
    }

    /// The gate. With the setting off, Pi never reaches `ui.terminal.setProgress`, so no transition
    /// may produce a write and the keepalive must stay silent.
    #[test]
    fn the_setting_gates_every_transition() {
        let mut p = TerminalProgress::default();
        assert!(!p.enabled());
        assert_eq!(p.set(true), None);
        assert!(!p.is_active());
        assert!(!p.keepalive());
        assert_eq!(p.set(false), None);
        assert_eq!(p.take_pending(), None, "nothing may reach the terminal with the row off");
    }

    /// A transition is parked for the run loop to write, and draining it yields it exactly ONCE —
    /// the property that keeps one `agent_start` from re-writing the active sequence on every frame.
    #[test]
    fn a_transition_is_drained_exactly_once() {
        let mut p = TerminalProgress::with_enabled(true);
        p.set(true);
        assert_eq!(p.take_pending(), Some(true));
        assert_eq!(p.take_pending(), None, "already written");
        assert!(p.is_active(), "draining the write does not end the progress window");
        p.set(false);
        assert_eq!(p.take_pending(), Some(false));
        assert_eq!(p.take_pending(), None);
    }

    /// With the setting on, start/end map onto Pi's two sequences and the keepalive runs only
    /// between them.
    #[test]
    fn enabled_start_and_end_map_to_pis_two_sequences() {
        let mut p = TerminalProgress::with_enabled(true);
        assert!(!p.keepalive(), "idle before the first transition");
        assert_eq!(p.set(true), Some(true));
        assert!(p.is_active());
        assert!(p.keepalive(), "the 1s re-write runs for as long as progress is armed");
        assert_eq!(p.set(false), Some(false));
        assert!(!p.is_active());
        assert!(!p.keepalive());
    }

    /// Pi writes on EVERY call, not only on a change: `setProgress(true)` twice writes the active
    /// sequence twice. Deduplicating would drop the second `compaction_start` of a turn that
    /// auto-compacts more than once.
    #[test]
    fn repeated_transitions_are_not_deduplicated() {
        let mut p = TerminalProgress::with_enabled(true);
        assert_eq!(p.set(true), Some(true));
        assert_eq!(p.set(true), Some(true));
        assert_eq!(p.set(false), Some(false));
        assert_eq!(p.set(false), Some(false));
    }

    /// Turning the row off mid-turn clears — the documented `[CYRUP-DELTA]` on
    /// [`TerminalProgress::set_enabled`]. Turning it off while idle writes nothing, and turning it
    /// ON never writes on its own (Pi arms only from an `agent_start`/`compaction_start`).
    #[test]
    fn disabling_the_setting_mid_turn_clears_but_enabling_never_arms() {
        let mut p = TerminalProgress::with_enabled(true);
        p.set(true);
        p.take_pending();
        assert_eq!(p.set_enabled(false), Some(false), "a running indicator must be cleared");
        assert_eq!(p.take_pending(), Some(false), "and the clear must reach the terminal");
        assert!(!p.is_active());

        let mut idle = TerminalProgress::with_enabled(true);
        assert_eq!(idle.set_enabled(false), None, "nothing was lit, so nothing to clear");

        let mut off = TerminalProgress::default();
        assert_eq!(off.set_enabled(true), None, "turning the row on does not start a turn");
        assert!(!off.is_active());
    }

    /// `shutdown()` answers from the TERMINAL's armed bit, not the setting — Pi's
    /// `ProcessTerminal.stop()` clears whenever its interval is live. This is the case that keeps a
    /// setting flipped off mid-turn from stranding a lit taskbar past process exit.
    ///
    /// Serialised via [`GLOBAL_LOCK`]: [`PROGRESS_ARMED`] is process-global and libtest threads.
    #[test]
    fn shutdown_follows_the_terminal_not_the_setting() {
        let _g = lock_global();
        PROGRESS_ARMED.store(false, Ordering::Relaxed);
        let mut p = TerminalProgress::with_enabled(true);
        assert!(!p.shutdown(), "a session that never armed progress emits no exit sequence");

        // Whatever the interactive-mode gate now says, an armed terminal gets its clear.
        PROGRESS_ARMED.store(true, Ordering::Relaxed);
        let mut off_but_lit = TerminalProgress::with_enabled(true);
        off_but_lit.set(true);
        assert!(off_but_lit.shutdown(), "the row is off yet the terminal is lit — clear it");
        assert_eq!(
            off_but_lit.take_pending(),
            None,
            "shutdown writes its own clear; a parked transition must not be replayed after it"
        );
        PROGRESS_ARMED.store(false, Ordering::Relaxed);
    }

    /// [`write_terminal_progress`] keeps [`PROGRESS_ARMED`] in step with what it sent, which is what
    /// makes [`progress_is_armed`] usable from the panic hook. Writes go to the test harness's
    /// stdout; the sequences are invisible control codes.
    #[test]
    fn writing_tracks_the_global_armed_bit() {
        let _g = lock_global();
        write_terminal_progress(false);
        assert!(!progress_is_armed());
        write_terminal_progress(true);
        assert!(progress_is_armed(), "the terminal now has an indicator we put there");
        write_terminal_progress(false);
        assert!(!progress_is_armed());
    }

    /// [`PROGRESS_ARMED`] is process-global; the two tests that touch it must not interleave.
    static GLOBAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_global() -> std::sync::MutexGuard<'static, ()> {
        GLOBAL_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
