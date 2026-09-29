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

/// The program every Windows process-tree kill spawns: pi `killProcessTree`'s
/// `join(process.env.SystemRoot ?? "C:\\Windows", "System32", "taskkill.exe")`
/// (`utils/shell.ts:218-222` @v0.87.1, v0.84.4 #6596 — "Use the trusted System32 executable so
/// cleanup does not depend on PATH"). An absolute path skips `Command`'s program search entirely,
/// so a `taskkill.exe` beside `cyrup.exe` or earlier on `PATH` is never the one that runs.
pub fn taskkill_program() -> std::path::PathBuf {
    taskkill_program_under(std::env::var_os("SystemRoot").as_deref())
}

/// [`taskkill_program`] for a given `SystemRoot`, built with Windows separators on every host so
/// the ported join is pinned where the suite runs. `??` falls back only for an UNSET variable, so a
/// set-but-empty `SystemRoot` is kept and — as `path.join` drops empty segments — yields the bare
/// relative `System32\taskkill.exe`. A root already ending in a separator gets no second one.
fn taskkill_program_under(system_root: Option<&std::ffi::OsStr>) -> std::path::PathBuf {
    const TAIL: &str = "System32\\taskkill.exe";
    let root = system_root.unwrap_or(std::ffi::OsStr::new("C:\\Windows"));
    let mut program = root.to_os_string();
    match root.as_encoded_bytes().last() {
        None | Some(b'\\' | b'/') => {}
        Some(_) => program.push("\\"),
    }
    program.push(TAIL);
    program.into()
}

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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::taskkill_program_under;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    #[test]
    fn taskkill_is_resolved_under_system_root_not_by_name() {
        assert_eq!(
            taskkill_program_under(Some(OsStr::new("D:\\WINNT"))),
            PathBuf::from("D:\\WINNT\\System32\\taskkill.exe")
        );
        assert_eq!(
            taskkill_program_under(Some(OsStr::new("C:\\Windows\\"))),
            PathBuf::from("C:\\Windows\\System32\\taskkill.exe")
        );
    }

    #[test]
    fn an_unset_system_root_falls_back_to_c_windows() {
        assert_eq!(
            taskkill_program_under(None),
            PathBuf::from("C:\\Windows\\System32\\taskkill.exe")
        );
    }

    /// `??` is not `||`: an empty `SystemRoot` is a value, and `path.join("", "System32",
    /// "taskkill.exe")` is the relative `System32\taskkill.exe`.
    #[test]
    fn an_empty_system_root_is_kept_not_defaulted() {
        assert_eq!(
            taskkill_program_under(Some(OsStr::new(""))),
            PathBuf::from("System32\\taskkill.exe")
        );
    }
}
