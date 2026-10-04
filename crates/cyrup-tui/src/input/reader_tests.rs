//! [`TtyReader`] over an in-process pty: the slave in raw mode is the reader's tty, the test writes
//! to the master. The whole-process behaviours (a kernel `SIGWINCH`, the production stream, the
//! escalation exit) are in `crate::tests::input_pty`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

use crate::input::frame::DEFAULT_ESCAPE_TIMEOUT;

/// A raw-mode pty: the reader gets the slave, the test keeps the master.
fn pty() -> (std::fs::File, std::os::fd::OwnedFd) {
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    // rustix offers `OpenptFlags::CLOEXEC` only where `posix_openpt` accepts `O_CLOEXEC` (Linux,
    // FreeBSD, NetBSD); elsewhere (macOS) the flag is set right after opening.
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "netbsd"
    ))]
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC).unwrap();
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "netbsd"
    )))]
    let master = {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC).unwrap();
        master
    };
    grantpt(&master).unwrap();
    unlockpt(&master).unwrap();
    let path = ptsname(&master, Vec::new()).unwrap();
    let slave = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(rustix::fs::OFlags::NOCTTY.bits() as i32)
        .open(path.to_str().unwrap())
        .unwrap();
    let mut t = rustix::termios::tcgetattr(&slave).unwrap();
    t.make_raw();
    rustix::termios::tcsetattr(&slave, rustix::termios::OptionalActions::Now, &t).unwrap();
    (std::fs::File::from(master), slave.into())
}

fn fixed_size() -> io::Result<(u16, u16)> {
    Ok((132, 43))
}

fn reader(slave: std::os::fd::OwnedFd, wake: Option<UnixStream>) -> TtyReader {
    TtyReader::for_test(slave, wake, DEFAULT_ESCAPE_TIMEOUT, fixed_size)
}

/// Pump until `n` events have arrived or two seconds pass.
fn pump_for(r: &mut TtyReader, n: usize, may_read: &dyn Fn() -> bool) -> Vec<Event> {
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    while out.len() < n && Instant::now() < deadline {
        assert!(
            r.pump(Duration::from_millis(20), may_read, &mut out)
                .unwrap()
        );
    }
    out
}

fn key(code: KeyCode, mods: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, mods))
}

#[test]
fn bytes_written_to_the_terminal_come_back_as_events() {
    let (mut master, slave) = pty();
    let mut r = reader(slave, None);
    master
        .write_all("hé\x1b[A\x1bP>|xterm\x1b\\\r".as_bytes())
        .unwrap();
    assert_eq!(
        pump_for(&mut r, 4, &|| true),
        [
            key(KeyCode::Char('h'), KeyModifiers::NONE),
            key(KeyCode::Char('é'), KeyModifiers::NONE),
            key(KeyCode::Up, KeyModifiers::NONE),
            key(KeyCode::Enter, KeyModifiers::NONE),
        ]
    );
}

/// The poll is bounded by the held sequence's deadline: a lone `ESC` becomes Escape after the
/// escape timeout even though the caller asked to wait a whole second.
#[test]
fn a_lone_escape_is_released_by_the_deadline_not_the_callers_wait() {
    let (mut master, slave) = pty();
    let mut r = reader(slave, None);
    master.write_all(b"\x1b").unwrap();
    let start = Instant::now();
    let mut out = Vec::new();
    while out.is_empty() && start.elapsed() < Duration::from_secs(5) {
        r.pump(Duration::from_secs(1), &|| true, &mut out).unwrap();
    }
    assert_eq!(out, [key(KeyCode::Esc, KeyModifiers::NONE)]);
    assert!(
        start.elapsed() < Duration::from_millis(900),
        "released after {:?}, not after the 1 s wait",
        start.elapsed()
    );
}

/// While the terminal is released the reader leaves its bytes where they are — for `$EDITOR` —
/// and picks up whatever is still there once it has the terminal back.
#[test]
fn a_released_terminal_is_not_read() {
    let (mut master, slave) = pty();
    let probe = slave.try_clone().unwrap();
    let mut r = reader(slave, None);
    master.write_all(b"xyz").unwrap();
    assert!(pump_for(&mut r, 1, &|| false).is_empty());
    // The bytes are still queued on the tty for whoever owns it.
    let mut fds = [PollFd::new(&probe, PollFlags::IN)];
    assert_eq!(
        rustix::event::poll(&mut fds, Some(&timespec(Duration::ZERO))).unwrap(),
        1,
        "the typed bytes must still be unread"
    );
    assert_eq!(
        pump_for(&mut r, 3, &|| true),
        [
            key(KeyCode::Char('x'), KeyModifiers::NONE),
            key(KeyCode::Char('y'), KeyModifiers::NONE),
            key(KeyCode::Char('z'), KeyModifiers::NONE),
        ]
    );
}

/// A wake-up on the resize socket is one [`Event::Resize`] with the size the size function reports,
/// however many signals arrived — and it is served while the terminal is released too.
#[test]
fn a_winch_wakeup_is_one_resize_event() {
    let (_master, slave) = pty();
    let (wake, notify) = UnixStream::pair().unwrap();
    let mut r = reader(slave, Some(wake));
    (&notify).write_all(&[0, 0, 0]).unwrap();
    assert_eq!(pump_for(&mut r, 1, &|| false), [Event::Resize(132, 43)]);
    let mut out = Vec::new();
    r.pump(Duration::from_millis(30), &|| true, &mut out)
        .unwrap();
    assert!(out.is_empty(), "three signals are one resize: {out:?}");
}

/// A hung-up terminal ends the reader instead of spinning on it.
#[test]
fn a_hung_up_terminal_stops_the_reader() {
    let (master, slave) = pty();
    let mut r = reader(slave, None);
    drop(master);
    let mut out = Vec::new();
    let ended = matches!(
        r.pump(Duration::from_secs(1), &|| true, &mut out),
        Ok(false) | Err(_)
    );
    assert!(ended);
}

/// The blocking read the startup selector uses hands events out one at a time, in order.
#[test]
fn next_event_returns_events_in_order() {
    let (mut master, slave) = pty();
    let mut r = reader(slave, None);
    master.write_all(b"\x1b[B\x03").unwrap();
    assert_eq!(
        r.next_event().unwrap(),
        key(KeyCode::Down, KeyModifiers::NONE)
    );
    assert_eq!(
        r.next_event().unwrap(),
        key(KeyCode::Char('c'), KeyModifiers::CONTROL)
    );
}

/// The startup selector's wait: `None` when nothing arrives in time (so its loop can look at the
/// channel a streamed listing reports on), the event as soon as one does.
#[test]
fn next_event_timeout_returns_none_when_nothing_arrives_and_the_event_when_it_does() {
    let (mut master, slave) = pty();
    let mut r = reader(slave, None);
    let started = Instant::now();
    assert_eq!(
        r.next_event_timeout(Duration::from_millis(40)).unwrap(),
        None
    );
    assert!(started.elapsed() >= Duration::from_millis(30));
    assert!(started.elapsed() < Duration::from_secs(2));

    master.write_all(b"\x1b[B").unwrap();
    assert_eq!(
        r.next_event_timeout(Duration::from_secs(2)).unwrap(),
        Some(key(KeyCode::Down, KeyModifiers::NONE))
    );
}
