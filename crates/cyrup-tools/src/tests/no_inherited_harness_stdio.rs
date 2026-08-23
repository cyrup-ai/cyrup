//! The structural pin for this crate's `LEAK-FAIL` class: **no `Command` built anywhere under
//! `crates/cyrup-tools/**` may leave a stdio handle at its `inherit()` default.**
//!
//! ## What the bug was
//!
//! `cargo nextest run -p cyrup-tools` intermittently reported one `LEAK-FAIL`. nextest hands each
//! test process a pipe for its stdout and stderr and, after that process exits, waits
//! `leak-timeout` (500 ms, `.config/nextest.toml:42`) for the pipe to reach EOF. EOF requires every
//! copy of the WRITE end to be closed — so any surviving descendant that inherited the test
//! process's fd 1 or fd 2 holds the pipe open and turns a passing test into a red.
//!
//! `std::process::Command` defaults every handle the caller does not name to `Stdio::inherit()`, and
//! `dup2` is what disconnects a child from the harness: naming a handle replaces it, leaving a
//! handle unnamed passes the harness's own pipe straight through. So the leak class is created by
//! **omission**, which is exactly the kind of thing that is invisible in review.
//!
//! At the time the flake was observed, one spawn in this crate omitted two of the three:
//! `ops/shell.rs`'s `path_probe_is_bounded` fixture spawned `sleep 30` naming only `stdout`, which
//! handed the child the harness's stdin AND stderr — and its `Err(_) => break` arm left that child
//! running, unkilled and unreaped, for its full 30 seconds. Both halves are fixed at their source
//! (`ops/shell.rs`); this test is what stops the shape coming back, in that file or a new one.
//!
//! ## Why this is a source scan and not a runtime probe
//!
//! The invariant is a property of every `Command` in the crate, including the `#[cfg(windows)]`
//! `taskkill` spawns that cannot execute on this host at all. A runtime probe could only observe
//! the spawns that this platform reaches, which is the smaller half. Reading the crate's own source
//! covers all of them and cannot flake.
//!
//! ## The rule
//!
//! For each `Command::new(` occurrence (which matches `std::process::` and `tokio::process::`
//! alike), the window that follows must EITHER name all three of `.stdin(` / `.stdout(` /
//! `.stderr(`, OR terminate in `.output()`. `.output()` is the one std constructor that overrides
//! all three by itself (stdin null, stdout and stderr piped), so it is safe by construction;
//! `.status()` is deliberately NOT exempt, because it INHERITS stdout and stderr.

// Same allow-set the crate's other test modules carry. Without it `cargo clippy -p cyrup-tools
// --all-targets` is RED here (the workspace `deny`s `expect`/raw slicing), which a check-only gate
// never sees because clippy lints do not fire under `cargo check`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};

/// The window, in lines, in which a spawn's stdio must be pinned. Generous on purpose: cyrup's
/// builders interleave long citation comments with the builder calls (`build_command` spans 16
/// lines from `Command::new` to `.stderr(...)` in `ops/local/command.rs`), and the point of the
/// rule is to catch a spawn that names NOTHING, not to police formatting.
const WINDOW_LINES: usize = 60;

/// Every `.rs` file under `dir`, recursively.
fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `crates/cyrup-tools/{src,tests}` — both, because a leaked child from an out-of-crate integration
/// binary holds that binary's harness pipe exactly the same way.
fn scanned_roots() -> Vec<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    vec![manifest.join("src"), manifest.join("tests")]
}

#[test]
fn every_command_in_this_crate_pins_all_three_stdio_handles() {
    let mut files = Vec::new();
    for root in scanned_roots() {
        rust_sources(&root, &mut files);
    }
    // Assert PRESENCE before absence: a scan that found no files, or a `rust_sources` that silently
    // returned nothing, would make every assertion below vacuously true.
    assert!(
        files.len() >= 10,
        "the source scan found only {} files under crates/cyrup-tools/{{src,tests}} — the walk is \
         broken, so the spawn audit below would pass vacuously",
        files.len()
    );

    let mut spawn_sites = 0usize;
    let mut violations: Vec<String> = Vec::new();

    for file in &files {
        // This file names `Command::new(` inside its own documentation and its own failure
        // messages; scanning it would report itself.
        if file
            .file_name()
            .is_some_and(|n| n == "no_inherited_harness_stdio.rs")
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if !line.contains("Command::new(") {
                continue;
            }
            spawn_sites += 1;
            // The window ends at the next spawn site, so two adjacent spawns cannot satisfy the
            // rule with one set of handles between them.
            let end = lines
                .iter()
                .enumerate()
                .skip(index + 1)
                .find(|(_, l)| l.contains("Command::new("))
                .map_or(lines.len(), |(i, _)| i)
                .min(index + WINDOW_LINES);
            let window = lines[index..end].join("\n");
            if window.contains(".output()") {
                continue;
            }
            let missing: Vec<&str> = [".stdin(", ".stdout(", ".stderr("]
                .into_iter()
                .filter(|handle| !window.contains(handle))
                .collect();
            if !missing.is_empty() {
                violations.push(format!(
                    "{}:{} names {:?} but not {:?}",
                    file.display(),
                    index + 1,
                    [".stdin(", ".stdout(", ".stderr("]
                        .into_iter()
                        .filter(|h| window.contains(h))
                        .collect::<Vec<_>>(),
                    missing
                ));
            }
        }
    }

    // Second presence assertion: the crate DOES spawn processes, so a zero here means the scan
    // stopped matching (a rename of `Command::new`, a macro, a helper) rather than that the crate
    // became spawn-free.
    assert!(
        spawn_sites >= 6,
        "the audit matched only {spawn_sites} `Command::new(` sites in a crate that spawns bash, \
         `which`, `sleep` and `taskkill` — the matcher has stopped seeing real spawns, so a green \
         result proves nothing"
    );

    assert!(
        violations.is_empty(),
        "every `Command` under crates/cyrup-tools/** must name all three stdio handles (or end in \
         `.output()`), or the child inherits the nextest harness's own stdout/stderr pipe and a \
         survivor turns the test red as a LEAK-FAIL (.config/nextest.toml:42). Offenders:\n  {}",
        violations.join("\n  ")
    );
}

/// The THIRD shape in this class, and the one that survived two passes because it is not a
/// `Command::new` at all: a fixture whose SHELL SCRIPT forks a long-lived descendant that no
/// `Stdio` setting can reach and that `exec_argv`'s single-pid kill is upstream-correct not to
/// touch (`exec.ts:34-63` @v0.83.0 — pi spawns without `detached` and calls a bare, un-negated
/// `proc.kill`, so a grandchild survives by design).
///
/// Such a survivor is what makes the `LEAK-FAIL` victim ARBITRARY. macOS has no `pipe2(2)`, so
/// Rust's `anon_pipe` is `pipe(2)` + a separate `ioctl(FIOCLEX)`; a spawn landing inside another
/// pipe's pre-`FIOCLEX` window inherits that pipe's write end above fd 2, where no `dup2` reaches.
/// nextest creates every test's stdout/stderr pipes in ONE process while concurrently spawning test
/// binaries, so the stray write end can belong to a completely different test — and it stays
/// observable exactly as long as some process holds it. The test process itself releases it on
/// exit; a `sleep 1` grandchild does not, and `leak-timeout` is 500 ms
/// (`.config/nextest.toml:42`). See `ops/local/tests/mod.rs`'s `SleeperMarker` for the measurement.
///
/// So: any fixture script in this crate that forks a sleeper in a loop must also record its pid
/// (`echo $!`) so the fixture can reap it. `echo $!` is the marker every already-correct fixture in
/// `ops/local/tests/` uses; requiring it is what stops a new fixture reintroducing the shape.
#[test]
fn fixture_scripts_that_fork_a_sleeper_record_its_pid_so_the_fixture_can_reap_it() {
    let mut files = Vec::new();
    for root in scanned_roots() {
        rust_sources(&root, &mut files);
    }
    let mut loops = 0usize;
    let mut violations: Vec<String> = Vec::new();
    for file in &files {
        // Self-exclusion for the same reason as the scan above: this file names the pattern in its
        // own documentation and failure message.
        if file
            .file_name()
            .is_some_and(|n| n == "no_inherited_harness_stdio.rs")
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            // Prose, not code: this rule is about what a fixture RUNS.
            if line.trim_start().starts_with("//") {
                continue;
            }
            // The shape: a shell script that forks `sleep` out of a loop or into the background. A
            // one-shot `sleep 30` as the DIRECT child is not this — `exec_argv`/`exec` kill that
            // pid themselves.
            if !(line.contains("do sleep")
                || line.contains("sleep 1 &")
                || line.contains("sleep 5 &")
                || line.contains("sleep 30 &"))
            {
                continue;
            }
            loops += 1;
            // `exec_spec(...)` is `LocalProc::exec`, the `bash`-tool/immediate-bash path, whose
            // `build_command` `setsid`s the shell into its OWN process group and whose every
            // termination path — timeout, cancel and `KillTreeOnDrop` — is a `killpg` of that whole
            // group (pi's `killProcessTree`, `shell.ts:200-225`). The descendant is reaped by the
            // production code there, so the fixture owes no marker. Only the single-pid-kill
            // `exec_argv` path (pi's `execCommand`/`killProcess`, `exec.ts:34-63`) leaves one.
            if line.contains("exec_spec(") {
                continue;
            }
            if !line.contains("echo $!") {
                violations.push(format!("{}:{}: {}", file.display(), index + 1, line.trim()));
            }
        }
    }

    // Presence before absence: these fixtures are the point of the rule, so zero matches means the
    // matcher stopped seeing them, not that the crate stopped forking sleepers.
    assert!(
        loops >= 4,
        "the scan matched only {loops} forked-sleeper fixture scripts in a crate whose exec tests \
         are built on them — the matcher has stopped seeing them, so a green result proves nothing"
    );
    assert!(
        violations.is_empty(),
        "a fixture script that forks a `sleep` must record its pid with `echo $!` so the fixture \
         can reap it before returning; an unreaped one-second grandchild holds any fd it inherited \
         past nextest's 500ms leak-timeout and turns an UNRELATED test into a LEAK-FAIL. \
         Offenders:\n  {}",
        violations.join("\n  ")
    );
}

/// The other half of the same failure: a spawn that IS pinned but is never reaped still leaves a
/// live process behind, and if it was pinned to `piped()` its parent's read end is what the pipe
/// hangs off. Pins the specific arm that regressed — every `try_wait` polling loop in
/// `ops/shell.rs` must kill and wait on BOTH its expiry path and its error path.
///
/// Kept as a source assertion rather than a behavioural one for the same reason as above: the
/// `Err(_)` arm of `try_wait` is not reachable on demand from a test.
#[test]
fn shell_probe_loops_reap_on_the_error_arm_not_just_the_deadline() {
    let shell = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/ops/shell.rs");
    let text = std::fs::read_to_string(&shell).expect("ops/shell.rs must be readable");
    // Presence before absence: if `try_wait` ever leaves this file, the absence check below would
    // pass against a file that no longer contains the construct at all.
    let loops = text.matches("child.try_wait()").count();
    assert!(
        loops >= 2,
        "expected the bounded-probe `try_wait` loops in ops/shell.rs (the production probe and its \
         fixture); found {loops}"
    );
    // `Err(_) => break` was the leaking arm: it left the spawned child alive and unreaped. Every
    // `Err` arm must kill and wait first.
    assert!(
        !text.contains("Err(_) => break"),
        "ops/shell.rs has a bare `Err(_) => break` in a `try_wait` loop again — that arm abandons a \
         live child without `kill()`/`wait()`, which is the \"spawns and does not reap\" shape \
         behind the cyrup-tools LEAK-FAIL flake"
    );
}
