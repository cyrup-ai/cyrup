//! System-clipboard **write** — a port of Pi `copyToClipboard` (`coding-agent/src/utils/
//! clipboard.ts:73-174`), the single writer behind `/copy` (`handleCopyCommand`,
//! `interactive-mode.ts:6002-6019`) and the Ctrl+Shift+C-style keybindings routed to it.
//!
//! # Why this is not a `#[cfg(unix)]` shell-out
//!
//! It used to be. `app.rs` carried a `#[cfg(unix)]` arm that probed `pbcopy`/`wl-copy`/`xclip` and
//! a `#[cfg(not(unix))]` arm that was literally `fn copy_to_clipboard(_text: &str) {}` — a silent,
//! total no-op that still let the caller print `copied last message (N chars)`. That was a
//! divergence from Pi twice over: Pi has a working `win32` arm (`clipboard.ts:109-110`,
//! `execSync("clip", …)`), and Pi's writer *throws* when nothing worked so `handleCopyCommand`
//! can `showError` (`interactive-mode.ts:6016-6018`). The same file already read the clipboard on
//! every platform through `arboard` (`app::read_clipboard_image_to_temp`), so cyrup could read a
//! Windows clipboard it deliberately could not write.
//!
//! # The ported chain
//!
//! Pi's order, preserved exactly ([`clipboard_write_plan`] is the decision, [`copy_to_clipboard`]
//! the execution):
//!
//! 1. The **native** clipboard API, unless the platform is Linux (`clipboard.ts:88-92`). Pi skips
//!    Linux because its native addon (`clipboard-rs`) is X11-only and drops selection ownership as
//!    soon as `setText` resolves, so the write silently does nothing on Wayland
//!    (`clipboard.ts:82-87`). `arboard` — already this crate's clipboard dependency, used for the
//!    Ctrl+V image read — has the *same* ownership model on X11/Wayland, so the skip ports as-is.
//! 2. The **platform CLI**, fed the text on stdin: `pbcopy` (darwin), `clip` (win32), and on Linux
//!    `termux-clipboard-set` → `wl-copy` → `xclip -selection clipboard` → `xsel --clipboard
//!    --input`, gated on the very env vars Pi gates on (`clipboard.ts:104-160`, `copyToX11Clipboard`
//!    `:12-18`). These tools daemonize and keep ownership, which is the whole reason Pi prefers
//!    them on Linux.
//! 3. **OSC 52**, emitted when the session is remote (`SSH_CONNECTION`/`SSH_CLIENT`/
//!    `MOSH_CONNECTION`) *or* when nothing above worked (`clipboard.ts:166-169`) — remote included
//!    even after a successful local write, because the local clipboard is the wrong machine's.
//!
//! 3b. On Linux under **WSL**, before the OSC 52 decision: Windows Terminal gets the escape
//!    directly, and otherwise the text is written through Windows interop
//!    ([`copy_via_windows_clipboard`], `clipboard.ts:110-114`) — WSL without WSLg has no Linux
//!    display, so no step above it can have run.
//!
//! Nothing worked → [`copy_to_clipboard`] returns pi's throw ladder as a [`ClipboardError`], whose
//! five messages name the missing helper instead of the single generic string this used to report
//! (`clipboard.ts:124-137`).

use std::process::Stdio;
use std::time::Duration;

/// Pi's `NativeClipboardExecOptions.timeout` (`clipboard.ts:9`, `:103`) — a clipboard helper that
/// never exits must not wedge the run loop.
const CLIPBOARD_COMMAND_TIMEOUT: Duration = Duration::from_millis(5000);

/// Pi `MAX_OSC52_ENCODED_LENGTH` (`clipboard.ts:20`). Past this, the escape is not emitted at all:
/// a multi-hundred-KB OSC 52 payload desynchronizes terminal rendering (`clipboard.ts:78-80`).
pub(crate) const MAX_OSC52_ENCODED_LENGTH: usize = 100_000;

/// One attempt in Pi's ordered write chain. Tried in order; the first success wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardWrite {
    /// The native clipboard API (`arboard`) — Pi's `clipboard.setText` addon (`clipboard.ts:89`).
    Native,
    /// A platform CLI given the text on stdin — Pi's `execSync(bin, { input: text, … })`.
    Command(&'static str, &'static [&'static str]),
}

/// The environment inputs Pi's writer branches on, snapshotted so the platform decision is a pure
/// function that can be exercised for *every* target from any host (see `tests/clipboard.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ClipboardEnv {
    /// Pi `isRemoteSession` (`clipboard.ts:22-24`): `SSH_CONNECTION` | `SSH_CLIENT` |
    /// `MOSH_CONNECTION`. Forces the OSC 52 emit even when a local write succeeded.
    pub(crate) remote: bool,
    /// `process.env.TERMUX_VERSION` (`clipboard.ts:113`).
    pub(crate) termux: bool,
    /// `process.env.WAYLAND_DISPLAY` (`clipboard.ts:123`).
    pub(crate) wayland_display: bool,
    /// Pi `isWaylandSession` (`clipboard-image.ts:22-24`): `WAYLAND_DISPLAY` set **or**
    /// `XDG_SESSION_TYPE == "wayland"`.
    pub(crate) wayland_session: bool,
    /// `process.env.DISPLAY` (`clipboard.ts:98`, `:133`).
    pub(crate) x11_display: bool,
    /// Pi `isWSL` (`wsl.ts:4-15`): `WSL_DISTRO_NAME` | `WSLENV`, else `/microsoft|wsl/i` against
    /// `/proc/version`. Selects the Windows-interop write chain (`clipboard.ts:110-114`).
    pub(crate) wsl: bool,
    /// `process.env.WT_SESSION` (`clipboard.ts:112`) — Windows Terminal, which supports OSC 52, so
    /// the escape is preferred over the slower PowerShell round trip.
    pub(crate) wt_session: bool,
}

impl ClipboardEnv {
    /// Read the live process environment. Pi reads `process.env` at each branch; this reads once
    /// per `/copy`, which is the same thing for a single invocation.
    pub(crate) fn from_process() -> Self {
        let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
        let wayland_display = set("WAYLAND_DISPLAY");
        Self {
            remote: set("SSH_CONNECTION") || set("SSH_CLIENT") || set("MOSH_CONNECTION"),
            termux: set("TERMUX_VERSION"),
            wayland_display,
            wayland_session: wayland_display
                || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v == "wayland"),
            x11_display: set("DISPLAY"),
            // Pi `isWSL` verbatim (`wsl.ts:5-14`): the two env vars first, then the `/proc/version`
            // sniff, and any read error is `false` (upstream's `catch`).
            wsl: set("WSL_DISTRO_NAME")
                || set("WSLENV")
                || std::fs::read_to_string("/proc/version").is_ok_and(|release| {
                    let release = release.to_ascii_lowercase();
                    release.contains("microsoft") || release.contains("wsl")
                }),
            wt_session: set("WT_SESSION"),
        }
    }
}

/// The reason nothing reached a clipboard — pi's throw ladder (`clipboard.ts:124-137`), which
/// `handleCopyCommand` turns into `showError(error.message)` and the fullscreen renderer flashes
/// (`tui-renderer.ts:37-44`, `tui-alt-screen.ts:1456-1462`).
///
/// An enum rather than a `String` so the five messages are one table the tests can compare
/// byte-for-byte, and so the flash duration decision ([`crate::app::run_action::copy_flash_for`])
/// cannot be written against a message it had to re-parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardError {
    /// `throw new Error("Clipboard unavailable: text exceeds the OSC 52 size limit")`
    /// (`clipboard.ts:125`) — the escape was the only route left and the payload is past
    /// [`MAX_OSC52_ENCODED_LENGTH`].
    Oversized,
    /// `clipboard.ts:128` — `TERMUX_VERSION` is set, so `termux-clipboard-set` was tried and is
    /// missing or failed.
    Termux,
    /// `clipboard.ts:131` — `WAYLAND_DISPLAY` is set, so `wl-copy` was tried.
    Wayland,
    /// `clipboard.ts:134` — `DISPLAY` is set, so `xclip`/`xsel` were tried.
    X11,
    /// `clipboard.ts:137`, the fallthrough: a non-Linux platform, or a Linux box with no display
    /// server env at all whose OSC 52 emit still did not copy.
    Unavailable,
}

impl ClipboardError {
    /// Pi's five throw strings, byte-for-byte (`clipboard.ts:125-137`).
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::Oversized => "Clipboard unavailable: text exceeds the OSC 52 size limit",
            Self::Termux => {
                "Clipboard unavailable: install the Termux:API app and `termux-api` package"
            }
            Self::Wayland => {
                "Clipboard unavailable: install `wl-clipboard` (`wl-copy`) or check Wayland access"
            }
            Self::X11 => "Clipboard unavailable: install `xclip` or `xsel`, or check X11 access",
            Self::Unavailable => "Clipboard unavailable",
        }
    }
}

/// Whether the native clipboard backend (`arboard`) may be constructed at all — pi
/// `getNativeClipboard()` (`tui/src/native-platform.ts:58-63` @v0.87.1), the load gate every
/// native read and write goes through:
///
/// ```ts
/// if (process.platform !== "linux") return getNativePlatformHelper(); // darwin | win32, else undefined
/// if (!process.env.DISPLAY) return undefined;
/// return loadNativePlatformHelper("linux", "-x11");
/// ```
///
/// CFG-066. This is the v0.87.1 form of the `hasDisplay` / `TERMUX_VERSION` gate pi used to keep
/// in `clipboard-native.ts`: Linux needs an X11 `DISPLAY` (Termux and a headless box have none, and
/// `WAYLAND_DISPLAY` alone no longer counts — the helper is X11-only, as is `arboard` built without
/// its `wayland-data-control` feature), and a platform with no native helper (the BSDs) never
/// constructs one. Upstream's `arch` check (`x64`/`arm64`) guards which prebuilt `.node` exists;
/// `arboard` is compiled for the host, so it has no counterpart.
pub(crate) fn native_clipboard_available(os: &str, env: &ClipboardEnv) -> bool {
    match os {
        "linux" => env.x11_display,
        "macos" | "windows" => true,
        _ => false,
    }
}

/// The ordered write chain for `os` (an [`std::env::consts::OS`] value) under `env` — Pi
/// `copyToClipboard`'s branch structure with the execution stripped out.
///
/// `os` is a parameter rather than a `cfg!` so the Windows chain is asserted from a macOS CI host:
/// the defect this replaced was precisely a target-gated arm nobody could see.
pub(crate) fn clipboard_write_plan(os: &str, env: &ClipboardEnv) -> Vec<ClipboardWrite> {
    let mut steps: Vec<ClipboardWrite> = Vec::new();
    // `if (p !== "linux") { const clipboard = getNativeClipboard(); if (clipboard?.setText) … }`
    // (`coding-agent/src/utils/clipboard.ts:80-90` @v0.87.1) — the native write goes first,
    // Windows very much included (the platform whose arm used to be an empty function body).
    //
    // This used to be a `[CYRUP-DELTA]` narrowing pi's `p !== "linux"` to "macOS or Windows" so the
    // BSDs would not take arboard's non-persistent X11 write. At v0.87.1 upstream says the same
    // thing itself: `getNativePlatformHelper()` answers `undefined` off `darwin`/`win32`
    // (`tui/src/native-platform.ts:53-56`), so `p !== "linux"` plus the helper's existence is
    // exactly these two platforms — [`native_clipboard_available`] is that helper gate.
    if os != "linux" && native_clipboard_available(os, env) {
        steps.push(ClipboardWrite::Native);
    }
    match os {
        // `if (p === "darwin") execSync("pbcopy", options)` (`clipboard.ts:107-108`).
        "macos" => steps.push(ClipboardWrite::Command("pbcopy", &[])),
        // `else if (p === "win32") execSync("clip", options)` (`clipboard.ts:109-110`).
        "windows" => steps.push(ClipboardWrite::Command("clip", &[])),
        // Pi's `else` — written "Linux" but reached by every other platform too
        // (`clipboard.ts:111-160`).
        _ => {
            if env.termux {
                steps.push(ClipboardWrite::Command("termux-clipboard-set", &[]));
            }
            // `if (isWayland && hasWaylandDisplay)` (`clipboard.ts:126`) — both, not either: a
            // Wayland *session* with no `WAYLAND_DISPLAY` has no socket for `wl-copy` to use.
            if env.wayland_session && env.wayland_display {
                steps.push(ClipboardWrite::Command("wl-copy", &[]));
            }
            // `copyToX11Clipboard` (`clipboard.ts:12-18`): xclip, then xsel on its failure. Reached
            // both as Pi's `else if (hasX11Display)` and as the post-`wl-copy` fallback
            // (`clipboard.ts:145-148`), which is the same list in the same order.
            if env.x11_display {
                steps.push(ClipboardWrite::Command(
                    "xclip",
                    &["-selection", "clipboard"],
                ));
                steps.push(ClipboardWrite::Command("xsel", &["--clipboard", "--input"]));
            }
        }
    }
    steps
}

/// Pi `emitOsc52` (`clipboard.ts:26-32`): `ESC ] 52 ; c ; <base64> BEL`, or `None` when the encoded
/// payload exceeds [`MAX_OSC52_ENCODED_LENGTH`] (Pi returns `false` and stays silent).
pub(crate) fn osc52_sequence(text: &str) -> Option<String> {
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    if encoded.len() > MAX_OSC52_ENCODED_LENGTH {
        return None;
    }
    Some(format!("\u{1b}]52;c;{encoded}\u{7}"))
}

/// Pi's `headless` (`clipboard.ts:118`): `p === "linux" && !DISPLAY && !WAYLAND_DISPLAY &&
/// !TERMUX_VERSION` — no local clipboard route exists at all, so the terminal is the only one
/// (containers, WSL without WSLg).
///
/// `[CYRUP-DELTA]` cyrup's [`ClipboardEnv`] carries both `wayland_display` and the wider
/// `wayland_session` (`XDG_SESSION_TYPE == "wayland"` counts), and this reads the NARROWER one
/// deliberately: upstream tests `env.WAYLAND_DISPLAY` only (`clipboard.ts:118`), so a Wayland
/// session advertised with no socket to talk to *is* headless upstream — and it genuinely is, since
/// `wl-copy` has nothing to connect to. Reading `wayland_session` here would make that box report a
/// failure while suppressing the one route that still works.
pub(crate) fn headless(os: &str, env: &ClipboardEnv) -> bool {
    os == "linux" && !env.x11_display && !env.wayland_display && !env.termux
}

/// Pi's `if (!osc52Emitted && (isRemoteSession(env) || (!copied && headless)))`
/// (`clipboard.ts:120`): the escape is emitted when the session is **remote** — even after a
/// successful local write, because the clipboard that was written belongs to the machine the user is
/// not sitting at — or when nothing local worked **and** there was no local route to begin with.
///
/// The `headless` conjunct is the correction this carries over the previous `remote || !copied`:
/// "OSC 52 cannot be verified, so a desktop session with a display reports the failure instead
/// (#9618)" (`clipboard.ts:115-117`). With `DISPLAY` set and `xclip`/`xsel` missing, the old gate
/// emitted an unverifiable escape and returned success, so `/copy` said `copied selection (N chars)`
/// over a clipboard nothing had been written to.
pub(crate) fn osc52_required(remote: bool, copied: bool, headless: bool) -> bool {
    remote || (!copied && headless)
}

/// Which Windows-interop route a WSL box takes — Pi `clipboard.ts:110-114`, extracted as a pure
/// decision for the same reason [`clipboard_write_plan`] is one: neither arm can be exercised on a
/// CI host, so the branch is asserted rather than compiled and hoped for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WslRoute {
    /// `if (env.WT_SESSION) osc52Emitted = emitOsc52(text)` (`clipboard.ts:112`) — Windows Terminal
    /// supports OSC 52, which beats the PowerShell round trip.
    Osc52,
    /// `copied = osc52Emitted || (await copyViaWindowsClipboard(text))` (`clipboard.ts:113`).
    PowerShell,
}

/// `if (!copied && p === "linux" && isWSL(env))` (`clipboard.ts:110`) resolved to its route, or
/// `None` when the arm does not apply. The `!copied` half stays at the call site, since it is
/// execution state rather than environment.
pub(crate) fn wsl_route(os: &str, env: &ClipboardEnv) -> Option<WslRoute> {
    if os != "linux" || !env.wsl {
        return None;
    }
    Some(if env.wt_session {
        WslRoute::Osc52
    } else {
        WslRoute::PowerShell
    })
}

/// Pi's throw ladder, in its exact precedence order (`clipboard.ts:124-137`): the oversized OSC 52
/// payload first, then the Linux display-server env vars termux → Wayland → X11, then the bare
/// fallthrough that also covers every non-Linux platform.
pub(crate) fn clipboard_failure(os: &str, env: &ClipboardEnv, oversized: bool) -> ClipboardError {
    if oversized {
        return ClipboardError::Oversized;
    }
    if os == "linux" {
        if env.termux {
            return ClipboardError::Termux;
        }
        if env.wayland_display {
            return ClipboardError::Wayland;
        }
        if env.x11_display {
            return ClipboardError::X11;
        }
    }
    ClipboardError::Unavailable
}

/// Run one step, returning whether it actually put the text on a clipboard.
async fn run_step(step: ClipboardWrite, text: &str) -> bool {
    match step {
        // Synchronous, like Pi's awaited addon call: on the two platforms this step runs
        // (see the `[CYRUP-DELTA]` in `clipboard_write_plan`) it is an NSPasteboard / `OpenClipboard`
        // write that returns immediately, not an X11 round trip, so there is nothing to offload.
        ClipboardWrite::Native => arboard::Clipboard::new()
            .and_then(|mut c| c.set_text(text))
            .is_ok(),
        ClipboardWrite::Command(bin, args) => run_command(bin, args, text).await,
    }
}

/// Spawn `bin args`, write `text` to its stdin, and wait for a clean exit — Pi's
/// `execSync(bin, { input: text, timeout: 5000, stdio: ["pipe", "ignore", "ignore"] })`, and for
/// `wl-copy` its explicit `spawn` + exit-code check (`clipboard.ts:132-145`).
///
/// Only a **zero exit status** counts as copied. The previous implementation returned on a
/// successful *spawn*, so a `wl-copy` that started and failed reported success and suppressed every
/// remaining fallback.
async fn run_command(bin: &str, args: &[&str], text: &str) -> bool {
    use tokio::io::AsyncWriteExt as _;
    let spawned = tokio::process::Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // The timeout below drops the child; without this the process would outlive us.
        .kill_on_drop(true)
        .spawn();
    let Ok(mut child) = spawned else {
        // A missing helper is Pi's `catch` → fall through to the next candidate.
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        // Pi ignores EPIPE from a helper that exits early (`clipboard.ts:139-141`); the exit status
        // below is what decides, not the write.
        let _ = stdin.write_all(text.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }
    matches!(
        tokio::time::timeout(CLIPBOARD_COMMAND_TIMEOUT, child.wait()).await,
        Ok(Ok(status)) if status.success()
    )
}

/// Pi's `runClipboardCommand("wslpath", …, { timeoutMs: 1000 })` (`clipboard.ts:33`) — a path
/// translation, not a clipboard helper, hence its own much shorter bound.
const WSLPATH_TIMEOUT: Duration = Duration::from_millis(1000);

/// Run `bin args` to completion under `timeout` and capture stdout — Pi's
/// `runClipboardCommand(bin, args, { timeoutMs })` on the WSL path (`clipboard.ts:33`, `:38-40`),
/// whose `undefined` return covers a spawn failure, a non-zero exit and the timeout alike.
///
/// `[CYRUP-DELTA]` this deliberately does NOT reuse the sync [`run_capture`] below, whose own
/// `[CYRUP-DELTA]` records that its timeout stops *waiting* rather than killing the child. Here
/// `kill_on_drop(true)` plus `tokio::time::timeout` dropping the future does kill it, matching
/// Node's `killSignal`, so a wedged `powershell.exe` cannot be left behind holding a temp file.
async fn run_capture_async(bin: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => String::from_utf8(out.stdout).ok(),
        // A missing `wslpath`/`powershell.exe`, a non-zero exit, or the timeout — Pi's `catch`
        // and its `undefined` (`clipboard.ts:42-43`).
        _ => None,
    }
}

/// Pi `copyViaWindowsClipboard` (`clipboard.ts:24-51`), doc comment included: WSL without WSLg has
/// no Linux display, so the Windows clipboard is written through interop, and **PowerShell reads the
/// text from a file** because `clip.exe` and PowerShell stdin decode piped bytes with the console
/// code page, which mangles non-ASCII UTF-8.
async fn copy_via_windows_clipboard(text: &str) -> bool {
    // `join(tmpdir(), `pi-wsl-clip-${randomUUID()}.txt`)` (`clipboard.ts:30`). `[CYRUP-DELTA]` v7
    // rather than pi's v4, matching the crate's other temp-file namer
    // (`app/event_extract.rs:115`): uniqueness is all either side needs of it, and `create_new(true)`
    // below is what actually refuses a collision.
    let tmp = std::env::temp_dir().join(format!("cyrup-wsl-clip-{}.txt", uuid::Uuid::now_v7()));
    let copied = copy_via_windows_clipboard_file(text, &tmp).await;
    // Pi's `finally { try { unlinkSync(tmpFile) } catch {} }` (`clipboard.ts:44-50`) — the file may
    // never have been created, and either way it must not survive the call.
    let _ = std::fs::remove_file(&tmp);
    copied
}

/// The body of [`copy_via_windows_clipboard`], split out only so the unlink can be a caller-side
/// `finally` on **every** exit path rather than repeated at each `return false`.
async fn copy_via_windows_clipboard_file(text: &str, tmp: &std::path::Path) -> bool {
    // `writeFileSync(tmpFile, text, { encoding: "utf8", mode: 0o600 })` (`clipboard.ts:32`). The
    // mode matters: the clipboard text is user content sitting in a world-readable directory.
    {
        use std::io::Write as _;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let Ok(mut file) = opts.open(tmp) else {
            return false;
        };
        if file.write_all(text.as_bytes()).is_err() || file.flush().is_err() {
            return false;
        }
    }
    let tmp_str = tmp.to_string_lossy().to_string();
    // `?.toString("utf8").trim()`, then `if (!winPath) return false` (`clipboard.ts:33-36`).
    let Some(winpath) = run_capture_async("wslpath", &["-w", &tmp_str], WSLPATH_TIMEOUT).await
    else {
        return false;
    };
    let winpath = winpath.trim();
    if winpath.is_empty() {
        return false;
    }
    // `winPath.replaceAll("'", "''")` (`clipboard.ts:37`) — PowerShell single-quote escaping.
    let quoted = winpath.replace('\'', "''");
    let script = format!(
        "Set-Clipboard -Value ([System.IO.File]::ReadAllText('{quoted}', [System.Text.Encoding]::UTF8))"
    );
    run_capture_async(
        "powershell.exe",
        &["-NoProfile", "-Command", &script],
        CLIPBOARD_COMMAND_TIMEOUT,
    )
    .await
    .is_some()
}

/// Copy `text` to the system clipboard — Pi `copyToClipboard` (`clipboard.ts:74-138`).
///
/// `Err` carries Pi's throw (`clipboard.ts:124-137`), which `handleCopyCommand` turns into
/// `showError(error.message)` (`interactive-mode.ts:6016-6018`) and the fullscreen renderer flashes
/// for five seconds (`tui-renderer.ts:37-44`). A silent `Ok` for a write that did nothing is the
/// exact failure this function was rewritten to remove — do not reintroduce it by ignoring the
/// return value (`Result` is `#[must_use]`, so the compiler holds that line).
pub(crate) async fn copy_to_clipboard(text: &str) -> Result<(), ClipboardError> {
    let os = std::env::consts::OS;
    let env = ClipboardEnv::from_process();
    let mut copied = false;
    for step in clipboard_write_plan(os, &env) {
        if run_step(step, text).await {
            copied = true;
            break;
        }
    }
    // `if (!copied && p === "linux" && isWSL(env))` (`clipboard.ts:110-114`). Nothing above can have
    // run on WSL without WSLg — there is no Linux display — so this is the first route that exists.
    let mut osc52_emitted = false;
    if !copied && let Some(route) = wsl_route(os, &env) {
        if route == WslRoute::Osc52
            && let Some(seq) = osc52_sequence(text)
        {
            write_stdout(&seq);
            osc52_emitted = true;
        }
        // `copied = osc52Emitted || (await copyViaWindowsClipboard(text))` (`clipboard.ts:113`):
        // the interop write is skipped only when the escape actually went out.
        copied = osc52_emitted || copy_via_windows_clipboard(text).await;
    }
    // `if (!osc52Emitted && (isRemoteSession(env) || (!copied && headless)))`
    // (`clipboard.ts:120-123`): `emitOsc52` returning false is Pi's `oversized = true`, which becomes
    // the first rung of the throw ladder rather than a silent success.
    let mut oversized = false;
    if !osc52_emitted && osc52_required(env.remote, copied, headless(os, &env)) {
        match osc52_sequence(text) {
            Some(seq) => {
                write_stdout(&seq);
                copied = true;
            }
            None => oversized = true,
        }
    }
    // `if (copied) return;` then the ladder (`clipboard.ts:124-137`).
    if copied {
        Ok(())
    } else {
        Err(clipboard_failure(os, &env, oversized))
    }
}

// ------------------------------------------------------------------ the READ side (DRIFT-045) --

/// Pi `READ_CLIPBOARD_OPTIONS.timeout` (`clipboard.ts:39` @v0.84.2). Same 5 s as the write side,
/// spelled separately because upstream spells it separately.
const CLIPBOARD_READ_TIMEOUT: Duration = Duration::from_millis(5000);

/// One attempt in Pi `readClipboardText`'s two-step chain (`clipboard.ts:52-69` @v0.84.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardRead {
    /// `execFileSync("wl-paste", ["--no-newline", "--type", "text"], READ_CLIPBOARD_OPTIONS)`
    /// (`clipboard.ts:42-48`). Added upstream by pi `bfc679d5e`: the native addon is X11-oriented,
    /// so on Wayland it reads nothing.
    WlPaste,
    /// `await clipboard.getText()` (`clipboard.ts:65`) — `arboard` here, the same backend the
    /// Ctrl+V **image** read already uses.
    Native,
}

/// The ordered read chain for `os` under `env` — Pi `readClipboardText`'s branch structure with
/// the execution stripped out, for the same reason [`clipboard_write_plan`] is a pure function.
///
/// The Wayland gate is Pi's three-way conjunction verbatim: `platform() === "linux" &&
/// isWaylandSession() && process.env.WAYLAND_DISPLAY` (`clipboard.ts:53`). Note this is a
/// *narrower* platform test than the write side's `[CYRUP-DELTA]`, and deliberately so: a failed
/// read degrades to "no text", never to the silent false success that justified widening the write
/// arm off `p !== "linux"`.
///
/// The native step is behind [`native_clipboard_available`] — upstream's
/// `getNativeClipboard()?.getText()` (`clipboard.ts:62-66` @v0.87.1), whose optional chain skips the
/// read outright when the helper is not loaded (CFG-066).
pub(crate) fn clipboard_read_plan(os: &str, env: &ClipboardEnv) -> Vec<ClipboardRead> {
    let mut steps = Vec::new();
    if os == "linux" && env.wayland_session && env.wayland_display {
        steps.push(ClipboardRead::WlPaste);
    }
    if native_clipboard_available(os, env) {
        steps.push(ClipboardRead::Native);
    }
    steps
}

/// Pi `readWaylandClipboardText` (`clipboard.ts:42-49`), whose return type is the three-state
/// `ClipboardReadResult = { ok: true; text: string | null } | { ok: false }` (`:35`).
///
/// That third state is load-bearing and is why this returns `Option<Option<String>>` rather than
/// `Option<String>`: `readClipboardText` returns `result.text` whenever `ok` — **including when the
/// clipboard is empty** — and only falls through to the native backend when `wl-paste` itself
/// *failed* (`clipboard.ts:54-58`). Collapsing "ok but empty" into "failed" would make an empty
/// Wayland clipboard silently fall back to a stale X11 selection.
///
/// * `None` — `wl-paste` is missing, timed out, or exited non-zero (Pi's `catch` → `{ ok: false }`).
/// * `Some(None)` — ran, produced nothing (Pi's `text || null`).
/// * `Some(Some(text))` — ran, produced text.
fn read_wayland_clipboard_text() -> Option<Option<String>> {
    let out = run_capture("wl-paste", &["--no-newline", "--type", "text"])?;
    Some(Some(out).filter(|t| !t.is_empty()))
}

/// Run `bin args` to completion and capture stdout — Pi's `execFileSync(..., { encoding: "utf8",
/// maxBuffer: 50MB, timeout: 5000 })` (`clipboard.ts:36-40`). `None` on a spawn failure, a
/// non-zero exit, non-UTF-8 output, or the timeout.
///
/// `[CYRUP-DELTA]`, second half, stated rather than smoothed: Node's `timeout` KILLS the child
/// (`execFileSync` sends `killSignal`, SIGTERM by default). This does not — it stops WAITING. A
/// `wl-paste` that hangs forever therefore survives this call, along with the helper thread
/// holding it, where upstream would have reaped a terminated one. Not fixed here because
/// `Command::output()` consumes the `Child`, so killing from outside needs a raw pid and a
/// platform-specific signal, and a hung `wl-paste --no-newline` is not a state either side has
/// been observed in; the caller's 5 s bound is upstream's either way.
///
/// `[CYRUP-DELTA]` — the wait happens on a helper thread and the result comes back over a channel,
/// where Node has a timeout built into `execFileSync`. `std::process::Command::output()` reads the
/// pipe to EOF (so a 50 MB clipboard cannot deadlock on a full pipe buffer, which a `try_wait`
/// poll loop over a piped stdout would) but has no timeout of its own; `recv_timeout` supplies it.
/// The helper thread outlives the timeout only until the child exits, and it holds the `Child`, so
/// the process is still reaped rather than left a zombie. Behaviour matches upstream: the caller
/// waits at most 5 s and a hung helper yields "no text".
fn run_capture(bin: &str, args: &[&str]) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let bin = bin.to_string();
    let args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
    // A failure to spawn the thread leaves `rx` disconnected, which `recv_timeout` reports at once.
    let _ = std::thread::Builder::new()
        .name("cyrup-clipboard-read".to_string())
        .spawn(move || {
            let out = std::process::Command::new(bin)
                .args(args)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output();
            let _ = tx.send(out);
        });
    match rx.recv_timeout(CLIPBOARD_READ_TIMEOUT) {
        Ok(Ok(out)) if out.status.success() => String::from_utf8(out.stdout).ok(),
        // A missing helper, a non-zero exit, or the timeout — all Pi's `catch` (`clipboard.ts:46`).
        _ => None,
    }
}

/// Read plain text from the system clipboard — Pi `readClipboardText` (`clipboard.ts:52-69`
/// @v0.84.2). `None` when the clipboard holds no text or on any error (Pi returns `null` for both).
///
/// Synchronous where upstream is `async`, and that is the faithful shape rather than a shortcut:
/// pi's Wayland branch is `execFileSync`, i.e. it blocks its event loop too, and its native branch
/// is an addon call that resolves immediately. The one caller
/// ([`crate::app::App::handle_input`]) is on the render thread for the same reason pi's is on the
/// event loop.
pub(crate) fn read_clipboard_text() -> Option<String> {
    let env = ClipboardEnv::from_process();
    for step in clipboard_read_plan(std::env::consts::OS, &env) {
        match step {
            // `if (result.ok) return result.text` (`clipboard.ts:55-57`) — an `ok` result WINS,
            // empty or not; only a failure continues to the native step.
            ClipboardRead::WlPaste => {
                if let Some(text) = read_wayland_clipboard_text() {
                    return text;
                }
            }
            // `const text = await clipboard.getText(); return text || null` (`clipboard.ts:64-67`),
            // wrapped in Pi's `catch { return null }`.
            ClipboardRead::Native => {
                return arboard::Clipboard::new()
                    .and_then(|mut c| c.get_text())
                    .ok()
                    .filter(|t| !t.is_empty());
            }
        }
    }
    None
}

/// Write an escape straight to stdout and flush — the same shape as
/// [`crate::write_terminal_progress`], for the same reason: a buffered stdout would hold the
/// sequence until the next frame, and the terminal is the consumer, not the ratatui buffer.
fn write_stdout(seq: &str) {
    use std::io::Write as _;
    let mut out = crate::dead_terminal::terminal_stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
}
