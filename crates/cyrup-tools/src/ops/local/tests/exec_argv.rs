//! [`LocalProc::exec_argv`] — the WASM `exec` capability path, whose escalation is a SINGLE-pid
//! `SIGTERM`-then-grace-then-`SIGKILL` and never a process-group kill.
//!
//! [`LocalProc::exec_argv`]: crate::ops::ProcOps::exec_argv
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use cyrup_core::CancelToken;

/// A normal (SIGTERM-obeying) child dies well within the grace period on timeout — no SIGKILL
/// escalation needed. Guards against a regression that makes EVERY timeout/cancel wait out the
/// full grace period regardless of whether the tree already died (mirrors
/// `cyrup_ext::caps::proc::kill_terminates_a_real_running_child_and_the_os_process_is_gone`).
///
/// `sleep` does not trap SIGTERM, so it dies to the RAW signal (no exit code) — Pi's own
/// `code ?? 0` null-coalescing case (`exec.ts:97`) — which `exit_from` reports as `Signaled`;
/// `killed` is still `true` because a termination WAS initiated (orthogonal to `status`, see
/// `ArgvOutput`'s doc comment). This must NOT collapse to the bare `TimedOut` reason tag — that
/// was the bug (a real terminal status discarded whenever `pending` was `Some`).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_argv_timeout_kills_a_normal_child_well_within_grace() {
    let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_secs(5));
    let started = tokio::time::Instant::now();
    let out = proc
        .exec_argv(
            argv("sleep", &["30"]),
            CancelToken::new(),
            Some(Duration::from_millis(200)),
        )
        .await
        .expect("exec_argv runs");
    assert_eq!(
        out.status,
        ExitStatus::Signaled,
        "the REAL observed status (died to the raw signal) is reported, not the bare TimedOut tag"
    );
    assert!(
        out.killed,
        "a timeout-initiated kill is still `killed`, independent of `status`"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a SIGTERM-obeying child (`sleep`) must die well within the 5s grace period, got {:?}",
        started.elapsed()
    );
}

/// L4 round-12 finding #3: an [`ArgvSpec`] with an EMPTY `cwd` must NOT hard-fail the spawn —
/// `build_argv_command` skips `current_dir` entirely for an empty path, matching Node's real
/// `child_process.spawn`, which treats a falsy `cwd` as "no override" and inherits the parent's
/// own ambient cwd (verified live: Node `spawn("pwd",[],{cwd:""})` exits 0), unlike
/// `std::process::Command::current_dir("")`, which hard-fails with `Os { code: 2, kind:
/// NotFound, .. }` (also verified live). Proven by actually running `pwd` with `cwd:
/// PathBuf::new()` and reading its REAL stdout: it must equal THIS TEST PROCESS's own ambient
/// cwd (Rust's `Command` default when `.current_dir()` is never called at all).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_argv_with_an_empty_cwd_inherits_the_ambient_cwd_instead_of_hard_failing() {
    let proc = LocalProc::new(ShellConfig::detect());
    let spec = ArgvSpec {
        program: "pwd".to_string(),
        args: Vec::new(),
        cwd: std::path::PathBuf::new(),
        env: Vec::new(),
    };
    let out = proc
        .exec_argv(spec, CancelToken::new(), None)
        .await
        .expect("exec_argv must not hard-fail on an empty cwd");
    assert_eq!(
        out.status,
        ExitStatus::Exited(0),
        "pwd must run and exit cleanly"
    );
    let printed =
        std::fs::canonicalize(String::from_utf8_lossy(&out.stdout).trim_end()).unwrap_or_default();
    let ambient = std::env::current_dir().expect("this test process has a cwd");
    assert_eq!(
        printed,
        std::fs::canonicalize(&ambient).unwrap_or(ambient),
        "an empty cwd must inherit the ambient process cwd, not error or default to something else"
    );
}

/// THE regression this fix closes: a well-behaved child that TRAPS SIGTERM and exits itself
/// with its OWN real, nonzero exit code mid-grace must have that REAL code reported — 1:1 with
/// Pi's `waitForChildProcess`/`finalize(exitCode)` (`child-process.ts:73-80`), which always
/// resolves with the actual observed `code`, `killed` bolted on separately (`exec.ts:97`). The
/// old cyrup behavior collapsed this to a hard-coded `code 0` any time a kill was in flight.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_argv_timeout_preserves_the_real_code_of_a_graceful_sigterm_handler() {
    let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_secs(5));
    let sleeper = SleeperMarker::new("gracefulterm");
    let out = proc
        .exec_argv(
            argv(
                "sh",
                &["-c", &sleeper.reapable_sleep_loop("trap 'exit 7' TERM; ")],
            ),
            CancelToken::new(),
            Some(Duration::from_millis(200)),
        )
        .await
        .expect("exec_argv runs");
    assert_eq!(
        out.status,
        ExitStatus::Exited(7),
        "the child's OWN real exit code from its SIGTERM handler must survive, not be \
         discarded to 0 because a kill was in flight"
    );
    assert!(
        out.killed,
        "a timeout-initiated kill is still `killed`, independent of `status`"
    );
    sleeper.reap();
}

/// The FORCED SIGKILL escalation, exercised deterministically (mirrors
/// `cyrup_ext::caps::proc::kill_escalates_to_sigkill_when_the_child_ignores_sigterm`): a
/// process-group leader that traps SIGTERM and loops forever ignores the graceful signal
/// outright, so `exec_argv`'s timeout branch MUST wait out the (test-shortened) grace period
/// and then SIGKILL the whole group — which cannot be ignored — to actually terminate it.
/// Proves the escalation is real, not just documented (closes L4 §2.4).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_argv_timeout_escalates_to_sigkill_when_the_child_ignores_sigterm() {
    let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_millis(150));
    let sleeper = SleeperMarker::new("argvtimeoutkill");
    let started = tokio::time::Instant::now();
    let out = proc
        .exec_argv(
            argv(
                "sh",
                &["-c", &sleeper.reapable_sleep_loop("trap '' TERM; ")],
            ),
            CancelToken::new(),
            Some(Duration::from_millis(100)),
        )
        .await
        .expect("exec_argv runs");
    sleeper.reap();
    assert_eq!(
        out.status,
        ExitStatus::Signaled,
        "a forced SIGKILL reports the real (signal, no code) status, not the bare TimedOut tag"
    );
    assert!(out.killed, "a timeout-initiated kill is still `killed`");
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "the 100ms timeout + 150ms grace period was genuinely waited out before escalating to \
         SIGKILL, got {:?}",
        started.elapsed()
    );
}

/// The same escalation on the `cancel` path (not just `timeout`): an abort mid-run SIGTERMs
/// first, and only SIGKILLs the SIGTERM-ignoring group after the grace period elapses.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_argv_cancel_escalates_to_sigkill_when_the_child_ignores_sigterm() {
    let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_millis(150));
    let sleeper = SleeperMarker::new("argvcancelkill");
    let cancel = CancelToken::new();
    let started = tokio::time::Instant::now();
    let task = tokio::spawn({
        let cancel = cancel.clone();
        let spec = argv(
            "sh",
            &["-c", &sleeper.reapable_sleep_loop("trap '' TERM; ")],
        );
        async move { proc.exec_argv(spec, cancel, None).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();
    let out = task.await.expect("task joins").expect("exec_argv runs");
    sleeper.reap();
    assert_eq!(
        out.status,
        ExitStatus::Signaled,
        "a forced SIGKILL reports the real (signal, no code) status, not the bare Killed tag"
    );
    assert!(out.killed, "a cancel-initiated kill is still `killed`");
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "the grace period was genuinely waited out before escalating to SIGKILL, got {:?}",
        started.elapsed()
    );
}

/// THE regression this fix closes: bytes already sitting in the kernel pipe buffer at the
/// instant the grace-elapsed arm forces a SIGKILL must NOT be silently dropped. The old code
/// sent SIGKILL, `child.wait()`ed, and `break`ed immediately — never re-polling `read_chunk` —
/// so whatever the child had already written but this loop hadn't yet drained was lost.
///
/// Ground-truth harness: a SIGTERM-ignoring child appends an increasing counter to an
/// independent file via an fd opened ONCE (`exec 3>>`, so the loop spins as fast as the shell
/// can manage rather than being disk-syscall-bound), written BEFORE the matching stdout
/// `printf` each iteration with no `sleep`, so the file's last line is always >= whatever made
/// it to stdout. With the fix, `read_chunk` keeps draining until TRUE EOF (the kernel only
/// signals EOF once every byte written before the writer's fd closed has been delivered to the
/// reader) — so captured stdout can lag the ground truth by AT MOST the single in-flight
/// iteration straddling the SIGKILL instant, never by a whole buffered chunk's worth. Repeated
/// several times since the exact SIGKILL timing relative to the child's write cadence is
/// inherently racy — verified live: with this exact script reverted to the pre-fix
/// (immediate-`break`) behavior, this test failed deterministically on trial 0 across 3
/// separate runs (deficits of 2-3 lines each); with the fix, 5 separate runs (40 trials total)
/// were all clean.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_argv_forced_sigkill_does_not_drop_buffered_stdout_already_sitting_in_the_pipe() {
    for trial in 0..8u32 {
        let gt_path = std::env::temp_dir().join(format!(
            "cyrup-exec-argv-gt-{}-{trial}.txt",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&gt_path);
        let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_millis(15));
        // fd 3 is opened ONCE (`exec 3>>`) rather than re-opened every iteration (a fresh
        // `>>` open/write/close per line is disk-syscall-bound and slow enough that the async
        // reader never falls behind) — this lets the loop spin as fast as the shell can manage
        // (bounded only by `printf`/arithmetic), maximizing how many iterations land inside the
        // short grace window and thus the odds of catching the exact SIGKILL race.
        let script = format!(
            "exec 3>>{}; trap '' TERM; i=0; while true; do printf '%s\\n' \"$i\" >&3; \
             printf '%s\\n' \"$i\"; i=$((i+1)); done",
            gt_path.display()
        );
        // TOOL-030/TOOL-020: the RUN window (250ms) is decoupled from the KILL GRACE (15ms,
        // configured above). The grace is what this test exercises — SIGTERM is trapped, so
        // the forced-SIGKILL escalation still fires 15ms after the timeout — while the run
        // window only has to guarantee the child completed at least one loop iteration before
        // being killed. At 15ms that guarantee was a scheduling gamble (fork + exec of
        // `/bin/sh` plus one iteration inside roughly 30ms); at 250ms it holds by construction
        // on any host that can start a process at all, with the SIGKILL race the test is
        // actually about completely unchanged.
        let out = proc
            .exec_argv(
                argv("sh", &["-c", &script]),
                CancelToken::new(),
                Some(Duration::from_millis(250)),
            )
            .await
            .expect("exec_argv runs");
        assert!(
            out.killed,
            "trial {trial}: a SIGTERM-ignoring child must be force-killed"
        );

        let ground_truth = std::fs::read_to_string(&gt_path).unwrap_or_default();
        let _ = std::fs::remove_file(&gt_path);
        let gt_last: i64 = ground_truth
            .lines()
            .next_back()
            .and_then(|l| l.parse().ok())
            .unwrap_or(-1);
        let captured = String::from_utf8_lossy(&out.stdout);
        let stdout_last: i64 = captured
            .lines()
            .next_back()
            .and_then(|l| l.parse().ok())
            .unwrap_or(-1);

        assert!(
            gt_last >= 0,
            "trial {trial}: the child must have run at least one loop iteration before being \
             killed (ground truth file was empty) — with a 250ms run window this is a real \
             failure, not the scheduling race the old 15ms window made it"
        );
        // TOOL-020 claimed this bound "assumes the host `ShellConfig::detect()` shell flushes
        // stdout once per iteration". That half is REFUTED at HEAD: `exec_argv` runs the
        // program it is handed, and this call hands it `argv("sh", …)` literally — the
        // `ShellConfig::detect()` passed to `with_kill_grace` is only consulted by `exec`, not
        // by `exec_argv`. The dependence is on `/bin/sh`'s builtin `printf`, which flushes per
        // command, and is identical on every POSIX host.
        assert!(
            gt_last - stdout_last <= 1,
            "trial {trial}: captured stdout (last line {stdout_last}) lagged the ground-truth \
             file (last line {gt_last}) by more than the one single in-flight iteration the \
             SIGKILL can legitimately straddle — buffered pipe bytes were dropped at the \
             forced-SIGKILL boundary"
        );
    }
}

/// L4 round-17 finding #1: `exec_argv`'s kill escalation MUST signal only the single spawned
/// pid — Pi's real `execCommand`/`killProcess` (`exec.ts:34-63`) spawns with no `detached`
/// option and kills via a bare, un-negated `proc.kill("SIGTERM"/"SIGKILL")`, which Node always
/// delivers to `this.pid` alone, never a process group. Proven by actually spawning a SIBLING
/// process in the exact same process group as the `exec_argv`-spawned command (both inherit
/// THIS TEST's own group, since [`build_argv_command`] deliberately does not `setsid`), letting
/// `exec_argv`'s timeout escalate all the way to `SIGKILL`, and confirming the sibling survived.
/// The regression this guards against (`killpg` targeting the whole group) would have killed
/// this sibling as collateral damage — and, worse, in production would signal the WASM guest
/// engine's own ambient process group, since a real `exec_argv` caller is never `setsid`'d either.
///
/// [`build_argv_command`]: crate::ops::local::command::build_argv_command
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_argv_kill_signals_only_the_single_pid_never_the_process_group() {
    let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_millis(150));
    let marker = std::env::temp_dir().join(format!(
        "cyrup-exec-argv-singlepid-{}.pid",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&marker);
    // The outer `sh` is what `exec_argv` directly spawns and kills; it backgrounds a SIBLING
    // `sleep 30` (via `&`, no `setsid`) that inherits the SAME process group and writes its own
    // pid to `marker` before the outer shell blocks on `wait`.
    let script = format!("sleep 30 & echo $! > {}; wait", marker.display());
    let out = proc
        .exec_argv(
            argv("sh", &["-c", &script]),
            CancelToken::new(),
            Some(Duration::from_millis(100)),
        )
        .await
        .expect("exec_argv runs");
    assert!(out.killed, "the timeout must have initiated a kill");

    let sibling_pid: u32 = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(s) = std::fs::read_to_string(&marker)
                && let Ok(pid) = s.trim().parse()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the background sibling must have written its pid before the outer shell died");
    let _ = std::fs::remove_file(&marker);

    // `terminate_pid` doubles as the liveness probe AND cleanup here: `Ok(true)` means the
    // sibling was genuinely still alive (proving `exec_argv`'s kill never reached it) and also
    // terminates it so the test doesn't leak a real `sleep 30` process; `Err` (`ESRCH`) would
    // mean it was already dead — exactly what the group-kill regression would cause.
    let sibling_was_alive = terminate_pid(sibling_pid).unwrap_or(false);
    assert!(
        sibling_was_alive,
        "a same-process-group sibling of the exec_argv-spawned command must survive its \
         SIGTERM/SIGKILL escalation — exec_argv's kill must target only the single spawned \
         pid, mirroring Pi's real execCommand/killProcess (exec.ts:34-63), never `killpg`"
    );
}

/// Reproduces the exact hang class Pi's `EXIT_STDIO_GRACE_MS` idle timer exists to close
/// (`waitForChildProcess`, `child-process.ts:49-137`, earendil-works/pi#5303): the spawned
/// command backgrounds a descendant (`sleep 5 &`) that inherits our stdout pipe and then exits
/// itself immediately. Without an idle-grace fallback, `child.wait()` never runs (gated on both
/// streams reaching EOF) and the still-open pipe never reaches EOF either — an unconditional
/// hang. With the fix, the loop must finalize within `EXIT_STDIO_GRACE` of the parent's own
/// exit, well under the backgrounded descendant's 5s lifetime.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_argv_does_not_hang_on_a_backgrounded_descendant_holding_the_pipe_open() {
    let proc = LocalProc::new(ShellConfig::detect());
    // The descendant records its own pid so this fixture can REAP it. The pipe-holding shape it
    // exists to prove is unchanged — `sleep` is still backgrounded out of a subshell that exits
    // immediately, still inherits the exec stdout/stderr pipes, and is still alive for the whole
    // of the assertion window below — but it no longer survives the test process by ~4.9s. A
    // fixture that deliberately orphans a process is the exact "spawns and does not reap" shape
    // the surrounding suite is being audited for, and under `cargo nextest run` a survivor with
    // inherited handles is what turns into a `LEAK-FAIL`.
    let marker = std::env::temp_dir().join(format!(
        "cyrup-exec-argv-idlegrace-{}.pid",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&marker);
    let script = format!("( sleep 5 & echo $! > {} ) ; exit 0", marker.display());
    let started = tokio::time::Instant::now();
    let out = tokio::time::timeout(
        Duration::from_secs(3),
        proc.exec_argv(argv("sh", &["-c", &script]), CancelToken::new(), None),
    )
    .await
    .expect("exec_argv must not hang past the idle-grace fallback")
    .expect("exec_argv runs");
    assert_eq!(
        out.status,
        ExitStatus::Exited(0),
        "the parent's own clean exit is reported"
    );
    assert!(
        !out.killed,
        "a natural exit with no cancel/timeout is never `killed`"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "must finalize within EXIT_STDIO_GRACE of the parent's exit, not wait on the \
         backgrounded descendant's pipe, got {:?}",
        started.elapsed()
    );

    // `terminate_pid` doubles as the liveness PROOF and the cleanup: `Ok(true)` means the
    // descendant was genuinely still alive at this point — i.e. `exec_argv` really did finalize
    // while the pipe was still held open, which is the whole premise of the timing assertion
    // above — and the same call is what stops it outliving this process.
    let descendant = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .expect("the backgrounded descendant must have written its pid");
    let _ = std::fs::remove_file(&marker);
    assert!(
        terminate_pid(descendant).unwrap_or(false),
        "the backgrounded descendant must still be alive here — otherwise the idle-grace \
         fallback was never the thing that let `exec_argv` return"
    );
}
