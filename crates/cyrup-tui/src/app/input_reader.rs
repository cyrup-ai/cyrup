use super::*;

/// Write the OSC 0 window-title sequence — Pi `ProcessTerminal.setTitle`
/// (`pi/packages/tui/src/terminal.ts:504-507`, `\x1b]0;${title}\x07`).
///
/// `[CYRUP-DELTA]`: control characters are stripped first. Pi interpolates the extension-supplied
/// string verbatim, so a title containing `BEL`/`ESC` would close the OSC early and let the rest of
/// the string be interpreted as terminal commands. Stripping keeps an extension from driving the
/// terminal through a title.
pub fn write_terminal_title(title: &str) {
    use std::io::Write;
    let safe: String = title.chars().filter(|c| !c.is_control()).collect();
    let mut out = crate::dead_terminal::terminal_stdout();
    let _ = out.write_all(format!("\x1b]0;{safe}\x07").as_bytes());
    let _ = out.flush();
}

/// The longest single wait of the reader thread. It bounds how late the thread notices a cancelled
/// token, a dropped receiver or the end of a `$EDITOR` handoff, and how often the TUI-092
/// escalation ladder is ticked; input itself wakes the thread at once.
pub(crate) const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The much shorter poll used while [`EscapeReassembler`] or [`StrayReplyFilter`] is holding
/// events. A held prefix (`ESC [`, an OSC 11 opener, `Alt+]`) is released after at most this long,
/// and so is a lone `Esc` on a local link (see [`resolve_escape_timeout`]), so a real `Escape` press
/// costs one imperceptible tick rather than a full [`INPUT_POLL_INTERVAL`] — the standard
/// escape-timeout trade every terminal app makes to tell `ESC` from an escape *sequence*.
#[cfg(not(unix))]
pub(crate) const HELD_FLUSH_INTERVAL: Duration = Duration::from_millis(20);

/// How long a lone `ESC` is held on a local link.
///
/// On unix this is pi's `DEFAULT_ESCAPE_TIMEOUT_MS`, 10 ms (`terminal.ts:115`,
/// `stdin-buffer.ts:24`): the byte reader waits on the framer's own deadline, so it can honour
/// pi's value exactly. cyrup used 20 ms while it could only see crossterm's events, because a hold
/// could end no sooner than the next idle poll; that is still the case on Windows, where
/// crossterm's console reader remains.
#[cfg(unix)]
pub(crate) const LOCAL_ESCAPE_TIMEOUT: Duration = crate::input::frame::DEFAULT_ESCAPE_TIMEOUT;
/// See the unix definition.
#[cfg(not(unix))]
pub(crate) const LOCAL_ESCAPE_TIMEOUT: Duration = HELD_FLUSH_INTERVAL;

/// Pi `DEFAULT_SSH_ESCAPE_TIMEOUT_MS` (v0.84.2 `tui/src/terminal.ts:116`, #7899): how long a lone
/// `ESC` is held under SSH, where the two halves of a split `Alt+Enter` or arrow key can arrive
/// further apart than [`LOCAL_ESCAPE_TIMEOUT`].
pub(crate) const SSH_ESCAPE_TIMEOUT: Duration = Duration::from_millis(100);

/// Pi's `PI_TUI_ESC_TIMEOUT` in cyrup's spelling. Only the `CYRUP_` name is read, as with
/// `CYRUP_HYPERLINKS` and the other capability overrides.
pub(crate) const ENV_ESC_TIMEOUT: &str = "CYRUP_TUI_ESC_TIMEOUT";

/// Pi `resolveEscapeTimeoutMs` (v0.84.2 `tui/src/terminal.ts:123-131`): how long the reader holds a
/// lone `ESC` before releasing it as the `Escape` key. A positive [`ENV_ESC_TIMEOUT`] wins; else
/// [`SSH_ESCAPE_TIMEOUT`] when `SSH_CONNECTION` or `SSH_TTY` is set (JS truthiness: non-empty);
/// else [`LOCAL_ESCAPE_TIMEOUT`].
///
/// The override is parsed the way `Number(env.PI_TUI_ESC_TIMEOUT)` parses it ([`js_number`]), and
/// the result is bounded the way Node's `setTimeout` bounds the delay Pi hands it: anything outside
/// `1..=2^31-1` ms becomes 1 ms (`lib/internal/timers.js`, `TIMEOUT_MAX`). That bound also keeps
/// the reader's poll clear of an unrepresentable deadline.
pub(crate) fn resolve_escape_timeout(env: impl Fn(&str) -> Option<String>) -> Duration {
    const NODE_TIMEOUT_MAX_MS: f64 = 2_147_483_647.0;
    let configured = env(ENV_ESC_TIMEOUT).as_deref().and_then(js_number);
    if let Some(ms) = configured.filter(|ms| ms.is_finite() && *ms > 0.0) {
        let ms = if (1.0..=NODE_TIMEOUT_MAX_MS).contains(&ms) {
            ms
        } else {
            1.0
        };
        return Duration::from_secs_f64(ms / 1000.0);
    }
    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
    if set("SSH_CONNECTION") || set("SSH_TTY") {
        return SSH_ESCAPE_TIMEOUT;
    }
    LOCAL_ESCAPE_TIMEOUT
}

/// JavaScript's `Number(string)`, as far as a timeout needs it: surrounding whitespace is ignored,
/// an empty string is `0`, a `0x`/`0o`/`0b` prefix selects the radix, and anything else is a decimal
/// literal. `None` is JS's `NaN`. Rust's `f64` parser is looser than JS only on non-finite spellings
/// (`inf`, `nan`), which the caller rejects either way.
fn js_number(s: &str) -> Option<f64> {
    let s = s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if s.is_empty() {
        return Some(0.0);
    }
    let radix = match s.get(..2).map(str::to_ascii_lowercase).as_deref() {
        Some("0x") => 16,
        Some("0o") => 8,
        Some("0b") => 2,
        _ => return s.parse().ok(),
    };
    let digits = s.get(2..).filter(|d| !d.is_empty())?;
    digits.chars().try_fold(0.0, |acc: f64, c| {
        c.to_digit(radix)
            .map(|d| acc * f64::from(radix) + f64::from(d))
    })
}

/// The reader thread's next `event::poll` timeout. A lone held `ESC` waits `escape_timeout` (Pi
/// picks `escapeTimeoutMs` only when `this.buffer === ESC`, v0.84.2 `stdin-buffer.ts:388`); any
/// other held prefix waits [`HELD_FLUSH_INTERVAL`]; nothing held waits [`INPUT_POLL_INTERVAL`].
///
/// Only the reassembler's state decides the lone-`ESC` case: the filter never keeps a bare `Esc`
/// across a poll unless the reassembler is holding the `Esc` behind it (`Esc` `Esc`), because every
/// other way an `Esc` reaches the filter is followed by its successor in the same push or by
/// [`StrayReplyFilter::flush`] on the same idle tick.
///
/// Windows only: the unix byte reader waits on [`crate::input::frame::StdinBuffer::deadline`].
#[cfg(not(unix))]
pub(crate) fn reader_poll_wait(
    reassembler: &EscapeReassembler,
    filter: &StrayReplyFilter,
    escape_timeout: Duration,
) -> Duration {
    if reassembler.is_holding_lone_escape() {
        escape_timeout
    } else if reassembler.is_holding() || filter.is_holding() {
        HELD_FLUSH_INTERVAL
    } else {
        INPUT_POLL_INTERVAL
    }
}

// ------------------------------------------------- TUI-092: the unblockable escape hatch ----
//
// The run loop is one tokio task and the sole drain of the input channel, so any handler that
// stops returning also stops input being read — and the exit keys are downstream of the thing
// that broke. The reader thread below is an `std::thread`: it is the one context in the process
// still running when the loop is wedged, so it is where the escape lives.

/// Bumped by [`App::run`]'s input arm once it has finished servicing one [`InputEvent`]. The reader
/// thread reads it to tell a run loop that is still SERVICING INPUT from one that is merely still
/// ITERATING — a distinction that is not academic here, because `biased;` lets the 80 ms spinner arm
/// starve the input arm indefinitely once a frame costs more than a tick (TUI-092 §2.5, the defect
/// the arm order in `App::run` now fixes). Anything counted outside the input arm would call that
/// state healthy.
///
/// A process-global `static` rather than a threaded-through `Arc`, for the same reason
/// [`crate::terminal_progress`]'s `PROGRESS_ARMED` is one (`terminal_progress.rs:84`): there is
/// exactly one interactive run loop per process, and [`crossterm_input_stream`] has a single
/// production caller (`crates/cyrup/src/main.rs`). Threading a handle through both would change two
/// public signatures — and `EventStream<T>` is `Pin<Box<dyn Stream + Send>>`
/// (`cyrup-core/src/lib.rs:44`), so there is nowhere to smuggle one back — purely to express a
/// singleton. `Relaxed` is sufficient: the reader only asks "is this the value I saw", and never
/// orders other memory against it.
pub(crate) static INPUT_SERVICED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// One input event has been fully serviced.
pub(crate) fn mark_input_serviced() {
    INPUT_SERVICED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// How many input events the run loop has serviced, read from the reader thread.
pub(crate) fn input_serviced() -> u64 {
    INPUT_SERVICED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Set for as long as the run loop has deliberately handed the terminal to a child process, so the
/// watchdog does not read a by-design block as a wedge.
///
/// A first-party flag owned by the loop, **not** an inference from
/// `crossterm::terminal::is_raw_mode_enabled()`. The inference looks equivalent and is not: it is
/// only true in the steady state, it says nothing on a console editor that keeps raw mode on, and a
/// `Ctrl+Z` suspend re-enables raw mode *before* the loop resumes servicing — so the probe would
/// read "raw, and not servicing" for the whole `fg` resume window and promote a working feature
/// into an app exit.
pub(crate) static TERMINAL_RELEASED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// RAII marker for the two paths that block the run loop by design: [`App::suspend`] and
/// [`App::edit_in_external_editor`]. A guard rather than a pair of calls because both bodies return
/// early on `?`.
pub(crate) struct TerminalReleased;

impl TerminalReleased {
    pub(crate) fn enter() -> Self {
        TERMINAL_RELEASED.store(true, std::sync::atomic::Ordering::Relaxed);
        Self
    }
}

impl Drop for TerminalReleased {
    fn drop(&mut self) {
        TERMINAL_RELEASED.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Whether the loop is blocked by design right now.
pub(crate) fn terminal_released() -> bool {
    TERMINAL_RELEASED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The budget an arm body of [`App::run`] is expected to finish inside.
///
/// Sized off the healthy ceiling, not off intuition: the `events` arm can legitimately spend
/// 2 × [`super::extension_render_impl::EXTENSION_RENDER_TIMEOUT`] = 4 s on a single event (two
/// `run_renderer` calls per `EntryAppended`). 8 s is twice that, so a working-but-slow guest
/// renderer never files a report.
///
/// This is a REPORTING threshold only. It bounds nothing and cannot promote anything — the escape
/// hatch is driven entirely by unserviced chords, never by elapsed time — so an arm that
/// legitimately runs long (a lifecycle hook fan-out is N extensions × `DEFAULT_INVOKE_BUDGET`,
/// `cyrup-ext/src/dispatch.rs:21`) costs a transcript warning and nothing else.
pub(crate) const ARM_BUDGET: Duration = Duration::from_secs(8);

/// The arm currently executing, and since when — written by [`ArmGuard`], read by the input
/// reader's watchdog so a hard exit can name what the loop was stuck in. `&'static str` only, so the
/// critical section is two assignments and the reader never allocates.
pub(crate) static ACTIVE_ARM: std::sync::Mutex<Option<(&'static str, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// The last arm to exceed [`ARM_BUDGET`], drained by the run loop into the transcript on its next
/// healthy iteration — so the report reaches the user without ever writing to a raw-mode terminal
/// from a `Drop`.
pub(crate) static OVER_BUDGET_ARM: std::sync::Mutex<Option<&'static str>> =
    std::sync::Mutex::new(None);

/// Marks an arm body as entered for as long as it is held, and records an overrun on the way out.
///
/// A guard rather than a pair of calls precisely because these bodies exit by `break`, `continue`,
/// `return` and `?` as often as they fall off the end — `Drop` covers all five paths.
pub(crate) struct ArmGuard(pub(crate) &'static str, pub(crate) std::time::Instant);

impl ArmGuard {
    pub(crate) fn enter(arm: &'static str) -> Self {
        let now = std::time::Instant::now();
        if let Ok(mut slot) = ACTIVE_ARM.lock() {
            *slot = Some((arm, now));
        }
        Self(arm, now)
    }
}

impl Drop for ArmGuard {
    fn drop(&mut self) {
        if let Ok(mut slot) = ACTIVE_ARM.lock() {
            *slot = None;
        }
        if self.1.elapsed() >= ARM_BUDGET
            && let Ok(mut over) = OVER_BUDGET_ARM.lock()
        {
            *over = Some(self.0);
        }
    }
}

/// Unserviced escalate chords that mean "leave now, unconditionally".
///
/// Three, because chord #1 carries its own HEAD meaning and must not be a stage: `Ctrl+C` clears
/// the editor (`Action::Clear`, pi's `handleCtrlC`) and `Ctrl+D` is forward-delete on a non-empty
/// buffer. #2 is the cooperative cancel, #3 is the hard exit — `crates/cyrup/src/signals.rs`'s two
/// deliveries, reproduced on the key path because raw mode means `Ctrl+C` never becomes SIGINT.
pub(crate) const PANIC_PRESSES: u32 = 3;

/// The minimum spacing between chords that [`PANIC_PRESSES`] will count.
///
/// Load-bearing, not a nicety. A terminal's key auto-repeat delivers a held `Ctrl+D` as a stream of
/// ordinary press events at roughly 30 ms intervals — the `KeyEventKind::Press` filter in
/// [`is_escalate_chord`] cannot tell those from real presses, because on unix they ARE real presses
/// (`REPORT_EVENT_TYPES` is not pushed, so `Repeat` never appears). Without a floor, leaning on
/// `Ctrl+D` — which is forward-delete on a non-empty buffer and a delete key inside `/resume` —
/// would spend all three presses in under 100 ms and hard-exit a perfectly healthy app. 250 ms is
/// below any human double-tap (pi's own `Ctrl+C` window is 500 ms) and an order of magnitude above
/// auto-repeat.
pub(crate) const PANIC_MIN_GAP: Duration = Duration::from_millis(250);

/// `Ctrl+C` or `Ctrl+D`, pressed — not auto-repeated, not released.
///
/// The `kind` filter is load-bearing **on Windows**, where crossterm sets `KeyEventKind`
/// unconditionally (`kind` is "Only set if: Unix: `REPORT_EVENT_TYPES` … Windows: always",
/// crossterm 0.29 `event.rs:941-946`), so one physical press would otherwise arrive as a press AND
/// a release and burn two of [`PANIC_PRESSES`]. This check necessarily runs BEFORE [`map_event`],
/// which is where `Release` is normally filtered. On unix `kind` is only populated under
/// `REPORT_EVENT_TYPES`, which [`App::into_stdout`] does not push — it pushes
/// `DISAMBIGUATE_ESCAPE_CODES` alone — so every unix event already arrives as `Press`.
pub(crate) fn is_escalate_chord(ev: &Event) -> bool {
    matches!(
        ev,
        Event::Key(k)
            if k.kind == KeyEventKind::Press
                && k.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('d'))
    )
}

/// Leave now, from the one context a wedged run loop cannot block.
///
/// Order is [`App::drain_and_restore`]'s followed by `signals.rs`'s repeat watcher's. The drain must
/// precede the restore — `stdin_is_drainable` (`drain.rs`) requires raw mode to still be on — and it
/// matters here more than anywhere: the user has just pressed the chord three times, and those bytes
/// would otherwise land in the parent shell. `try_lock`, never `lock`: this path must not be able to
/// block on a poisoned or contended mutex.
pub(crate) fn hard_exit_from_reader() -> ! {
    let _ = crate::drain::drain_stdin_before_exit();
    crate::panic_hook::restore_terminal_best_effort();
    // Cooked mode again, so stderr is readable rather than a staircase. This line is the whole
    // diagnostic yield of a wedge: it names the arm that never returned.
    if let Ok(slot) = ACTIVE_ARM.try_lock()
        && let Some((arm, since)) = *slot
    {
        eprintln!(
            "cyrup: run loop wedged in arm `{arm}` for {:?}",
            since.elapsed()
        );
    }
    cyrup_tools::kill_tracked_detached_children();
    // PERF-004 §3.5: the wedge escalation never reaches `runtime.dispose()`, so drain the session
    // fsync queue here. Nothing is lost without it — the bytes are already in the page cache —
    // but one flush round is ~200 µs and this is the last chance to make them power-loss durable.
    cyrup_session_svc::flush_session_writes();
    // `ShutdownSignal::Interrupt.exit_code()` — the shell's `128 + SIGINT` (`signals.rs`).
    std::process::exit(130)
}

/// How far up the escalation ladder the unserviced escalate chords have climbed.
///
/// There is no timer in here, deliberately. "Promote once a chord has gone unserviced for N
/// seconds" needs an N above the longest LEGITIMATE inline stall, and no such constant exists: a
/// session-lifecycle hook fan-out is N extensions × `DEFAULT_INVOKE_BUDGET` and a swap replay is M
/// messages × [`super::extension_render_impl::EXTENSION_RENDER_TIMEOUT`], both scaling with the
/// user's configuration. Every transition here is instead caused by a chord the run loop was then
/// shown not to have serviced, so the ladder cannot be climbed by a slow-but-working operation no
/// matter how long it takes.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Escalation {
    /// Nothing outstanding.
    Idle,
    /// `presses` chords have been forwarded, each at least [`PANIC_MIN_GAP`] after the last, with
    /// the run loop's serviced count stuck at `serviced` throughout. `last` is the previous counted
    /// chord, for the auto-repeat floor. At `presses == 2` the cooperative cancel has already fired.
    Armed {
        serviced: u64,
        last: std::time::Instant,
        presses: u32,
    },
}

impl Escalation {
    /// Keep the reader thread alive past `cancel` so the next chord can still reach
    /// [`Self::on_press`] — see the loop condition in [`crossterm_input_stream`].
    pub(crate) const fn holds_open(self) -> bool {
        !matches!(self, Self::Idle)
    }

    /// A chord was just read. The caller forwards it regardless of what this returns.
    pub(crate) fn on_press(self, cancel: &CancelToken) -> Self {
        // Checked here as well as in `tick`: a burst of chords can arrive between two reader
        // iterations, so a by-design block must disarm on the press path too, or the ladder could
        // be climbed from inside `$EDITOR`.
        if terminal_released() {
            return Self::Idle;
        }
        let serviced = input_serviced();
        let now = std::time::Instant::now();
        let Self::Armed {
            serviced: seen,
            last,
            presses,
        } = self
        else {
            // Chord #1: no evidence of anything yet. Arm and let the normal path handle it.
            return Self::Armed {
                serviced,
                last: now,
                presses: 1,
            };
        };
        // The loop drained input since the last chord: it IS servicing, and that chord already did
        // its HEAD job (cleared the editor, deleted a char, quit). Back to the bottom of the ladder.
        if seen != serviced {
            return Self::Armed {
                serviced,
                last: now,
                presses: 1,
            };
        }
        // Auto-repeat floor: a held key is a stream of genuine `Press` events on unix, so only
        // deliberately-spaced chords climb.
        if now.duration_since(last) < PANIC_MIN_GAP {
            return Self::Armed {
                serviced: seen,
                last,
                presses,
            };
        }
        let presses = presses.saturating_add(1);
        if presses >= PANIC_PRESSES {
            // Chord #3 against a loop that has serviced nothing since chord #1.
            hard_exit_from_reader();
        }
        // Chord #2: the cooperative half of `signals.rs`'s escalation. Unblocks the loop's `cancel`
        // arm if it can still run at all; if it cannot, chord #3 leaves.
        cancel.cancel();
        Self::Armed {
            serviced: seen,
            last: now,
            presses,
        }
    }

    /// One reader iteration with no chord. Disarms only — it can never promote.
    pub(crate) fn tick(self) -> Self {
        let Self::Armed { serviced, .. } = self else {
            return self;
        };
        // The loop deliberately released the terminal: `Ctrl+G` external editor
        // ([`App::edit_in_external_editor`], which `restore()`s and then blocks in
        // `Command::status()`) or `Ctrl+Z` suspend ([`App::suspend`], SIGTSTP until `fg`). Both stop
        // the loop servicing input for minutes BY DESIGN, and the chord belongs to the child that
        // now owns the tty.
        //
        // Read from the loop's own flag, NOT from `is_raw_mode_enabled()`: `suspend` re-enables raw
        // mode BEFORE it redraws and resumes servicing, so the probe would report "raw, and not
        // servicing" across the whole `fg` resume. [`TerminalReleased`] is cleared by its `Drop`,
        // i.e. only once the loop is genuinely back.
        if terminal_released() || input_serviced() != serviced {
            return Self::Idle;
        }
        self
    }
}

/// The terminal input stream: a reader thread that owns the tty and forwards [`InputEvent`]s over
/// an unbounded channel. Stops when `cancel` fires (see the loop condition for the one exception).
///
/// On unix the thread is [`crate::input::reader::TtyReader`] — cyrup's port of pi's `StdinBuffer`
/// (`stdin-buffer.ts` @v0.87.1) over raw `read(2)` chunks, decoding to the crossterm events every
/// consumer already takes. It replaces crossterm's event source, which could not be handed bytes
/// and so could not be taught the escape-sequence framing pi does (`TUI-045`, `TUI-046`,
/// `TUI-047`, `TUI-050`); see [`crate::input`]. The lone-`ESC` hold is
/// [`resolve_escape_timeout`], read once here as pi reads it once in `ProcessTerminal.start`.
///
/// While `$EDITOR` or a `Ctrl+Z` suspend owns the terminal ([`TerminalReleased`]) the thread does
/// not read the tty at all, as pi's `ProcessTerminal.stop` pauses stdin (`terminal.ts:462`). The
/// crossterm thread it replaces kept draining the tty under the editor.
///
/// If no terminal can be opened the stream ends at once, as the crossterm thread's first failed
/// `event::poll` ended it.
#[cfg(unix)]
pub fn crossterm_input_stream(cancel: CancelToken) -> EventStream<InputEvent> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<InputEvent>();
    let escape_timeout = resolve_escape_timeout(|k| std::env::var(k).ok());
    // Opened on the caller's thread: the `SIGWINCH` listener registers on the caller's runtime.
    if let Ok(mut reader) = crate::input::reader::TtyReader::open(escape_timeout) {
        std::thread::spawn(move || {
            let mut events: Vec<Event> = Vec::new();
            let mut escalation = Escalation::Idle;
            let may_read = || !terminal_released();
            // TUI-092 — NOT `while !cancel.is_cancelled()`. This thread FIRES that token
            // (`Escalation::on_press`), and that condition would retire the one reader still able
            // to see the NEXT chord at the exact moment that chord becomes the only way out.
            //
            // `!tx.is_closed()` is the LEADING conjunct, and that ordering is the whole safety
            // argument: the receiver is dropped when `App::run` returns, so a real SIGTERM/SIGHUP
            // teardown (`signals.rs` → the biased cancel arm → `drain_and_restore` → return) still
            // ends this thread even with an escalation armed. `holds_open()` can only extend the
            // reader's life across the window where teardown has been REQUESTED but has not
            // COMPLETED — which is precisely the window a wedged teardown must remain escapable in.
            'reader: while !tx.is_closed() && (!cancel.is_cancelled() || escalation.holds_open()) {
                let open = reader.pump(INPUT_POLL_INTERVAL, &may_read, &mut events);
                for ev in events.drain(..) {
                    // TUI-092 — counted on the decoded event, before the send below, which starts
                    // failing the moment the run loop breaks and drops the receiver. A `Ctrl+C`
                    // byte is never held by the framer, so nothing delays the escape hatch.
                    if is_escalate_chord(&ev) {
                        escalation = escalation.on_press(&cancel);
                    }
                    if let Some(mapped) = map_event(ev)
                        && tx.send(mapped).is_err()
                    {
                        break 'reader;
                    }
                }
                // TUI-092 — the disarm tick, on EVERY iteration (at most one
                // `INPUT_POLL_INTERVAL` apart). It never promotes; it only drops a stale ladder once
                // the loop resumes servicing input or announces a by-design block, so a chord
                // pressed before a `Ctrl+Z` is not still armed minutes later.
                escalation = escalation.tick();
                if !matches!(open, Ok(true)) {
                    break;
                }
            }
        });
    }
    Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
}

/// Windows: a terminal input stream backed by a blocking `event::read()` reader thread (the async crossterm
/// `EventStream` feature is not enabled in this build; arch-10 §5 fallback). Maps `crossterm::Event`
/// to [`InputEvent`] and forwards over an unbounded channel; stops when `cancel` fires.
///
/// Every event passes through two machines, in this order.
///
/// [`EscapeReassembler`] first — the cyrup half of Pi's `tui/src/stdin-buffer.ts`. crossterm emits a
/// bare `Key(Esc)` and clears its buffer whenever a `read(2)` that did not fill its 1,024-byte
/// buffer ends on `0x1B` (`parse.rs:34-41`), so an escape sequence split at the `ESC` byte reaches
/// the app as `Esc` plus its tail typed as literal characters — and that `Esc` aborts a running turn
/// (`TUI-045`, reproduced live 2026-08-13). The reassembler puts the CSI/SS3 sequence back together
/// and emits the key that was actually pressed.
///
/// Then [`StrayReplyFilter`], the port of Pi's `consumeOsc11BackgroundResponse` guard
/// (`tui/src/tui.ts:788-794`): a terminal that answers the boot-time OSC 11 probe *after*
/// [`crate::terminal_query`]'s deadline would otherwise have its reply decoded by crossterm into
/// keystrokes and typed into the prompt. The filter only ever removes a complete, terminated OSC 11
/// frame; anything it holds is replayed the moment the match fails or the input goes idle — see that
/// module's safety contract.
///
/// Both hold, so both are flushed on the *same* idle tick and in the same order: a lone `Escape`
/// costs one escape timeout in total ([`resolve_escape_timeout`], read once here as Pi reads it
/// once in `ProcessTerminal.start`), not one per machine.
#[cfg(not(unix))]
pub fn crossterm_input_stream(cancel: CancelToken) -> EventStream<InputEvent> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<InputEvent>();
    let escape_timeout = resolve_escape_timeout(|k| std::env::var(k).ok());
    std::thread::spawn(move || {
        let mut reassembler = EscapeReassembler::new();
        let mut filter = StrayReplyFilter::new();
        let mut reassembled: Vec<Event> = Vec::new();
        let mut released: Vec<Event> = Vec::new();
        let mut escalation = Escalation::Idle;
        // TUI-092 — NOT `while !cancel.is_cancelled()`. This thread now FIRES that token
        // (`Escalation::on_press`), and the old condition would retire the one reader still able to
        // see the NEXT chord at the exact moment that chord becomes the only way out.
        //
        // `!tx.is_closed()` is the LEADING conjunct, and that ordering is the whole safety
        // argument: the receiver is dropped when `App::run` returns, so a real SIGTERM/SIGHUP
        // teardown (`signals.rs` → the biased cancel arm → `drain_and_restore` → return) still ends
        // this thread even with an escalation armed. `holds_open()` can only extend the reader's
        // life across the window where teardown has been REQUESTED but has not COMPLETED — which is
        // precisely the window a wedged teardown must remain escapable in.
        'reader: while !tx.is_closed() && (!cancel.is_cancelled() || escalation.holds_open()) {
            let wait = reader_poll_wait(&reassembler, &filter, escape_timeout);
            match event::poll(wait) {
                Ok(true) => match event::read() {
                    Ok(ev) => {
                        // TUI-092 — recognised BEFORE `EscapeReassembler`/`StrayReplyFilter`: a
                        // machine mid-hold would otherwise delay the one chord that exists to
                        // escape a wedge by up to the escape timeout, and could swallow it into
                        // a reassembled sequence. Read-only on a borrow; the event is pushed below
                        // unchanged, so neither machine's state is disturbed. It must also run
                        // before the `tx.send` at the foot of this loop, which starts failing the
                        // moment the run loop breaks and drops the receiver.
                        if is_escalate_chord(&ev) {
                            escalation = escalation.on_press(&cancel);
                        }
                        reassembler.push(ev, &mut reassembled);
                        for ev in reassembled.drain(..) {
                            filter.push(ev, &mut released);
                        }
                    }
                    Err(_) => break,
                },
                // Idle: nothing more is coming, so release whatever either machine is holding.
                Ok(false) => {
                    reassembler.flush(&mut reassembled);
                    for ev in reassembled.drain(..) {
                        filter.push(ev, &mut released);
                    }
                    filter.flush(&mut released);
                }
                Err(_) => break,
            }
            // TUI-092 — the disarm tick, on EVERY iteration (at most one `INPUT_POLL_INTERVAL`
            // apart). It never promotes; it only drops a stale ladder once the loop resumes
            // servicing input or announces a by-design block, so a chord pressed before a `Ctrl+Z`
            // is not still armed minutes later.
            escalation = escalation.tick();
            for ev in released.drain(..) {
                if let Some(mapped) = map_event(ev)
                    && tx.send(mapped).is_err()
                {
                    break 'reader;
                }
            }
        }
    });
    Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
}

/// Map a crossterm event to our [`InputEvent`] (filtering non-press key kinds).
///
/// Key presses first go through [`crate::native_modifiers::rescue_native_shift_enter`] — upstream's
/// `ProcessTerminal.forwardInputSequence` normalization (v0.83.0 `tui/src/terminal.ts:305-312`).
/// On Apple Terminal (and, since v0.84.1, the Windows console) a bare `\r` is all the terminal
/// sends for BOTH `Enter` and `Shift+Enter`, so the modifier is recovered from the live keyboard
/// state instead of the byte stream. Everywhere else the event passes through untouched.
pub(crate) fn map_event(ev: Event) -> Option<InputEvent> {
    // `TERM_PROGRAM` is read only for the one key that can need it (a bare `Enter`), so no other
    // keystroke pays for a `getenv`.
    let term_program = match &ev {
        Event::Key(k)
            if k.code == ratatui::crossterm::event::KeyCode::Enter
                && k.modifiers == ratatui::crossterm::event::KeyModifiers::NONE =>
        {
            std::env::var("TERM_PROGRAM").ok()
        }
        _ => None,
    };
    map_event_on(
        ev,
        crate::native_modifiers::host_platform(),
        term_program.as_deref(),
        crate::native_modifiers::is_native_modifier_pressed,
    )
}

/// [`map_event`] with `process.platform`, `process.env.TERM_PROGRAM` and the native modifier helper
/// lifted into parameters, so the Apple-Terminal / Windows-console branch of the Shift+Enter rescue
/// is reachable from a test on any host (the same pattern as
/// [`crate::image::detect_capabilities_on_platform`]).
///
/// The one input NOT lifted is the mouse gate: `Event::Mouse` consults the process-global set by
/// `altscreen::mouse::MouseSetup` (ADR-0005 §B-4). It is deliberately not a parameter, because it
/// is not a host fact a test would want to vary — it records whether THIS process wrote a mouse
/// enable sequence, it is `false` until the alternate-screen renderer writes one, and every other
/// arm is unaffected by it.
pub(crate) fn map_event_on(
    ev: Event,
    platform: &str,
    term_program: Option<&str>,
    probe: impl Fn(crate::native_modifiers::ModifierKey) -> bool,
) -> Option<InputEvent> {
    match ev {
        Event::Key(k) if !matches!(k.kind, KeyEventKind::Release) => Some(InputEvent::Key(
            crate::native_modifiers::rescue_native_shift_enter(k, platform, term_program, probe),
        )),
        Event::Key(_) => None,
        Event::Paste(s) => Some(InputEvent::Paste(s)),
        Event::Resize(w, h) => Some(InputEvent::Resize(w, h)),
        Event::FocusGained => Some(InputEvent::FocusGained),
        Event::FocusLost => Some(InputEvent::FocusLost),
        // ADR-0005 §B-4. This was an unconditional `=> None`, correct while nothing in the process
        // ever asked a terminal for mouse reports. The alternate-screen renderer does ask
        // (`altscreen/mouse.rs`, pi `tui-alt-screen.ts:293`), so the discard moves behind the gate
        // that knows whether it did — and stays a discard for the whole of every regular-mode
        // session, where the gate is never armed.
        //
        // §B-14 NOTE — still a discard in BOTH modes, and the remaining blocker is not here.
        // `AltScreen::handle_mouse` (the §B-3 dispatcher over `wheel`, `scrollbar_drag` and
        // `selection`, in upstream's `:564-575` order) exists and is complete; what does not exist
        // is a way to carry a `MouseEvent` to it, because [`InputEvent`] has no `Mouse` variant.
        // The follow-up is two lines and neither is in this file: `Mouse(MouseEvent)` on the enum
        // in `component.rs`, and `Some(InputEvent::Mouse(ev))` in place of
        // `map_reader_event`'s armed-branch `None` (`altscreen/mouse.rs:217-223`, whose own doc
        // says the same). Until then wheel scrolling, scrollbar drags, text selection, the
        // `PointerOutcome::Copy` clipboard write and the `PointerOutcome::Paste` editor insert are
        // all unreachable in fullscreen.
        Event::Mouse(m) => crate::altscreen::mouse::map_reader_event(m),
    }
}

#[cfg(test)]
mod tests {
    //! TUI-106 — the lone-`ESC` hold. These live INLINE because `input_reader` is a private module
    //! of `crate::app` and its items are not re-exported to `crate::tests`.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::*;
    #[cfg(not(unix))]
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// An env lookup over a fixed table (missing keys ⇒ `None`, exactly like `std::env::var`).
    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[cfg(not(unix))]
    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// Pi `resolveEscapeTimeoutMs`'s three outcomes: override, SSH, local.
    #[test]
    fn escape_timeout_is_widened_under_ssh_and_overridable() {
        assert_eq!(resolve_escape_timeout(env_of(&[])), LOCAL_ESCAPE_TIMEOUT);
        for var in ["SSH_CONNECTION", "SSH_TTY"] {
            assert_eq!(
                resolve_escape_timeout(env_of(&[(var, "10.0.0.1 5000 10.0.0.2 22")])),
                Duration::from_millis(100),
                "{var} set ⇒ 100 ms"
            );
            assert_eq!(
                resolve_escape_timeout(env_of(&[(var, "")])),
                LOCAL_ESCAPE_TIMEOUT,
                "an empty {var} is falsy in JS"
            );
        }
        // A positive override wins over SSH.
        assert_eq!(
            resolve_escape_timeout(env_of(&[
                (ENV_ESC_TIMEOUT, "250"),
                ("SSH_TTY", "/dev/pts/3")
            ])),
            Duration::from_millis(250)
        );
        // Pi reads only its own spelling; cyrup reads only `CYRUP_`.
        assert_eq!(
            resolve_escape_timeout(env_of(&[("PI_TUI_ESC_TIMEOUT", "250")])),
            LOCAL_ESCAPE_TIMEOUT
        );
    }

    /// `Number(...)`'s parse, then `> 0 && isFinite`: a non-positive or unparseable override falls
    /// through to the SSH / local choice rather than disabling the hold.
    #[test]
    fn escape_timeout_override_parses_like_js_number() {
        let ssh = |v: &'static str| {
            resolve_escape_timeout(env_of(&[(ENV_ESC_TIMEOUT, v), ("SSH_TTY", "/dev/pts/3")]))
        };
        for fallthrough in ["", "0", "-5", "abc", "12ms", "Infinity", "NaN", "0x"] {
            assert_eq!(ssh(fallthrough), SSH_ESCAPE_TIMEOUT, "{fallthrough:?}");
        }
        assert_eq!(ssh(" 40\n"), Duration::from_millis(40));
        assert_eq!(ssh("1e2"), Duration::from_millis(100));
        assert_eq!(ssh("0x40"), Duration::from_millis(64));
        assert_eq!(ssh("12.5"), Duration::from_micros(12_500));
        // Node's `setTimeout` turns a delay outside `1..=2^31-1` into 1 ms.
        assert_eq!(ssh("0.5"), Duration::from_millis(1));
        assert_eq!(ssh("1e12"), Duration::from_millis(1));
    }

    /// The unix reader's hold: the escape timeout applies to a lone held `ESC` and to nothing else;
    /// any other partial sequence waits pi's 50 ms sequence timeout (`stdin-buffer.ts:388`).
    #[cfg(unix)]
    #[test]
    fn only_a_lone_held_escape_waits_the_escape_timeout() {
        use crate::input::frame::DEFAULT_SEQUENCE_TIMEOUT;
        use crate::input::reader::ByteDecoder;
        let timeout = SSH_ESCAPE_TIMEOUT;
        let mut d = ByteDecoder::new(timeout);
        let mut out = Vec::new();
        let t0 = std::time::Instant::now();
        assert_eq!(d.deadline(), None);
        d.feed(b"\x1b", t0, &mut out);
        assert_eq!(d.deadline(), Some(t0 + timeout));
        // `ESC [` is a sequence prefix, not a lone `ESC`: the sequence timeout.
        d.feed(b"[", t0, &mut out);
        assert_eq!(d.deadline(), Some(t0 + DEFAULT_SEQUENCE_TIMEOUT));
        assert!(out.is_empty());
    }

    /// The reader's poll: the escape timeout applies to a lone held `ESC` and to nothing else.
    #[cfg(not(unix))]
    #[test]
    fn only_a_lone_held_escape_waits_the_escape_timeout_windows() {
        let timeout = SSH_ESCAPE_TIMEOUT;
        let mut r = EscapeReassembler::new();
        let f = StrayReplyFilter::new();
        let mut out = Vec::new();
        assert_eq!(reader_poll_wait(&r, &f, timeout), INPUT_POLL_INTERVAL);
        r.push(key(KeyCode::Esc), &mut out);
        assert_eq!(reader_poll_wait(&r, &f, timeout), timeout);
        // `ESC [` is a sequence prefix, not a lone `ESC`: the short hold.
        r.push(key(KeyCode::Char('[')), &mut out);
        assert_eq!(reader_poll_wait(&r, &f, timeout), HELD_FLUSH_INTERVAL);
        assert!(out.is_empty());
    }

    /// `REPRO-LOG.md`'s `TUI-045` split (`1b`, 60 ms, `5b 41`) under SSH, driven through the same
    /// two machines and the same wait the reader thread uses. `event::poll(wait)` returns `false`
    /// (idle ⇒ flush) exactly when the next byte is further away than `wait`. The unix byte reader's
    /// version is `tests::input_pipeline::a_split_arrow_over_ssh_reassembles_across_a_60ms_gap`.
    #[cfg(not(unix))]
    #[test]
    fn a_split_arrow_over_ssh_reassembles_across_a_60ms_gap() {
        let deliver = |escape_timeout: Duration| {
            let mut r = EscapeReassembler::new();
            let mut f = StrayReplyFilter::new();
            let (mut mid, mut out) = (Vec::new(), Vec::new());
            let gap = Duration::from_millis(60);
            for (before, ev) in [
                (Duration::ZERO, key(KeyCode::Esc)),
                (gap, key(KeyCode::Char('['))),
                (
                    Duration::ZERO,
                    Event::Key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)),
                ),
            ] {
                if before >= reader_poll_wait(&r, &f, escape_timeout) {
                    r.flush(&mut mid);
                    for ev in mid.drain(..) {
                        f.push(ev, &mut out);
                    }
                    f.flush(&mut out);
                }
                r.push(ev, &mut mid);
                for ev in mid.drain(..) {
                    f.push(ev, &mut out);
                }
            }
            r.flush(&mut mid);
            for ev in mid.drain(..) {
                f.push(ev, &mut out);
            }
            f.flush(&mut out);
            out
        };
        let ssh = resolve_escape_timeout(env_of(&[("SSH_CONNECTION", "a 1 b 22")]));
        assert_eq!(deliver(ssh), vec![key(KeyCode::Up)], "one Up, no Escape");
        let local = resolve_escape_timeout(env_of(&[]));
        assert_eq!(
            deliver(local)[0],
            key(KeyCode::Esc),
            "a local 20 ms hold still releases the ESC across a 60 ms gap"
        );
    }
}
