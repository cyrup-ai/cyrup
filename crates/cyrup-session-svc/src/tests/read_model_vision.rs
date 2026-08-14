//! `read`'s non-vision-model warning must describe the LIVE model (pi `tools/read.ts`).
//!
//! The tools-side seam (`ReadOpts::model_vision` / `ModelVisionHandle`) was built first and left
//! UNWIRED: in production `read` received `model_vision: None`, so `supports_images_now()` fell
//! back to its `true` default, the warning was unreachable, and an image handed to a text-only
//! model produced a raw provider error instead of the tool's own diagnostic.
//!
//! This asserts the WIRING specifically. The seam's own unit tests in `cyrup-tools` covered the
//! mechanism and passed happily against the unwired build — which is exactly why the adversarial
//! reviewer refused the "fixed" label until this existed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_provider::faux::FauxProvider;
use cyrup_provider::Provider;
use crate::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// A built session must EXPOSE the handle the `read` tool reads through. Before the wiring the
/// field did not exist on `AgentSession` at all, so this file would not compile — which is the
/// point: the production path had no way to tell `read` anything about the model.
#[tokio::test]
async fn a_built_session_wires_the_read_tools_vision_handle() {
    let fx = fixture();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(provider, base_config(&fx)).build().await.expect("build");

    // The handle exists and answers. Its VALUE is whatever the resolved faux model declares; the
    // defect was never a wrong bool, it was that `read` saw `None` and defaulted to `true`.
    let seeded = session.read_model_vision().get();

    // And it is a live channel, not a snapshot: pushing a capability change is observable, which is
    // what makes the `/model`-switch push in `apply_model_change` meaningful.
    session.read_model_vision().set(!seeded);
    assert_eq!(session.read_model_vision().get(), !seeded, "the handle is live");
    session.read_model_vision().set(seeded);
    assert_eq!(session.read_model_vision().get(), seeded, "restored");
}
