//! TUI-163 — the Windows console's OS half on a real pseudo-console (ConPTY): the terminal's
//! mode-2031 colour-scheme report, sent as terminal input, re-resolves the theme with no user
//! action and never arrives as a typed key.
//!
//! The child is this test binary again, running only [`probe_child`] (`CYRUP_TUI_CONPTY_PROBE`
//! set), which runs `cyrup_tui::conpty_probe` on the pseudo-console: raw mode, the terminal start
//! that turns on virtual-terminal input, the console reader, the run loop's listener and the theme
//! controller's re-resolve — and prints each event. The parent writes what a terminal would send
//! and reads the probe's lines off the console's screen.
//!
//! Windows only: everything above the OS — the pump, the decoder, the listener, `App`'s re-theme
//! — is tested on every host (`src/input/console_tests.rs`, `src/tests/system_theme_surfaces.rs`'s
//! `windows_console` module).

#![cfg(windows)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::FromRawHandle as _;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

const PROBE: &str = "CYRUP_TUI_CONPTY_PROBE";

/// A child process on its own pseudo-console, and everything the console has painted so far.
struct ConPty {
    console: HPCON,
    process: PROCESS_INFORMATION,
    attributes: Vec<u8>,
    input: File,
    screen: Arc<Mutex<Vec<u8>>>,
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// A Unicode environment block: this process's environment with `overrides` on top, sorted, each
/// entry NUL-terminated and the block ended by one more NUL.
fn environment(overrides: &[(&str, &OsStr)]) -> Vec<u16> {
    let mut vars: std::collections::BTreeMap<String, std::ffi::OsString> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?.to_uppercase(), v)))
        .collect();
    for (k, v) in overrides {
        vars.insert(k.to_uppercase(), v.to_os_string());
    }
    let mut block = Vec::new();
    for (k, v) in vars {
        block.extend(OsStr::new(&k).encode_wide());
        block.push(u16::from(b'='));
        block.extend(v.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

impl ConPty {
    fn spawn(program: &Path, args: &[&str], env: &[(&str, &OsStr)]) -> Self {
        let mut in_read: HANDLE = std::ptr::null_mut();
        let mut in_write: HANDLE = std::ptr::null_mut();
        let mut out_read: HANDLE = std::ptr::null_mut();
        let mut out_write: HANDLE = std::ptr::null_mut();
        // SAFETY: each call writes two fresh pipe handles into the out-pointers; no security
        // attributes, default buffer size.
        unsafe {
            assert_ne!(
                CreatePipe(&mut in_read, &mut in_write, std::ptr::null(), 0),
                0
            );
            assert_ne!(
                CreatePipe(&mut out_read, &mut out_write, std::ptr::null(), 0),
                0
            );
        }
        let mut console: HPCON = 0;
        // SAFETY: the two pipe ends are valid handles the pseudo-console duplicates; the out-pointer
        // receives the new console.
        let hr = unsafe {
            CreatePseudoConsole(COORD { X: 120, Y: 40 }, in_read, out_write, 0, &mut console)
        };
        assert_eq!(hr, 0, "CreatePseudoConsole: HRESULT {hr:#x}");
        // SAFETY: the console holds its own duplicates; these ends are ours to close.
        unsafe {
            CloseHandle(in_read);
            CloseHandle(out_write);
        }

        let mut size = 0usize;
        // SAFETY: the sizing call reads nothing through the null list and writes the size.
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size) };
        let mut attributes = vec![0u8; size];
        let list: LPPROC_THREAD_ATTRIBUTE_LIST = attributes.as_mut_ptr().cast();
        // SAFETY: `list` points at `size` writable bytes, as the sizing call asked for; the
        // attribute value is the console handle itself, as `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`
        // requires, and it outlives the child's creation.
        unsafe {
            assert_ne!(InitializeProcThreadAttributeList(list, 1, 0, &mut size), 0);
            assert_ne!(
                UpdateProcThreadAttribute(
                    list,
                    0,
                    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                    console as *const core::ffi::c_void,
                    std::mem::size_of::<HPCON>(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                ),
                0
            );
        }
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = u32::try_from(std::mem::size_of::<STARTUPINFOEXW>()).unwrap();
        startup.lpAttributeList = list;
        let mut command_line = format!("\"{}\"", program.display());
        for arg in args {
            command_line.push(' ');
            command_line.push_str(arg);
        }
        let mut command_line = wide(OsStr::new(&command_line));
        let environment = environment(env);
        let mut process = PROCESS_INFORMATION::default();
        // SAFETY: every pointer is to a live, NUL-terminated buffer owned by this frame (the command
        // line mutable, as `CreateProcessW` requires), the environment block is UTF-16 as the flag
        // says, `startup` carries the attribute list built above, and a null directory is the
        // parent's.
        let created = unsafe {
            CreateProcessW(
                std::ptr::null(),
                command_line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                environment.as_ptr().cast(),
                std::ptr::null(),
                &startup.StartupInfo,
                &mut process,
            )
        };
        assert_ne!(
            created,
            0,
            "CreateProcessW: {}",
            std::io::Error::last_os_error()
        );

        // SAFETY: both pipe ends are open handles owned by nothing else from here on.
        let (input, mut output) = unsafe {
            (
                File::from_raw_handle(in_write),
                File::from_raw_handle(out_read),
            )
        };
        let screen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&screen);
        // Drains the console, so the child never blocks on a full pipe; ends when the console closes.
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = output.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&chunk[..n]);
            }
        });
        Self {
            console,
            process,
            attributes,
            input,
            screen,
        }
    }

    /// Bytes the terminal sends the child: what the user types, and the terminal's own reports.
    fn send(&mut self, bytes: &[u8]) {
        self.input.write_all(bytes).unwrap();
        self.input.flush().unwrap();
    }

    fn painted(&self) -> Vec<u8> {
        self.screen.lock().unwrap().clone()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.painted()).into_owned()
    }

    /// Wait up to `bound` for the screen to show `needle`.
    fn wait_for(&self, needle: &str, bound: Duration) -> bool {
        let started = Instant::now();
        while started.elapsed() < bound {
            if self.text().contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }
}

impl Drop for ConPty {
    fn drop(&mut self) {
        // SAFETY: the handles were created above and are closed once, here.
        unsafe {
            TerminateProcess(self.process.hProcess, 1);
            WaitForSingleObject(self.process.hProcess, 5_000);
            CloseHandle(self.process.hProcess);
            CloseHandle(self.process.hThread);
            ClosePseudoConsole(self.console);
            DeleteProcThreadAttributeList(self.attributes.as_mut_ptr().cast());
        }
    }
}

/// The child: the probe, on the `light/dark` pair. Not a test of its own — without
/// `CYRUP_TUI_CONPTY_PROBE` it returns at once; the two tests below run it on a pseudo-console.
#[test]
fn probe_child() {
    if std::env::var_os(PROBE).is_none() {
        return;
    }
    cyrup_tui::conpty_probe("light/dark", Duration::from_secs(30)).unwrap();
}

/// The probe on its own pseudo-console, on a terminal `COLORFGBG` says is dark, reading.
fn probe() -> ConPty {
    let pty = ConPty::spawn(
        &std::env::current_exe().unwrap(),
        &["probe_child", "--exact", "--nocapture", "--test-threads=1"],
        &[(PROBE, OsStr::new("1")), ("COLORFGBG", OsStr::new("15;0"))],
    );
    assert!(
        pty.wait_for("cyrup-probe: ready theme=dark", Duration::from_secs(30)),
        "the probe started on the dark theme: {:?}",
        pty.text()
    );
    pty
}

/// The TUI-163 Verify clause: a terminal turning light re-resolves the theme of a session on an
/// automatic pair, with no keystroke.
///
/// Red at HEAD by construction: the Windows reader was crossterm's, which has no event for the
/// report — the console reader and the probe this test needs did not exist.
#[test]
fn a_light_report_on_the_console_re_resolves_the_theme() {
    let mut pty = probe();
    pty.send(b"\x1b[?997;2n");
    let heard = pty.wait_for(
        "cyrup-probe: scheme=Light theme=light",
        Duration::from_secs(10),
    );
    pty.send(b"z");
    assert!(heard, "the report re-themed the session: {:?}", pty.text());
}

/// The report is the terminal talking, not the user typing: sent between two typed characters,
/// only the characters arrive as keys.
///
/// Red at HEAD by construction, as above; crossterm's reader took each of the report's bytes as a
/// typed key, which is what `key=` lines for `[`, `?`, `9`, … would show here.
#[test]
fn the_report_on_the_console_is_never_a_typed_key() {
    let mut pty = probe();
    pty.send(b"q");
    pty.send(b"\x1b[?997;2n");
    pty.send(b"z");
    assert!(
        pty.wait_for("cyrup-probe: key=z", Duration::from_secs(10)),
        "the typed keys arrived: {:?}",
        pty.text()
    );
    let text = pty.text();
    let keys: Vec<&str> = text
        .match_indices("cyrup-probe: key=")
        .filter_map(|(at, tag)| text.get(at + tag.len()..at + tag.len() + 1))
        .collect();
    assert_eq!(keys, ["q", "z"], "only the typed keys are keys: {text:?}");
}
