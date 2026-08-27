//! `windowsHide: true` — the console half, for every `std::process::Command` this crate builds
//! whose real Pi counterpart passes the option.
//!
//! Node's `windowsHide: true` lowers to libuv's `UV_PROCESS_WINDOWS_HIDE`, which is TWO
//! suppressions: the creation flag `CREATE_NO_WINDOW` (a CONSOLE-subsystem child never allocates a
//! console) and `STARTUPINFO.wShowWindow = SW_HIDE` + `STARTF_USESHOWWINDOW` (the first window of a
//! GUI-subsystem child). Only the first is reachable from stable Rust:
//! `CommandExt::creation_flags` is stable since 1.16 and safe, while `CommandExt::show_window` is
//! `#[unstable(feature = "windows_process_extensions_show_window", issue = "127544")]`. Every
//! program spawned through this crate (`bash.exe`, `sh`, `where.exe`, `taskkill.exe`) is a
//! console-subsystem binary, so the console half is the half that governs all of them. RECORDED
//! DELTA: a GUI-subsystem child would show its first window here where Pi hides it.
//!
//! NOT applied to `super::local::command::build_argv_command` — see that function's doc comment.

/// `CREATE_NO_WINDOW` (`winbase.h`).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Apply `windowsHide: true`'s console half to `cmd`. A no-op everywhere but Windows, so call sites
/// stay `cfg`-free and cannot drift between platforms.
///
/// `CommandExt::creation_flags` ASSIGNS the flag word (`self.flags = flags` in
/// `std::sys::process::windows`), it does not OR into it — std then ORs in its own
/// `CREATE_UNICODE_ENVIRONMENT` before `CreateProcessW`. Nothing else under
/// `crates/cyrup-tools/**` sets creation flags today; anything that later needs one MUST pass
/// `CREATE_NO_WINDOW | …` in a single call rather than adding a second call, which would silently
/// replace this one.
pub(crate) fn windows_hide(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        let _ = cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}
