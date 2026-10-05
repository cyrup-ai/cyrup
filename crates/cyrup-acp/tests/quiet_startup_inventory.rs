//! `quietStartup` is `true | false | "header"` (`settings-manager.ts:112` @v1.0.0). The ACP startup
//! prelude has no header, so the only decision it takes from the setting is pi's
//! `shouldShowStartupDetails` — `quietStartup === false` — which `true` and `"header"` both fail:
//! either one suppresses the inventory.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup_acp::startup::StartupInventory;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{SessionBuilder, SessionConfig};

async fn show_listing(quiet: &str) -> bool {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("project");
    let agent_dir = dir.path().join("agent");
    let home = dir.path().join("home");
    for d in [&cwd, &agent_dir, &home] {
        std::fs::create_dir_all(d).unwrap();
    }
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.home = home;
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let session = SessionBuilder::new(provider, cfg)
        .cli_settings(
            cyrup_config::settings::Settings::parse(&format!("{{\"quietStartup\": {quiet}}}"))
                .unwrap(),
        )
        .build()
        .await
        .unwrap();
    StartupInventory::of(&session).show_listing
}

#[tokio::test]
async fn only_an_unset_quiet_startup_shows_the_inventory() {
    assert!(show_listing("false").await, "false shows the inventory");
    assert!(!show_listing("true").await, "true hides it");
    assert!(
        !show_listing("\"header\"").await,
        "\"header\" hides the details too"
    );
}
