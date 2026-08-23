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
    let mut out = io::stdout();
    let _ = out.write_all(format!("\x1b]0;{safe}\x07").as_bytes());
    let _ = out.flush();
}

/// How long the reader thread idles between `event::poll` rounds when nothing is held.
pub(crate) const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The much shorter poll used while [`StrayReplyFilter`] is holding events. A held opener (a bare
/// `Esc`, or `Alt+]`) is released after at most this long, so a real `Escape` press costs one
/// imperceptible tick rather than a full [`INPUT_POLL_INTERVAL`] — the standard escape-timeout
/// trade every terminal app makes to tell `ESC` from an escape *sequence*.
pub(crate) const HELD_FLUSH_INTERVAL: Duration = Duration::from_millis(20);

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
pub(crate) static INPUT_SERVICED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
pub(crate) static TERMINAL_RELEASED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
pub(crate) static OVER_BUDGET_ARM: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(None);

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
        eprintln!("cyrup: run loop wedged in arm `{arm}` for {:?}", since.elapsed());
    }
    cyrup_tools::kill_tracked_detached_children();
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
    Armed { serviced: u64, last: std::time::Instant, presses: u32 },
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
        let Self::Armed { serviced: seen, last, presses } = self else {
            // Chord #1: no evidence of anything yet. Arm and let the normal path handle it.
            return Self::Armed { serviced, last: now, presses: 1 };
        };
        // The loop drained input since the last chord: it IS servicing, and that chord already did
        // its HEAD job (cleared the editor, deleted a char, quit). Back to the bottom of the ladder.
        if seen != serviced {
            return Self::Armed { serviced, last: now, presses: 1 };
        }
        // Auto-repeat floor: a held key is a stream of genuine `Press` events on unix, so only
        // deliberately-spaced chords climb.
        if now.duration_since(last) < PANIC_MIN_GAP {
            return Self::Armed { serviced: seen, last, presses };
        }
        let presses = presses.saturating_add(1);
        if presses >= PANIC_PRESSES {
            // Chord #3 against a loop that has serviced nothing since chord #1.
            hard_exit_from_reader();
        }
        // Chord #2: the cooperative half of `signals.rs`'s escalation. Unblocks the loop's `cancel`
        // arm if it can still run at all; if it cannot, chord #3 leaves.
        cancel.cancel();
        Self::Armed { serviced: seen, last: now, presses }
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

/// A terminal input stream backed by a blocking `event::read()` reader thread (the async crossterm
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
/// costs one [`HELD_FLUSH_INTERVAL`] in total, not one per machine.
pub fn crossterm_input_stream(cancel: CancelToken) -> EventStream<InputEvent> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<InputEvent>();
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
            let wait = if reassembler.is_holding() || filter.is_holding() {
                HELD_FLUSH_INTERVAL
            } else {
                INPUT_POLL_INTERVAL
            };
            match event::poll(wait) {
                Ok(true) => match event::read() {
                    Ok(ev) => {
                        // TUI-092 — recognised BEFORE `EscapeReassembler`/`StrayReplyFilter`: a
                        // machine mid-hold would otherwise delay the one chord that exists to
                        // escape a wedge by up to `HELD_FLUSH_INTERVAL`, and could swallow it into
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
    map_event_on(ev, crate::native_modifiers::host_platform(), term_program.as_deref(), |k| {
        crate::native_modifiers::is_native_modifier_pressed(k)
    })
}

/// [`map_event`] with `process.platform`, `process.env.TERM_PROGRAM` and the native modifier helper
/// lifted into parameters, so the Apple-Terminal / Windows-console branch of the Shift+Enter rescue
/// is reachable from a test on any host (the same pattern as
/// [`crate::image::detect_capabilities_on_platform`]).
pub(crate) fn map_event_on(
    ev: Event,
    platform: &str,
    term_program: Option<&str>,
    probe: impl Fn(crate::native_modifiers::ModifierKey) -> bool,
) -> Option<InputEvent> {
    match ev {
        Event::Key(k) if !matches!(k.kind, KeyEventKind::Release) => {
            Some(InputEvent::Key(crate::native_modifiers::rescue_native_shift_enter(
                k,
                platform,
                term_program,
                probe,
            )))
        }
        Event::Key(_) => None,
        Event::Paste(s) => Some(InputEvent::Paste(s)),
        Event::Resize(w, h) => Some(InputEvent::Resize(w, h)),
        Event::FocusGained => Some(InputEvent::FocusGained),
        Event::FocusLost => Some(InputEvent::FocusLost),
        Event::Mouse(_) => None,
    }
}

