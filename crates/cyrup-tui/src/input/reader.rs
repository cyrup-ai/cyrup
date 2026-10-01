//! The unix fd loop: `poll(2)` + `read(2)` on the terminal, framed by [`StdinBuffer`] and decoded
//! by [`decode`] — the part of pi's `ProcessTerminal` that feeds `StdinBuffer`
//! (`terminal.ts:173-259` @v0.87.1), with crossterm's event source gone.
//!
//! - **Which fd.** Stdin if it is a terminal, else `/dev/tty` — crossterm's own choice
//!   (`tty_fd`), so nothing changes for a user whose stdin is redirected.
//! - **Reads keep their boundaries.** Each `read(2)` of up to 1 KiB is one [`StdinBuffer::push`],
//!   so the framer sees the same chunks pi's `data` events carry.
//! - **Timeouts.** The poll never outlasts [`StdinBuffer::deadline`], so a held `ESC` becomes
//!   the Escape key on time (pi's `setTimeout`, `stdin-buffer.ts:387-396`).
//! - **Resize.** `SIGWINCH` wakes the poll through a socket a tokio signal task writes to, and
//!   becomes [`Event::Resize`] with the size crossterm's `terminal::size` reports — what crossterm's
//!   own `SIGWINCH` handling emitted.
//! - **Released terminal.** While `$EDITOR` or a `Ctrl+Z` suspend owns the tty the caller says so,
//!   and the tty is neither polled nor read: its bytes belong to the program in the foreground.
//!   pi stops reading the same way (`process.stdin.pause()` in `ProcessTerminal.stop`,
//!   `terminal.ts:462`). crossterm's reader thread had no such switch and kept draining the tty
//!   under the editor (`tests::input_pty::a_released_terminal_is_left_to_the_program_that_owns_it`).

use std::collections::VecDeque;
use std::io::{self, IsTerminal, Read};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::Event;
use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::io::Errno;

use super::decode::{Decoded, decode};
use super::frame::{Frame, StdinBuffer};

/// The read size — crossterm's `TTY_BUFFER_SIZE`, and more than any one keystroke.
const CHUNK: usize = 1024;

/// The terminal the reader owns.
enum Tty {
    Stdin(io::Stdin),
    Dev(std::fs::File),
    #[cfg(test)]
    Fd(std::os::fd::OwnedFd),
}

impl AsFd for Tty {
    fn as_fd(&self) -> BorrowedFd<'_> {
        match self {
            Self::Stdin(s) => s.as_fd(),
            Self::Dev(f) => f.as_fd(),
            #[cfg(test)]
            Self::Fd(fd) => fd.as_fd(),
        }
    }
}

/// The read end of the `SIGWINCH` wake-up, and the switch that stops the task writing to it.
struct Winch {
    wake: UnixStream,
    _stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Winch {
    /// Listen for `SIGWINCH` on the current tokio runtime. `None` outside a runtime, or if the
    /// signal cannot be registered — the reader then works without resize events, which is what a
    /// crossterm reader that failed to register its handler did too.
    fn spawn() -> Option<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        let handle = tokio::runtime::Handle::try_current().ok()?;
        let (wake, notify) = UnixStream::pair().ok()?;
        wake.set_nonblocking(true).ok()?;
        notify.set_nonblocking(true).ok()?;
        // Registered here, on the caller's thread, so a resize straight after `open` is not missed.
        let mut sig = {
            let _enter = handle.enter();
            signal(SignalKind::window_change()).ok()?
        };
        let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
        handle.spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    got = sig.recv() => {
                        if got.is_none() {
                            break;
                        }
                        // A full socket already holds a pending wake-up; nothing is lost.
                        let _ = std::io::Write::write(&mut &notify, &[0]);
                    }
                }
            }
        });
        Some(Self {
            wake,
            _stop: Some(stop),
        })
    }

    /// Consume every pending wake-up. Several signals between two polls are one resize.
    fn drain(&self) {
        let mut sink = [0u8; 64];
        while matches!((&self.wake).read(&mut sink), Ok(n) if n > 0) {}
    }
}

/// Reads the terminal and turns its bytes into crossterm events.
pub(crate) struct TtyReader {
    tty: Tty,
    winch: Option<Winch>,
    bytes: ByteDecoder,
    /// Events decoded but not yet handed out by [`Self::next_event_timeout`].
    ready: VecDeque<Event>,
    /// The terminal size a resize reports.
    size: fn() -> io::Result<(u16, u16)>,
}

impl TtyReader {
    /// The reader for this process's terminal, holding a lone `ESC` for `escape_timeout`.
    pub(crate) fn open(escape_timeout: Duration) -> io::Result<Self> {
        let stdin = io::stdin();
        let tty = if stdin.is_terminal() {
            Tty::Stdin(stdin)
        } else {
            Tty::Dev(
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open("/dev/tty")?,
            )
        };
        Ok(Self::with(
            tty,
            Winch::spawn(),
            escape_timeout,
            ratatui::crossterm::terminal::size,
        ))
    }

    fn with(
        tty: Tty,
        winch: Option<Winch>,
        escape_timeout: Duration,
        size: fn() -> io::Result<(u16, u16)>,
    ) -> Self {
        Self {
            tty,
            winch,
            bytes: ByteDecoder::new(escape_timeout),
            ready: VecDeque::new(),
            size,
        }
    }

    /// A reader over any tty fd, with `wake` standing in for the `SIGWINCH` task.
    #[cfg(test)]
    pub(crate) fn for_test(
        fd: std::os::fd::OwnedFd,
        wake: Option<UnixStream>,
        escape_timeout: Duration,
        size: fn() -> io::Result<(u16, u16)>,
    ) -> Self {
        let winch = wake.map(|wake| {
            let _ = wake.set_nonblocking(true);
            Winch { wake, _stop: None }
        });
        Self::with(Tty::Fd(fd), winch, escape_timeout, size)
    }

    /// Wait at most `max_wait` — less if a held sequence falls due first — for the terminal or a
    /// resize, then append every event that produced to `out`.
    ///
    /// `may_read` is asked before the tty is polled and again before it is read: while it answers
    /// `false` the tty is left alone, and only resizes and the held sequence's timeout are served.
    /// The second question narrows the window in which a byte meant for `$EDITOR` could be taken
    /// to the instant between the check and the `read(2)`.
    ///
    /// `Ok(false)` once the terminal is gone (end of file, or a hangup).
    pub(crate) fn pump(
        &mut self,
        max_wait: Duration,
        may_read: &dyn Fn() -> bool,
        out: &mut Vec<Event>,
    ) -> io::Result<bool> {
        self.bytes.flush_due(Instant::now(), out);
        let wait = match self.bytes.deadline() {
            Some(due) => due.saturating_duration_since(Instant::now()).min(max_wait),
            None => max_wait,
        };
        let read_tty = may_read();
        let (tty_ready, winch_ready) = {
            let mut fds = Vec::with_capacity(2);
            if read_tty {
                fds.push(PollFd::new(&self.tty, PollFlags::IN));
            }
            if let Some(winch) = &self.winch {
                fds.push(PollFd::new(&winch.wake, PollFlags::IN));
            }
            match rustix::event::poll(&mut fds, Some(&timespec(wait))) {
                Ok(_) => {}
                // A signal (a `SIGWINCH` among them) interrupted the wait; its wake-up is read on
                // the next round.
                Err(Errno::INTR) => return Ok(true),
                Err(e) => return Err(e.into()),
            }
            let fired = |fd: Option<&PollFd<'_>>| fd.is_some_and(|fd| !fd.revents().is_empty());
            let (tty_fd, winch_fd) = if read_tty {
                (fds.first(), fds.get(1))
            } else {
                (None, fds.first())
            };
            (fired(tty_fd), fired(winch_fd))
        };
        if winch_ready && let Some(winch) = &self.winch {
            winch.drain();
            if let Ok((cols, rows)) = (self.size)() {
                out.push(Event::Resize(cols, rows));
            }
        }
        if tty_ready && may_read() {
            let mut chunk = [0u8; CHUNK];
            match rustix::io::read(&self.tty, &mut chunk[..]) {
                Ok(0) => return Ok(false),
                Ok(n) => self
                    .bytes
                    .feed(chunk.get(..n).unwrap_or_default(), Instant::now(), out),
                Err(Errno::INTR | Errno::AGAIN) => {}
                Err(e) => return Err(e.into()),
            }
        }
        self.bytes.flush_due(Instant::now(), out);
        Ok(true)
    }

    /// Wait at most `wait` for the next event; `Ok(None)` if it elapsed with nothing to hand out.
    ///
    /// The startup selector's `event::poll` + `event::read`: it lets a loop that also serves a
    /// channel (the `--resume` picker's streamed listing) come back and look at it.
    pub(crate) fn next_event_timeout(&mut self, wait: Duration) -> io::Result<Option<Event>> {
        if let Some(ev) = self.ready.pop_front() {
            return Ok(Some(ev));
        }
        let mut out = Vec::new();
        if !self.pump(wait, &|| true, &mut out)? {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        self.ready.extend(out);
        Ok(self.ready.pop_front())
    }

    /// Block until the next event.
    #[cfg(test)]
    pub(crate) fn next_event(&mut self) -> io::Result<Event> {
        loop {
            if let Some(ev) = self.next_event_timeout(Duration::from_secs(3600))? {
                return Ok(ev);
            }
        }
    }
}

/// The pure half of the reader: bytes and the clock in, events out. [`TtyReader`] drives it from
/// the fd; tests drive it directly with injected time.
#[derive(Debug)]
pub(crate) struct ByteDecoder {
    buffer: StdinBuffer,
    frames: Vec<Frame>,
}

impl ByteDecoder {
    /// A decoder holding a lone `ESC` for `escape_timeout`.
    pub(crate) fn new(escape_timeout: Duration) -> Self {
        Self {
            buffer: StdinBuffer::new(escape_timeout),
            frames: Vec::new(),
        }
    }

    /// When a held sequence falls due, if one is held.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.buffer.deadline()
    }

    /// One `read(2)` chunk, received at `now`.
    pub(crate) fn feed(&mut self, bytes: &[u8], now: Instant, out: &mut Vec<Event>) {
        self.buffer.push(bytes, now, &mut self.frames);
        self.decode_frames(out);
    }

    /// Flush the held sequence if its timeout has passed by `now`.
    pub(crate) fn flush_due(&mut self, now: Instant, out: &mut Vec<Event>) {
        if self.buffer.deadline().is_some_and(|due| due <= now) {
            self.buffer.flush(&mut self.frames);
            self.decode_frames(out);
        }
    }

    fn decode_frames(&mut self, out: &mut Vec<Event>) {
        for frame in self.frames.drain(..) {
            if let Decoded::Event(ev) = decode(frame) {
                out.push(ev);
            }
        }
    }
}

fn timespec(d: Duration) -> Timespec {
    Timespec {
        tv_sec: i64::try_from(d.as_secs()).unwrap_or(i64::MAX) as _,
        tv_nsec: i64::from(d.subsec_nanos()) as _,
    }
}

#[cfg(test)]
#[path = "reader_tests.rs"]
mod tests;
