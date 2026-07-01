//! WASM EXEC CAPABILITY END-TO-END (arch-08 exec grant; L4 capability surface — the dead-but-advertised
//! `exec` interface, now GRANTED). Proves that the capability-scoped `exec` a LIVE wasm guest calls
//! (`pi.exec("echo", ["hi"])` → Pi `execCommand`, exec.ts:34-46) reaches the session's REAL local
//! process ops through the injected `LiveHostServices` (arch-08 §5.6) and returns the REAL captured
//! stdout — not a stub, not a canned answer.
//!
//! The invariant mirrored from Pi: LOADED == TRUSTED-BY-CONSTRUCTION. The guest runs in a TRUSTED
//! project (`trust_override = Some(true)`), so its `LiveHostServices` grants exec unconditionally (the
//! deny-all `DenyServices` path — the untrusted analog — is proven in cyrup-ext/tests/wasm_component.rs).
//! The `/execdemo` command runs `echo hi` shell:false-argv and notifies `exec stdout: hi`, which we
//! observe host-side after driving the command through the REAL run path.
//!
//! The fixture is the bundled `cyrup-ext-sdk` demo extension (its `example.rs` registers `/execdemo`),
//! built to a `wasm32-wasip2` component. Set `CYRUP_EXT_FIXTURE_COMPONENT` to a prebuilt component to
//! skip the nested build.
#![cfg(feature = "wasm-host")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use cyrup_core::{ExtensionId, StopReason};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

/// Build (or locate) the demo guest component (mirrors wasm_slash_command.rs).
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

/// THE headline proof: a TRUSTED live wasm guest's `pi.exec("echo",["hi"])` runs through the session's
/// injected `LiveHostServices` → real local process ops (shell:false argv) and returns the REAL stdout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_exec_runs_a_real_command_through_the_assembled_session() {
    let bytes = std::fs::read(fixture_component()).expect("read fixture component bytes");

    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();

    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true); // TRUSTED project ⇒ the guest's exec grant is live.
    cfg.no_extensions = true; // only the explicitly-loaded guest is present.

    let session = SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, cfg)
        .build()
        .await
        .unwrap();

    // Load the guest COMPONENT through the session's host, injecting the session's OWN LiveHostServices
    // (arch-08 §5.6) — the same backend whose `exec` grant runs argv commands through the real ProcOps.
    let ext = session
        .load_wasm_extension(ExtensionId::from("demo"), &bytes)
        .await
        .expect("load + init the live wasm extension");

    // The guest's `init` registered `/execdemo`.
    assert!(
        session.services().ext_host.registry().command_names().unwrap().iter().any(|n| n == "execdemo"),
        "the guest-registered `/execdemo` command is in the host command registry"
    );

    // Drive the command through the REAL public entry point (prompt → prepare →
    // _tryExecuteExtensionCommand → the guest's `execute-command` export → `pi.exec` → the WIT `exec`
    // import → LiveHostServices::exec → cyrup-tools ProcOps argv path → `echo hi`).
    let _ = session.prompt("/execdemo").await.unwrap();
    session.wait_for_idle().await;

    // The GUEST saw the REAL captured stdout across the wasm boundary: `echo hi` printed "hi", which the
    // command handler surfaced via `ctx.ui().notify("exec stdout: hi")`.
    assert!(
        ext.guest().notifications().iter().any(|n| n == "exec stdout: hi"),
        "the guest exec returned the real `echo hi` stdout: {:?}",
        ext.guest().notifications()
    );
}
