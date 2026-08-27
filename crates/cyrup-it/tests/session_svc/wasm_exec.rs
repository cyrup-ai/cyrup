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
//! The fixture is the bundled `cyrup-ext-sdk` demo extension (its `example/commands_capability.rs`
//! registers `/execdemo`),
//! built to a `wasm32-wasip2` component. Set `CYRUP_EXT_FIXTURE_COMPONENT` to a prebuilt component to
//! skip the nested build.
// The original `#![cfg(feature = "wasm-host")]` is deliberately GONE. It named
// cyrup-session-svc's own feature, which that crate enables in its `default` — so it was
// always true here. Re-spelled in cyrup-it it would name THIS crate's `wasm-host`, which
// `--features it` does not enable, and every test below would SILENTLY not compile in.
// See the `[[test]]` note in crates/cyrup-it/Cargo.toml.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use cyrup_core::{ExtensionId, StopReason};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

// MIGRATION (docs/TEST-ARCHITECTURE.md §3.4): this file used to carry its own `fixture_component()`
// that shelled out to `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` into the SHARED, fixed
// `std::env::temp_dir()/cyrup-session-svc-fixture-target` — one of ten byte-identical copies that
// serialized on each other's cargo build lock and never cleaned up. `cyrup-it`'s `build.rs` now
// builds the component ONCE for the whole suite and exports its path; `CYRUP_EXT_FIXTURE_COMPONENT`
// still overrides it, at that one place instead of ten.
use crate::support::bins;

fn faux_with_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    faux
}

/// THE headline proof: a TRUSTED live wasm guest's `pi.exec("echo",["hi"])` runs through the session's
/// injected `LiveHostServices` → real local process ops (shell:false argv) and returns the REAL stdout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_exec_runs_a_real_command_through_the_assembled_session() {
    let bytes = bins::component_bytes();

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
        .load_wasm_extension(
            ExtensionId::from("demo"),
            &bytes,
            // EXT-059: the grant is now explicit. `host_granted()` is the TOTAL grant these
            // fixtures previously got implicitly from `load_wasm_extension`'s `load_wasm` call.
            &cyrup_ext::Capabilities::host_granted(),
        )
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
