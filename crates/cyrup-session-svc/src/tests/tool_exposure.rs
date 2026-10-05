//! CODE-005, session half — a registered tool's exposure decides whether it is ACTIVE, whether it
//! is CALLABLE and whether the PROVIDER RECEIVES its declaration.
//!
//! Every schema assertion reads `Context::tools` inside the faux provider's response factory, which
//! is the exact value a real provider adapter serialises, so a tool that leaked into the request
//! fails here regardless of which internal list carried it. pi's rules, read at v1.0.1:
//! `_isActivatedOnRegistration` (`core/agent-session.ts:3554`), `_refreshToolRegistry`
//! (`:3445-3545`), `_getCallableTools` (`:1515-1520`), `_applyToolLoadout` (`:1528-1572`),
//! `_rebuildSystemPrompt` (`:1651-1678`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cyrup_core::{
    CancelToken, Content, ExtensionId, LoadoutView, StopReason, Tool, ToolCallId, ToolError,
    ToolExposure, ToolLoadoutChanges, ToolNamespace, ToolResult, ToolUpdateSink,
};
use cyrup_ext::{ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use cyrup_provider::{Provider, ToolDef};
use tempfile::TempDir;

use crate::{AgentSession, SessionBuilder, SessionConfig};

type Hook = Arc<dyn Fn(&LoadoutView<'_>) -> Result<ToolLoadoutChanges, ToolError> + Send + Sync>;

/// A tool with a settable exposure, a ran-flag, a prompt snippet and an optional loadout hook.
struct Probe {
    name: String,
    exposure: ToolExposure,
    default_active: bool,
    namespace: Option<ToolNamespace>,
    params: serde_json::Value,
    ran: Arc<AtomicBool>,
    hook: Option<Hook>,
}

impl Probe {
    fn new(name: &str, exposure: ToolExposure) -> Self {
        Self {
            name: name.to_string(),
            exposure,
            default_active: true,
            namespace: None,
            params: serde_json::json!({"type": "object", "properties": {}}),
            ran: Arc::new(AtomicBool::new(false)),
            hook: None,
        }
    }

    fn inactive(mut self) -> Self {
        self.default_active = false;
        self
    }

    fn in_namespace(mut self, name: &str) -> Self {
        self.namespace = Some(ToolNamespace {
            name: name.to_string(),
            description: Some("a group".to_string()),
            instructions: None,
        });
        self
    }

    fn hiding(
        mut self,
        hidden: &'static str,
        describing: Option<(&'static str, &'static str)>,
    ) -> Self {
        self.hook = Some(Arc::new(move |_| {
            let mut descriptions = BTreeMap::new();
            if let Some((tool, text)) = describing {
                descriptions.insert(tool.to_string(), text.to_string());
            }
            Ok(ToolLoadoutChanges {
                descriptions,
                hidden_declarations: vec![hidden.to_string()],
            })
        }));
        self
    }

    fn failing(mut self) -> Self {
        self.hook = Some(Arc::new(|_| Err(ToolError::new("hook exploded"))));
        self
    }

    fn ran(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.ran)
    }

    fn arc(self) -> Arc<dyn Tool> {
        Arc::new(self)
    }
}

#[async_trait::async_trait]
impl Tool for Probe {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn description(&self) -> &str {
        "probe tool"
    }
    fn prompt_snippet(&self) -> Option<&str> {
        // Leaked once per probe name: the trait wants `&str` borrowed from `self`, and the snippet
        // text is derived from the name so a test can search the prompt for it.
        Some(Box::leak(format!("SNIPPET-{}", self.name).into_boxed_str()))
    }
    fn exposure(&self) -> ToolExposure {
        self.exposure
    }
    fn namespace(&self) -> Option<&ToolNamespace> {
        self.namespace.as_ref()
    }
    fn default_active(&self) -> bool {
        self.default_active
    }
    fn prepare_loadout(&self, view: &LoadoutView<'_>) -> Result<ToolLoadoutChanges, ToolError> {
        match &self.hook {
            Some(f) => f(view),
            None => Ok(ToolLoadoutChanges::default()),
        }
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: serde_json::Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.ran.store(true, Ordering::SeqCst);
        Ok(ToolResult {
            content: vec![Content::text("ran")],
            ..Default::default()
        })
    }
}

/// Registers its tools at `init`, the way an extension's `registerTool` calls do.
struct ToolsExt(Vec<Arc<dyn Tool>>);

#[async_trait::async_trait]
impl NativeExtension for ToolsExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("exposure-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        for t in &self.0 {
            api.register_tool(Arc::clone(t));
        }
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

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

fn config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

/// What the provider received on each request: the declarations and the system prompt.
type Requests = Arc<Mutex<Vec<(Vec<ToolDef>, String)>>>;

#[derive(Clone)]
enum Reply {
    Call(&'static str),
    Text(&'static str),
}

fn script(requests: &Requests, replies: Vec<Reply>) -> Arc<FauxProvider> {
    let steps: Vec<FauxResponseStep> = replies
        .into_iter()
        .map(|reply| {
            let seen = Arc::clone(requests);
            FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                seen.lock().unwrap().push((
                    ctx.tools.clone(),
                    ctx.system_prompt.clone().unwrap_or_default(),
                ));
                match reply {
                    Reply::Call(name) => faux_assistant_message(
                        vec![faux_tool_call(name.to_string(), serde_json::json!({}))],
                        StopReason::ToolUse,
                    ),
                    Reply::Text(t) => {
                        faux_assistant_message(vec![faux_text(t.to_string())], StopReason::Stop)
                    }
                }
            })
        })
        .collect();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(steps);
    faux
}

async fn session_with(
    fx: &Fixture,
    provider: Arc<FauxProvider>,
    tools: Vec<Arc<dyn Tool>>,
) -> Arc<AgentSession> {
    SessionBuilder::new(provider as Arc<dyn Provider>, config(fx))
        .with_native_extension(Arc::new(ToolsExt(tools)))
        .build()
        .await
        .unwrap()
        .into_shared()
}

fn names(tools: &[ToolDef]) -> Vec<&str> {
    tools.iter().map(|t| t.name.as_str()).collect()
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn one_of_each() -> (Vec<Arc<dyn Tool>>, BTreeMap<&'static str, Arc<AtomicBool>>) {
    let probes = vec![
        Probe::new("direct_t", ToolExposure::Direct),
        Probe::new("model_only_t", ToolExposure::ModelOnly),
        Probe::new("codemode_t", ToolExposure::Codemode).in_namespace("mcp__docs"),
        Probe::new("deferred_t", ToolExposure::Deferred),
        Probe::new("hidden_t", ToolExposure::Hidden),
        Probe::new("direct_inactive_t", ToolExposure::Direct).inactive(),
    ];
    let mut flags = BTreeMap::new();
    let mut tools = Vec::new();
    for p in probes {
        let key: &'static str = Box::leak(p.name.clone().into_boxed_str());
        flags.insert(key, p.ran());
        tools.push(p.arc());
    }
    (tools, flags)
}

/// HEADLINE. A session whose extension registers one tool of every exposure: the provider's request
/// declares exactly the tools registration activates (`direct`, `model-only`), and the tools that
/// are registered but not activated (`codemode`, `deferred`, `hidden`, `defaultActive: false`) are
/// absent from it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_schema_the_provider_receives_declares_only_what_registration_activates() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = script(&requests, vec![Reply::Text("done")]);
    let (tools, _) = one_of_each();
    let session = session_with(&fx, faux, tools).await;

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    let reqs = requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1);
    let declared = names(&reqs[0].0);
    assert!(
        declared.contains(&"direct_t"),
        "direct is declared: {declared:?}"
    );
    assert!(
        declared.contains(&"model_only_t"),
        "model-only is declared: {declared:?}"
    );
    for absent in ["codemode_t", "deferred_t", "hidden_t", "direct_inactive_t"] {
        assert!(
            !declared.contains(&absent),
            "{absent} must not reach the provider: {declared:?}"
        );
    }
    // …and the system prompt does not list them either.
    let prompt = &reqs[0].1;
    assert!(prompt.contains("SNIPPET-direct_t"), "{prompt}");
    for absent in ["codemode_t", "deferred_t", "hidden_t", "direct_inactive_t"] {
        assert!(
            !prompt.contains(&format!("SNIPPET-{absent}")),
            "{absent} must not be listed in the prompt: {prompt}"
        );
    }
}

/// The active set, the callable set and `getAllTools` rows for the same registration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_callable_and_registered_views_follow_pis_predicates() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = script(&requests, vec![Reply::Text("done")]);
    let (tools, _) = one_of_each();
    let session = session_with(&fx, faux, tools).await;

    let active = session.active_tool_names();
    for (name, want) in [
        ("direct_t", true),
        ("model_only_t", true),
        ("codemode_t", false),
        ("deferred_t", false),
        ("hidden_t", false),
        ("direct_inactive_t", false),
    ] {
        assert_eq!(
            active.iter().any(|n| n == name),
            want,
            "{name} active? {active:?}"
        );
    }

    // Callable: the active `direct` tools and every registered codemode/deferred tool; never
    // model-only, never hidden, never an inactive direct tool.
    let callable = session.callable_tool_names();
    for (name, want) in [
        ("direct_t", true),
        ("model_only_t", false),
        ("codemode_t", true),
        ("deferred_t", true),
        ("hidden_t", false),
        ("direct_inactive_t", false),
    ] {
        assert_eq!(
            callable.iter().any(|n| n == name),
            want,
            "{name} callable? {callable:?}"
        );
    }

    // Registered: every tool, with its exposure and namespace on the row.
    let rows = session.all_tools();
    let row = |n: &str| {
        rows.iter()
            .find(|r| r.name == n)
            .unwrap_or_else(|| panic!("{n} registered"))
    };
    assert_eq!(row("codemode_t").exposure, ToolExposure::Codemode);
    assert_eq!(row("hidden_t").exposure, ToolExposure::Hidden);
    assert_eq!(row("direct_t").exposure, ToolExposure::Direct);
    assert_eq!(
        row("codemode_t").namespace.as_ref().unwrap().name,
        "mcp__docs"
    );
    assert!(row("direct_t").namespace.is_none());
}

/// Explicitly naming a `codemode`/`deferred` tool in the active set declares it (what `tool_search`
/// loading a tool does); naming a `hidden` one does nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_activation_declares_deferred_and_codemode_but_never_hidden() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = script(&requests, vec![Reply::Text("done")]);
    let (tools, _) = one_of_each();
    let session = session_with(&fx, faux, tools).await;

    let mut names_now = session.active_tool_names();
    names_now.extend(strings(&["deferred_t", "codemode_t", "hidden_t"]));
    session.set_active_tools_by_name(&names_now).await;

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;
    let reqs = requests.lock().unwrap().clone();
    let declared = names(&reqs[0].0);
    assert!(declared.contains(&"deferred_t"), "{declared:?}");
    assert!(declared.contains(&"codemode_t"), "{declared:?}");
    assert!(
        !declared.contains(&"hidden_t"),
        "hidden is never declared: {declared:?}"
    );
    assert!(
        !session.active_tool_names().iter().any(|n| n == "hidden_t"),
        "activating a hidden tool has no effect"
    );
}

/// A tool whose declaration a `prepare_loadout` hook hides: absent from the request and the prompt
/// listing, yet the loop still runs it when the model calls it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hidden_declaration_leaves_the_request_and_prompt_but_is_still_dispatched() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = script(&requests, vec![Reply::Call("echo_t"), Reply::Text("done")]);
    let echo = Probe::new("echo_t", ToolExposure::Direct);
    let echo_ran = echo.ran();
    let session = session_with(
        &fx,
        faux,
        vec![
            Probe::new("hider_t", ToolExposure::Direct)
                .hiding("echo_t", None)
                .arc(),
            echo.arc(),
        ],
    )
    .await;

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    let reqs = requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2, "call + final answer");
    for (i, (decls, prompt)) in reqs.iter().enumerate() {
        assert!(
            names(decls).contains(&"hider_t"),
            "request {i}: {:?}",
            names(decls)
        );
        assert!(
            !names(decls).contains(&"echo_t"),
            "request {i}: {:?}",
            names(decls)
        );
        assert!(
            !prompt.contains("SNIPPET-echo_t"),
            "request {i}: the prompt must not list a tool the request does not declare: {prompt}"
        );
        assert!(prompt.contains("SNIPPET-hider_t"), "request {i}: {prompt}");
    }
    assert!(
        echo_ran.load(Ordering::SeqCst),
        "a tool whose declaration is hidden is still active and executable"
    );
    assert!(
        session.active_tool_names().iter().any(|n| n == "echo_t"),
        "hiding a declaration does not deactivate the tool"
    );
}

/// A `prepare_loadout` description reaches the provider's declaration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_loadout_description_reaches_the_provider() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = script(&requests, vec![Reply::Text("done")]);
    let session = session_with(
        &fx,
        faux,
        vec![
            Probe::new("describer_t", ToolExposure::Direct)
                .hiding("nothing_t", Some(("plain_t", "REWRITTEN")))
                .arc(),
            Probe::new("plain_t", ToolExposure::Direct).arc(),
        ],
    )
    .await;
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;
    let reqs = requests.lock().unwrap().clone();
    let plain = reqs[0].0.iter().find(|t| t.name == "plain_t").unwrap();
    assert_eq!(plain.description, "REWRITTEN");
}

/// CONTROL. A plain `direct` extension tool, with no hook, is declared with its own description:
/// every absence assertion above is only meaningful because this one passes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plain_direct_extension_tool_is_declared_unchanged() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = script(&requests, vec![Reply::Text("done")]);
    let session = session_with(
        &fx,
        faux,
        vec![Probe::new("plain_t", ToolExposure::Direct).arc()],
    )
    .await;
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;
    let reqs = requests.lock().unwrap().clone();
    let plain = reqs[0].0.iter().find(|t| t.name == "plain_t").unwrap();
    assert_eq!(plain.description, "probe tool");
    assert!(reqs[0].1.contains("SNIPPET-plain_t"));
}

/// Registration AFTER build runs through `merge_registered`: a late `direct` tool is activated, a
/// late `deferred` one is registered and callable but not active, and the late `deferred` tool
/// does not reach the next request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_registration_is_activated_only_when_its_exposure_says_so() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = script(&requests, vec![Reply::Text("done")]);
    let session = session_with(&fx, faux, Vec::new()).await;

    let owner = ExtensionId::from("late-ext");
    let host = session.services().ext_host.clone();
    host.register_late_tool(
        owner.clone(),
        Probe::new("late_direct", ToolExposure::Direct).arc(),
    )
    .unwrap();
    host.register_late_tool(
        owner.clone(),
        Probe::new("late_deferred", ToolExposure::Deferred).arc(),
    )
    .unwrap();
    host.register_late_tool(owner, Probe::new("late_hidden", ToolExposure::Hidden).arc())
        .unwrap();
    session.refresh_extension_tools().await;

    let active = session.active_tool_names();
    assert!(active.iter().any(|n| n == "late_direct"), "{active:?}");
    assert!(!active.iter().any(|n| n == "late_deferred"), "{active:?}");
    assert!(!active.iter().any(|n| n == "late_hidden"), "{active:?}");
    let callable = session.callable_tool_names();
    assert!(
        callable.iter().any(|n| n == "late_deferred"),
        "{callable:?}"
    );
    assert!(!callable.iter().any(|n| n == "late_hidden"), "{callable:?}");

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;
    let reqs = requests.lock().unwrap().clone();
    let declared = names(&reqs[0].0);
    assert!(declared.contains(&"late_direct"), "{declared:?}");
    assert!(!declared.contains(&"late_deferred"), "{declared:?}");
    assert!(!declared.contains(&"late_hidden"), "{declared:?}");
}

/// A `hidden` tool whose exposure later changes to `direct` is activated like a new tool (pi
/// `previousActivatedOnRegistration`, `agent-session.ts:3446-3449`): re-registering it with a new
/// exposure must activate it, not leave it dormant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_whose_exposure_changes_to_direct_is_activated_like_a_new_tool() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = script(&requests, vec![Reply::Text("done")]);
    let session = session_with(
        &fx,
        faux,
        vec![Probe::new("flip_t", ToolExposure::Hidden).arc()],
    )
    .await;
    assert!(!session.active_tool_names().iter().any(|n| n == "flip_t"));

    session
        .services()
        .ext_host
        .register_late_tool(
            ExtensionId::from("exposure-ext"),
            Probe::new("flip_t", ToolExposure::Direct).arc(),
        )
        .unwrap();
    session.refresh_extension_tools().await;
    assert!(
        session.active_tool_names().iter().any(|n| n == "flip_t"),
        "{:?}",
        session.active_tool_names()
    );
}

/// The guest-facing `getAllTools` rows carry pi v1.0.1's `exposure` (always) and `namespace` (when
/// set) keys.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_guest_get_all_tools_rows_carry_exposure_and_namespace() {
    use cyrup_ext::HostServices as _;
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = script(&requests, vec![Reply::Text("done")]);
    let (tools, _) = one_of_each();
    let session = session_with(&fx, faux, tools).await;

    let rows = session
        .services()
        .host_services
        .all_tools()
        .expect("a live session answers");
    let row = |n: &str| rows.iter().find(|r| r["name"] == n).unwrap().clone();
    assert_eq!(row("codemode_t")["exposure"], "codemode");
    assert_eq!(row("codemode_t")["namespace"]["name"], "mcp__docs");
    assert_eq!(row("hidden_t")["exposure"], "hidden");
    assert_eq!(row("direct_t")["exposure"], "direct");
    assert!(row("direct_t").get("namespace").is_none());
}

/// A `prepare_loadout` hook that fails is reported on the extension error channel with event
/// `prepare_loadout` (pi `emitError`, `agent-session.ts:1556-1561` @v1.0.1) and changes nothing:
/// the failing tool and its neighbours stay declared.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_loadout_hook_is_reported_and_leaves_the_loadout_intact() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = script(&requests, vec![Reply::Text("done")]);
    let errors: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let session = session_with(
        &fx,
        faux,
        vec![
            Probe::new("bad_t", ToolExposure::Direct).failing().arc(),
            Probe::new("plain_t", ToolExposure::Direct).arc(),
        ],
    )
    .await;
    let sink = Arc::clone(&errors);
    session.services().ext_host.add_error_listener(Arc::new(
        move |e: &cyrup_ext::ExtensionError| {
            sink.lock()
                .unwrap()
                .push((e.event.to_string(), e.error.clone()));
        },
    ));

    // Any re-application of the active set runs the hooks again.
    let names_now = session.active_tool_names();
    session.set_active_tools_by_name(&names_now).await;

    let seen = errors.lock().unwrap().clone();
    assert!(
        seen.iter()
            .any(|(event, msg)| event == "prepare_loadout" && msg.contains("hook exploded")),
        "{seen:?}"
    );
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;
    let reqs = requests.lock().unwrap().clone();
    let declared = names(&reqs[0].0);
    assert!(
        declared.contains(&"bad_t") && declared.contains(&"plain_t"),
        "{declared:?}"
    );
}

/// The prompt REBUILT after the session started (a `setActiveTools`) must also omit the snippet of a
/// tool whose declaration a hook hides, exactly as the one built at start does (pi
/// `_rebuildSystemPrompt`, `agent-session.ts:1651-1678`): the listing has to match the request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rebuilt_prompt_does_not_list_a_hidden_declaration() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = script(&requests, vec![Reply::Text("done")]);
    let session = session_with(
        &fx,
        faux,
        vec![
            Probe::new("hider_t", ToolExposure::Direct)
                .hiding("echo_t", None)
                .arc(),
            Probe::new("echo_t", ToolExposure::Direct).arc(),
        ],
    )
    .await;
    // Deactivate and reactivate so the base prompt is rebuilt from the dynamic-tool state.
    session.set_active_tools_by_name(&strings(&["read"])).await;
    session
        .set_active_tools_by_name(&strings(&["read", "hider_t", "echo_t"]))
        .await;
    let prompt = session.base_system_prompt();
    assert!(prompt.contains("SNIPPET-hider_t"), "{prompt}");
    assert!(!prompt.contains("SNIPPET-echo_t"), "{prompt}");
}
