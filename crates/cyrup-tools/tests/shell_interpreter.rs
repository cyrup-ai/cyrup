//! ADR-0003 D8(1)+(2) — cyrup never silently picks an interpreter the user did not choose.
//!
//! Pi's `getShellConfig` (`pi/packages/coding-agent/src/utils/shell.ts:67-120` @v0.83.0) takes
//! exactly one input, `customShellPath`, and reads **no environment variable as a shell selector**;
//! its only `process.env` reads are the Windows installation-location lookups `ProgramFiles` /
//! `ProgramFiles(x86)` (`:79`, `:83`) that build the Git Bash candidate list. cyrup had grown a
//! `CYRUP_SHELL` arm ahead of the `/bin/bash` probe, which is what TOOL-039 records and what
//! ADR-0003 D1 deletes.
//!
//! # Why this file holds exactly ONE test
//!
//! It mutates the process environment, and `std::env::set_var` is `unsafe` in Rust 2024 because it
//! races ANY concurrent `getenv` — not only a reader looking for the same key. The boundary that
//! makes a mutation sound is therefore the THREAD, not the test and not the binary: it is sound
//! only when nothing else in the process is running.
//!
//! One `#[test]` in its own binary satisfies that. Two did not, and this file used to hold two,
//! serialized on a `Mutex` — which made the mutation unobserved rather than sound, and put the
//! burden on every future test added here to remember to take it. The sibling
//! (`cyrup_shell_appears_nowhere_under_crates`) needed no environment at all and now lives in
//! `tests/shell_interpreter_literal_absent.rs`; the lock went with it.
//!
//! Keep it that way. A second test in this file re-creates the race, and no lock fixes it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
// Exempt from the workspace `disallowed-methods` guard: this file proves `detect` IGNORES
// `CYRUP_SHELL`, so the variable must really be set in the process — no injected lookup can prove
// the ABSENCE of a read. Sound because this binary holds one `#[test]`, so no other thread in the
// process runs concurrently with the mutation. See the module doc for why that is the criterion
// and why a lock was not.
#![allow(clippy::disallowed_methods)]

use cyrup_tools::ops::ShellConfig;
use std::path::{Path, PathBuf};

/// D8(1) — `ShellConfig::try_detect()` ignores `CYRUP_SHELL`.
///
/// RED before ADR-0003 D1 (the deleted arm at `ops/shell.rs:101-105` returned the sentinel first,
/// ahead of the `/bin/bash` probe at `:109-110`); GREEN after.
#[test]
fn detect_ignores_cyrup_shell_env_var() {
    // SAFETY: this is the only `#[test]` in this binary and it spawns no threads and starts no
    // runtime, so nothing in this process runs concurrently with these mutations. That is the
    // condition `set_var` actually requires — it races any concurrent `getenv` for any key.
    unsafe {
        std::env::set_var("CYRUP_SHELL", "/no/such/interpreter/sentinel");
    }

    let cfg = ShellConfig::try_detect().expect("unix detection cannot fail (shell.ts:119)");
    assert_ne!(
        cfg.program,
        PathBuf::from("/no/such/interpreter/sentinel"),
        "the environment must not select the interpreter (shell.ts:67-120 reads no such variable)"
    );

    #[cfg(unix)]
    {
        // Pi's unix order (shell.ts:109-119): `/bin/bash`, then `which bash`, then `sh -c`.
        let expected = if Path::new("/bin/bash").exists() {
            PathBuf::from("/bin/bash")
        } else {
            PathBuf::from("sh")
        };
        if expected == *"/bin/bash" {
            assert_eq!(cfg.program, expected);
        } else {
            // No `/bin/bash`: either `which bash` found one, or the `sh` fallback fired.
            assert!(
                cfg.program == *"sh" || cfg.program.file_name().is_some_and(|n| n == "bash"),
                "got {:?}",
                cfg.program
            );
        }
    }

    // SAFETY: unchanged from the write above — one `#[test]` in this binary, no threads spawned
    // and no runtime started, so nothing in this process runs concurrently with the scrub. The
    // criterion is the THREAD and it covers ANY key.
    unsafe {
        std::env::remove_var("CYRUP_SHELL");
    }
}
