//! TUI-037 — `/reload` persists an implicitly-granted project trust.
//!
//! ```ts
//! // pi v0.84.4 coding-agent/src/modes/interactive/interactive-mode.ts:4921-4941
//! private maybeSaveImplicitProjectTrustAfterReload(): boolean {
//!     const cwd = this.sessionManager.getCwd();
//!     if (this.autoTrustOnReloadCwd !== cwd) return false;
//!     if (!this.settingsManager.isProjectTrusted() || !hasTrustRequiringProjectResources(cwd)) return false;
//!     const trustStore = new ProjectTrustStore(this.runtimeHost.services.agentDir);
//!     try {
//!         if (trustStore.get(cwd) !== null) { this.autoTrustOnReloadCwd = undefined; return false; }
//!         trustStore.set(cwd, true);
//!         this.autoTrustOnReloadCwd = undefined;
//!         return true;
//!     } catch (error) {
//!         this.showWarning(`Could not save project trust after reload: ${…}`);
//!         return false;
//!     }
//! }
//! // …and at :5995-6003 the reload status gains `; saved project trust` when it returned true.
//! ```
//!
//! These drive the real `/reload` arm (`App::execute_command(AppCommand::Reload, …)`) against a
//! real `AgentSessionRuntime` whose factory carries the `trust.json` store — the production wiring
//! of `crates/cyrup/src/session_launch.rs` `build_factory` — then read the store back with the
//! same `TrustStore` the next launch would, the rebuilt session's own `project_trusted`, and the
//! committed scrollback the user sees.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{App, AppCommand, UiTheme};
use cyrup_config::trust::{TrustDecision, TrustStore};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AgentSessionRuntime, SessionConfig, SessionFactory, SessionTarget};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

const PLAIN: &str = "Reloaded keybindings, extensions, skills, prompts, themes, and context files";
const SAVED: &str = "Reloaded keybindings, extensions, skills, prompts, themes, and context files; saved project trust";

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
    config: SessionConfig,
}

impl Fixture {
    fn trust_json(&self) -> PathBuf {
        self.agent_dir.join("trust.json")
    }

    /// One of `has_trust_requiring_resources`'s `.cyrup` markers (`trust.rs:212-222`) — the
    /// item's own scenario, a project that grows a skill mid-session.
    fn grow_resources(&self) {
        let skills = self.cwd.join(".cyrup").join("skills");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(skills.join("x.md"), "---\nname: x\n---\nhello\n").unwrap();
    }

    async fn saved_decision(&self) -> Option<TrustDecision> {
        TrustStore::new(self.trust_json())
            .nearest(&self.cwd)
            .await
            .unwrap()
            .map(|e| e.decision)
    }
}

/// A project with NO trust-requiring resources and no `--approve`/`--no-approve`, so the builder
/// grants trust implicitly (`decide_trust` step 2) — the only launch pi arms
/// `autoTrustOnReloadCwd` for (`main.ts:701-704`). `dir/home` is HOME so the `.agents/skills`
/// ancestor walk cannot escape into the developer's real home directory.
fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let mut config = SessionConfig::new(cwd.clone(), agent_dir.clone());
    config.home = home;
    config.trust_override = None;
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
        config,
    }
}

/// The production factory shape: `build_factory` wires `<agent_dir>/trust.json` for every host
/// (`session_launch.rs`), which is what lets the REBUILT session read the saved decision back.
async fn runtime(fx: &Fixture) -> Arc<AgentSessionRuntime> {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let factory = Arc::new(
        SessionFactory::new(provider, fx.config.clone())
            .trust_store(Arc::new(TrustStore::new(fx.trust_json()))),
    );
    AgentSessionRuntime::create(factory, SessionTarget::New)
        .await
        .unwrap()
}

fn app() -> App<TestBackend> {
    // Wide enough that the status sentence is not wrapped, so `contains` reads the real string.
    App::new(TestBackend::new(200, 24), UiTheme::dark()).unwrap()
}

/// Drive `/reload` exactly as the run loop does, then the re-bind the generation watch performs,
/// and return the committed scrollback.
async fn reload(app: &mut App<TestBackend>, rt: &Arc<AgentSessionRuntime>) -> String {
    let session = rt.session().await;
    let generation = rt.generation().await;
    app.execute_command(AppCommand::Reload, &session, Some(rt))
        .await;
    assert_eq!(
        rt.generation().await,
        generation + 1,
        "/reload rebuilds the session"
    );
    // A warning the outcome carried is pending on the transcript; commit it before the re-bind
    // resets the view, as the run loop's next frame would.
    app.draw().unwrap();
    app.rebind_session();
    app.draw().unwrap();
    app.scrollback_text()
}

/// The item's own Verify clause, RED at HEAD: after `/reload` the store holds `cwd → true`, the
/// status carries pi's `; saved project trust` variant, and — the part cyrup's rebuild makes
/// necessary — the rebuilt session is trusted with the new resources loaded.
#[tokio::test]
async fn reload_persists_an_implicitly_granted_project_trust() {
    let fx = fixture();
    let rt = runtime(&fx).await;
    assert!(
        rt.session().await.services().project_trusted,
        "fixture must be implicitly trusted at boot"
    );
    assert!(fx.saved_decision().await.is_none(), "nothing saved at boot");
    let mut app = app();
    app.set_auto_trust_on_reload_cwd(Some(fx.cwd.clone()));
    fx.grow_resources();

    let out = reload(&mut app, &rt).await;

    assert_eq!(
        fx.saved_decision().await,
        Some(TrustDecision::Trusted),
        "trust.json holds cwd → true"
    );
    assert!(
        out.contains(SAVED),
        "status lacks the saved variant:\n{out}"
    );
    let rebuilt = rt.session().await;
    assert!(
        rebuilt.services().project_trusted,
        "the rebuilt session reads the saved decision back"
    );
    assert!(
        app.state().auto_trust_on_reload_cwd.is_none(),
        "the arm is dropped once the grant is persisted"
    );
    assert!(
        !out.contains("Could not save project trust"),
        "no warning on the happy path:\n{out}"
    );
}

/// `if (this.autoTrustOnReloadCwd !== cwd) return false;` — a launch whose trust was decided
/// (a flag, a saved entry, the prompt) is never armed, and `/reload` leaves the store alone.
#[tokio::test]
async fn reload_without_an_implicit_grant_leaves_the_store_untouched() {
    let fx = fixture();
    let rt = runtime(&fx).await;
    let mut app = app();
    fx.grow_resources();

    let out = reload(&mut app, &rt).await;

    assert!(!fx.trust_json().exists(), "no trust.json is created");
    assert!(out.contains(PLAIN), "status is the plain variant:\n{out}");
    assert!(!out.contains(SAVED), "status must not claim a save:\n{out}");
}

/// `if (!… || !hasTrustRequiringProjectResources(cwd)) return false;` — nothing to gate yet, so
/// nothing is written and the arm STAYS for a later `/reload` that finds resources.
#[tokio::test]
async fn reload_with_no_resources_keeps_the_arm_and_writes_nothing() {
    let fx = fixture();
    let rt = runtime(&fx).await;
    let mut app = app();
    app.set_auto_trust_on_reload_cwd(Some(fx.cwd.clone()));

    let out = reload(&mut app, &rt).await;

    assert!(!fx.trust_json().exists(), "no trust.json is created");
    assert!(out.contains(PLAIN), "status is the plain variant:\n{out}");
    assert!(!out.contains(SAVED));
    assert_eq!(
        app.state().auto_trust_on_reload_cwd.as_deref(),
        Some(fx.cwd.as_path()),
        "the arm survives a reload that had nothing to persist"
    );
}

/// `if (trustStore.get(cwd) !== null) { this.autoTrustOnReloadCwd = undefined; return false; }` —
/// an ancestor decision already covers the cwd: the file is left byte-identical and the arm is
/// dropped without a save being reported.
#[tokio::test]
async fn reload_with_a_saved_decision_disarms_without_writing() {
    let fx = fixture();
    let ancestor: &Path = fx.cwd.parent().unwrap();
    TrustStore::new(fx.trust_json())
        .set(ancestor, Some(TrustDecision::Trusted))
        .await
        .unwrap();
    let before = std::fs::read_to_string(fx.trust_json()).unwrap();
    let rt = runtime(&fx).await;
    let mut app = app();
    app.set_auto_trust_on_reload_cwd(Some(fx.cwd.clone()));
    fx.grow_resources();

    let out = reload(&mut app, &rt).await;

    assert_eq!(
        std::fs::read_to_string(fx.trust_json()).unwrap(),
        before,
        "an existing decision is never overwritten"
    );
    assert!(out.contains(PLAIN), "status is the plain variant:\n{out}");
    assert!(!out.contains(SAVED));
    assert!(
        app.state().auto_trust_on_reload_cwd.is_none(),
        "the arm is dropped once a decision is found"
    );
}

/// pi's `catch`: `showWarning("Could not save project trust after reload: …")`, the plain status,
/// and the arm left in place. A directory where `trust.json` should be makes the write fail.
#[tokio::test]
async fn a_store_failure_warns_and_keeps_the_plain_status() {
    let fx = fixture();
    std::fs::create_dir_all(fx.trust_json()).unwrap();
    let rt = runtime(&fx).await;
    let mut app = app();
    app.set_auto_trust_on_reload_cwd(Some(fx.cwd.clone()));
    fx.grow_resources();

    let out = reload(&mut app, &rt).await;

    assert!(
        out.contains("Warning: Could not save project trust after reload: "),
        "pi's warning is missing:\n{out}"
    );
    assert!(out.contains(PLAIN), "status is the plain variant:\n{out}");
    assert!(
        !out.contains(SAVED),
        "a failed save must not be reported:\n{out}"
    );
    assert_eq!(
        app.state().auto_trust_on_reload_cwd.as_deref(),
        Some(fx.cwd.as_path()),
        "pi's catch does not clear autoTrustOnReloadCwd"
    );
}
