//! AGENT-027 — building a session resets Pi's `extensions` startup-timing namespace, then fills it.
//!
//! Pi `DefaultResourceLoader.reload()` opens with `resetTimings("extensions")`
//! (`core/resource-loader.ts:389` @v0.87.1) and every extension it loads marks
//! `${extensionPath} factory` (`core/extensions/loader.ts:553`). The builder is cyrup's reload: the
//! rows a `CYRUP_TIMING=1` run prints must be this session's own load, with nothing left over from
//! whatever the process marked before it.
//!
//! Timings are gated on `CYRUP_TIMING=1`, read ONCE per process (Pi's module-load `ENABLED`), so
//! the test re-executes its own test binary, filtered to itself, with the variable set; the child
//! drives the real [`SessionBuilder::build`].
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use crate::{SessionBuilder, SessionConfig};
use cyrup_core::ExtensionId;
use cyrup_core::timings::{TimingLabel, snapshot, time};
use cyrup_ext::{ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use tempfile::TempDir;

/// Set in the re-executed child so it runs the body instead of spawning again.
const CHILD_ENV: &str = "CYRUP_SVC_STARTUP_TIMINGS_CHILD";
/// Printed by the child once its assertions pass, so a filter that matched no test (and therefore
/// exits 0 having run nothing) cannot pass for a green child.
const CHILD_DONE: &str = "svc-startup-timings-child-done";

struct QuietNative;

#[async_trait::async_trait]
impl NativeExtension for QuietNative {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("timing-native")
    }
    async fn init(&self, _api: &mut InitApi) -> Result<(), ExtError> {
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_build_resets_then_fills_the_extensions_timing_namespace() {
    if std::env::var_os(CHILD_ENV).is_none() {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "tests::startup_timings::session_build_resets_then_fills_the_extensions_timing_namespace",
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

    assert!(
        cyrup_core::timings::enabled(),
        "the child must run under CYRUP_TIMING=1"
    );
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);

    // A row an earlier pass left behind.
    time("stale row", TimingLabel::Extensions);
    let _session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .with_native_extension(Arc::new(QuietNative))
        .build()
        .await
        .unwrap();

    let rows: Vec<String> = snapshot()
        .into_iter()
        .find(|(k, _)| *k == TimingLabel::Extensions)
        .map(|(_, rows)| rows.into_iter().map(|(l, _)| l).collect())
        .unwrap_or_default();
    assert_eq!(
        rows,
        vec!["timing-native factory".to_string()],
        "the build reset the namespace, then its one native marked its factory"
    );
    println!("{CHILD_DONE}");
}
