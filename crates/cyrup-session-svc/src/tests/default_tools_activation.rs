//! `defaultTools` reaches the tools an extension registered — `codemode` first of all.
//!
//! `"defaultTools": ["+codemode"]` is the documented way to switch `codemode` on. The built-in
//! selector only walks the built-in registry, so the setting reached `read`/`bash`/`edit`/`write`
//! and nothing else: the session started with `codemode` registered and inactive, and
//! `["read", "codemode"]` produced a session with `read` alone.
//!
//! Every case here goes through the REAL [`SessionBuilder`] — settings document in, the session's
//! active tool names out — with the real `codemode` extension attached the way the binary attaches
//! it. A test of the selector alone could not have caught this: the selector was never handed the
//! extension's tools.
//!
//! Pi, read at v1.0.4: `sdk.ts:274-276` builds `initialActiveToolNames` from `options.tools ??
//! (options.noTools ? [] : (configuredDefaultToolNames ?? DEFAULT_TOOL_NAMES))`, the session
//! activates every registered tool the list names (`_refreshToolRegistry`,
//! `agent-session.ts:3489-3578`), and `reload()` activates the names newly added to `defaultTools`
//! without deactivating the ones removed (`:3652-3665`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_codemode_runtime::CodemodeExtension;
use cyrup_codemode_runtime::testkit::{ScriptedSandboxFactory, completed};
use cyrup_config::{FileSettingsStore, InMemorySettingsStore, SettingsScope};
use cyrup_core::{
    CancelToken, ExtensionId, StopReason, Tool, ToolCallId, ToolError, ToolExposure, ToolResult,
    ToolUpdateSink,
};
use cyrup_ext::{ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
use futures::FutureExt as _;
use serde_json::Value;
use tempfile::TempDir;

use crate::{
    AgentSession, AgentSessionRuntime, NoTools, SessionBuilder, SessionConfig, SessionFactory,
    SessionTarget,
};

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
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

/// An extension tool with a settable exposure that is registered inactive, as `codemode` is.
struct Probe {
    name: String,
    exposure: ToolExposure,
    default_active: bool,
    params: Value,
}

impl Probe {
    fn arc(name: &str, exposure: ToolExposure, default_active: bool) -> Arc<dyn Tool> {
        Arc::new(Self {
            name: name.to_owned(),
            exposure,
            default_active,
            params: serde_json::json!({ "type": "object", "properties": {} }),
        })
    }
}

#[async_trait::async_trait]
impl Tool for Probe {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    fn description(&self) -> &str {
        "probe tool"
    }
    fn exposure(&self) -> ToolExposure {
        self.exposure
    }
    fn default_active(&self) -> bool {
        self.default_active
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::default())
    }
}

struct ProbesExt(Vec<Arc<dyn Tool>>);

#[async_trait::async_trait]
impl NativeExtension for ProbesExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("default-tools-probes")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        for tool in &self.0 {
            api.register_tool(Arc::clone(tool));
        }
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

fn codemode_extension() -> CodemodeExtension {
    let script: cyrup_codemode_runtime::testkit::Script =
        Arc::new(|_code, _env| async { completed(Vec::new(), None) }.boxed());
    CodemodeExtension::new(
        Default::default(),
        Arc::new(ScriptedSandboxFactory::new(script)),
    )
}

/// One session's inputs: the settings documents and the command-line selection.
#[derive(Default)]
struct Spec {
    /// The global `settings.json`.
    global: Option<&'static str>,
    /// The project `.cyrup/settings.json`.
    project: Option<&'static str>,
    /// `--tools`.
    tools: Option<&'static [&'static str]>,
    /// `--no-tools` / `--no-builtin-tools`.
    no_tools: Option<NoTools>,
    /// `--exclude-tools`.
    exclude: &'static [&'static str],
    /// The project is trusted (the default) or not.
    untrusted: bool,
    /// Extra tools an extension registers at `init`.
    probes: Vec<Arc<dyn Tool>>,
}

fn config(fx: &Fixture, spec: &Spec) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(!spec.untrusted);
    cfg.persist = false;
    cfg.no_extensions = false;
    cfg.tools = spec
        .tools
        .map(|names| names.iter().map(|n| (*n).to_owned()).collect());
    cfg.no_tools = spec.no_tools;
    cfg.exclude_tools = spec.exclude.iter().map(|n| (*n).to_owned()).collect();
    cfg
}

async fn build(spec: Spec) -> (Arc<AgentSession>, Fixture) {
    let fx = fixture();
    let store = Arc::new(InMemorySettingsStore::new());
    if let Some(global) = spec.global {
        store.seed(SettingsScope::Global, global);
    }
    if let Some(project) = spec.project {
        store.seed(SettingsScope::Project, project);
    }
    let session = SessionBuilder::new(
        Arc::new(FauxProvider::new()) as Arc<dyn Provider>,
        config(&fx, &spec),
    )
    .settings_store(store)
    .with_codemode(codemode_extension())
    .with_native_extension(Arc::new(ProbesExt(spec.probes)))
    .build()
    .await
    .unwrap()
    .into_shared();
    (session, fx)
}

/// The names the session starts with, sorted so the assertion reads as a set.
async fn active(spec: Spec) -> Vec<String> {
    let (session, _fx) = build(spec).await;
    sorted(session.active_tool_names())
}

fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names
}

fn set(names: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
    v.sort();
    v
}

// ---------------------------------------------------------------------------------------- codemode

/// THE blocker: `{"defaultTools": ["+codemode"]}` keeps the four defaults and adds `codemode`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plus_codemode_activates_codemode_on_top_of_the_defaults() {
    let got = active(Spec {
        global: Some(r#"{"defaultTools":["+codemode"]}"#),
        ..Spec::default()
    })
    .await;
    assert_eq!(got, set(&["read", "bash", "edit", "write", "codemode"]));
}

/// The control: with no setting `codemode` stays registered and inactive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_the_setting_codemode_stays_inactive() {
    let got = active(Spec::default()).await;
    assert_eq!(got, set(&["read", "bash", "edit", "write"]));
}

/// A plain list REPLACES the defaults, and names an extension tool as well: `["read", "codemode"]`
/// gave `read` alone before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plain_list_replaces_the_defaults_and_includes_codemode() {
    let got = active(Spec {
        global: Some(r#"{"defaultTools":["read","codemode"]}"#),
        ..Spec::default()
    })
    .await;
    assert_eq!(got, set(&["read", "codemode"]));
}

/// `-name` applies to the baseline in order: `+codemode` then `-bash`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_minus_entry_removes_a_default_while_a_plus_entry_adds_codemode() {
    let got = active(Spec {
        global: Some(r#"{"defaultTools":["+codemode","-bash"]}"#),
        ..Spec::default()
    })
    .await;
    assert_eq!(got, set(&["read", "edit", "write", "codemode"]));
}

// ----------------------------------------------------------------------------- the command line

/// An explicit `--tools` allowlist replaces the configured list, as it does upstream (`options.tools
/// ?? …`): the setting's `codemode` is not added to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicit_allowlist_wins_over_the_setting() {
    let got = active(Spec {
        global: Some(r#"{"defaultTools":["+codemode"]}"#),
        tools: Some(&["read"]),
        ..Spec::default()
    })
    .await;
    assert_eq!(got, set(&["read"]));
}

/// …and naming `codemode` in the allowlist still activates it, which is what worked before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_allowlist_naming_codemode_still_activates_it() {
    let got = active(Spec {
        tools: Some(&["read", "bash", "edit", "write", "codemode"]),
        ..Spec::default()
    })
    .await;
    assert_eq!(got, set(&["read", "bash", "edit", "write", "codemode"]));
}

/// `--exclude-tools` removes a configured tool: pi filters the initial names by it (`sdk.ts:276`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exclude_tools_removes_a_configured_codemode() {
    let got = active(Spec {
        global: Some(r#"{"defaultTools":["+codemode"]}"#),
        exclude: &["codemode", "bash"],
        ..Spec::default()
    })
    .await;
    assert_eq!(got, set(&["read", "edit", "write"]));
}

/// `--no-builtin-tools` starts with an empty initial list (`options.noTools ? [] : …`), so the
/// configured names do not apply; extension tools registered active still do.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_builtin_tools_ignores_the_configured_names_but_keeps_default_active_extension_tools() {
    let got = active(Spec {
        global: Some(r#"{"defaultTools":["+codemode","+configured_ext"]}"#),
        no_tools: Some(NoTools::Builtin),
        probes: vec![
            Probe::arc("always_on", ToolExposure::Direct, true),
            Probe::arc("configured_ext", ToolExposure::Direct, false),
        ],
        ..Spec::default()
    })
    .await;
    assert_eq!(got, set(&["always_on"]));
}

/// `--no-tools` leaves nothing, whatever the setting says.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_tools_leaves_nothing_active() {
    let got = active(Spec {
        global: Some(r#"{"defaultTools":["+codemode"]}"#),
        no_tools: Some(NoTools::All),
        ..Spec::default()
    })
    .await;
    assert!(got.is_empty(), "{got:?}");
}

// ---------------------------------------------------------------------- layers and project trust

/// CFG-097: a project list of only modifiers APPENDS to the user's list, in a trusted project.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_trusted_projects_plus_codemode_appends_to_the_users_list() {
    let got = active(Spec {
        global: Some(r#"{"defaultTools":["read","bash"]}"#),
        project: Some(r#"{"defaultTools":["+codemode"]}"#),
        ..Spec::default()
    })
    .await;
    assert_eq!(got, set(&["read", "bash", "codemode"]));
}

/// …and an untrusted project's settings are not read at all, so its `+codemode` does nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_projects_plus_codemode_does_nothing() {
    let got = active(Spec {
        global: Some(r#"{"defaultTools":["read","bash"]}"#),
        project: Some(r#"{"defaultTools":["+codemode"]}"#),
        untrusted: true,
        ..Spec::default()
    })
    .await;
    assert_eq!(got, set(&["read", "bash"]));
}

// ------------------------------------------------------------------ other extension tool shapes

/// Every shape `registration` leaves inactive activates when the setting names it: registered
/// `defaultActive: false`, and the `codemode` and `deferred` exposures. A `hidden` tool does not:
/// "Activating it has no effect".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_named_extension_tool_activates_whatever_made_it_inactive() {
    let probes = || {
        vec![
            Probe::arc("inactive_direct", ToolExposure::Direct, false),
            Probe::arc("deferred_t", ToolExposure::Deferred, true),
            Probe::arc("codemode_t", ToolExposure::Codemode, true),
            Probe::arc("hidden_t", ToolExposure::Hidden, true),
            Probe::arc("unnamed", ToolExposure::Direct, false),
        ]
    };
    let got = active(Spec {
        global: Some(
            r#"{"defaultTools":["+inactive_direct","+deferred_t","+codemode_t","+hidden_t"]}"#,
        ),
        probes: probes(),
        ..Spec::default()
    })
    .await;
    assert_eq!(
        got,
        set(&[
            "read",
            "bash",
            "edit",
            "write",
            "inactive_direct",
            "deferred_t",
            "codemode_t"
        ])
    );
}

// ------------------------------------------------------------------------------------ diagnostics

/// A configured name that matches nothing is reported once, as a startup warning, after every
/// extension has registered — and a name that does match, or an MCP name whose server has not
/// connected yet, is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_default_tool_that_matches_nothing_is_reported_once() {
    let (session, _fx) = build(Spec {
        global: Some(
            r#"{"defaultTools":["+codemode","+codemdoe","+hidden_t","+mcp__docs__find","+codemdoe"]}"#,
        ),
        probes: vec![Probe::arc("hidden_t", ToolExposure::Hidden, true)],
        ..Spec::default()
    })
    .await;
    assert_eq!(
        session.services().startup_diagnostics.settings,
        vec![
            "defaultTools: no activatable tool is registered as \"codemdoe\", \"hidden_t\""
                .to_owned()
        ]
    );
}

/// Nothing to report when every configured name matched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clean_setting_reports_nothing() {
    let (session, _fx) = build(Spec {
        global: Some(r#"{"defaultTools":["+codemode"]}"#),
        ..Spec::default()
    })
    .await;
    assert!(session.services().startup_diagnostics.settings.is_empty());
}

/// An explicit `--tools` makes the setting irrelevant, so a name in it that matches nothing is not
/// worth a warning.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_warning_when_the_command_line_replaces_the_setting() {
    let (session, _fx) = build(Spec {
        global: Some(r#"{"defaultTools":["+nonesuch"]}"#),
        tools: Some(&["read"]),
        ..Spec::default()
    })
    .await;
    assert!(session.services().startup_diagnostics.settings.is_empty());
}

// ------------------------------------------------------------------------- tools registered later

fn done_steps() -> Vec<FauxResponseStep> {
    vec![FauxResponseStep::factory(|_ctx, _opts, _state, _model| {
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
    })]
}

/// A configured name no tool has registered at build time activates when one registers before the
/// first run — an MCP server that connects after the session is built. (CYRUP-DELTA: pi drops it.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_configured_tool_that_registers_later_is_activated() {
    let (session, _fx) = build(Spec {
        global: Some(r#"{"defaultTools":["+mcp__docs__find"]}"#),
        ..Spec::default()
    })
    .await;
    assert!(
        !session
            .active_tool_names()
            .contains(&"mcp__docs__find".to_owned())
    );
    session
        .services()
        .ext_host
        .register_late_tool(
            ExtensionId::from("mcp"),
            Probe::arc("mcp__docs__find", ToolExposure::Deferred, false),
        )
        .unwrap();
    session.refresh_extension_tools().await;
    assert!(
        session
            .active_tool_names()
            .contains(&"mcp__docs__find".to_owned()),
        "{:?}",
        session.active_tool_names()
    );
}

/// …but only until the first run starts: a tool that never registers does not stay pending (pi
/// `_runAgentPrompt` clears `_pendingToolNames`), so a registration after the run is just a
/// registration, and an inactive tool stays inactive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_names_still_waiting_when_a_run_starts_are_dropped() {
    let fx = fixture();
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(
        SettingsScope::Global,
        r#"{"defaultTools":["+mcp__docs__find"]}"#,
    );
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(done_steps());
    let spec = Spec::default();
    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, config(&fx, &spec))
        .settings_store(store)
        .with_codemode(codemode_extension())
        .build()
        .await
        .unwrap()
        .into_shared();
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    session
        .services()
        .ext_host
        .register_late_tool(
            ExtensionId::from("mcp"),
            Probe::arc("mcp__docs__find", ToolExposure::Deferred, false),
        )
        .unwrap();
    session.refresh_extension_tools().await;
    assert!(
        !session
            .active_tool_names()
            .contains(&"mcp__docs__find".to_owned()),
        "{:?}",
        session.active_tool_names()
    );
}

// -------------------------------------------------------------------------------------- /reload

/// A runtime over a settings file that `/reload` re-reads, with one persisted run so the session
/// has a transcript that declares its loadout.
async fn runtime_over(fx: &Fixture, initial: &str) -> Arc<AgentSessionRuntime> {
    let settings_path = fx.agent_dir.join("settings.json");
    std::fs::write(&settings_path, initial).unwrap();
    let store = Arc::new(FileSettingsStore::new(
        settings_path,
        fx.cwd.join(".cyrup").join("settings.json"),
    ));
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(done_steps());
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.persist = true;
    cfg.no_extensions = false;
    let factory = Arc::new(
        SessionFactory::new(faux as Arc<dyn Provider>, cfg)
            .settings_store(store)
            .with_codemode(codemode_extension()),
    );
    let runtime = AgentSessionRuntime::create(factory, SessionTarget::New)
        .await
        .unwrap();
    let session = runtime.session().await;
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;
    runtime
}

/// pi `reload()`: "Activate tools newly added to defaultTools. Removed ones stay active, and tools
/// disabled during the session stay disabled unless the setting newly adds them."
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_activates_an_added_name_and_keeps_the_rest_as_the_session_left_them() {
    let fx = fixture();
    let runtime = runtime_over(&fx, r#"{"defaultTools":["read","bash","edit","write"]}"#).await;
    // The user turned `bash` off during the session.
    let session = runtime.session().await;
    session
        .set_active_tools_by_name(&["read".to_owned(), "edit".to_owned(), "write".to_owned()])
        .await;
    let _ = session.prompt("again").await;
    session.wait_for_idle().await;

    // The setting now adds `codemode` and drops `write`.
    std::fs::write(
        fx.agent_dir.join("settings.json"),
        r#"{"defaultTools":["read","bash","edit","codemode"]}"#,
    )
    .unwrap();
    runtime.reload(None).await.unwrap();

    let got = sorted(runtime.session().await.active_tool_names());
    assert_eq!(
        got,
        set(&["read", "edit", "write", "codemode"]),
        "codemode is new, `write` was removed from the setting but stays on, and `bash` was \
         turned off during the session and is not re-enabled by an unchanged setting"
    );
}

/// A reload with the setting unchanged restores the session's loadout and adds nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_with_an_unchanged_setting_adds_nothing() {
    let fx = fixture();
    let runtime = runtime_over(&fx, r#"{"defaultTools":["+codemode"]}"#).await;
    assert!(
        runtime
            .session()
            .await
            .active_tool_names()
            .contains(&"codemode".to_owned())
    );
    let session = runtime.session().await;
    session
        .set_active_tools_by_name(&["read".to_owned(), "bash".to_owned()])
        .await;
    let _ = session.prompt("again").await;
    session.wait_for_idle().await;

    runtime.reload(None).await.unwrap();
    let got = sorted(runtime.session().await.active_tool_names());
    assert_eq!(got, set(&["read", "bash"]));
}

/// SEAM-155: pi's `reload()` diffs the `defaultTools` the session had before the reload against the
/// ones after it (`previousDefaultTools`), so a name that was turned off, then removed from the
/// setting by one reload and added again by the next, is newly added the second time and activates.
/// The tools the transcript's first system message declared are no stand-in for that: they still
/// list the name, whatever the settings did since.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_activates_a_name_that_was_removed_from_the_setting_and_is_added_again() {
    let fx = fixture();
    let runtime = runtime_over(
        &fx,
        r#"{"defaultTools":["read","bash","edit","write","codemode"]}"#,
    )
    .await;
    assert!(
        runtime
            .session()
            .await
            .active_tool_names()
            .contains(&"codemode".to_owned())
    );
    // The user turns `codemode` off, and a run records it.
    let session = runtime.session().await;
    session
        .set_active_tools_by_name(&set(&["read", "bash", "edit", "write"]))
        .await;
    let _ = session.prompt("again").await;
    session.wait_for_idle().await;

    // The setting drops `codemode`: nothing is added, and it stays off.
    std::fs::write(
        fx.agent_dir.join("settings.json"),
        r#"{"defaultTools":["read","bash","edit","write"]}"#,
    )
    .unwrap();
    runtime.reload(None).await.unwrap();
    assert_eq!(
        sorted(runtime.session().await.active_tool_names()),
        set(&["read", "bash", "edit", "write"])
    );

    // The setting adds it back: relative to the setting the session was last loaded with it is new.
    std::fs::write(
        fx.agent_dir.join("settings.json"),
        r#"{"defaultTools":["read","bash","edit","write","codemode"]}"#,
    )
    .unwrap();
    runtime.reload(None).await.unwrap();
    assert_eq!(
        sorted(runtime.session().await.active_tool_names()),
        set(&["read", "bash", "edit", "write", "codemode"]),
        "the name the setting gained again is activated by the reload"
    );
}
