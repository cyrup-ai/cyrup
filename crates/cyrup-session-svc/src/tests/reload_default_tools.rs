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

// ---- SEAM-148: a `+name`/`-name`-only `tools` list -------------------------------------------
//
// pi v1.1.0 (`ddaa0a034`) applies such a list to the default selection instead of reading it as an
// allowlist: `core/sdk.ts:280-294` @f1b2e77f5 (`applyToolModifiers(defaultToolNames, tools)`,
// `allowedToolNames` `undefined` unless `noTools === "all"`), `usesDefaultTools` stays true with
// `defaultToolModifiers` kept (`:472-473`), and `reload` reapplies them (`agent-session.ts:3666-
// 3669`). RED before the fix: every case below started with NO tools, because `select_active_tools`
// matched the literal name `"+codemode"`, and `resolve_allowed_tool_names` pinned an allowlist of it.

fn allowed(names: Option<&std::collections::HashSet<String>>) -> Option<Vec<String>> {
    names.map(|set| {
        let mut v: Vec<String> = set.iter().cloned().collect();
        v.sort();
        v
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modifier_tools_list_adjusts_the_default_selection() {
    let added = rig("{}", |cfg| cfg.tools = Some(vec!["+codemode".to_owned()])).await;
    assert_eq!(
        added.active().await,
        sorted(&["read", "bash", "edit", "write", "codemode"])
    );
    let session = added.runtime.session().await;
    assert_eq!(
        allowed(session.services().allowed_tool_names.as_ref()),
        None
    );
    assert!(session.services().uses_default_tools);

    let grep = rig("{}", |cfg| cfg.tools = Some(vec!["+grep".to_owned()])).await;
    assert_eq!(
        grep.active().await,
        sorted(&["read", "bash", "edit", "write", "grep"])
    );

    // The modifiers apply to the `defaultTools` setting when it is configured.
    let over_setting = rig(r#"{ "defaultTools": ["read"] }"#, |cfg| {
        cfg.tools = Some(vec!["+grep".to_owned()]);
    })
    .await;
    assert_eq!(over_setting.active().await, sorted(&["read", "grep"]));

    // `noTools: "all"` makes the post-modifier set the allowlist (`sdk.ts:287-288`).
    let all = rig("{}", |cfg| {
        cfg.no_tools = Some(NoTools::All);
        cfg.tools = Some(vec!["+read".to_owned()]);
    })
    .await;
    assert_eq!(all.active().await, sorted(&["read"]));
    let session = all.runtime.session().await;
    assert_eq!(
        allowed(session.services().allowed_tool_names.as_ref()),
        Some(sorted(&["read"]))
    );
    assert!(!session.services().uses_default_tools);

    // `noTools: "builtin"` starts the modifiers from an empty base and pins nothing.
    let builtin = rig("{}", |cfg| {
        cfg.no_tools = Some(NoTools::Builtin);
        cfg.tools = Some(vec!["+read".to_owned(), "+codemode".to_owned()]);
    })
    .await;
    assert_eq!(builtin.active().await, sorted(&["read", "codemode"]));
    let session = builtin.runtime.session().await;
    assert_eq!(
        allowed(session.services().allowed_tool_names.as_ref()),
        None
    );
}

/// `-bash` stays removed across `/reload`, while a name the setting newly adds is activated
/// (`agent-session.ts:3666-3677` @f1b2e77f5). A tool the user turned off stays off too: the
/// `/reload` rebuild restores a modifier session's transcript loadout, as pi's `reload` keeps the
/// live active set (`:3682-3686`). RED with that restore gated off: `write` came back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removed_tool_stays_removed_across_reload() {
    let minus = rig("{}", |cfg| cfg.tools = Some(vec!["-bash".to_owned()])).await;
    assert_eq!(minus.active().await, sorted(&["read", "edit", "write"]));
    minus.turn().await;

    minus.reload_with(r#"{ "defaultTools": ["+grep"] }"#).await;
    assert_eq!(
        minus.active().await,
        sorted(&["read", "edit", "write", "grep"])
    );

    // Turn `write` off, then reload with an unchanged setting: nothing comes back.
    minus.set_active(&["read", "edit", "grep"]).await;
    minus.turn().await;
    minus.reload_with(r#"{ "defaultTools": ["+grep"] }"#).await;
    assert_eq!(minus.active().await, sorted(&["read", "edit", "grep"]));

    // The reapply itself: a setting that newly names `bash` does not bring it back, because the
    // `-bash` modifier is applied to both sides of the comparison (pi `getDefaultTools()` before
    // and after `settingsManager.reload()`). Without it, `bash` reads as newly added.
    let narrowed = rig(r#"{ "defaultTools": ["read"] }"#, |cfg| {
        cfg.tools = Some(vec!["-bash".to_owned()]);
    })
    .await;
    assert_eq!(narrowed.active().await, sorted(&["read"]));
    narrowed.turn().await;
    narrowed
        .reload_with(r#"{ "defaultTools": ["read", "bash", "grep"] }"#)
        .await;
    assert_eq!(narrowed.active().await, sorted(&["read", "grep"]));
}

/// A list mixing plain names with modifiers, or a modifier with a `*`, is refused before the build
/// (pi `core/sdk.ts:280-281` @f1b2e77f5: `Invalid tools option: ${getToolListError(tools)}`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mixed_or_patterned_modifier_list_refuses_the_build() {
    for (tools, expected) in [
        (
            vec!["read", "+grep"],
            "Invalid tools option: tool names cannot be mixed with +name or -name entries",
        ),
        (
            vec!["+mcp__*"],
            "Invalid tools option: +name and -name entries take exact tool names, not patterns: +mcp__*",
        ),
    ] {
        let tmp = TempDir::new().unwrap();
        let mut cfg = SessionConfig::new(tmp.path().join("p"), tmp.path().join("a"));
        cfg.trust_override = Some(true);
        cfg.tools = Some(tools.iter().map(|t| (*t).to_owned()).collect());
        let factory = Arc::new(SessionFactory::new(
            Arc::new(FauxProvider::new()) as Arc<dyn Provider>,
            cfg,
        ));
        let err = AgentSessionRuntime::create(factory, SessionTarget::New)
            .await
            .err()
            .expect("the build is refused");
        assert_eq!(err.to_string(), expected);
    }
}

/// A `--continue`/`--resume` launch applies the modifiers over whatever loadout the transcript
/// saved: pi `sdk.ts:294` @f1b2e77f5 always passes `initialActiveToolNames`, and
/// `agent-session.ts:525` restores from the transcript only when it is `undefined`. `/reload` is the
/// one rebuild that keeps the saved loadout (`a_removed_tool_stays_removed_across_reload`). RED
/// while every modifier build restored the transcript: `--tools -bash` resumed WITH `bash`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resumed_launch_applies_the_modifiers_over_the_saved_loadout() {
    // The first run saves the full default loadout, `bash` included.
    let first = rig("{}", |_| {}).await;
    first.turn().await;
    assert_eq!(
        first.active().await,
        sorted(&["read", "bash", "edit", "write"])
    );
    let file = first.runtime.session().await.session_file().await.unwrap();
    let root = first._tmp.path().to_path_buf();

    for (tools, expected) in [
        (vec!["-bash"], sorted(&["read", "edit", "write"])),
        (
            vec!["+grep"],
            sorted(&["read", "bash", "edit", "write", "grep"]),
        ),
    ] {
        let mut cfg = SessionConfig::new(root.join("project"), root.join("agent"));
        cfg.trust_override = Some(true);
        cfg.persist = true;
        cfg.tools = Some(tools.iter().map(|t| (*t).to_owned()).collect());
        let factory = Arc::new(SessionFactory::new(
            Arc::new(FauxProvider::new()) as Arc<dyn Provider>,
            cfg,
        ));
        let resumed = AgentSessionRuntime::create(factory, SessionTarget::Resume(file.clone()))
            .await
            .unwrap();
        let mut active = resumed.session().await.active_tool_names();
        active.sort();
        assert_eq!(active, expected, "--tools {tools:?} on a resumed launch");
    }
}

// ---- SEAM-159: `/reload` keeps the LIVE active set ---------------------------------------------
//
// pi `reload()` rebuilds with `activeToolNames: [...this.getActiveToolNames(), ...addedDefaultTools]`
// (`core/agent-session.ts:3678-3684` @v1.1.0) — the names active right now, not the ones the
// transcript last recorded. The tests above all ran a turn after `set_active`, which records the
// change, so they could not see the difference.

/// The SEAM-159 Verify clause. RED at HEAD: `write` came back, because the rebuild restored the
/// loadout the transcript recorded at the last run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_turned_off_without_a_run_stays_off_across_reload() {
    let rig = rig("{}", |_| {}).await;
    rig.turn().await;
    rig.set_active(&["read", "bash", "edit"]).await;
    rig.reload_with("{}").await;
    assert_eq!(rig.active().await, sorted(&["read", "bash", "edit"]));
}

/// The other direction. RED at HEAD: `grep` was dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_turned_on_without_a_run_stays_on_across_reload() {
    let rig = rig("{}", |_| {}).await;
    rig.turn().await;
    rig.set_active(&["read", "bash", "edit", "write", "grep"])
        .await;
    rig.reload_with("{}").await;
    assert_eq!(
        rig.active().await,
        sorted(&["read", "bash", "edit", "write", "grep"])
    );
}

/// A cold resume (a new process) has no live set and no earlier `defaultTools` to compare with: the
/// transcript's recorded loadout is the active set, and its FIRST system message is the baseline a
/// configured name is new against (`default_tools`' CYRUP-DELTA). A tool turned off before the
/// resume stays off; a name the setting adds since the session began activates. Passes at HEAD —
/// the NON-REGRESSION GUARD for the cold-resume baseline the SEAM-159 change makes explicit
/// (`DefaultToolsBaseline::FirstSystemMessage`); reading the baseline from the LAST loadout instead
/// turns it red, because `write` is then the off tool's own record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cold_resume_compares_the_setting_with_the_loadout_the_session_began_with() {
    let first = rig("{}", |_| {}).await;
    first.turn().await;
    first.set_active(&["read", "bash", "edit"]).await;
    first.turn().await;
    let file = first.runtime.session().await.session_file().await.unwrap();
    let root = first._tmp.path().to_path_buf();

    let mut cfg = SessionConfig::new(root.join("project"), root.join("agent"));
    cfg.trust_override = Some(true);
    cfg.persist = true;
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(
        SettingsScope::Global,
        r#"{ "defaultTools": ["+codemode"] }"#,
    );
    let factory = Arc::new(
        SessionFactory::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
            .settings_store(store)
            .with_codemode(CodemodeExtension::new(
                Default::default(),
                Arc::new(cyrup_codemode_runtime::tool::EngineSandboxFactory),
            )),
    );
    let resumed = AgentSessionRuntime::create(factory, SessionTarget::Resume(file))
        .await
        .unwrap();
    let mut active = resumed.session().await.active_tool_names();
    active.sort();
    assert_eq!(active, sorted(&["read", "bash", "edit", "codemode"]));
}
