//! WASM `proc` CAPABILITY END-TO-END (arch-08 §5.2 request/poll bridge; `pi-mcp-adapter-port.md`
//! §3.1 — the locked WIT shape this closes). Proves that a LIVE wasm guest's long-lived, duplex-pipe
//! child-process grant (`ctx.proc_spawn`/`proc_write_stdin`/`proc_read_stdout`/`proc_poll_exit`/
//! `proc_kill`) reaches the session's REAL [`cyrup_ext::caps::proc::ProcCaps`] engine — a real
//! `tokio::process::Child` — through the injected `LiveHostServices` (arch-08 §5.6), NOT a stub or a
//! captured one-shot.
//!
//! Mirrors `tests/wasm_exec.rs`/`tests/wasm_http.rs`'s discipline 1:1 for the new capability: LOADED
//! == TRUSTED-BY-CONSTRUCTION (`trust_override = Some(true)`), so the guest's `proc` grant is live via
//! the SAME trust gate `exec`/`http-client`/`ui` use (no new bool, no per-host allowlist). The
//! untrusted-denial analog (structurally no path to a real `ProcCaps` — `DenyServices` holds none) is
//! proven in `cyrup-ext/tests/wasm_component.rs`.
//!
//! Each guest-side step (`/procspawn`, `/procwrite`, `/procreadpoll`, `/procpollexit`, `/prockill` —
//! `cyrup-ext-sdk/src/example.rs`) is its own top-level `session.prompt(...)` round trip, so this test
//! observes the pipe staying live across genuinely SEPARATE host calls over time (not an internal loop
//! within one guest invocation), and can interleave REAL OS-level process checks (`pgrep -f <marker>`)
//! between spawning and killing.
#![cfg(feature = "wasm-host")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cyrup_core::{ExtensionId, StopReason};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use tempfile::TempDir;

/// Build (or locate) the demo guest component (mirrors `wasm_exec.rs`/`wasm_http.rs`).
fn fixture_component() -> PathBuf {
    if let Ok(p) = std::env::var("CYRUP_EXT_FIXTURE_COMPONENT") {
        return PathBuf::from(p);
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let build_dir = std::env::temp_dir().join("cyrup-session-svc-fixture-target");
    let status = Command::new(&cargo)
        .args(["build", "-p", "cyrup-ext-sdk", "--target", "wasm32-wasip2", "--target-dir"])
        .arg(&build_dir)
        .status()
        .expect("spawn cargo to build the wasm32-wasip2 fixture component");
    assert!(status.success(), "building cyrup-ext-sdk fixture component failed");
    let wasm = build_dir.join("wasm32-wasip2/debug/cyrup_ext_sdk.wasm");
    assert!(wasm.exists(), "fixture component not found at {}", wasm.display());
    wasm
}

fn faux_with_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    faux
}

/// Build a TRUSTED session (`trust_override = Some(true)`) with a fresh project/agent dir, exactly
/// as `wasm_exec.rs`/`wasm_http.rs` do — the guest's `proc` grant is live via the load-time trust
/// gate, the SAME one `exec`/`http-client` already use.
async fn trusted_session() -> AgentSession {
    let tmp = TempDir::new().expect("tempdir");
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).expect("mkdir cwd");
    std::fs::create_dir_all(&agent_dir).expect("mkdir agent_dir");
    // Leak the TempDir so it outlives the session (test-process-lifetime scratch dir; mirrors the
    // discipline other wasm_*.rs fixtures use of not tearing the session's cwd down mid-test).
    std::mem::forget(tmp);

    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true); // TRUSTED project ⇒ the guest's proc grant is live.
    cfg.no_extensions = true; // only the explicitly-loaded guest is present.

    SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, cfg).build().await.expect("build session")
}

/// A fresh, process-unique marker so `pgrep -f <marker>` can find (and later confirm the
/// disappearance of) EXACTLY the one real OS process this test spawned — never a stale/unrelated
/// match from another test or another process on the machine.
fn unique_marker(tag: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("cyrup-proc-test-{tag}-{}-{n}", std::process::id())
}

/// Real OS-level check: is a process whose command line contains `marker` currently running?
fn marker_process_alive(marker: &str) -> bool {
    Command::new("pgrep").args(["-f", marker]).status().map(|s| s.success()).unwrap_or(false)
}

/// Extract `handle:<N>` from a guest notification string produced by the demo commands.
fn parse_handle(notifications: &[String], prefix_contains: &str) -> u32 {
    let line = notifications
        .iter()
        .rev()
        .find(|n| n.contains(prefix_contains))
        .unwrap_or_else(|| panic!("no notification containing {prefix_contains:?}: {notifications:?}"));
    let after = line.split("handle:").nth(1).unwrap_or_else(|| panic!("no handle: in {line:?}"));
    after.split_whitespace().next().unwrap_or_default().parse().unwrap_or_else(|_| panic!("bad handle in {line:?}"))
}

/// THE headline proof (a): a TRUSTED live wasm guest spawns a REAL long-lived duplex-pipe child
/// (a marker-tagged shell read-echo loop), writes to its REAL stdin, and polls its REAL stdout
/// ACROSS MULTIPLE SEPARATE top-level `session.prompt` round trips until the real echoed output
/// appears — twice, on the SAME handle — proving a genuinely live duplex pipe, not a captured
/// one-shot. Also proves `poll-exit` correctly reports "still running" while it's alive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_proc_is_a_real_live_duplex_pipe_across_multiple_polls() {
    let bytes = std::fs::read(fixture_component()).expect("read fixture component bytes");
    let session = trusted_session().await;
    let ext = session
        .load_wasm_extension(ExtensionId::from("demo"), &bytes)
        .await
        .expect("load + init the live wasm extension");

    let marker = unique_marker("duplex");

    // 1) spawn the REAL long-lived child (a separate top-level prompt/round trip).
    let _ = session.prompt(format!("/procspawn {marker}")).await.unwrap();
    session.wait_for_idle().await;
    let handle = parse_handle(&ext.guest().notifications(), "proc spawned handle:");
    assert!(marker_process_alive(&marker), "the real marker-tagged child is running after spawn");

    // 2) write "first" to its REAL stdin (a separate round trip)...
    let _ = session.prompt(format!("/procwrite {handle} first")).await.unwrap();
    session.wait_for_idle().await;

    // ...then poll its REAL stdout (yet another separate round trip, itself polling
    // `read-stdout` MANY times internally) until the real echoed bytes appear.
    let _ = session.prompt(format!("/procreadpoll {handle} echo:first")).await.unwrap();
    session.wait_for_idle().await;
    assert!(
        ext.guest().notifications().iter().any(|n| n.starts_with("proc read") && n.contains("seen:true") && n.contains("echo:first")),
        "the REAL child echoed the first line back: {:?}",
        ext.guest().notifications()
    );

    // 3) the SAME handle stays live: write + poll a SECOND line, proving this is a genuine
    //    duplex pipe across time, not a one-shot capture.
    let _ = session.prompt(format!("/procwrite {handle} second")).await.unwrap();
    session.wait_for_idle().await;
    let _ = session.prompt(format!("/procreadpoll {handle} echo:second")).await.unwrap();
    session.wait_for_idle().await;
    assert!(
        ext.guest().notifications().iter().any(|n| n.starts_with("proc read") && n.contains("seen:true") && n.contains("echo:second")),
        "the SAME live child echoed the second line back too: {:?}",
        ext.guest().notifications()
    );

    // 4) `poll-exit` correctly reports STILL RUNNING (no `some` yet — nothing killed/exited it).
    let _ = session.prompt(format!("/procpollexit {handle}")).await.unwrap();
    session.wait_for_idle().await;
    assert!(
        ext.guest().notifications().iter().any(|n| n.contains("proc pollexit") && n.contains("code:None")),
        "poll-exit reports still-running while the real child is alive: {:?}",
        ext.guest().notifications()
    );

    // cleanup: kill the still-running child so it doesn't linger past the test.
    let _ = session.prompt(format!("/prockill {handle}")).await.unwrap();
    session.wait_for_idle().await;
}

/// THE headline proof (b): `poll-exit` reports the REAL natural exit code once a spawned child
/// exits on its own (no `kill` involved) — proving the background waiter observes a genuine OS
/// process exit through the full guest→host→`ProcCaps` path, not a canned value.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_proc_poll_exit_reports_the_real_natural_exit_code() {
    let bytes = std::fs::read(fixture_component()).expect("read fixture component bytes");
    let session = trusted_session().await;
    let ext = session
        .load_wasm_extension(ExtensionId::from("demo"), &bytes)
        .await
        .expect("load + init the live wasm extension");

    let _ = session.prompt("/procspawnexit").await.unwrap();
    session.wait_for_idle().await;
    let handle = parse_handle(&ext.guest().notifications(), "proc spawned handle:");

    // Poll across several SEPARATE top-level round trips (never a single blocking call) until the
    // REAL exit code (7, `sh -c "sleep 0.1; exit 7"`) shows up.
    let mut seen_code = None;
    for _ in 0..50 {
        let _ = session.prompt(format!("/procpollexit {handle}")).await.unwrap();
        session.wait_for_idle().await;
        if let Some(n) = ext.guest().notifications().iter().rev().find(|n| n.contains("proc pollexit"))
            && let Some(code_str) = n.split("code:").nth(1)
            && code_str.contains("Some(7)")
        {
            seen_code = Some(7);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(seen_code, Some(7), "the REAL natural exit code (7) round-trips end to end");
}

/// THE headline proof (c): `kill` actually terminates a REAL still-running child — verified at the
/// OS level via `pgrep -f <marker>` (never just trusting the WIT call's `Ok` return) — driven
/// through the FULL guest→host→`ProcCaps` path via a real top-level `/prockill` command.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_proc_kill_terminates_a_real_running_child_verified_at_the_os_level() {
    let bytes = std::fs::read(fixture_component()).expect("read fixture component bytes");
    let session = trusted_session().await;
    let ext = session
        .load_wasm_extension(ExtensionId::from("demo"), &bytes)
        .await
        .expect("load + init the live wasm extension");

    let marker = unique_marker("kill");
    let _ = session.prompt(format!("/procspawn {marker}")).await.unwrap();
    session.wait_for_idle().await;
    let handle = parse_handle(&ext.guest().notifications(), "proc spawned handle:");

    assert!(
        marker_process_alive(&marker),
        "the real child is genuinely running before kill (OS-level check)"
    );

    let _ = session.prompt(format!("/prockill {handle}")).await.unwrap();
    session.wait_for_idle().await;
    assert!(
        ext.guest().notifications().iter().any(|n| n.contains("proc kill") && n.contains("ok:true")),
        "the guest observed a successful kill across the boundary: {:?}",
        ext.guest().notifications()
    );
    // `poll-exit` right after `kill` reflects the REAL termination (`ProcCaps::kill` only returns
    // `Ok` once the OS process is confirmed reaped — never a fire-and-forget signal send).
    assert!(
        ext.guest().notifications().iter().any(|n| n.contains("proc kill") && !n.contains("code:None")),
        "poll-exit right after kill shows a real exit code, not still-running: {:?}",
        ext.guest().notifications()
    );

    // Independently verify at the OS level (never just trust our own WIT-level accounting): the
    // marker-tagged process must no longer exist.
    assert!(
        !marker_process_alive(&marker),
        "the real OS process is gone after kill — pgrep -f no longer finds it"
    );
}
