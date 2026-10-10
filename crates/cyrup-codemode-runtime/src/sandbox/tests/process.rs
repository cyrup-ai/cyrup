//! The sandbox process: a script whose isolate lives in a process of its own cannot take the host
//! down, however it dies.
//!
//! The "sandbox process" under test is this test binary run again on [`sandbox_process_entry`],
//! the way the `cyrup` binary runs itself on `__codemode-sandbox`. The crash tests start a shell
//! instead, which dies or goes quiet in the ways a real engine can.

use std::time::{Duration, Instant};

use serde_json::json;

use super::support::*;
use crate::sandbox::{CodemodeSandbox, HostCommand, Isolation, run_sandbox_process};
use crate::types::{CodemodeResult, Deadline, ErrorKind, ExecuteOptions, SandboxOptions};

const ENTRY_ENV: &str = "CYRUP_CODEMODE_TEST_SANDBOX_PROCESS";
const PROBE_ENV: &str = "CYRUP_CODEMODE_TEST_SANDBOX_LIMITS";
const ENTRY: &str = "sandbox::tests::process::sandbox_process_entry";
const PROBE: &str = "sandbox::tests::process::sandbox_limits_probe_entry";

/// Not a test of its own: the entry point of the process the tests below start. Under an ordinary
/// run it does nothing and passes.
#[test]
fn sandbox_process_entry() {
    if std::env::var_os(ENTRY_ENV).is_some() {
        std::process::exit(run_sandbox_process());
    }
}

/// Not a test of its own either: reports the limits [`run_sandbox_process`] sets on itself, as a
/// crash frame the parent test reads.
#[test]
fn sandbox_limits_probe_entry() {
    if std::env::var_os(PROBE_ENV).is_none() {
        return;
    }
    #[cfg(unix)]
    {
        use nix::sys::resource::{Resource, getrlimit};

        use crate::sandbox::child::{limit_resources, process_ceiling};
        use crate::sandbox::protocol::IsolateInit;

        let limit = 256 * 1024 * 1024;
        limit_resources(&IsolateInit {
            code: String::new(),
            tools_json: String::new(),
            globals_json: String::new(),
            store_json: String::new(),
            memory_limit: Some(limit),
            active_limit_ms: None,
        });
        let data = getrlimit(Resource::RLIMIT_DATA).unwrap();
        let core = getrlimit(Resource::RLIMIT_CORE).unwrap();
        // Writable memory beyond the ceiling is refused by the operating system.
        let over = process_ceiling(Some(limit)) + (64 << 20);
        let refused = Vec::<u8>::new().try_reserve_exact(over).is_err();
        let report = format!(
            "data={}/{} core={}/{} ceiling={} over_ceiling_refused={refused}",
            data.0,
            data.1,
            core.0,
            core.1,
            process_ceiling(Some(limit)),
        );
        let frame = json!({ "Crash": report });
        println!("\u{1}cyrup-codemode-sandbox\u{1}{frame}");
    }
    std::process::exit(0);
}

fn host(entry: &str, env: &str) -> HostCommand {
    HostCommand::new(
        std::env::current_exe().unwrap(),
        ["--exact", entry, "--nocapture"],
    )
    .with_env(env, "1")
}

fn isolated(command: HostCommand, options: SandboxOptions) -> TestSandbox {
    TestSandbox::new(CodemodeSandbox::with_isolation(options, Isolation::Process(command)).unwrap())
}

fn real(memory_limit: u64) -> TestSandbox {
    isolated(
        host(ENTRY, ENTRY_ENV),
        SandboxOptions {
            memory_limit_bytes: Some(memory_limit),
            deadline: Deadline::After(Duration::from_secs(60)),
            ..SandboxOptions::default()
        },
    )
}

fn shell(script: &str) -> TestSandbox {
    isolated(
        HostCommand::new("/bin/sh", ["-c", script]),
        SandboxOptions {
            deadline: Deadline::After(Duration::from_secs(20)),
            ..SandboxOptions::default()
        },
    )
}

fn sandbox_failure(result: &CodemodeResult) -> String {
    let failure = error(result);
    assert_eq!(failure.kind, ErrorKind::Sandbox, "{result:?}");
    failure.message.clone()
}

// ------------------------------------------------------------------------------------------------
// A real engine in the process
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_script_runs_in_the_process_and_its_tools_and_output_cross_the_pipe() {
    let sandbox = {
        let options = SandboxOptions {
            tools: vec![echo()],
            deadline: Deadline::After(Duration::from_secs(60)),
            memory_limit_bytes: Some(256 << 20),
            ..SandboxOptions::default()
        };
        isolated(host(ENTRY, ENTRY_ENV), options)
    };
    let result = run(
        &sandbox,
        r#"
        text("printed");
        console.log({ a: 1 }, "two");
        const echoed = await tools.echo({ n: [1, 2, 3], s: "ünï\ncode \u{1} \u0001" });
        store("k", { v: 1 });
        return { echoed, pid: typeof Deno };
    "#,
    )
    .await;
    assert_eq!(
        value(&result),
        Some(
            json!({ "echoed": { "n": [1, 2, 3], "s": "ünï\ncode \u{1} \u{1}" }, "pid": "undefined" })
        )
    );
    assert_eq!(output(&result), [text("printed"), console("{\"a\":1} two")]);
    assert_eq!(call_summary(&result).len(), 1);
    let CodemodeResult::Completed { store_writes, .. } = &result else {
        panic!("{result:?}");
    };
    assert_eq!(store_writes.set.get("k"), Some(&json!({ "v": 1 })));

    let thrown = run(&sandbox, "text('before'); throw new TypeError('boom')").await;
    assert_eq!(error(&thrown).name.as_deref(), Some("TypeError"));
    assert_eq!(output(&thrown), [text("before")]);
    wait_until_idle(&sandbox).await;
}

/// [CYRUP-DELTA] The errors a script never looked at cross the pipe with its end.
#[tokio::test]
async fn the_errors_a_script_never_looked_at_cross_the_pipe() {
    let sandbox = {
        let options = SandboxOptions {
            tools: vec![
                sync_tool("boom", |_| Err("kaboom".to_owned())),
                sync_tool("fine", |_| Ok(Some(json!("fine")))),
            ],
            deadline: Deadline::After(Duration::from_secs(60)),
            memory_limit_bytes: Some(256 << 20),
            ..SandboxOptions::default()
        };
        isolated(host(ENTRY, ENTRY_ENV), options)
    };
    let result = run(
        &sandbox,
        "tools.boom({});\nPromise.reject(new Error('plain'));\nawait tools.fine({});\ntools.boom({});\nreturn 1;",
    )
    .await;
    assert_eq!(value(&result), Some(json!(1)));
    let (total, shown) = unobserved(&result);
    assert_eq!(total, 3, "{shown:?}");
    assert_eq!(
        shown
            .iter()
            .map(|(call, _)| call.as_deref())
            .collect::<Vec<_>>(),
        // In the order the engine found them: the plain rejection on the first line, then the call
        // the isolate heard of, then the one it never did.
        [None, Some("boom"), Some("boom")]
    );
    wait_until_idle(&sandbox).await;
}

/// [CYRUP-DELTA] A call something was waiting on is not an error the script lost when it fails after
/// the script ended, in the process as in the host's own thread: the script below ends on the first
/// failure its `Promise.all` hands it, and the other two calls fail afterwards. They cross the pipe
/// apart from the errors nobody handled.
#[tokio::test]
async fn calls_a_promise_all_took_in_cross_the_pipe_as_late_failures_when_the_script_ends_first() {
    let sandbox = {
        let options = SandboxOptions {
            tools: vec![sync_tool("boom", |_| Err("kaboom".to_owned()))],
            deadline: Deadline::After(Duration::from_secs(60)),
            memory_limit_bytes: Some(256 << 20),
            ..SandboxOptions::default()
        };
        isolated(host(ENTRY, ENTRY_ENV), options)
    };
    let result = run(
        &sandbox,
        r#"
        try { await Promise.all([tools.boom({}), tools.boom({}), tools.boom({})]); }
        catch (error) { text("caught " + error.message); }
        return 1;
    "#,
    )
    .await;
    assert_eq!(value(&result), Some(json!(1)));
    assert_eq!(output(&result), [text("caught kaboom")]);
    assert_eq!(unobserved(&result), (0, Vec::new()));
    assert_eq!(
        late_failures(&result),
        (
            2,
            vec![
                (Some("boom".to_owned()), "kaboom".to_owned()),
                (Some("boom".to_owned()), "kaboom".to_owned())
            ]
        )
    );
    wait_until_idle(&sandbox).await;
}

/// A script that ends while calls it did not await are still in flight is a completed script: the
/// replies the host sends afterwards reach a process whose isolate has ended, and the process must
/// not exit under the thread that is still writing the script's end. It did: the late reply made
/// the reader thread exit the process, the end of the script never reached the host, and the
/// script failed as "the sandbox process crashed ... it exited with status 0".
#[tokio::test]
async fn replies_that_arrive_after_the_script_ended_do_not_cost_it_its_result() {
    let sandbox = {
        let options = SandboxOptions {
            tools: vec![echo()],
            deadline: Deadline::After(Duration::from_secs(60)),
            memory_limit_bytes: Some(256 << 20),
            ..SandboxOptions::default()
        };
        isolated(host(ENTRY, ENTRY_ENV), options)
    };
    // The process writes a frame for each call before the one that ends the script, and the host
    // answers the early calls while it is still doing so.
    let result = run(
        &sandbox,
        "for (let i = 0; i < 4000; i++) tools.echo({ i }); return 'finished';",
    )
    .await;
    assert_eq!(value(&result), Some(json!("finished")), "{result:?}");
    wait_until_idle(&sandbox).await;
}

/// The measured blocker: `new Array(2 ** 27).fill(0)` under a 256 MiB heap limit aborted the whole
/// `cyrup` process with SIGTRAP after eight seconds, because V8 cannot recover from an allocation
/// the heap cannot satisfy. In a process of its own it is the script that fails.
#[tokio::test]
async fn one_allocation_the_heap_cannot_satisfy_fails_the_script_and_the_host_carries_on() {
    let sandbox = real(256 << 20);
    for hostile in [
        "const a = new Array(2 ** 27).fill(0); return a.length",
        "return Array.from({ length: 2 ** 28 }).length",
    ] {
        let started = Instant::now();
        let result = run(&sandbox, hostile).await;
        let failure = error(&result);
        assert_eq!(failure.kind, ErrorKind::Script, "{hostile}: {result:?}");
        assert_eq!(failure.name.as_deref(), Some("InternalError"), "{hostile}");
        assert_eq!(failure.message, "out of memory", "{hostile}");
        assert!(started.elapsed() < Duration::from_secs(50), "{hostile}");
        // The same sandbox runs the next script.
        let next = run(&sandbox, "return [1, 2, 3].map((x) => x * 2)").await;
        assert_eq!(value(&next), Some(json!([2, 4, 6])), "after {hostile}");
    }
    wait_until_idle(&sandbox).await;
}

#[tokio::test]
async fn output_printed_before_the_engine_aborts_still_reaches_the_host() {
    let sandbox = real(256 << 20);
    let result = run(
        &sandbox,
        "text('first'); console.log('second'); const a = new Array(2 ** 27).fill(0); return a.length",
    )
    .await;
    assert_eq!(error(&result).message, "out of memory", "{result:?}");
    assert_eq!(output(&result), [text("first"), console("second")]);
}

/// [CYRUP-DELTA] The running-time limit is kept by the isolate's own process and reported over the
/// pipe, so the supervisor learns of it as a timeout rather than as a process that went quiet.
#[tokio::test]
async fn a_spinning_script_in_a_process_stops_at_the_active_limit_and_is_reported_as_a_timeout() {
    let sandbox = isolated(
        host(ENTRY, ENTRY_ENV),
        SandboxOptions {
            memory_limit_bytes: Some(256 << 20),
            deadline: Deadline::After(Duration::from_secs(60)),
            active_limit: Some(Duration::from_millis(500)),
            ..SandboxOptions::default()
        },
    );
    let started = Instant::now();
    let result = run(&sandbox, "text('spinning'); for (;;) {}").await;
    let failure = error(&result);
    assert_eq!(failure.kind, ErrorKind::Timeout, "{result:?}");
    assert!(
        failure.message.contains("500 ms of its own time"),
        "{failure:?}"
    );
    assert_eq!(output(&result), [text("spinning")]);
    assert!(started.elapsed() < Duration::from_secs(20));
    wait_until_idle(&sandbox).await;
}

#[tokio::test]
async fn a_spinning_script_is_killed_at_its_deadline_and_the_process_is_reaped() {
    let sandbox = real(256 << 20);
    let started = Instant::now();
    let result = run_with(&sandbox, "for (;;) {}", deadline_ms(700)).await;
    assert_eq!(error(&result).kind, ErrorKind::Timeout, "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(20));
    // Idle means the supervisor's threads ended, and they end after the process is reaped.
    wait_until_idle(&sandbox).await;
    assert_eq!(value(&run(&sandbox, "return 7").await), Some(json!(7)));
}

#[tokio::test]
async fn closing_the_sandbox_kills_a_running_process() {
    let sandbox = std::sync::Arc::new(real(256 << 20));
    let running = {
        let sandbox = std::sync::Arc::clone(&sandbox);
        tokio::spawn(async move {
            sandbox
                .execute("for (;;) {}", ExecuteOptions::default())
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(800)).await;
    let started = Instant::now();
    sandbox.close().await;
    assert!(started.elapsed() < Duration::from_secs(10));
    let result = running.await.unwrap().unwrap();
    assert_eq!(error(&result).kind, ErrorKind::Aborted, "{result:?}");
    assert_eq!(sandbox.live(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn the_process_lowers_its_own_core_and_memory_limits() {
    let command = host(PROBE, PROBE_ENV);
    let sandbox = isolated(command, SandboxOptions::default());
    let result = run(&sandbox, "return 1").await;
    let report = sandbox_failure(&result);
    let ceiling = crate::sandbox::child::process_ceiling(Some(256 << 20));
    assert_eq!(
        report,
        format!("data={ceiling}/{ceiling} core=0/0 ceiling={ceiling} over_ceiling_refused=true")
    );
}

// ------------------------------------------------------------------------------------------------
// A process that dies, or never answers, in ways the engine cannot report
// ------------------------------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn a_process_that_aborts_is_a_failed_script_that_names_the_signal() {
    let sandbox = shell("kill -ABRT $$");
    let message = sandbox_failure(&run(&sandbox, "return 1").await);
    assert_eq!(
        message,
        "The sandbox process crashed before the script settled: it was terminated by signal 6 (SIGABRT)."
    );
    wait_until_idle(&sandbox).await;
}

/// A sandbox process limited by `ulimit -v` dies on a trap before it says a word (measured: SIGTRAP
/// at 16, 20, 24 and 32 GiB). The bare signal never named the limit; the message now does, but only
/// for that case.
#[cfg(unix)]
#[test]
fn a_signal_before_a_word_in_a_limited_address_space_names_the_limit() {
    use std::os::unix::process::ExitStatusExt as _;

    use crate::sandbox::process::describe_exit_in;

    let trapped = || Some(std::process::ExitStatus::from_raw(5));
    let message = describe_exit_in(trapped(), false, Some(16 << 30));
    assert!(
        message.starts_with(
            "The sandbox process crashed before the script settled: it was terminated by signal 5 (SIGTRAP)."
        ),
        "{message}"
    );
    assert!(
        message.contains("limited to 16 GiB (`ulimit -v`)") && message.contains("more than 32 GiB"),
        "{message}"
    );
    // No limit: the bare signal, as before.
    assert_eq!(
        describe_exit_in(trapped(), false, None),
        "The sandbox process crashed before the script settled: it was terminated by signal 5 (SIGTRAP)."
    );
    // It ran something first: the limit is not the explanation.
    assert!(!describe_exit_in(trapped(), true, Some(16 << 30)).contains("ulimit"));
    // An exit code is not a signal.
    let exited = Some(std::process::ExitStatus::from_raw(3 << 8));
    assert!(!describe_exit_in(exited, false, Some(16 << 30)).contains("ulimit"));
}

#[test]
fn address_space_is_shown_in_gib_with_at_most_one_decimal() {
    use crate::sandbox::process::gib;

    assert_eq!(gib(16 << 30), "16 GiB");
    assert_eq!(gib(3 << 29), "1.5 GiB");
    assert_eq!(gib(0), "0 GiB");
}

#[cfg(unix)]
#[tokio::test]
async fn a_process_that_exits_without_a_word_is_a_failed_script_that_names_the_status() {
    let sandbox = shell("exit 3");
    let message = sandbox_failure(&run(&sandbox, "return 1").await);
    // [CYRUP-DELTA] An exit code before a word is a binary that did not understand the host (a newer
    // cyrup installed over this one), which a restart cures.
    assert_eq!(
        message,
        "The sandbox process crashed before the script settled: it exited with status 3. If cyrup was upgraded or reinstalled while this session was open, restart cyrup."
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_process_that_exits_after_speaking_does_not_blame_an_upgrade() {
    // It ran something, so it is not a binary that failed to start: no advice to restart.
    let sandbox = shell(r#"printf '\001cyrup-codemode-sandbox\001{"Text":"hi"}\n'; exit 3"#);
    let message = sandbox_failure(&run(&sandbox, "return 1").await);
    assert_eq!(
        message,
        "The sandbox process crashed before the script settled: it exited with status 3."
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_fatal_out_of_memory_report_is_the_scripts_out_of_memory_error() {
    let sandbox =
        shell(r#"printf '\001cyrup-codemode-sandbox\001"OutOfMemory"\n'; exec sleep 3600"#);
    let result = run(&sandbox, "return 1").await;
    let failure = error(&result);
    assert_eq!(failure.kind, ErrorKind::Script, "{result:?}");
    assert_eq!(failure.name.as_deref(), Some("InternalError"));
    assert_eq!(failure.message, "out of memory");
    // The process that reported it is killed, not waited for.
    wait_until_idle(&sandbox).await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_message_the_host_cannot_read_ends_the_script_and_the_process() {
    let sandbox = shell(r#"printf '\001cyrup-codemode-sandbox\001{not json\n'; exec sleep 3600"#);
    let message = sandbox_failure(&run(&sandbox, "return 1").await);
    assert!(
        message.starts_with("The sandbox process sent an unreadable message:"),
        "{message}"
    );
    wait_until_idle(&sandbox).await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_process_that_ignores_everything_is_killed_at_the_deadline() {
    // `sleep` never reads its input or looks at anything the isolate would: only a kill stops it.
    let sandbox = shell("exec sleep 3600");
    let started = Instant::now();
    let result = run_with(&sandbox, "return 1", deadline_ms(300)).await;
    assert_eq!(error(&result).kind, ErrorKind::Timeout, "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
    wait_until_idle(&sandbox).await;
}

#[cfg(unix)]
#[tokio::test]
async fn the_process_starts_with_an_empty_environment() {
    // Reports `$CARGO_MANIFEST_DIR` and `$HOME`, which the test run has and a script's process must not.
    assert!(std::env::var_os("CARGO_MANIFEST_DIR").is_some());
    let sandbox = isolated(
        HostCommand::new(
            "/bin/sh",
            [
                "-c",
                r#"printf '\001cyrup-codemode-sandbox\001{"Crash":"manifest=[%s] home=[%s] set=[%s]"}\n' "$CARGO_MANIFEST_DIR" "$HOME" "$KEPT"; exec sleep 3600"#,
            ],
        )
        .with_env("KEPT", "yes"),
        SandboxOptions::default(),
    );
    let message = sandbox_failure(&run(&sandbox, "return 1").await);
    assert_eq!(message, "manifest=[] home=[] set=[yes]");
}

#[tokio::test]
async fn a_program_that_cannot_start_is_a_failed_script() {
    let sandbox = isolated(
        HostCommand::new("/nonexistent/cyrup-sandbox", ["--none"]),
        SandboxOptions::default(),
    );
    let message = sandbox_failure(&run(&sandbox, "return 1").await);
    // [CYRUP-DELTA] A missing program is, for the executable cyrup started from, an upgrade or a
    // reinstall while the session was open: the message names the program and says to restart.
    assert_eq!(
        message,
        "Failed to start the sandbox process: No such file or directory (os error 2): /nonexistent/cyrup-sandbox does not exist. If cyrup was upgraded or reinstalled while this session was open, restart cyrup."
    );
    assert_eq!(sandbox.live(), 0);
}

// ------------------------------------------------------------------------------------------------
// The binary this session runs is no longer the file it started from
// ------------------------------------------------------------------------------------------------

const REPLACED_ENV: &str = "CYRUP_CODEMODE_TEST_REPLACED_EXE";
const REPLACED: &str = "sandbox::tests::process::replaced_executable_entry";

/// Not a test of its own: the process [`a_session_still_starts_sandbox_processes_after_its_binary_is_removed`]
/// and its sibling run. It builds the sandbox command, then removes or replaces its own executable
/// as `REPLACED_ENV` says, then runs a script.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn replaced_executable_entry() {
    let Some(how) = std::env::var_os(REPLACED_ENV) else {
        return;
    };
    let exe = std::env::current_exe().unwrap();
    let command = HostCommand::running_executable(["--exact", ENTRY, "--nocapture"])
        .unwrap()
        .with_env(ENTRY_ENV, "1");
    if how == "remove" {
        std::fs::remove_file(&exe).unwrap();
    } else {
        // A different program renamed over the path, as an installer does.
        use std::os::unix::fs::PermissionsExt;
        let other = exe.with_extension("other");
        std::fs::write(&other, "#!/bin/sh\nexit 3\n").unwrap();
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(&other, &exe).unwrap();
    }
    let sandbox = isolated(
        command,
        SandboxOptions {
            deadline: Deadline::After(Duration::from_secs(60)),
            ..SandboxOptions::default()
        },
    );
    let result = run(&sandbox, "return 1 + 1").await;
    assert_eq!(value(&result), Some(json!(2)), "{result:#?}");
    println!("replaced-executable-ok");
}

/// Runs this test binary from a copy of itself, which it removes or replaces once it has built the
/// sandbox command.
#[cfg(target_os = "linux")]
fn run_from_a_copy(how: &str) {
    let exe = std::env::current_exe().unwrap();
    let copy = exe.with_file_name(format!("replaced-{how}-{}", std::process::id()));
    // Next to the original, so a hard link is always possible; the copy is the fallback.
    std::fs::hard_link(&exe, &copy)
        .or_else(|_| std::fs::copy(&exe, &copy).map(drop))
        .unwrap();
    let output = std::process::Command::new(&copy)
        .args(["--exact", REPLACED, "--nocapture"])
        .env(REPLACED_ENV, how)
        .output();
    let _ = std::fs::remove_file(&copy);
    let output = output.unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("replaced-executable-ok"),
        "status {:?}\nstdout:\n{stdout}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_session_still_starts_sandbox_processes_after_its_binary_is_removed() {
    run_from_a_copy("remove");
}

#[cfg(target_os = "linux")]
#[test]
fn a_session_still_starts_sandbox_processes_after_another_binary_replaced_its_own() {
    run_from_a_copy("replace");
}

// ------------------------------------------------------------------------------------------------
// What a returned value costs the host
// ------------------------------------------------------------------------------------------------

const MEMORY_ENV: &str = "CYRUP_CODEMODE_TEST_HOST_MEMORY";
const MEMORY: &str = "sandbox::tests::process::host_memory_probe_entry";

/// Peak resident memory of this process so far, in KiB (`VmHWM`).
#[cfg(target_os = "linux")]
fn peak_resident_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap()
}

/// Not a test of its own: runs a script that returns a 16 MB array of small objects in a sandbox
/// process and prints how far this process's peak resident memory grew meanwhile. This process is
/// the host side only; the script and its value live in the sandbox process.
#[cfg(target_os = "linux")]
#[test]
fn host_memory_probe_entry() {
    if std::env::var_os(MEMORY_ENV).is_none() {
        return;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (grew, length) = runtime.block_on(async {
        let sandbox = real(256 << 20);
        // The first script pays for starting the first sandbox process and everything lazy.
        run(&sandbox, "return 1").await;
        let before = peak_resident_kib();
        let result = run(
            &sandbox,
            "return Array.from({ length: 1250000 }, (_, i) => ({ a: i }));",
        )
        .await;
        let after = peak_resident_kib();
        let CodemodeResult::Completed {
            value: Some(value), ..
        } = &result
        else {
            panic!("{result:?}");
        };
        (after.saturating_sub(before), value.as_json().len())
    });
    println!("host-memory length={length} grew_kib={grew}");
    std::process::exit(0);
}

/// `CODE-043`: the host read a returned value into a `serde_json::Value`, which holds data made of
/// many small objects at tens of times the size of its text; a 16 MB array of `{ a: n }` took it
/// about 700 MB, for a value it only prints. It keeps the text now, and that array costs it a few
/// copies of its text. Measured in a process of its own, because the peak of a process that also
/// runs other tests is not this test's.
#[cfg(target_os = "linux")]
#[test]
fn a_large_returned_value_costs_the_host_a_few_copies_of_its_text() {
    /// Far below the 700 MB the tree took, far above the 100 MB the text takes.
    const LIMIT_KIB: u64 = 300 * 1024;
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", MEMORY, "--nocapture"])
        .env(MEMORY_ENV, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let field = |name: &str| -> u64 {
        stdout
            .split_whitespace()
            .find_map(|word| word.strip_prefix(name))
            .and_then(|number| number.parse().ok())
            .unwrap_or_else(|| {
                panic!(
                    "no `{name}` in the probe's output\nstatus {:?}\nstdout:\n{stdout}\nstderr:\n{}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                )
            })
    };
    // The workload is the one the limit is about: 16388891 characters of JSON.
    assert_eq!(field("length="), 16_388_891, "{stdout}");
    let grew = field("grew_kib=");
    assert!(
        grew < LIMIT_KIB,
        "the host grew by {} MiB to hold a 16 MB returned value",
        grew / 1024
    );
}
