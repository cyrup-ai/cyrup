//! AGENT-027 — the extension host fills Pi's `extensions` startup-timing namespace.
//!
//! Pi marks `${extensionPath} module import` once an extension's module has been imported and
//! `${extensionPath} factory` once its factory has run (`core/extensions/loader.ts:568,553`
//! @v0.87.1), and `DefaultResourceLoader.reload()` resets the namespace first
//! (`core/resource-loader.ts:389`) — in cyrup `SessionBuilder::build`, which every `/reload`
//! runs (cyrup-session-svc's `startup_timings` test). Before this, `TimingLabel::Extensions` had no producer anywhere
//! in the workspace, so `CYRUP_TIMING=1` could never print `--- Startup Timings: extensions ---`.
//!
//! The one seam this file uses that its siblings do not: timings are gated on `CYRUP_TIMING=1`,
//! read ONCE per process (Pi's module-load `ENABLED`), so the enabled half cannot be reached from a
//! process that started without it. The test re-executes its own test binary, filtered to itself,
//! with the variable set, and the child drives the real host entry points.
//!
//! Whole-file `wasm-host`: the child loads a component, which needs the Wasmtime host.
#![cfg(feature = "wasm-host")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::contract::HookOutcome;
use crate::event::HostEvent;
use crate::native::{HostCtx, InitApi, NativeExtension};
use crate::{ExtError, ExtensionHost, HostConfig};
use cyrup_core::ExtensionId;
use cyrup_core::timings::{TimingLabel, snapshot};
use std::sync::Arc;

/// Set in the re-executed child so it runs the body instead of spawning again.
const CHILD_ENV: &str = "CYRUP_EXT_STARTUP_TIMINGS_CHILD";
/// Printed by the child once its assertions pass, so a filter that matched no test (and therefore
/// exits 0 having run nothing) cannot pass for a green child.
const CHILD_DONE: &str = "startup-timings-child-done";

/// A native built-in whose `init` succeeds or fails on demand.
struct TimedNative {
    id: &'static str,
    fail: bool,
}

#[async_trait::async_trait]
impl NativeExtension for TimedNative {
    fn id(&self) -> ExtensionId {
        self.id.into()
    }
    async fn init(&self, _api: &mut InitApi) -> Result<(), ExtError> {
        if self.fail {
            Err(ExtError::Component("init refused".into()))
        } else {
            Ok(())
        }
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

fn extension_rows() -> Vec<String> {
    snapshot()
        .into_iter()
        .find(|(k, _)| *k == TimingLabel::Extensions)
        .map(|(_, rows)| rows.into_iter().map(|(l, _)| l).collect())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extension_loads_fill_the_extensions_timing_namespace() {
    if std::env::var_os(CHILD_ENV).is_none() {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "tests::startup_timings::extension_loads_fill_the_extensions_timing_namespace",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, "1")
            .env("CYRUP_TIMING", "1")
            .output()
            .expect("re-exec the test binary");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success() && stdout.contains(CHILD_DONE),
            "child failed or ran nothing\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        return;
    }
    child_body().await;
    println!("{CHILD_DONE}");
}

async fn child_body() {
    use crate::loader::{DiscoveryRoots, discover};
    use crate::{DenyServices, ExtMode};

    assert!(
        cyrup_core::timings::enabled(),
        "the child must run under CYRUP_TIMING=1"
    );

    // A trusted project holding two components. `empty.wasm` is the smallest VALID component
    // (magic, component version, layer): it compiles — Pi's module import succeeds — but exports
    // no `init`, so instantiation fails — Pi's factory never runs. `ok.wasm` instantiates against
    // the real world and its `init` succeeds, so it marks both rows, import first.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let cwd = std::env::temp_dir().join(format!(
        "cyrup-ext-startup-timings-{nanos}-{}",
        std::process::id()
    ));
    let ext_dir = cwd.join(".cyrup").join("extensions");
    std::fs::create_dir_all(&ext_dir).unwrap();
    std::fs::write(ext_dir.join("empty.wasm"), b"\0asm\x0d\x00\x01\x00").unwrap();
    let guest = wat::parse_str(include_str!("startup_timings_guest.wat")).unwrap();
    std::fs::write(ext_dir.join("ok.wasm"), guest).unwrap();
    let roots = DiscoveryRoots {
        project_cwd: Some(cwd.clone()),
        agent_dir: None,
        configured: vec![],
        disabled: Vec::new(),
    };
    let found = discover(&roots);
    assert_eq!(found.len(), 2, "{found:?}");
    // The rows the load must leave, in discovery (load) order: every component marks its import,
    // and only the one whose `init` succeeds marks its factory after it.
    let mut expected = Vec::new();
    for disc in &found {
        let path = disc.dir.to_string_lossy().into_owned();
        let ok = disc.wasm.as_ref().is_some_and(|w| w.ends_with("ok.wasm"));
        expected.push(format!("{path} module import"));
        if ok {
            expected.push(format!("{path} factory"));
        }
    }
    assert_eq!(
        expected.len(),
        3,
        "exactly one discovered component is `ok.wasm`: {found:?}"
    );

    let host = ExtensionHost::with_wasm(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: cwd.clone(),
    })
    .expect("host with wasm");

    let result = host
        .discover_and_load(&roots, true, Arc::new(DenyServices))
        .await;
    assert!(
        result.loaded.len() == 1 && result.errors.len() == 1,
        "the init-less component compiles and then fails to instantiate; the real one loads: \
         {result:?}"
    );

    // Natives: Pi's inline factories — a factory row on success, nothing on failure.
    host.load_native(Arc::new(TimedNative {
        id: "timing-ok",
        fail: false,
    }))
    .await
    .unwrap();
    host.load_native(Arc::new(TimedNative {
        id: "timing-bad",
        fail: true,
    }))
    .await
    .unwrap_err();

    expected.push("timing-ok factory".to_string());
    assert_eq!(
        extension_rows(),
        expected,
        "each component marked its import, and only the one whose init succeeded marked its \
         factory, after the import; only the native whose init succeeded marked a factory"
    );
    let _ = std::fs::remove_dir_all(&cwd);
}
