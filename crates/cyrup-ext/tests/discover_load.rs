//! LIVE discover -> trust-gate -> load -> route-command -> reload, end-to-end (arch-08 §6.2/§6.5).
//! Builds the `cyrup-ext-sdk` demo COMPONENT, drops it into a temp project's
//! `.cyrup/extensions/demo/` with an `extension.json`, and drives the facade orchestration: an
//! untrusted project records an `Untrusted` error (R-08-002); a trusted project loads the component
//! and routes a guest slash command across the boundary (R-08-016); `/reload` cache-busts and
//! re-loads (R-08-005).
#![cfg(feature = "wasm-host")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_core::CancelToken;
use cyrup_ext::loader::DiscoveryRoots;
use cyrup_ext::{
    CannedResponses, ControlOp, ExtMode, ExtensionHost, HostConfig, RecordingServices,
};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

fn fixture_component() -> PathBuf {
    if let Ok(p) = std::env::var("CYRUP_EXT_FIXTURE_COMPONENT") {
        return PathBuf::from(p);
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(&cargo)
        .args(["build", "-p", "cyrup-ext-sdk", "--target", "wasm32-wasip2"])
        .status()
        .expect("spawn cargo to build the wasm32-wasip2 fixture component");
    assert!(status.success(), "building cyrup-ext-sdk fixture component failed");
    let target_dir = std::env::var("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")
    });
    let wasm = target_dir.join("wasm32-wasip2/debug/cyrup_ext_sdk.wasm");
    assert!(wasm.exists(), "fixture component not found at {}", wasm.display());
    wasm
}

fn temp_project(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("cyrup-ext-discover-{name}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discover_trust_load_command_reload() {
    let bytes = std::fs::read(fixture_component()).expect("read fixture component");

    // Lay out a project: <cwd>/.cyrup/extensions/demo/{extension.json, demo.wasm}.
    let cwd = temp_project("proj");
    let ext_dir = cwd.join(".cyrup").join("extensions").join("demo");
    std::fs::create_dir_all(&ext_dir).unwrap();
    // EXT-028: interpolate `HOST_WORLD` rather than a literal — a fixture pinned to a stale world
    // string stops reaching the real load path the moment the world is bumped (`check_world` would
    // refuse it first, and this test would silently stop proving anything about instantiation).
    std::fs::write(
        ext_dir.join("extension.json"),
        format!(
            r#"{{ "id": "demo", "version": "1.0.0", "world": "{}" }}"#,
            cyrup_ext::HOST_WORLD
        ),
    )
    .unwrap();
    std::fs::write(ext_dir.join("demo.wasm"), &bytes).unwrap();

    let roots = DiscoveryRoots {
        project_cwd: Some(cwd.clone()),
        agent_dir: None,
        configured: vec![],
    };

    let cfg = HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: cwd.clone() };
    let host = ExtensionHost::with_wasm(cfg).expect("host with wasm");

    // A concrete NON-deny backend: grants control/UI/exec and records the effects (gap-08 #7).
    let rec = Arc::new(RecordingServices::new(CannedResponses::default()));

    // 1) UNTRUSTED project: the project-local extension is NOT loaded; an Untrusted error is recorded.
    let untrusted = host.discover_and_load(&roots, false, rec.clone()).await;
    assert!(untrusted.loaded.is_empty(), "nothing loads in an untrusted project");
    assert_eq!(untrusted.errors.len(), 1, "the project-local ext is recorded as an error");
    assert!(
        untrusted.errors[0].error.to_lowercase().contains("untrust"),
        "got: {}",
        untrusted.errors[0].error
    );

    // 2) TRUSTED project: the component loads.
    let trusted = host.discover_and_load(&roots, true, rec.clone()).await;
    assert_eq!(trusted.loaded.len(), 1, "one extension loaded, errors={:?}", trusted.errors);
    assert_eq!(trusted.loaded[0].to_string(), "demo");
    assert!(trusted.errors.is_empty(), "no errors: {:?}", trusted.errors);

    // 3) the guest slash command routes across the boundary via the facade (R-08-016).
    let cancel = CancelToken::new();
    let out = host.run_command("greet", "world", &cancel).await.expect("command runs");
    assert_eq!(out.as_deref(), Some("hello, world!"));
    let comps = host.command_completions("greet", "te").await.expect("completions");
    assert_eq!(comps, vec!["team".to_string()]);

    // the command's COMMAND-tier control op (`compact`) reached the NON-deny backend (R-08-008) —
    // WITH its `CompactOptions.customInstructions` payload (Pi types.ts:296-300,344), which the
    // guest passed to `ctx.compact_with(...)`. Asserting the payload and not just the variant is
    // what keeps the opts-json leg of the `control.compact` import honest.
    assert!(
        rec.control_ops().iter().any(|op| matches!(
            op,
            ControlOp::Compact { custom_instructions }
                if custom_instructions.as_deref() == Some("demo: keep the greeting")
        )),
        "non-deny backend recorded the command's control op with its instructions: {:?}",
        rec.control_ops()
    );

    // 4) hot reload: cache-bust + re-load; the command still routes afterwards.
    let reloaded = host.reload(&roots, true, rec.clone(), &cancel).await.expect("reload");
    assert_eq!(reloaded.loaded.len(), 1, "reload re-loaded the extension");
    let out2 = host.run_command("greet", "team", &cancel).await.expect("command runs post-reload");
    assert_eq!(out2.as_deref(), Some("hello, team!"));

    let _ = std::fs::remove_dir_all(&cwd);
}
