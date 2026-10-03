//! The production input stream ([`crate::crossterm_input_stream`]) driven through a REAL
//! pseudo-terminal.
//!
//! Each test re-runs this test binary as a child whose stdin is the slave side of a fresh pty and
//! whose controlling terminal it is (`setsid -c`), so the child reads a genuine tty exactly as
//! `cyrup` does: raw mode, `read(2)` chunks that follow the parent's writes, and a kernel
//! `SIGWINCH` when the parent resizes the master. The child reports what the stream delivered on
//! stderr, one `@@ ` line per event; the parent writes bytes, with real pauses between them, and
//! checks the report.
//!
//! The parent side needs nothing from the child's address space, so these are the tests that
//! prove behaviour at the process boundary: bytes typed while `$EDITOR` owns the tty, a resize,
//! a split UTF-8 character, a late terminal reply, a megabyte paste.
#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::InputEvent;

/// Set in the child's environment; its value names the child's behaviour.
pub(super) const CHILD_ENV: &str = "CYRUP_TUI_PTY_CHILD";

/// Which side of the test this process is. `None` in the parent.
pub(super) fn child_mode() -> Option<String> {
    std::env::var(CHILD_ENV).ok()
}

// ------------------------------------------------------------------------------ parent -------

/// A child process reading a pty whose master this holds.
pub(super) struct PtyChild {
    pub(super) master: std::fs::File,
    child: Child,
    lines: mpsc::Receiver<String>,
    /// Every stderr line that was not a report, for the failure message.
    pub(super) noise: Vec<String>,
    /// What the child wrote to its stdout, which is then the pty too; `None` when stdout is
    /// `/dev/null` (the default).
    screen: Option<mpsc::Receiver<Vec<u8>>>,
}

impl PtyChild {
    /// Spawn `test` (its full libtest path) as a child in `mode`, on an 80×24 pty, and wait for it
    /// to report `ready`.
    pub(super) fn spawn(test: &str, mode: &str, env: &[(&str, &str)]) -> Self {
        Self::spawn_with(test, mode, env, false)
    }

    /// [`PtyChild::spawn`] with the child's stdout on the pty as well, readable through
    /// [`PtyChild::screen_until`]: what a terminal would be sent.
    pub(super) fn spawn_capturing(test: &str, mode: &str, env: &[(&str, &str)]) -> Self {
        Self::spawn_with(test, mode, env, true)
    }

    fn spawn_with(test: &str, mode: &str, env: &[(&str, &str)], capture: bool) -> Self {
        use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
        let master =
            openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        set_size(&master, 80, 24);
        let path = ptsname(&master, Vec::new()).unwrap();
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(rustix::fs::OFlags::NOCTTY.bits() as i32)
            .open(path.to_str().unwrap())
            .unwrap();
        let stdout = if capture {
            Stdio::from(slave.try_clone().unwrap())
        } else {
            Stdio::null()
        };
        let mut cmd = Command::new("setsid");
        cmd.arg("-c")
            .arg(std::env::current_exe().unwrap())
            .args([test, "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD_ENV, mode)
            .stdin(slave)
            .stdout(stdout)
            .stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let master = std::fs::File::from(master);
        let screen = capture.then(|| {
            let (tx, rx) = mpsc::channel();
            let mut reader = master.try_clone().unwrap();
            std::thread::spawn(move || {
                use std::io::Read as _;
                let mut chunk = [0u8; 4096];
                // Ends with `EIO` once the child and every other slave holder are gone.
                while let Ok(n) = reader.read(&mut chunk) {
                    if n == 0 || tx.send(chunk[..n].to_vec()).is_err() {
                        break;
                    }
                }
            });
            rx
        });
        let mut me = Self {
            master,
            child,
            lines,
            noise: Vec::new(),
            screen,
        };
        me.expect("ready");
        me
    }

    /// The next report line, or `None` once `timeout` passes.
    pub(super) fn next(&mut self, timeout: Duration) -> Option<String> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => match line.strip_prefix("@@ ") {
                    Some(report) => return Some(report.to_owned()),
                    None => self.noise.push(line),
                },
                Err(_) => return None,
            }
        }
    }

    pub(super) fn expect(&mut self, want: &str) {
        let got = self.next(Duration::from_secs(20));
        assert_eq!(
            got.as_deref(),
            Some(want),
            "child stderr: {:#?}",
            self.noise
        );
    }

    /// Every report up to (not including) `end`.
    fn until(&mut self, end: &str) -> Vec<String> {
        let mut out = Vec::new();
        loop {
            match self.next(Duration::from_secs(20)) {
                Some(line) if line == end => return out,
                Some(line) => out.push(line),
                None => panic!(
                    "no {end:?} from the child; reports so far {out:#?}, stderr {:#?}",
                    self.noise
                ),
            }
        }
    }

    pub(super) fn write(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
        self.master.flush().unwrap();
    }

    /// Read what the child writes to the terminal into `seen` until `needle` is in it, and say
    /// whether it was before `timeout` passed. `seen` accumulates across calls.
    pub(super) fn screen_until(
        &mut self,
        seen: &mut Vec<u8>,
        needle: &str,
        timeout: Duration,
    ) -> bool {
        let rx = self.screen.as_ref().expect("spawned with spawn_capturing");
        let deadline = Instant::now() + timeout;
        loop {
            if String::from_utf8_lossy(seen).contains(needle) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(bytes) => seen.extend_from_slice(&bytes),
                Err(_) => return false,
            }
        }
    }

    /// Write each chunk, pausing after it.
    fn play(&mut self, script: &[(&[u8], u64)]) {
        for (bytes, pause_ms) in script {
            self.write(bytes);
            std::thread::sleep(Duration::from_millis(*pause_ms));
        }
    }

    /// Wait for the child to exit, returning its exit code.
    pub(super) fn exit_code(&mut self, timeout: Duration) -> Option<i32> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }
}

impl Drop for PtyChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn set_size(master: &impl std::os::fd::AsFd, cols: u16, rows: u16) {
    rustix::termios::tcsetwinsize(
        master,
        rustix::termios::Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )
    .unwrap();
}

// ------------------------------------------------------------------------------- child -------

pub(super) fn report(line: impl std::fmt::Display) {
    eprintln!("@@ {line}");
}

/// One event as a report line. Pastes are summarised (length and FNV-1a) so a megabyte paste is
/// one short line.
fn describe(ev: &InputEvent) -> String {
    match ev {
        InputEvent::Key(k) => format!("key {:?} {:?}", k.code, k.modifiers),
        InputEvent::Paste(s) => format!("paste {} {:016x}", s.len(), fnv1a(s.as_bytes())),
        InputEvent::Resize(c, r) => format!("resize {c} {r}"),
        InputEvent::FocusGained => "focus in".into(),
        InputEvent::FocusLost => "focus out".into(),
        InputEvent::Mouse(m) => format!("mouse {m:?}"),
    }
}

fn key_line(code: KeyCode, mods: KeyModifiers) -> String {
    describe(&InputEvent::Key(KeyEvent::new(code, mods)))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Raw mode on the pty, a tokio runtime, and the production stream. `body` runs once `ready` is
/// reported.
fn child_run<F>(
    body: impl FnOnce(cyrup_core::EventStream<InputEvent>, cyrup_core::CancelToken) -> F,
) where
    F: std::future::Future<Output = ()>,
{
    ratatui::crossterm::terminal::enable_raw_mode().unwrap();
    // Ask for bracketed paste, as `App::into_stdout` does, so the child also works as a live
    // target under a real terminal (tmux's `paste-buffer -p` brackets only when asked). In the pty
    // tests stdout is `/dev/null` and this is a no-op.
    let _ = std::io::stdout().write_all(b"\x1b[?2004h");
    let _ = std::io::stdout().flush();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let cancel = cyrup_core::CancelToken::new();
        let stream = crate::crossterm_input_stream(cancel.clone());
        report("ready");
        body(stream, cancel).await;
    });
    let _ = ratatui::crossterm::terminal::disable_raw_mode();
}

/// Report every event until `Ctrl+Q`, then `done`.
async fn echo_until_ctrl_q(mut stream: cyrup_core::EventStream<InputEvent>) {
    let quit = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while let Ok(Some(ev)) = tokio::time::timeout_at(deadline, stream.next()).await {
        if matches!(&ev, InputEvent::Key(k) if *k == quit) {
            break;
        }
        report(describe(&ev));
    }
    report("done");
}

/// Collect every event that arrives within `window`.
async fn drain_for(
    stream: &mut cyrup_core::EventStream<InputEvent>,
    window: Duration,
) -> Vec<String> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + window;
    while let Ok(Some(ev)) = tokio::time::timeout_at(deadline, stream.next()).await {
        out.push(describe(&ev));
    }
    out
}

/// Read what is waiting on the tty directly, as a program that owns it would.
fn read_tty_directly(window: Duration) -> Vec<u8> {
    use rustix::event::{PollFd, PollFlags, Timespec};
    let stdin = std::io::stdin();
    let deadline = Instant::now() + window;
    let mut got = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return got;
        }
        let ts = Timespec {
            tv_sec: left.as_secs() as _,
            tv_nsec: left.subsec_nanos() as _,
        };
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        if rustix::event::poll(&mut fds, Some(&ts)).unwrap_or(0) == 0 {
            return got;
        }
        let mut chunk = [0u8; 256];
        match rustix::io::read(&stdin, &mut chunk[..]) {
            Ok(0) | Err(_) => return got,
            Ok(n) => got.extend_from_slice(&chunk[..n]),
        }
    }
}

// ------------------------------------------------------------------------------- tests -------

/// While `$EDITOR` or a `Ctrl+Z` suspend owns the tty ([`crate::app::TerminalReleased`]), the
/// reader must not read it: the bytes the user types belong to the program in the foreground.
///
/// RED at `2a3d68ec` (crossterm's reader thread): `event::poll` kept draining the tty while the
/// terminal was released, so the "editor" read nothing and cyrup's stream received the keys.
#[test]
fn a_released_terminal_is_left_to_the_program_that_owns_it() {
    if child_mode().as_deref() == Some("released") {
        return child_run(|mut stream, _cancel| async move {
            let released = crate::app::TerminalReleased::enter();
            // `$EDITOR` starts; the user types into it.
            tokio::time::sleep(Duration::from_millis(600)).await;
            let editor = read_tty_directly(Duration::from_millis(300));
            report(format!("editor {}", String::from_utf8_lossy(&editor)));
            drop(released);
            report(format!(
                "stream {:?}",
                drain_for(&mut stream, Duration::from_millis(400)).await
            ));
        });
    }
    let mut pty = PtyChild::spawn(
        "tests::input_pty::a_released_terminal_is_left_to_the_program_that_owns_it",
        "released",
        &[],
    );
    std::thread::sleep(Duration::from_millis(100));
    pty.write(b"abc");
    let editor = pty.next(Duration::from_secs(20));
    let stream = pty.next(Duration::from_secs(20));
    assert_eq!(
        (editor.as_deref(), stream.as_deref()),
        (Some("editor abc"), Some("stream []")),
        "stderr {:#?}",
        pty.noise
    );
}

/// A resize of the real terminal reaches the app as [`InputEvent::Resize`] with the new size,
/// through the kernel's `SIGWINCH` to the child's foreground process group.
#[test]
fn a_resize_arrives_through_sigwinch_on_a_real_pty() {
    if child_mode().as_deref() == Some("winch") {
        return child_run(|mut stream, _cancel| async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            while let Ok(Some(ev)) = tokio::time::timeout_at(deadline, stream.next()).await {
                if let InputEvent::Resize(..) = ev {
                    report(describe(&ev));
                    return;
                }
            }
            report("no resize");
        });
    }
    let mut pty = PtyChild::spawn(
        "tests::input_pty::a_resize_arrives_through_sigwinch_on_a_real_pty",
        "winch",
        &[],
    );
    std::thread::sleep(Duration::from_millis(200));
    set_size(&pty.master, 100, 30);
    pty.expect("resize 100 30");
}

/// Everything the reader decides from bytes, over a real tty with real gaps between writes. The
/// escape timeout is widened to 150 ms (`CYRUP_TUI_ESC_TIMEOUT`) so the "inside the timeout"
/// splits below survive a loaded machine; the "after the timeout" ones wait well past it.
///
/// RED at `2a3d68ec` (crossterm's reader) on four rows: the DCS and APC replies were typed as
/// `Alt+P`/`Alt+_` plus their payload (`TUI-047`), `0xE1` produced nothing and took the next key
/// with it (`TUI-050`), and `ESC [ 224 u` + `à` produced two `à` (`TUI-046`).
#[test]
fn live_pty_bytes_decode_as_the_terminal_meant_them() {
    if child_mode().as_deref() == Some("echo") {
        return child_run(|stream, _cancel| echo_until_ctrl_q(stream));
    }
    let mut pty = PtyChild::spawn(
        "tests::input_pty::live_pty_bytes_decode_as_the_terminal_meant_them",
        "echo",
        &[("CYRUP_TUI_ESC_TIMEOUT", "150")],
    );
    let late = 450;
    pty.play(&[
        (b"ab", 0),
        // A UTF-8 character split across reads.
        (&[0xc3], 20),
        (&[0xa9], 0),
        // TUI-045's split at the ESC byte.
        (b"\x1b", 20),
        (b"[A", 0),
        // TUI-047: late DCS (split at its ST) and APC replies, and the OSC 11 reply.
        (b"\x1bP>|xterm(392)", 5),
        (b"\x1b\\", 0),
        (b"\x1b_Gi=31;OK\x1b\\", 0),
        (b"\x1b]11;rgb:0c0c/0b0b/1313\x07", 0),
        // Late CSI replies: Kitty flags, DA1, a cursor position, a colour-scheme report.
        (b"\x1b[?1u\x1b[?62;22c\x1b[12;40R\x1b[?997;1n", 0),
        (b"\x1b[I\x1b[O", 0),
        // An SGR mouse report with no mouse mode enabled reaches nothing.
        (b"\x1b[<0;10;5M", 0),
        // TUI-046: a Kitty printable sequence and the raw duplicate some terminals send after it.
        ("\x1b[224uà".as_bytes(), 0),
        // TUI-050: an 8-bit meta byte, decided by the escape timeout.
        (&[0xe1], late),
        (b"x", 0),
        // A lone ESC past the timeout is Escape; the key after it is its own.
        (b"\x1b", late),
        (b"b", 0),
        (b"\x1b[200~hello\r\nworld\x1b[201~", 0),
        (b"\x11", 0),
    ]);
    let got = pty.until("done");
    let paste = InputEvent::Paste("hello\r\nworld".into());
    let want = vec![
        key_line(KeyCode::Char('a'), KeyModifiers::NONE),
        key_line(KeyCode::Char('b'), KeyModifiers::NONE),
        key_line(KeyCode::Char('é'), KeyModifiers::NONE),
        key_line(KeyCode::Up, KeyModifiers::NONE),
        "focus in".to_owned(),
        "focus out".to_owned(),
        key_line(KeyCode::Char('à'), KeyModifiers::NONE),
        key_line(KeyCode::Char('a'), KeyModifiers::ALT),
        key_line(KeyCode::Char('x'), KeyModifiers::NONE),
        key_line(KeyCode::Esc, KeyModifiers::NONE),
        key_line(KeyCode::Char('b'), KeyModifiers::NONE),
        describe(&paste),
    ];
    assert_eq!(got, want);
}

/// A 1 MiB bracketed paste written in 64 KiB pieces 60 ms apart is ONE paste event with every
/// byte intact — paste mode has no timeout (pi `stdin-buffer.ts:324-343`).
#[test]
fn a_megabyte_paste_across_60ms_gaps_is_one_paste() {
    if child_mode().as_deref() == Some("echo") {
        return child_run(|stream, _cancel| echo_until_ctrl_q(stream));
    }
    let mut pty = PtyChild::spawn(
        "tests::input_pty::a_megabyte_paste_across_60ms_gaps_is_one_paste",
        "echo",
        &[],
    );
    let unit = "lorem ipsum 世界 🎉 dolor\n";
    let mut body = String::new();
    while body.len() < 1 << 20 {
        body.push_str(unit);
    }
    pty.write(b"\x1b[200~");
    for piece in body.as_bytes().chunks(64 * 1024) {
        pty.write(piece);
        std::thread::sleep(Duration::from_millis(60));
    }
    pty.write(b"\x1b[201~\x11");
    let got = pty.until("done");
    assert_eq!(got, vec![describe(&InputEvent::Paste(body))]);
}

/// TUI-092's escape hatch with the chords arriving as BYTES: three `Ctrl+C` (`0x03`) presses, 300 ms
/// apart, against a run loop that services nothing. The second fires the cooperative cancel, the
/// third exits 130 from the reader thread.
#[test]
fn three_unserviced_ctrl_c_bytes_climb_the_escalation_ladder() {
    if child_mode().as_deref() == Some("escalate") {
        return child_run(|mut stream, cancel| async move {
            // Consume the stream, as a wedged-but-alive loop's channel would be; never service.
            let pump = async { while stream.next().await.is_some() {} };
            tokio::select! {
                () = cancel.cancelled() => report("cancelled"),
                () = pump => {}
            }
            // Keep the process alive for the third chord's hard exit.
            tokio::time::sleep(Duration::from_secs(20)).await;
            report("still alive");
        });
    }
    let mut pty = PtyChild::spawn(
        "tests::input_pty::three_unserviced_ctrl_c_bytes_climb_the_escalation_ladder",
        "escalate",
        &[],
    );
    pty.write(&[0x03]);
    std::thread::sleep(Duration::from_millis(300));
    pty.write(&[0x03]);
    pty.expect("cancelled");
    std::thread::sleep(Duration::from_millis(300));
    pty.write(&[0x03]);
    assert_eq!(pty.exit_code(Duration::from_secs(10)), Some(130));
}
