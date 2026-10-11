//! CODE-015, CODE-017 and the structured result of a guest tool — through a REAL session.
//!
//! The `cyrup-ext-sdk` demo component registers three tools for this surface: `structured_demo`
//! (declares an `outputSchema` and annotations, and returns `structuredContent`), `failing_demo`
//! (reports a failure without throwing) and `loadout_demo` (supplies a `prepareLoadout` hook).
//! Each test drives the component through `registration.register-tool`, the session's tool
//! registry, the provider request and a codemode script, so the WIT record, the host lift and the
//! `Tool` accessors are crossed, not hand-built.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_codemode_runtime::CodemodeExtension;
use cyrup_codemode_runtime::testkit::{ScriptEnv, ScriptedSandboxFactory, completed};
use cyrup_codemode_runtime::types::CodemodeResult;
use cyrup_core::{ExtensionId, Message, NestedCallStatus, StopReason};
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use cyrup_provider::{Provider, ToolDef};
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use futures::FutureExt as _;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::support::bins;

type Script = cyrup_codemode_runtime::testkit::Script;
type Requests = Arc<Mutex<Vec<Vec<ToolDef>>>>;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(String, ScriptEnv) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = CodemodeResult> + Send + 'static,
{
    Arc::new(move |code, env| f(code, env).boxed())
}

struct Rig {
    session: Arc<AgentSession>,
    faux: Arc<FauxProvider>,
    requests: Requests,
    _tmp: TempDir,
}

/// A session with the codemode extension (its sandbox is `script`) and the demo guest component.
async fn rig(script: Script) -> Rig {
    let tmp = TempDir::new().unwrap();
    let cwd: PathBuf = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let faux = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    // `codemode` is a built-in of the path tier `--no-extensions` collapses.
    cfg.no_extensions = false;
    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, cfg)
        .with_codemode(CodemodeExtension::new(
            Default::default(),
            Arc::new(ScriptedSandboxFactory::new(script)),
        ))
        .build()
        .await
        .unwrap()
        .into_shared();
    session
        .load_wasm_extension(
            ExtensionId::from("demo"),
            &bins::component_bytes(),
            &cyrup_ext::Capabilities::host_granted(),
        )
        .await
        .expect("load the demo guest");
    session.bind_extensions().await;
    Rig {
        session,
        faux,
        requests: Arc::default(),
        _tmp: tmp,
    }
}

impl Rig {
    async fn set_active(&self, names: &[&str]) {
        let names: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
        self.session.set_active_tools_by_name(&names).await;
    }

    /// One prompt. When `code` is given the model's reply calls `codemode` with it first; either
    /// way the provider records the tools of every request it receives.
    async fn prompt(&self, code: Option<&str>) {
        let replies = [code.map(|c| json!({ "code": c })), None];
        let steps = replies
            .into_iter()
            .map(|reply| {
                let seen = Arc::clone(&self.requests);
                FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                    seen.lock().unwrap().push(ctx.tools.clone());
                    match &reply {
                        Some(args) => faux_assistant_message(
                            vec![faux_tool_call("codemode".to_string(), args.clone())],
                            StopReason::ToolUse,
                        ),
                        None => faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
                    }
                })
            })
            .collect();
        self.faux.set_response_steps(steps);
        let _ = self.session.prompt("go").await.unwrap();
        self.session.wait_for_idle().await;
    }

    async fn codemode_result(&self) -> Message {
        self.session
            .messages()
            .await
            .into_iter()
            .rev()
            .find(|m| matches!(m, Message::ToolResult { tool_name, .. } if tool_name == "codemode"))
            .expect("a codemode tool result")
    }

    fn first_request_tools(&self) -> Vec<ToolDef> {
        self.requests.lock().unwrap().first().cloned().unwrap()
    }
}

/// The script's output text: the content after the `Script completed …` header.
fn script_output(message: &Message) -> String {
    let Message::ToolResult { content, .. } = message else {
        panic!("not a tool result");
    };
    content[1..]
        .iter()
        .filter_map(|block| match block {
            cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A guest tool that declares an `outputSchema` resolves, in a codemode script, to the
/// `structuredContent` it returned, not to its text (pi `toScriptValue`, `execute.ts:303-312`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_script_receives_the_structured_content_of_a_guest_tool_with_an_output_schema() {
    let rig = rig(script(|_code, env| async move {
        let value = env.tool("structured_demo", json!({ "n": 3 })).await;
        completed(Vec::new(), value.unwrap())
    }))
    .await;
    rig.set_active(&["codemode", "structured_demo"]).await;

    rig.prompt(Some("return await tools.structured_demo({ n: 3 });"))
        .await;

    let output = script_output(&rig.codemode_result().await);
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap_or(Value::String(output.clone())),
        json!({ "doubled": 6, "tags": ["a", "b"] }),
        "the script must get the structured value, not the tool's text"
    );
}

/// A guest that reports a failure without throwing (`ToolOutput::error`) is a failed nested call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_tool_that_reports_a_failure_is_a_failed_nested_call() {
    let rig = rig(script(|_code, env| async move {
        let value = env.tool("failing_demo", json!({})).await;
        completed(Vec::new(), value.unwrap())
    }))
    .await;
    rig.set_active(&["codemode", "failing_demo"]).await;

    rig.prompt(Some("return await tools.failing_demo({});"))
        .await;

    let Message::ToolResult { nested_calls, .. } = rig.codemode_result().await else {
        panic!("not a tool result");
    };
    let calls = nested_calls.expect("the nested calls are recorded").calls;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "failing_demo");
    assert_eq!(
        calls[0].status,
        NestedCallStatus::Error,
        "the guest's isError reached the nested call's verdict"
    );
}

/// `getAllTools` reports a guest tool's annotations (pi `ToolInfo.annotations`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_tools_annotations_reach_the_session_tool_info() {
    let rig = rig(script(|_code, _env| async { completed(Vec::new(), None) })).await;

    let tools = rig.session.all_tools();
    let info = tools
        .iter()
        .find(|t| t.name == "structured_demo")
        .expect("the guest tool is registered");
    let annotations = info.annotations.expect("annotations are reported");
    assert_eq!(annotations.read_only_hint, Some(true));
    assert_eq!(annotations.idempotent_hint, Some(true));
    assert_eq!(annotations.open_world_hint, Some(false));
    assert_eq!(
        annotations.destructive_hint, None,
        "an unset hint stays unset"
    );
    let plain = tools.iter().find(|t| t.name == "demo_echo").unwrap();
    assert!(plain.annotations.is_none(), "a tool with none reports none");
}

/// CODE-015: a guest tool's `prepareLoadout` hook runs when the active tools are applied. The demo's
/// hook lists the callable tools in its own description and hides `signal_probe`'s declaration
/// while that tool is declared.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_tools_prepare_loadout_hides_a_declaration_and_rewrites_a_description() {
    let rig = rig(script(|_code, _env| async { completed(Vec::new(), None) })).await;
    rig.set_active(&["loadout_demo", "signal_probe", "demo_echo"])
        .await;

    rig.prompt(None).await;

    let sent = rig.first_request_tools();
    let names: Vec<&str> = sent.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"loadout_demo"), "{names:?}");
    assert!(names.contains(&"demo_echo"), "{names:?}");
    assert!(
        !names.contains(&"signal_probe"),
        "the guest's hook hid the declaration: {names:?}"
    );
    let description = sent
        .iter()
        .find(|t| t.name == "loadout_demo")
        .unwrap()
        .description
        .clone();
    assert!(
        description.starts_with("Orchestrates other tools. Callable: ")
            && description.contains("signal_probe")
            && description.contains("demo_echo"),
        "the guest's hook rewrote the description from the loadout it was handed: {description}"
    );
    // Hidden from requests only: the tool is still active and callable.
    assert!(
        rig.session
            .active_tool_names()
            .contains(&"signal_probe".to_string())
    );
}

/// The other direction: the hook shows a declaration again once nothing makes it hide, here
/// because the tool that hid it is no longer active.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_declaration_is_shown_again_when_the_hook_that_hid_it_is_not_active() {
    let rig = rig(script(|_code, _env| async { completed(Vec::new(), None) })).await;
    rig.set_active(&["loadout_demo", "signal_probe"]).await;
    rig.set_active(&["signal_probe", "demo_echo"]).await;

    rig.prompt(None).await;

    let names: Vec<String> = rig
        .first_request_tools()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert!(names.contains(&"signal_probe".to_string()), "{names:?}");
}
