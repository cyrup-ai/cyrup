//! **DRIFT-053** — `/share` routes to Radius FIRST and only falls through to the private gist when
//! there is no radius credential.
//!
//! pi `modes/interactive/session-share.ts:46-89` @v0.84.4 (`shareSession`) and `:91-150`
//! (`tryShareViaRadius`). The two facts this file pins are the item's own Verify lines:
//!
//! * with a radius credential present, `/share` uploads to the artifacts endpoint and **never
//!   invokes `gh`** — and a failed upload reports, it does **not** fall back to the gist path
//!   (upstream returns `true` from both arms of `:132-140`, so a failed Radius attempt ends the
//!   command);
//! * with no radius credential, behaviour is what it was before this landed — the `gh`/gist chain,
//!   with no mention of Radius anywhere.
//!
//! No socket leaves the machine: the gateway is redirected to a closed local port through
//! [`crate::App::set_radius_share_gateway`], the seam this crate's "tests must never hit real
//! provider APIs" convention requires (the sibling of `set_login_provider_source`, which
//! `tests/login_flow.rs` uses for the same reason).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

use crate::transcript::Entry;
use crate::{App, AppCommand, UiTheme};

/// A port nothing listens on: the POST fails with a connection error, which is upstream's `catch`
/// (`session-share.ts:141-149`) — reported, and still `return true`.
const CLOSED_GATEWAY: &str = "http://127.0.0.1:1";

struct Fixture {
    _tmp: TempDir,
    session: Arc<AgentSession>,
}

/// `radius_credential` writes an `auth.json` holding a far-future OAuth credential for `radius`, so
/// `resolve_provider_auth` resolves it without attempting (and failing) a refresh.
async fn fixture(radius_credential: bool) -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    if radius_credential {
        std::fs::write(
            agent_dir.join("auth.json"),
            r#"{"radius":{"type":"oauth","refresh":"rt","access":"at-shared","expires":32503680000000}}"#,
        )
        .unwrap();
    }
    let mut config = SessionConfig::new(cwd, agent_dir);
    config.trust_override = Some(true);
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(provider, config).build().await.unwrap();
    Fixture {
        _tmp: tmp,
        session: Arc::new(session),
    }
}

fn app() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.set_radius_share_gateway(CLOSED_GATEWAY);
    app
}

fn lines(app: &App<TestBackend>) -> Vec<String> {
    app.state()
        .transcript
        .pending()
        .iter()
        .filter_map(|e| match e {
            Entry::Status(text) | Entry::Error(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The item's first Verify line. A session whose `auth.json` carries a radius credential takes
/// `tryShareViaRadius` (`:57`) — the transcript shows the Radius upload's own failure sentence
/// (`:144-146`), and NOTHING from the `gh` chain: neither its `gh auth status` pre-check messages
/// (`:60-67`) nor a gist URL.
#[tokio::test]
async fn a_radius_credential_routes_share_to_the_artifacts_endpoint_and_never_reaches_gh() {
    let f = fixture(true).await;
    let mut app = app();
    app.execute_command(AppCommand::Share, &f.session, None)
        .await;
    let lines = lines(&app);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Error: Failed to upload Radius artifact:")),
        "expected pi's `showError(\\`Failed to upload Radius artifact: …\\`)` (`:144-146`), got {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("GitHub CLI")),
        "the `gh auth status` pre-check (`:59-68`) must not run once Radius has been attempted — \
         a failed Radius upload returns `true` (`:148`) and does NOT fall back to a public-by-link \
         gist, which is the destination the Radius configuration exists to avoid. Got {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("Gist:")),
        "no gist may be created on this path: {lines:?}"
    );
}

/// The item's second Verify line: with no radius credential, `tryShareViaRadius` returns `false`
/// at `:98` and `/share` is byte-identical to what it was before DRIFT-053 — the `gh`/gist chain,
/// with no Radius message of any kind. What `gh` itself then reports depends on the machine, so
/// only the ABSENCE of the Radius path is asserted here.
#[tokio::test]
async fn no_radius_credential_falls_through_to_the_unchanged_gist_chain() {
    let f = fixture(false).await;
    let mut app = app();
    app.execute_command(AppCommand::Share, &f.session, None)
        .await;
    let lines = lines(&app);
    assert!(
        !lines.iter().any(|l| l.contains("Radius")),
        "with no credential the Radius attempt must be silent — `if (!token) return false` \
         (`:98`) — leaving `/share` exactly as it was. Got {lines:?}"
    );
}
