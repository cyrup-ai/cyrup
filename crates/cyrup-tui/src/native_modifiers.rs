//! Native modifier probing and the Shift+Enter rescue for terminals that swallow the modifier
//! (ports `pi/packages/tui/src/native-modifiers.ts` and the Shift+Enter half of
//! `pi/packages/tui/src/terminal.ts:10-52,316-327`).
//!
//! # The problem
//!
//! Apple Terminal does not encode modifiers on <kbd>Enter</kbd>. Whether the user presses
//! <kbd>Enter</kbd> or <kbd>Shift</kbd>+<kbd>Enter</kbd>, the byte stream carries exactly one
//! `\r` — no CSI-u sequence, no `modifyOtherKeys` wrapper. Nothing in the input stream can tell the
//! two apart, so a terminal-only implementation submits the message when the user meant to insert a
//! newline. Upstream's fix is to ask the OS: at the moment a bare `\r` arrives, read the *live*
//! keyboard modifier state through a small native helper and, if Shift is physically held, rewrite
//! the input to the Kitty sequence `\x1b[13;2u` — which the key decoder already reads as
//! `shift+enter`, i.e. `tui.input.newLine`.
//!
//! ```ts
//! // v0.83.0 tui/src/terminal.ts:44-47
//! export function normalizeAppleTerminalInput(data, isAppleTerminal, isShiftPressed) {
//!     if (isAppleTerminal && data === "\r" && isShiftPressed) return APPLE_TERMINAL_SHIFT_ENTER_SEQUENCE;
//!     return data;
//! }
//! ```
//!
//! At **v0.84.1** the function was renamed `normalizeNativeShiftEnterInput` and the gate widened to
//! `isAppleTerminalSession() || process.platform === "win32"` (`terminal.ts:44-55,316-327`), with
//! `loadNativeModifiersHelper` gaining a `win32` prebuild branch
//! (`native-modifiers.ts:24-36`). The Apple-Terminal half is v0.83.0 behaviour that was never
//! ported; the Windows half is post-baseline. Both are implemented here — the platform gate is a
//! parameter of [`should_detect_native_shift_enter`], so neither is a compile-time constant.
//!
//! # Mechanism note
//!
//! Upstream `cjsRequire`s a prebuilt `.node` addon and silently degrades to "no modifier is
//! pressed" whenever the addon is missing (`native-modifiers.ts:39-52`, and the explicit
//! `// Native helper not available` comment at `terminal.ts:305`). cyrup has no addon loader, so
//! the same optional-helper seam is a **registered probe**: [`set_native_modifier_probe`] installs
//! one, [`is_native_modifier_pressed`] consults it, and with none installed it answers `false` —
//! byte-for-byte upstream's helper-absent path.

use std::sync::{LazyLock, RwLock};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// A physical modifier key the native probe can be asked about (`native-modifiers.ts:7`
/// `type ModifierKey = "shift" | "command" | "control" | "option"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModifierKey {
    Shift,
    Command,
    Control,
    Option,
}

/// A live keyboard-modifier probe — the cyrup analogue of upstream's
/// `NativeModifiersHelper.isModifierPressed` (`native-modifiers.ts:9-11`).
pub type ModifierProbe = fn(ModifierKey) -> bool;

static PROBE: LazyLock<RwLock<Option<ModifierProbe>>> = LazyLock::new(|| RwLock::new(None));

/// Install the process-wide native modifier probe (upstream's successful
/// `loadNativeModifiersHelper()`, `native-modifiers.ts:21-50`). Returns the probe it replaced.
pub fn set_native_modifier_probe(probe: ModifierProbe) -> Option<ModifierProbe> {
    let mut slot = PROBE.write().unwrap_or_else(|e| e.into_inner());
    slot.replace(probe)
}

/// Remove the installed probe, returning the platform to upstream's helper-absent behaviour (every
/// modifier reads as released).
pub fn clear_native_modifier_probe() -> Option<ModifierProbe> {
    let mut slot = PROBE.write().unwrap_or_else(|e| e.into_inner());
    slot.take()
}

/// Whether `key` is physically held right now (`native-modifiers.ts:52-59`
/// `isNativeModifierPressed`). `false` when no probe is installed, exactly as upstream returns
/// `false` when the native helper cannot be loaded (`:54`).
pub fn is_native_modifier_pressed(key: ModifierKey) -> bool {
    let probe = *PROBE.read().unwrap_or_else(|e| e.into_inner());
    probe.is_some_and(|p| p(key))
}

/// Upstream `isAppleTerminalSession()` (v0.83.0 `terminal.ts:43-45`):
/// `process.platform === "darwin" && process.env.TERM_PROGRAM === "Apple_Terminal"`.
///
/// `platform` takes upstream's `process.platform` spelling (`"darwin"`, `"win32"`, `"linux"`, …) so
/// the darwin branch is reachable from a test on any host; [`host_platform`] supplies the real one.
/// The `TERM_PROGRAM` comparison is exact and case-sensitive, as upstream's `===` is.
pub fn is_apple_terminal_session(platform: &str, term_program: Option<&str>) -> bool {
    platform == "darwin" && term_program == Some("Apple_Terminal")
}

/// This build's `process.platform` string. A compile-time constant, hence never a substitute for
/// the `platform` parameter in a test.
pub fn host_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

/// Whether this key event is the ambiguous bare carriage return that needs the native probe —
/// upstream's `shouldDetectNativeShiftEnter` (v0.84.1 `terminal.ts:319-320`):
///
/// ```ts
/// const shouldDetectNativeShiftEnter =
///     sequence === "\r" && (isAppleTerminalSession() || process.platform === "win32");
/// ```
///
/// `sequence === "\r"` is a bare CR: no modifier was encoded in the stream. crossterm's equivalent
/// is `KeyCode::Enter` with **no** modifiers — a terminal that did encode Shift already reports
/// `SHIFT` and must be left alone. Key *releases* are excluded; upstream never sees them (its
/// decoder only forwards presses).
///
/// At v0.83.0 the second disjunct did not exist (`terminal.ts:311`, Apple Terminal only); the
/// `win32` arm is v0.84.1.
///
/// `[CYRUP-DELTA]` **The `win32` arm is ported and correct, and cannot be reached in this runtime.**
/// It is kept because it is what upstream does and because it becomes live the moment anything puts
/// the Windows console into VT-input mode. Nothing does today, and that is the whole difference.
///
/// Upstream's Windows helper is named for its primary job: `win32-console-mode.c:65-72` exports
/// `enable_virtual_terminal_input`, which runs `SetConsoleMode(handle, mode |
/// ENABLE_VIRTUAL_TERMINAL_INPUT)`. That switch replaces `INPUT_RECORD` key events with a VT escape
/// stream, discarding the modifier bits the console had already decoded — so pi must recover them
/// with `GetAsyncKeyState` (`:38-58`). The probe is the repair for a loss upstream elects.
///
/// cyrup does not elect it. crossterm 0.29 sets `ENABLE_VIRTUAL_TERMINAL_PROCESSING` on the OUTPUT
/// handle only (`ansi_support.rs:17`) and never the input flag; its Windows `enable_raw_mode` merely
/// clears `NOT_RAW_MODE_MASK` (`terminal/sys/windows.rs:31-38`); and its event source reads console
/// records, mapping `SHIFT_PRESSED` straight into `KeyModifiers::SHIFT`
/// (`event/sys/windows/parse.rs:81`). A real Shift+Enter therefore arrives with `SHIFT` already set
/// and is rejected by this function's own `ev.modifiers != KeyModifiers::NONE` guard, one line below,
/// before any probe is consulted. There is no byte-stream fallback to reach it by either:
/// `WindowsEventSource::new` is `Console::from(Handle::current_in_handle()?)`
/// (`event/source/windows.rs:28`), so with no console handle it fails to construct rather than
/// degrading to escapes.
///
/// The consequence is that Windows keeps the modifier data pi throws away, so on the configuration
/// analysed above this arm is inert.
///
/// **A Windows probe is registered regardless** (`crates/cyrup/src/main.rs`), and deliberately. The
/// analysis above is a source read of crossterm performed on Linux; it has never been observed on
/// Windows, and it holds only while nothing enables VT input. Weigh the two errors: an inert probe
/// costs a `#[cfg(windows)]` module, while a missing one costs every Windows user a Shift+Enter that
/// submits their message instead of inserting a newline, silently, the moment any of that changes.
/// This is the one place in this crate where an uncalled function is kept on purpose rather than
/// treated as an unported caller, and this paragraph is why.
pub fn should_detect_native_shift_enter(
    ev: &KeyEvent,
    platform: &str,
    term_program: Option<&str>,
) -> bool {
    if ev.code != KeyCode::Enter
        || ev.modifiers != KeyModifiers::NONE
        || matches!(ev.kind, KeyEventKind::Release)
    {
        return false;
    }
    is_apple_terminal_session(platform, term_program) || platform == "win32"
}

/// Upstream `normalizeNativeShiftEnterInput` (v0.84.1 `terminal.ts:44-52`; v0.83.0's
/// `normalizeAppleTerminalInput`, `:44-47`), with the byte rewrite expressed as the key event it
/// decodes to.
///
/// Upstream substitutes `\x1b[13;2u`, the Kitty encoding of `shift+enter`, which its own `keys.ts`
/// then matches against `tui.input.newLine`. cyrup's decoder is crossterm, so the equivalent value
/// is the `KeyEvent` that sequence produces: `Enter` + `SHIFT`. Everything else passes through
/// untouched.
pub fn normalize_native_shift_enter(
    ev: KeyEvent,
    should_detect: bool,
    is_shift_pressed: bool,
) -> KeyEvent {
    if should_detect && is_shift_pressed {
        return KeyEvent {
            modifiers: ev.modifiers | KeyModifiers::SHIFT,
            ..ev
        };
    }
    ev
}

/// The whole rescue, parameterised over the environment and the probe — upstream's
/// `ProcessTerminal.forwardInputSequence` (v0.84.1 `terminal.ts:316-327`) with `process.platform`,
/// `process.env.TERM_PROGRAM` and the native helper lifted into arguments.
///
/// Note the short-circuit: the probe is consulted **only** for a bare `Enter` on a platform that
/// needs it (upstream's `shouldDetectNativeShiftEnter && isNativeModifierPressed("shift")`,
/// `:323-325`), so no other keystroke pays for it.
pub fn rescue_native_shift_enter(
    ev: KeyEvent,
    platform: &str,
    term_program: Option<&str>,
    probe: impl Fn(ModifierKey) -> bool,
) -> KeyEvent {
    let should_detect = should_detect_native_shift_enter(&ev, platform, term_program);
    normalize_native_shift_enter(
        ev,
        should_detect,
        should_detect && probe(ModifierKey::Shift),
    )
}
