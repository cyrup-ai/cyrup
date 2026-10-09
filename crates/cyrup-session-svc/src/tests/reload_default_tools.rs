//! CFG-109 — `/reload` activates the tools newly added to the `defaultTools` setting (pi `reload`,
//! `core/agent-session.ts:3660-3685` @v1.1.0). Removed names stay active, and a tool disabled
//! during the session stays disabled unless the setting newly adds it.
//!
//! The session is persisted, so the rebuild resumes the loadout its transcript declares: without
//! the reload step the new setting never reaches the active set, which is what the end-to-end run
//! of the pi v1.1.0 tools pass saw.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;

use crate::{AgentSessionRuntime, NoTools, SessionConfig, SessionFactory, SessionTarget};
use cyrup_codemode_runtime::CodemodeExtension;
use cyrup_config::{InMemorySettingsStore, SettingsScope};
use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
use tempfile::TempDir;

struct Rig {
    _tmp: TempDir,
    runtime: Arc<AgentSessionRuntime>,
    store: Arc<InMemorySettingsStore>,
    faux: Arc<FauxProvider>,
}

async fn rig(settings: &str, configure: impl FnOnce(&mut SessionConfig)) -> Rig {
    let tmp = TempDir::new().unwrap();
    let cwd: PathBuf = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.persist = true;
    configure(&mut cfg);
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, settings);
    let faux = Arc::new(FauxProvider::new());
    let factory = Arc::new(
        SessionFactory::new(faux.clone() as Arc<dyn Provider>, cfg)
            .settings_store(store.clone())
            .with_codemode(CodemodeExtension::new(
                Default::default(),
                Arc::new(cyrup_codemode_runtime::tool::EngineSandboxFactory),
            )),
    );
    let runtime = AgentSessionRuntime::create(factory, SessionTarget::New)
        .await
        .unwrap();
    Rig {
        _tmp: tmp,
        runtime,
        store,
        faux,
    }
}

impl Rig {
    /// One plain turn, so the transcript records the loadout the reload resumes.
    async fn turn(&self) {
        self.faux
            .set_response_steps(vec![FauxResponseStep::factory(|_c, _o, _s, _m| {
                faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)
            })]);
        let session = self.runtime.session().await;
        let _ = session.prompt("go").await.unwrap();
        session.wait_for_idle().await;
    }

    async fn active(&self) -> Vec<String> {
        let mut names = self.runtime.session().await.active_tool_names();
        names.sort();
        names
    }

    async fn set_active(&self, names: &[&str]) {
        let names: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
        self.runtime
            .session()
            .await
            .set_active_tools_by_name(&names)
            .await;
    }

    async fn reload_with(&self, settings: &str) {
        self.store.seed(SettingsScope::Global, settings);
        self.runtime.reload(None).await.unwrap();
    }
}

fn sorted(names: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
    names.sort();
    names
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_activates_the_tools_newly_added_to_default_tools() {
    let rig = rig("{}", |_| {}).await;
    assert_eq!(
        rig.active().await,
        sorted(&["read", "bash", "edit", "write"])
    );
    // A tool disabled during the session…
    rig.set_active(&["read", "bash", "write"]).await;
    rig.turn().await;

    // …stays disabled across a reload that does not newly add it, while a built-in (`grep`) and
    // an extension tool that registration leaves inactive (`codemode`) that the setting newly
    // names are activated.
    rig.reload_with(r#"{ "defaultTools": ["+grep", "+codemode"] }"#)
        .await;
    assert_eq!(
        rig.active().await,
        sorted(&["read", "bash", "write", "grep", "codemode"])
    );
    rig.turn().await;

    // A name the setting drops stays active; an unchanged setting adds nothing.
    rig.reload_with(r#"{ "defaultTools": ["+codemode"] }"#)
        .await;
    assert_eq!(
        rig.active().await,
        sorted(&["read", "bash", "write", "grep", "codemode"])
    );
}

/// pi activates only when the initial tools came from the setting (`usesDefaultTools`, `sdk.ts:472`
/// @v1.1.0): an explicit `tools` list and `noTools` are left alone, and `excludeTools` still filters
/// what the setting adds. Mirrors `keeps explicit tool options on reload`
/// (`test/default-tools-setting.test.ts:269-290` @v1.1.0).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_keeps_explicit_tool_options() {
    let allowlisted = rig("{}", |cfg| cfg.tools = Some(vec!["read".to_owned()])).await;
    allowlisted.turn().await;
    allowlisted
        .reload_with(r#"{ "defaultTools": ["+grep"] }"#)
        .await;
    assert_eq!(allowlisted.active().await, sorted(&["read"]));

    let builtinless = rig("{}", |cfg| cfg.no_tools = Some(NoTools::Builtin)).await;
    builtinless.turn().await;
    builtinless
        .reload_with(r#"{ "defaultTools": ["+grep"] }"#)
        .await;
    assert_eq!(builtinless.active().await, Vec::<String>::new());

    let excluded = rig("{}", |cfg| cfg.exclude_tools = vec!["grep".to_owned()]).await;
    excluded.turn().await;
    excluded
        .reload_with(r#"{ "defaultTools": ["+grep", "+codemode"] }"#)
        .await;
    assert_eq!(
        excluded.active().await,
        sorted(&["bash", "edit", "codemode", "read", "write"])
    );
}
