//! ADR-0003 D8(1)+(2) — cyrup never silently picks an interpreter the user did not choose.
//!
//! Pi's `getShellConfig` (`pi/packages/coding-agent/src/utils/shell.ts:67-120` @v0.83.0) takes
//! exactly one input, `customShellPath`, and reads **no environment variable as a shell selector**;
//! its only `process.env` reads are the Windows installation-location lookups `ProgramFiles` /
//! `ProgramFiles(x86)` (`:79`, `:83`) that build the Git Bash candidate list. cyrup had grown a
//! `CYRUP_SHELL` arm ahead of the `/bin/bash` probe, which is what TOOL-039 records and what
//! ADR-0003 D1 deletes.
//!
//! Lives in its own integration binary on purpose: it mutates the process environment, so no test
//! in any OTHER file can observe the mutation. Inside this file there is a second test, and the two
//! are serialized on [`ENV_LOCK`] — see it for why "the sibling never reads the environment" is not
//! the same safety argument.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use cyrup_tools::ops::ShellConfig;
use std::path::{Path, PathBuf};

/// Serializes this file's two tests against each other.
///
/// `std::env::set_var` is `unsafe` in Rust 2024 because it is a data race against ANY concurrent
/// `getenv` in the process — not only against a reader that is looking for the same key. The
/// sibling below never reads `CYRUP_SHELL`, but it does walk the source tree, and the libc calls
/// underneath a directory walk are entitled to consult the environment (locale, `TZ`). The file's
/// original argument for soundness was that this was the only test in the binary; it has not been
/// true since the sibling landed, and this lock is what makes the `unsafe` blocks below sound again
/// rather than merely unobserved.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take [`ENV_LOCK`], ignoring poisoning — a sibling that panicked has already reported its own
/// failure, and refusing the lock here would turn that into a second, misleading one.
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// D8(1) — `ShellConfig::detect()` ignores `CYRUP_SHELL`.
///
/// RED before ADR-0003 D1 (the deleted arm at `ops/shell.rs:101-105` returned the sentinel first,
/// ahead of the `/bin/bash` probe at `:109-110`); GREEN after.
#[test]
fn detect_ignores_cyrup_shell_env_var() {
    let _env = env_lock();
    // SAFETY: `ENV_LOCK` is held for this whole body and the only other test in this binary takes
    // it too, so no thread in this process runs concurrently with these mutations.
    unsafe {
        std::env::set_var("CYRUP_SHELL", "/no/such/interpreter/sentinel");
    }

    let cfg = ShellConfig::detect();
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
        if expected == PathBuf::from("/bin/bash") {
            assert_eq!(cfg.program, expected);
        } else {
            // No `/bin/bash`: either `which bash` found one, or the `sh` fallback fired.
            assert!(
                cfg.program == PathBuf::from("sh")
                    || cfg.program.file_name().is_some_and(|n| n == "bash"),
                "got {:?}",
                cfg.program
            );
        }
    }

    unsafe {
        std::env::remove_var("CYRUP_SHELL");
    }
}

/// D8(2) — the literal must appear nowhere under `crates/`, so the arm cannot be reintroduced under
/// a different guise (a debug-only arm, a `#[cfg(test)]` arm, a `CYRUP_SHELL_PATH` alias).
#[test]
fn cyrup_shell_appears_nowhere_under_crates() {
    // Held for the same reason the sibling holds it: not because this test reads `CYRUP_SHELL`, but
    // because `set_var` races every `getenv` in the process, this one's directory walk included.
    let _env = env_lock();
    let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/cyrup-tools has a parent")
        .to_path_buf();
    let this_file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("shell_interpreter.rs");

    // Assembled at runtime so this guard does not match itself through a constant.
    let needle: String = ["CYRUP", "SHELL"].join("_");

    let mut offenders: Vec<String> = Vec::new();
    let mut stack = vec![crates_dir];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") && path != this_file {
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (i, line) in text.lines().enumerate() {
                    if line.contains(&needle) {
                        offenders.push(format!("{}:{}", path.display(), i + 1));
                    }
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "`{needle}` has no Pi analogue (shell.ts:67-120 reads no interpreter variable) and must \
         not exist under crates/ — ADR-0003 D1. Found at: {offenders:?}"
    );
}
