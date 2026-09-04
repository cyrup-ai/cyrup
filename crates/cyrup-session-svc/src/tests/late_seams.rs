//! SESSION-LEVEL proofs for two "wired but unread" seams on the live session: the scoped-model set
//! an extension reads, and the post-`build()` custom-tool registration path.
//!
//! Both are `pub` surfaces with no in-workspace caller, which is exactly why neither divergence had
//! ever been observed. Each test drives the REAL public entry point and asserts on the consumer side
//! of the seam, never on the setter's own return value.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use cyrup_core::{
    CancelToken, Content, StopReason, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_ext::HostServices as _;
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};

use crate::{AgentSession, ScopedModel, SessionBuilder, SessionConfig};
use tempfile::TempDir;

// ------------------------------------------------------------------------------ fixtures ----

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

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

fn empty_schema() -> serde_json::Value {
    serde_json::json!({ "type": "object", "properties": {} })
}

// -------------------------------------------------------------- ctx.scopedModels (EXT-045) ----

/// The scoped-model set the user configured must reach the EXTENSION seam, not just the
/// `/scoped-models` command.
///
/// pi exposes one value to both: `getScopedModels: () => this._scopedModels`
/// (`core/agent-session.ts:2416`) is bound onto the BASE extension context by
/// `get scopedModels() { runner.assertActive(); return getScopedModels() }`
/// (`core/extensions/runner.ts:706-709`), declared as
/// `scopedModels: readonly ScopedModel[]` (`core/extensions/types.ts:326`, "Same set the
/// `/scoped-models` command shows. Empty when no scoping is configured").
///
/// cyrup's `LiveHostServices` never overrode `scoped_models`, so it answered the trait default
/// `json!([])` forever — which a guest cannot distinguish from upstream's documented "no scoping
/// configured". A model-picking extension therefore selected freely from the full catalogue the
/// ADJACENT, working `models()` returned, silently ignoring a restriction the core was honouring.
///
/// RED before the fix: the post-`set_scoped_models` read returned `[]`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_scoped_models_reaches_the_extension_seam() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap();

    let services = session.services().host_services.clone();

    // Unscoped: upstream's documented empty answer.
    assert_eq!(
        services.scoped_models(),
        serde_json::json!([]),
        "an unscoped session reports pi's empty set"
    );

    // What `main.rs` does after `resolve_scoped_models_reporting`.
    let catalog = session.model_catalog();
    assert!(
        !catalog.is_empty(),
        "the faux provider offers at least one model"
    );
    let first = catalog[0].clone();
    session.set_scoped_models(vec![
        ScopedModel {
            model: first.clone(),
            thinking_level: Some(cyrup_core::ModelThinkingLevel::High),
        },
        ScopedModel {
            model: first.clone(),
            thinking_level: None,
        },
    ]);

    let scoped = services.scoped_models();
    let rows = scoped.as_array().expect("an array");
    assert_eq!(
        rows.len(),
        2,
        "the whole scoped set is visible to extensions: {scoped}"
    );
    assert_eq!(
        rows[0]["model"]["id"].as_str(),
        Some(first.id.as_str()),
        "pi's `ScopedModel.model` payload, not a bare name: {scoped}"
    );
    assert_eq!(
        rows[0]["thinkingLevel"],
        serde_json::json!("high"),
        "pi's optional per-model thinking level survives: {scoped}"
    );
    assert!(
        rows[1].get("thinkingLevel").is_none(),
        "an unset level is OMITTED, matching an `undefined` field upstream: {scoped}"
    );

    // And the seam tracks the authority: replacing the set replaces what the guest reads.
    session.set_scoped_models(Vec::new());
    assert_eq!(
        services.scoped_models(),
        serde_json::json!([]),
        "clearing the scope clears the guest-visible view too"
    );
}

// -------------------------------------------- AgentSession::register_custom_tools (SDK seam) ----

type SessionSlot = Arc<OnceLock<Weak<AgentSession>>>;

/// A custom tool that widens the active set while it runs. It NEVER sets `added_tool_names` — the
/// registered-tool WRAPPER derives that, and only if the tool was actually wrapped.
struct WideningCustomTool {
    slot: SessionSlot,
    params: serde_json::Value,
}

#[async_trait::async_trait]
impl Tool for WideningCustomTool {
    fn name(&self) -> &str {
        "custom_loader"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("ships the build")
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: serde_json::Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let session = self
            .slot
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| ToolError::new("no session"))?;
        let mut names = session.active_tool_names();
        names.push("custom_late".to_string());
        session.set_active_tools_by_name(&names).await;
        Ok(ToolResult {
            content: vec![Content::text("loaded")],
            ..Default::default()
        })
    }
}

/// The tool `custom_loader` activates — registered through the same seam, left inactive.
struct LateCustomTool {
    ran: Arc<AtomicUsize>,
    params: serde_json::Value,
}

#[async_trait::async_trait]
impl Tool for LateCustomTool {
    fn name(&self) -> &str {
        "custom_late"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: serde_json::Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.ran.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            content: vec![Content::text("late-ran")],
            ..Default::default()
        })
    }
}

/// The tool names offered to the model, one entry per provider request.
type OfferedTools = Arc<Mutex<Vec<Vec<String>>>>;

#[derive(Clone)]
enum Reply {
    Call(&'static str),
    Text(&'static str),
}

fn faux_script(offered: &OfferedTools, script: Vec<Reply>) -> Arc<FauxProvider> {
    let steps: Vec<FauxResponseStep> = script
        .into_iter()
        .map(|reply| {
            let cap = offered.clone();
            FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                cap.lock()
                    .unwrap()
                    .push(ctx.tools.iter().map(|t| t.name.clone()).collect());
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

/// `AgentSession::register_custom_tools` must be the BUILD-TIME custom-tool path, which is the
/// parity it claims — wrapped, and contributing to the system prompt.
///
/// The build-time route is
/// `registry_tools.extend(cfg.custom_tools.iter().map(|t| ext_host.wrap_tool(t.clone())))`
/// (`builder.rs`), whose own comment says the wrap exists so "a custom tool that widens the active
/// set also derives `addedToolNames`", and it folds every custom tool into the rebuilder's
/// contribution map. pi has no post-construction registration at all — `customTools` is
/// constructor-only (`core/agent-session.ts:383`) and is wrapped together with everything else in
/// one `wrapRegisteredTools` pass (`:2513`, over the `allCustomTools` list at `:2472-2478`) — so
/// there is no upstream shape in which an SDK-supplied tool runs unwrapped or guideline-less.
///
/// RED before the fix on BOTH assertions: `register_custom_tools` inserted the raw `Arc<dyn Tool>`
/// (no wrapper ⇒ no anchor was ever derived) and `DynamicToolState::register_custom` was a bare
/// `registry.insert` loop (no contribution ⇒ `PromptRebuilder::rebuild`'s
/// `filter_map(|n| self.contributions.get(n))` silently dropped the key and the model got the tool
/// schema with none of its guidance).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn register_custom_tools_wraps_and_contributes_like_the_build_time_path() {
    let fx = fixture();
    let offered: OfferedTools = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_script(
        &offered,
        vec![
            Reply::Call("custom_loader"),
            Reply::Call("custom_late"),
            Reply::Text("done"),
        ],
    );
    let slot: SessionSlot = Arc::new(OnceLock::new());
    let late_ran = Arc::new(AtomicUsize::new(0));

    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap()
        .into_shared();
    let _ = slot.set(Arc::downgrade(&session));

    // THE SEAM UNDER TEST: registration AFTER `build()`, the only reason this method is `pub`.
    session.register_custom_tools(vec![
        Arc::new(WideningCustomTool {
            slot: slot.clone(),
            params: empty_schema(),
        }),
        Arc::new(LateCustomTool {
            ran: late_ran.clone(),
            params: empty_schema(),
        }),
    ]);

    // Custom tools register INERT (pi's build-time `customTools` are activated by selection), so
    // activate the loader explicitly — leaving `custom_late` registered-but-inactive.
    let mut names: Vec<String> = session
        .active_tool_names()
        .into_iter()
        .filter(|n| n != "custom_late")
        .collect();
    names.push("custom_loader".to_string());
    session.set_active_tools_by_name(&names).await;
    let active = session.active_tool_names();
    assert!(
        active.iter().any(|n| n == "custom_loader"),
        "the custom tool is active: {active:?}"
    );
    assert!(
        !active.iter().any(|n| n == "custom_late"),
        "`custom_late` is inactive: {active:?}"
    );

    // (1) PROMPT CONTRIBUTION — the rebuilt base prompt carries the custom tool's snippet. Without
    //     the contribution upsert the name is silently dropped from `tool_contributions`.
    let prompt = session.base_system_prompt();
    assert!(
        prompt.contains("custom_loader: ships the build"),
        "the custom tool's prompt guidance reaches the model: {prompt}"
    );

    // (2) WRAPPER — run for real and read the anchor off the finalized transcript.
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    let anchors: Vec<(String, Vec<String>)> = session
        .agent_messages()
        .await
        .into_iter()
        .filter_map(|m| match m {
            cyrup_agent::AgentMessage::ToolResult(t) if !t.added_tool_names.is_empty() => {
                Some((t.tool_name, t.added_tool_names))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        anchors,
        vec![("custom_loader".to_string(), vec!["custom_late".to_string()])],
        "a tool registered through this seam is wrapped, so the host derives its anchor — exactly \
         as the build-time custom-tool path does"
    );

    // …and the derivation is not cosmetic: the newly-activated tool really was callable.
    let turns = offered.lock().unwrap().clone();
    assert!(
        turns.len() >= 2,
        "the run drove at least two turns: {turns:?}"
    );
    assert!(
        !turns[0].iter().any(|t| t == "custom_late"),
        "turn 1 did not offer `custom_late`: {:?}",
        turns[0]
    );
    assert!(
        turns[1].iter().any(|t| t == "custom_late"),
        "turn 2 offered `custom_late`: {:?}",
        turns[1]
    );
    assert_eq!(
        late_ran.load(Ordering::SeqCst),
        1,
        "`custom_late` actually executed"
    );
}

// ------------------------------------------- slash-command outcome surfacing (both tiers) ----

/// A slash command's OWN outcome must reach the user, and the NATIVE and WASM tiers must agree.
///
/// pi keeps native and wasm commands in ONE map and runs both through a single
/// `_tryExecuteExtensionCommand` (`core/agent-session.ts:1277-1301` @v0.83.0), so upstream there is
/// exactly one behaviour: a thrown handler goes to
/// `this._extensionRunner.emitError({extensionPath: `command:${commandName}`, event: "command",
/// error})` (`:1294-1299`) and the command still reports handled.
///
/// cyrup's two tiers had DIVERGED. The native arm surfaced both channels (with a long comment
/// citing that same `emitError`); the wasm arm was `let _ = self.services.ext_host.run_command(…)`,
/// throwing away the whole `Result<Option<String>, ExtError>` — so a trapping guest, an
/// epoch-deadline interrupt, an `ExtError::Cancelled`, or the guest's own `execute-command` error
/// return produced absolutely nothing (no transcript line, no toast, no RPC `extension_error`, no
/// log) while `try_execute_wasm_command` still returned `true`, swallowing the input.
///
/// `surface_command_outcome` is the one implementation both arms now call, so this pins the shared
/// contract. SCOPE NOTE: the end-to-end guest-fault run — a real wasm component whose
/// `execute-command` traps, driven through `prompt("/name")` — needs the compiled fixture that lives
/// in `crates/cyrup-it/tests/session_svc/wasm_slash_command.rs`, which this crate cannot build; the
/// native end-to-end equivalent is `crate::tests::native_slash_command_output`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_handlers_outcome_reaches_the_ui_channel_on_every_shape() {
    use crate::{NotifyKind, UiEffect};

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
    session.services().host_services.set_ui_effect_sink(tx);

    let drain = |rx: &mut tokio::sync::mpsc::UnboundedReceiver<UiEffect>| {
        let mut out = Vec::new();
        while let Ok(effect) = rx.try_recv() {
            if let UiEffect::Notify { message, kind } = effect {
                out.push((message, kind));
            }
        }
        out
    };

    // (1) Output → an Info toast, verbatim.
    session.surface_command_outcome("deploy", &Ok(Some("DEPLOYED 3 services".to_string())));
    assert_eq!(
        drain(&mut rx),
        vec![("DEPLOYED 3 services".to_string(), NotifyKind::Info)],
        "a command that answers with text speaks, on either tier"
    );

    // (2) A handler that deliberately says nothing stays silent — including whitespace-only output.
    session.surface_command_outcome("quiet", &Ok(None));
    session.surface_command_outcome("quiet", &Ok(Some("  \n\t ".to_string())));
    assert!(
        drain(&mut rx).is_empty(),
        "silence is preserved, not turned into an empty toast"
    );

    // (3) A FAULTED handler surfaces as pi's `command:<name>` error, rather than vanishing.
    session.surface_command_outcome(
        "deploy",
        &Err(cyrup_ext::ExtError::Component("guest trapped".to_string())),
    );
    let seen = drain(&mut rx);
    assert_eq!(
        seen.len(),
        1,
        "a fault produces exactly one notification: {seen:?}"
    );
    assert_eq!(seen[0].1, NotifyKind::Error);
    assert!(
        seen[0].0.starts_with("command:deploy: "),
        "pi's `extensionPath: command:<name>` prefix is preserved: {seen:?}"
    );
    assert!(
        seen[0].0.contains("guest trapped"),
        "…and carries the real cause: {seen:?}"
    );
}
