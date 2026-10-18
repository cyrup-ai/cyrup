//! SUBA-251 — the native `intercom` alias follows the agent dir the SESSION was built with.
//!
//! The orchestrator registers the supervisor channel under the bare name `intercom` only when
//! `cyrup-intercom` will not attach to own it (pi's `!hasTool(pi, "intercom")`,
//! `native-supervisor-channel.ts:637` @ad11b7ab). `InitApi` has no tool-registry query, so the gate
//! asks the signal that decides whether intercom attaches: its `<agent dir>/intercom/config.json`, or
//! `CYRUP_INTERCOM`. The question has to be asked of the directory intercom itself looks in, which is
//! the one the launcher resolved (`ConfigDirs::agent_dir`) and handed this extension as
//! `roots.agent_dir()`.
//!
//! The gate used to re-derive that directory from the process environment. The two agree while nothing
//! else decides a session's agent dir, and disagree for a caller that does: the gate looked in the
//! environment's directory, found no intercom, registered a second `intercom`, and the real extension
//! then failed to load with `Tool "intercom" conflicts with subagents`. The same disagreement the other
//! way round left the name with no owner.
//!
//! The pure gate (`native_intercom_alias_should_register`) has its own tests. What none of them could
//! see is which directory the extension hands it, so these load the real extension and read what it
//! registered. The environment is pinned through `SubagentExtensionConfig::env_overrides`: the crate
//! forbids `unsafe`, and moving a process-global variable would race every other reader anyway.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use crate::extension::SubagentsExtension;
use crate::paths::Roots;
use crate::registration::SubagentExtensionConfig;
use cyrup_ext::{ExtMode, ExtensionHost, HostConfig};

/// The orchestrator, loaded over `roots` and seeing exactly `env` of the environment.
async fn orchestrator(
    roots: Roots,
    env: BTreeMap<String, Option<String>>,
    cwd: &Path,
) -> Arc<ExtensionHost> {
    let config = SubagentExtensionConfig {
        roots,
        env_overrides: env,
        ..SubagentExtensionConfig::default()
    };
    let host = Arc::new(ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: cwd.to_path_buf(),
    }));
    host.load_native(Arc::new(SubagentsExtension::with_config_and_cwd(
        config,
        cwd.to_path_buf(),
    )))
    .await
    .expect("the subagents extension loads");
    host
}

/// An environment whose home is `home`, with every intercom signal scrubbed: the ladder the old gate
/// walked ends in `<home>/.cyrup/agent`, wherever the session's own agent dir is.
fn environment_at(home: &Path) -> BTreeMap<String, Option<String>> {
    let home = home.to_string_lossy().into_owned();
    BTreeMap::from([
        ("CYRUP_HOME".to_string(), Some(home.clone())),
        ("HOME".to_string(), Some(home)),
        ("CYRUP_INTERCOM".to_string(), None),
        ("CYRUP_CODING_AGENT_DIR".to_string(), None),
        ("CYRUP_AGENT_DIR".to_string(), None),
    ])
}

/// Install intercom in `agent_dir` the way a user does: a config file.
fn install_intercom(agent_dir: &Path, config: &str) {
    let dir = agent_dir.join("intercom");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), config).unwrap();
}

fn alias_registered(host: &ExtensionHost) -> bool {
    host.registry().tool("intercom").unwrap().is_some()
}

/// HEADLINE. The session's agent dir has intercom installed and the environment points somewhere
/// else: intercom attaches and owns the name, so the alias must not register. Before the fix it did,
/// and the real intercom's registration was rejected as a conflict.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_alias_stands_down_for_an_intercom_installed_in_the_sessions_agent_dir() {
    let session = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let roots = Roots::sandboxed(session.path());
    install_intercom(roots.agent_dir(), "{}");

    let host = orchestrator(roots, environment_at(elsewhere.path()), session.path()).await;

    assert!(
        !alias_registered(&host),
        "intercom is installed where this session reads its config, so it owns `intercom`"
    );
}

/// The other direction: the environment names an agent dir with intercom installed and the session's
/// own does not. Intercom does not attach, so nobody owns the name and the alias must register. Before
/// the fix the gate answered for the environment's directory and left `intercom` with no tool at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_alias_registers_when_only_the_environments_agent_dir_has_intercom() {
    let session = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let roots = Roots::sandboxed(session.path());
    install_intercom(&elsewhere.path().join(".cyrup").join("agent"), "{}");

    let host = orchestrator(roots, environment_at(elsewhere.path()), session.path()).await;

    assert!(
        alias_registered(&host),
        "intercom is not installed for this session, so nothing else would own `intercom`"
    );
}

/// The control: no intercom anywhere, so the alias is how a plain orchestrator keeps the name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_alias_registers_when_intercom_is_installed_nowhere() {
    let session = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let roots = Roots::sandboxed(session.path());

    let host = orchestrator(roots, environment_at(elsewhere.path()), session.path()).await;

    assert!(alias_registered(&host));
}

/// `CYRUP_INTERCOM` is the other half of intercom's install signal, and it still decides: it is read
/// from the environment the extension sees, not from a directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_install_env_var_still_stands_the_alias_down() {
    let session = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let roots = Roots::sandboxed(session.path());
    let mut env = environment_at(elsewhere.path());
    env.insert("CYRUP_INTERCOM".to_string(), Some("1".to_string()));

    let host = orchestrator(roots, env, session.path()).await;

    assert!(!alias_registered(&host));
}

/// A config that disables intercom is a config that does not attach it: the alias keeps the name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_disabled_intercom_config_leaves_the_alias_in_place() {
    let session = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let roots = Roots::sandboxed(session.path());
    install_intercom(roots.agent_dir(), r#"{"enabled": false}"#);

    let host = orchestrator(roots, environment_at(elsewhere.path()), session.path()).await;

    assert!(alias_registered(&host));
}
