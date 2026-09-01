//! ADR-0003 D8(2) — the `CYRUP_SHELL` literal must appear nowhere under `crates/`, so the deleted
//! interpreter-selection arm cannot be reintroduced under a different guise (a debug-only arm, a
//! `#[cfg(test)]` arm, a `CYRUP_SHELL_PATH` alias).
//!
//! # Why this is its own binary
//!
//! It used to live beside `detect_ignores_cyrup_shell_env_var` in `tests/shell_interpreter.rs`,
//! which MUTATES the process environment. Two tests in one binary run on two threads under the
//! default `cargo test` harness, and `std::env::set_var` is `unsafe` in Rust 2024 because it races
//! ANY concurrent `getenv` — not only a reader looking for the same key. This test never reads
//! `CYRUP_SHELL`, but it walks the source tree, and the libc calls underneath a directory walk are
//! entitled to consult the environment.
//!
//! The sibling file therefore held a `Mutex` that both tests took, purely so this one would not be
//! raced. A lock is the wrong tool: it makes the mutation *unobserved* rather than *sound*, it has
//! to be taken by every future test anyone adds to that file, and it is exactly the shape this
//! workspace has been removing. Splitting costs nothing — this test needs no environment at all —
//! and it leaves `shell_interpreter.rs` holding one test, where "no other thread is running"
//! is a property of the binary rather than a promise a lock has to keep.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;

/// D8(2) — the literal appears nowhere under `crates/`.
#[test]
fn cyrup_shell_appears_nowhere_under_crates() {
    let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/cyrup-tools has a parent")
        .to_path_buf();
    // Two files are allowed to contain it, and both are exclusions the guard has always needed:
    //
    // * `shell_interpreter.rs` — the test that proves `detect` IGNORES the variable has to set it;
    // * THIS file, which names it in prose. The original carried the same self-exclusion under the
    //   name `this_file`; splitting the two tests apart split the exemption too, and dropping half
    //   of it made this guard match its own module doc.
    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let exempt = [
        tests_dir.join("shell_interpreter.rs"),
        tests_dir.join("shell_interpreter_literal_absent.rs"),
    ];

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
            } else if path.extension().is_some_and(|e| e == "rs") && !exempt.contains(&path) {
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
