//! TUI-163 — the Windows console's OS half: the console mode, the wait and `ReadConsoleInputW`.
//! The records it reads go through [`ConsolePump`] (`super::console`), the platform-neutral half,
//! and from there through the same [`super::bytes::ByteDecoder`] the unix reader uses.
//!
//! This is pi's own Windows input: `ProcessTerminal.start()` adds `ENABLE_VIRTUAL_TERMINAL_INPUT`
//! to the stdin console handle right after raw mode (`enableWindowsVTInput`, `terminal.ts:196-200`,
//! `:377-388` @v1.0.0), and libuv's reader hands pi the bytes. cyrup's reader is its own, so the
//! bit is set — and the records read — here.
//!
//! # The console mode
//!
//! `ENABLE_VIRTUAL_TERMINAL_INPUT` is what makes the console hand over the terminal's bytes, the
//! colour-scheme report among them. It is cyrup's to set exactly while the TUI owns the terminal
//! AND a [`ConsoleReader`] reads it:
//!
//! | event | what happens here |
//! |---|---|
//! | the TUI's terminal starts (`color_scheme::terminal_started` — startup, resume after `$EDITOR` or suspend, an alternate screen entered) | [`claim`]: the bit is set when a reader is open |
//! | the TUI's terminal stops (`color_scheme::terminal_stopped` — exit, panic, `$EDITOR`, suspend) | [`release`]: the bit is cleared if cyrup set it |
//! | a reader opens | the bit is set if the terminal is owned, and the report becomes receivable |
//! | the last reader closes | the bit is cleared if cyrup set it, and the report is no longer receivable |
//!
//! Tying the bit to the terminal's start and stop is what keeps it off for everyone else: crossterm's
//! `disable_raw_mode` keeps every bit it did not clear itself, so a bit left set would make the
//! parent shell — or `$EDITOR` — receive escape sequences for its arrow keys. (pi gets this from
//! libuv, whose `setRawMode(false)` restores the console mode it found.) The panic hook reaches
//! [`release`] through `color_scheme::terminal_stopped` too.
//!
//! Everything goes through `crossterm_winapi`'s safe wrappers (the crate crossterm itself reads the
//! console with), so `#![forbid(unsafe_code)]` holds. That crate has no wait on the console handle,
//! so a reader with nothing to read polls the input count every [`POLL_TICK`].

use std::collections::VecDeque;
use std::io;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crossterm_winapi::{Console, ConsoleMode, Handle, InputRecord};
use ratatui::crossterm::event::Event;

use super::console::{ConsolePump, ConsoleRecord};

/// `ENABLE_VIRTUAL_TERMINAL_INPUT` (`consoleapi.h`).
const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

/// How often an idle reader looks for input. Well under any escape timeout, and well under what a
/// person can perceive as typing latency.
const POLL_TICK: Duration = Duration::from_millis(4);

/// Who holds the console mode.
#[derive(Debug)]
struct ModeState {
    /// Open [`ConsoleReader`]s.
    readers: usize,
    /// Between the TUI's terminal starting and stopping.
    owned: bool,
    /// cyrup set the bit (it was clear before), so cyrup clears it.
    added: bool,
    /// The bit is set, by cyrup or before it.
    vt: bool,
}

static MODE: Mutex<ModeState> = Mutex::new(ModeState {
    readers: 0,
    owned: false,
    added: false,
    vt: false,
});

fn with_mode<R>(f: impl FnOnce(&mut ModeState) -> R) -> R {
    f(&mut MODE.lock().unwrap_or_else(|e| e.into_inner()))
}

fn input_mode() -> io::Result<ConsoleMode> {
    Ok(ConsoleMode::from(Handle::current_in_handle()?))
}

/// Set the bit if it is clear; `true` when it is set afterwards.
fn set_bit(state: &mut ModeState) -> bool {
    let Ok(mode) = input_mode() else {
        return false;
    };
    let Ok(current) = mode.mode() else {
        return false;
    };
    if current & ENABLE_VIRTUAL_TERMINAL_INPUT == 0 {
        if mode
            .set_mode(current | ENABLE_VIRTUAL_TERMINAL_INPUT)
            .is_err()
        {
            return false;
        }
        state.added = true;
    }
    state.vt = true;
    true
}

/// Clear the bit if cyrup set it.
fn clear_bit(state: &mut ModeState) {
    if !std::mem::take(&mut state.added) {
        return;
    }
    state.vt = false;
    if let Ok(mode) = input_mode()
        && let Ok(current) = mode.mode()
    {
        let _ = mode.set_mode(current & !ENABLE_VIRTUAL_TERMINAL_INPUT);
    }
}

/// The TUI's terminal started: set the bit if a reader will read it. `true` when the report can be
/// received now.
pub(crate) fn claim() -> bool {
    with_mode(|state| {
        state.owned = true;
        state.readers > 0 && set_bit(state)
    })
}

/// The TUI's terminal stopped: clear the bit, so whoever reads the console next gets keys. Never
/// blocks — the panic hook calls it — and a lock held by a panicking thread is taken over.
pub(crate) fn release() {
    let mut guard = match MODE.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    guard.owned = false;
    clear_bit(&mut guard);
}

fn owned() -> bool {
    with_mode(|state| state.owned)
}

/// Whether the console is read as the terminal's bytes right now: a reader is open, the TUI owns
/// the terminal, and virtual-terminal input is on — so a reply to a terminal query reaches
/// [`crate::terminal_query::offer_reply`] instead of the prompt.
pub(crate) fn receives_bytes() -> bool {
    with_mode(|state| state.readers > 0 && state.owned && state.vt)
}

/// Whether this console accepts virtual-terminal input at all (Windows 10 1809 and later, and
/// every terminal hosted on ConPTY). The mode is left as it was found.
fn accepts_vt_input() -> io::Result<()> {
    let mode = input_mode()?;
    let current = mode.mode()?;
    if current & ENABLE_VIRTUAL_TERMINAL_INPUT != 0 {
        return Ok(());
    }
    mode.set_mode(current | ENABLE_VIRTUAL_TERMINAL_INPUT)?;
    let took = mode.mode()? & ENABLE_VIRTUAL_TERMINAL_INPUT != 0;
    mode.set_mode(current)?;
    if took {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the console does not accept virtual-terminal input",
        ))
    }
}

/// The console input, read as the bytes the terminal sent — the Windows counterpart of
/// [`super::reader::TtyReader`](crate::input).
pub(crate) struct ConsoleReader {
    console: Console,
    pump: ConsolePump,
    ready: VecDeque<Event>,
}

impl ConsoleReader {
    /// Open the console input. Fails when it is not a console, or the console refuses
    /// virtual-terminal input — the caller then falls back to crossterm's reader, and the
    /// colour-scheme report is never asked for.
    pub(crate) fn open(escape_timeout: Duration) -> io::Result<Self> {
        let console = Console::from(Handle::current_in_handle()?);
        accepts_vt_input()?;
        let receivable = with_mode(|state| {
            state.readers = state.readers.saturating_add(1);
            state.owned && set_bit(state)
        });
        if receivable {
            crate::color_scheme::set_receivable(true);
        }
        Ok(Self {
            console,
            pump: ConsolePump::new(escape_timeout),
            ready: VecDeque::new(),
        })
    }

    /// Read what the console has, waiting at most `max_wait` (less when a held escape falls due
    /// sooner), and append the decoded events to `out`. While `may_read` is false — `$EDITOR` or a
    /// suspend owns the terminal — or the TUI does not own the terminal, nothing is read. `Ok(false)`
    /// never happens: a console does not reach end of input.
    pub(crate) fn pump(
        &mut self,
        max_wait: Duration,
        may_read: &dyn Fn() -> bool,
        out: &mut Vec<Event>,
    ) -> io::Result<bool> {
        let start = Instant::now();
        self.pump.flush_due(start, out);
        let wait = match self.pump.deadline() {
            Some(due) => due.saturating_duration_since(start).min(max_wait),
            None => max_wait,
        };
        let until = start + wait;
        loop {
            if may_read() && owned() && self.console.number_of_console_input_events()? > 0 {
                let records: Vec<ConsoleRecord> = self
                    .console
                    .read_console_input()?
                    .iter()
                    .map(record)
                    .collect();
                self.pump.push(
                    &records,
                    Instant::now(),
                    &ratatui::crossterm::terminal::size,
                    out,
                );
                return Ok(true);
            }
            let now = Instant::now();
            if now >= until {
                self.pump.flush_due(now, out);
                return Ok(true);
            }
            std::thread::sleep(POLL_TICK.min(until - now));
        }
    }

    /// The next event, waiting at most `wait`.
    pub(crate) fn next_event_timeout(&mut self, wait: Duration) -> io::Result<Option<Event>> {
        if let Some(ev) = self.ready.pop_front() {
            return Ok(Some(ev));
        }
        let mut out = Vec::new();
        self.pump(wait, &|| true, &mut out)?;
        self.ready.extend(out);
        Ok(self.ready.pop_front())
    }
}

impl Drop for ConsoleReader {
    fn drop(&mut self) {
        let last = with_mode(|state| {
            state.readers = state.readers.saturating_sub(1);
            if state.readers == 0 {
                clear_bit(state);
                true
            } else {
                false
            }
        });
        if last {
            crate::color_scheme::set_receivable(false);
        }
    }
}

fn record(record: &InputRecord) -> ConsoleRecord {
    match record {
        InputRecord::KeyEvent(key) => ConsoleRecord::Key {
            key_down: key.key_down,
            unit: key.u_char,
            repeat: key.repeat_count,
        },
        InputRecord::WindowBufferSizeEvent(_) => ConsoleRecord::Resize,
        InputRecord::MouseEvent(_) | InputRecord::FocusEvent(_) | InputRecord::MenuEvent(_) => {
            ConsoleRecord::Other
        }
    }
}

/// TUI-163 — the child half of `tests/windows_conpty.rs`, and not API: run on a pseudo-console,
/// it follows the terminal's appearance the way a session does and prints what happened, one line
/// per event, for the test to read off the console's screen.
///
/// The chain is the session's own from the console down to the re-resolve: raw mode, the start
/// (which turns virtual-terminal input on), the run loop's listener, [`ConsoleReader`], and
/// [`crate::ThemeController::apply_color_scheme`] — the call `App::apply_color_scheme_report`
/// makes. What it leaves out is the drawing: an `App` needs a session to run.
///
/// Prints `cyrup-probe: ready theme=<name>` once reading, `cyrup-probe: scheme=<scheme>
/// theme=<name>` per report heard, `cyrup-probe: key=<char>` per typed character, and returns when
/// `z` is typed or `bound` has passed.
#[doc(hidden)]
pub fn conpty_probe(theme_setting: &str, bound: Duration) -> io::Result<()> {
    use std::io::Write as _;

    use ratatui::crossterm::event::{KeyCode, KeyEventKind};

    let mut out = io::stdout();
    ratatui::crossterm::terminal::enable_raw_mode()?;
    let mut controller = crate::ThemeController::boot_from_env(Some(theme_setting));
    let (tx, rx) = std::sync::mpsc::channel();
    crate::color_scheme::set_listener(Some(std::sync::Arc::new(move |scheme| {
        let _ = tx.send(scheme);
    })));
    crate::color_scheme::terminal_started(&mut out);
    crate::color_scheme::set_notifications(controller.auto_sync());
    let mut reader = ConsoleReader::open(Duration::from_millis(50))?;
    write!(
        out,
        "cyrup-probe: ready theme={}\r\n",
        controller.active_name()
    )?;
    out.flush()?;
    let started = Instant::now();
    'probe: while started.elapsed() < bound {
        let event = reader.next_event_timeout(Duration::from_millis(50))?;
        while let Ok(scheme) = rx.try_recv() {
            controller.apply_color_scheme(scheme);
            write!(
                out,
                "cyrup-probe: scheme={scheme:?} theme={}\r\n",
                controller.active_name()
            )?;
            out.flush()?;
        }
        if let Some(Event::Key(key)) = event
            && key.kind != KeyEventKind::Release
            && let KeyCode::Char(c) = key.code
        {
            write!(out, "cyrup-probe: key={c}\r\n")?;
            out.flush()?;
            if c == 'z' {
                break 'probe;
            }
        }
    }
    crate::color_scheme::terminal_stopped(&mut out);
    crate::color_scheme::set_listener(None);
    drop(reader);
    ratatui::crossterm::terminal::disable_raw_mode()
}
