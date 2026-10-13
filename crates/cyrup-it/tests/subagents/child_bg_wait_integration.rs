//! SUBA-232 / SUBA-231 end to end: a subagent CHILD's own `bg_wait`, on a real child session.
//!
//! Each session here is a real `SessionBuilder`-assembled `AgentSession`, shared exactly as the
//! production runtime shares it (`into_shared`), with the child-side extensions a real spawn loads:
//! the prompt runtime built by the production `prompt_runtime_from_env` from the variables the
//! parent writes (`CYRUP_SUBAGENT_CHILD=1` among them), and — for the fan-out child — the
//! `ChildSafe` subagents extension ahead of it, in launch order. The model's turns are scripted.
//!
//! - **The grandchild.** A fan-out child launches `worker` with `async: true` through its own
//!   restricted `subagent` tool, then calls `bg_wait`. The launch is the production one (hop 1 is a
//!   sleeping stand-in, as `child_tool_plan_not_predicted_from_parent` does it); hop 2 runs
//!   in-process through the production `run_with` with a scripted grandchild. The wait must see the
//!   run its own session launched — stamped with the child's session id, under the child's cwd —
//!   and hand back the grandchild's result inside the same turn.
//! - **The parent's steer.** A plain child blocks in `bg_wait` on a running run; the PARENT then
//!   steers it through the real steering inbox (`request_async_steer` → the runner's routing hop
//!   onto the child's `steer-targets/0` → the child's inbox poller → `send_user_message` → the
//!   host's `input` dispatch → the runtime's `Input` arm), and the wait ends with the non-error
//!   `user_input` yield.
//!
//! Each step waits for the state it needs. The one margin is the grandchild's own 2 s run, which
//! the child's `bg_wait` — held until the run is listable — must find still in progress.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cyrup_core::StopReason;
use cyrup_ext::NativeExtension;
use cyrup_ext_subagents::background::runner_main::{RunnerConfig, RunnerOverrides, run_with};
use cyrup_ext_subagents::background::{
    ResultFile, RunId, RunMode, RunPaths, RunState, RunStatus, run_artifact_roots_in,
};
use cyrup_ext_subagents::exec::tool_availability::{
    CHILD_TOOL_DIAGNOSTIC_PATH_ENV, tool_diagnostic_path_in,
};
use cyrup_ext_subagents::extension::{RegistrationMode, SubagentsExtension};
use cyrup_ext_subagents::identity::SessionId;
use cyrup_ext_subagents::native_supervisor::ENV_REQUIRED_CHILD_TOOLS;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::prompt_runtime::{
    INHERIT_PROJECT_CONTEXT_ENV, STEER_INBOX_ENV, SubagentPromptRuntime, prompt_runtime_from_env,
};
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::spawn::nested_events::{CHILD_ENV, FANOUT_CHILD_ENV};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use cyrup_session_svc::{AgentSession, AgentSessionEvent, SessionBuilder, SessionConfig};
use futures::StreamExt;

/// The wait's `user_input` yield text (pi `subagent-wait.ts:413` @ad11b7ab).
const USER_INPUT_YIELD_TEXT: &str = "Wait yielded because the user sent a message. Background \
                                     work remains active; read the message before waiting again.";

struct Project {
    _tmp: tempfile::TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
    home: PathBuf,
}

/// A child's project: a cwd with a `worker` persona, an empty agent dir and a sandboxed home.
fn project() -> Project {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    let home = tmp.path().join("home");
    for dir in [&cwd, &agent_dir, &home] {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    let agents = cwd.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents).expect("mkdir .cyrup/agents");
    std::fs::write(
        agents.join("worker.md"),
        "---\nname: worker\ndescription: SUBA-232 fixture persona\nmodel: fixture/model\ntools: \
         read\n---\n\nYou are the worker.\n",
    )
    .expect("write persona");
    Project {
        _tmp: tmp,
        cwd,
        agent_dir,
        home,
    }
}

fn write_script(dir: &Path, name: &str, script: &serde_json::Value) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, script.to_string()).expect("write fixture script");
    path
}

fn fixture_cmd(script: &Path) -> SpawnCommand {
    SpawnCommand {
        binary: crate::support::bins::subagent_fixture(),
        base_args: vec!["--fixture-script".to_string(), script.display().to_string()],
    }
}

fn message_end(text: &str) -> String {
    serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {
                "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
            },
            "stopReason": "stop"
        }
    })
    .to_string()
}

/// The variables a parent writes for this child, plus the sandboxed home so the child's own roots
/// are the ones the test's runs live under (`CYRUP_HOME` roots the run scratch exactly as
/// `Roots::sandboxed` does).
fn child_env(p: &Project, fanout: bool, extra: &[(&str, String)]) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = [
        (CHILD_ENV, "1".to_string()),
        (FANOUT_CHILD_ENV, if fanout { "1" } else { "0" }.to_string()),
        (INHERIT_PROJECT_CONTEXT_ENV, "1".to_string()),
        (cyrup_config::paths::ENV_HOME, p.home.display().to_string()),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    for (key, value) in extra {
        env.insert((*key).to_string(), value.clone());
    }
    env
}

fn child_runtime(env: &HashMap<String, String>) -> Arc<SubagentPromptRuntime> {
    Arc::new(
        prompt_runtime_from_env(&|key| env.get(key).cloned())
            .expect("no tool budget, so the runtime builds")
            .expect("a child env builds the runtime"),
    )
}

/// A real, shared child session over `extensions`, in the order given.
async fn child_session(
    p: &Project,
    faux: Arc<FauxProvider>,
    extensions: Vec<Arc<dyn NativeExtension>>,
) -> Arc<AgentSession> {
    let mut cfg = SessionConfig::new(p.cwd.clone(), p.agent_dir.clone());
    cfg.trust_override = Some(true);
    let mut builder = SessionBuilder::new(faux as Arc<dyn Provider>, cfg);
    for ext in extensions {
        builder = builder.with_native_extension(ext);
    }
    builder
        .build()
        .await
        .expect("a real child session builds")
        .into_shared()
}

fn scripted(steps: Vec<FauxResponseStep>) -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(steps);
    faux
}

fn tool_step(name: &str, args: serde_json::Value) -> FauxResponseStep {
    FauxResponseStep::from(faux_assistant_message(
        vec![faux_tool_call(name, args)],
        StopReason::ToolUse,
    ))
}

fn text_step(text: &str) -> FauxResponseStep {
    FauxResponseStep::from(faux_assistant_message(
        vec![faux_text(text)],
        StopReason::Stop,
    ))
}

fn tool_end(events: &[AgentSessionEvent], name: &str) -> (bool, serde_json::Value) {
    events
        .iter()
        .find_map(|event| match event {
            AgentSessionEvent::ToolExecutionEnd {
                tool_name,
                result,
                is_error,
                ..
            } if tool_name == name => Some((*is_error, result.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {name} result: {events:#?}"))
}

/// Find the one `runner-config.json` an async launch wrote under `root`.
fn find_runner_config(root: &Path) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == "runner-config.json") {
                return Some(path);
            }
        }
    }
    None
}

/// Hop 2's task, once [`bg_wait_after_hop2`] has started it.
type Hop2 = Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<RunnerConfig>>>>;

/// The child's SECOND model turn: it runs hop 2 — the grandchild — and only then calls `bg_wait`.
///
/// The turn is a model call, so it begins strictly after the `subagent` launch returned and its
/// `runner-config.json` is on disk. It starts hop 2 through the production `run_with`, waits until
/// hop 2 has published the run's `status.json` (a launch writes only the runner config, so the run
/// is not listable before that, and the wait's one up-front listing would find nothing), and then
/// answers with the `bg_wait` call. Nothing here depends on when the test's own event loop sees
/// the launch's `ToolExecutionEnd`.
fn bg_wait_after_hop2(home: PathBuf, grandchild: PathBuf, hop2: Hop2) -> FauxResponseStep {
    FauxResponseStep::async_factory(move |_context, _options, _state, _model| {
        let (home, grandchild, hop2) = (home.clone(), grandchild.clone(), hop2.clone());
        async move {
            let cfg_path = find_runner_config(&home)
                .expect("the async launch wrote runner-config.json under the sandboxed home");
            let raw = std::fs::read(&cfg_path).expect("read runner-config.json");
            let config: RunnerConfig = serde_json::from_slice(&raw).expect("a RunnerConfig");
            let run_paths =
                RunPaths::for_run(&config.async_root, &config.results_dir, &config.run_id);
            let status = run_paths.status.clone();
            let task = {
                let (home, config) = (home.clone(), config.clone());
                tokio::spawn(async move {
                    run_with(
                        &cfg_path,
                        &run_paths,
                        RunnerOverrides {
                            spawn_command: Some(fixture_cmd(&grandchild)),
                            roots: Some(Roots::sandboxed(&home)),
                            ..Default::default()
                        },
                    )
                    .await
                    .expect("run_with never returns Err");
                    config
                })
            };
            *hop2.lock().expect("hop 2 slot") = Some(task);
            tokio::time::timeout(Duration::from_secs(30), async {
                while !status.exists() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("hop 2 never published the grandchild's status.json");
            faux_assistant_message(
                vec![faux_tool_call("bg_wait", serde_json::json!({}))],
                StopReason::ToolUse,
            )
        }
    })
}

/// SUBA-232 — a fan-out child launches an async grandchild and COLLECTS its result with its own
/// `bg_wait`, inside the same turn (pi `subagent-prompt-runtime.ts:520` @ad11b7ab registers the
/// wait in every child, with `wait-tool.ts:19`'s "collect your owned descendants" description).
///
/// The child is the one upstream advises (`wait-tool.ts:29` @ad11b7ab, "keep `bg_wait` in the
/// child tool allowlist"): its agent declares `tools: [subagent, bg_wait]`, so the parent arms the
/// `agent_start` tool check (SUBA-045) with both names required.
///
/// **Proven red at HEAD** (before SUBA-232): the child has no `bg_wait`, so that check refuses the
/// run at `agent_start` — the turn never launches anything, and the grandchild's result never
/// reaches it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fan_out_child_collects_its_async_grandchilds_result_with_bg_wait() {
    let p = project();
    let sleeper = write_script(
        &p.cwd,
        "hop1-sleeper.json",
        &serde_json::json!({"steps": [{"kind": "sleep_ms", "ms": 20000}], "exit_code": 0}),
    );
    let grandchild = write_script(
        &p.cwd,
        "grandchild.json",
        &serde_json::json!({
            // Running for 2 s first: the child's `bg_wait` is held until this run is listable
            // (`bg_wait_after_hop2`) and must then find it still active, not already gone.
            "steps": [
                {"kind": "sleep_ms", "ms": 2000},
                {"kind": "emit", "line": message_end("grandchild finding: 42")}
            ],
            "exit_code": 0
        }),
    );
    let subagents = Arc::new(SubagentsExtension::with_mode(
        SubagentExtensionConfig {
            async_by_default: false,
            spawn_command: Some(fixture_cmd(&sleeper)),
            roots: Roots::sandboxed(&p.home),
            ..SubagentExtensionConfig::default()
        },
        p.cwd.clone(),
        RegistrationMode::ChildSafe,
    ));
    let diagnostic = tool_diagnostic_path_in(&p.home);
    let runtime = child_runtime(&child_env(
        &p,
        true,
        &[
            (
                CHILD_TOOL_DIAGNOSTIC_PATH_ENV,
                diagnostic.display().to_string(),
            ),
            (
                ENV_REQUIRED_CHILD_TOOLS,
                r#"["subagent","bg_wait"]"#.to_string(),
            ),
        ],
    ));
    let hop2: Hop2 = Arc::default();
    let faux = scripted(vec![
        tool_step(
            "subagent",
            serde_json::json!({"agent": "worker", "task": "find it", "async": true}),
        ),
        bg_wait_after_hop2(p.home.clone(), grandchild.clone(), hop2.clone()),
        text_step("collected"),
    ]);
    let session = child_session(
        &p,
        faux,
        vec![
            subagents as Arc<dyn NativeExtension>,
            runtime.clone() as Arc<dyn NativeExtension>,
        ],
    )
    .await;
    let child_session_id = session.session_id().as_str().to_string();

    let mut stream = session
        .prompt("launch the worker and collect it".to_string())
        .await
        .expect("the prompt is accepted");
    let mut events = Vec::new();
    let drive = async {
        while let Some(event) = stream.next().await {
            events.push(event);
        }
    };
    tokio::time::timeout(Duration::from_secs(60), drive)
        .await
        .expect("the child's turn must finish: bg_wait collects the grandchild, it does not hang");
    session.wait_for_idle().await;

    assert!(
        !diagnostic.exists(),
        "a child that declares bg_wait has one: the agent_start tool check must not refuse it"
    );
    let (launch_error, launch) = tool_end(&events, "subagent");
    assert!(
        !launch_error,
        "the async launch must be accepted: {launch:#}"
    );
    let task = hop2
        .lock()
        .expect("hop 2 slot")
        .take()
        .expect("the child's second turn ran hop 2");
    let config = task.await.expect("hop 2 task");
    let status: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            RunPaths::for_run(&config.async_root, &config.results_dir, &config.run_id).status,
        )
        .expect("the grandchild's status.json"),
    )
    .expect("status is JSON");
    assert_eq!(
        status["sessionId"].as_str(),
        Some(child_session_id.as_str()),
        "the grandchild is the CHILD's run — stamped with the child's own session id"
    );

    let (wait_error, waited) = tool_end(&events, "bg_wait");
    assert!(!wait_error, "a collected run is not an error: {waited:#}");
    let text = waited.to_string();
    assert!(
        text.contains(config.run_id.as_str()),
        "the wait names the grandchild it collected: {waited:#}"
    );
    assert!(
        text.contains("grandchild finding: 42"),
        "the grandchild's result reaches the child inside its turn: {waited:#}"
    );
    // Let hop 2's own terminal writes settle before the tempdir goes.
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// A `running` run owned by `session` — the on-disk shape the child's wait lists.
fn seed_running(async_root: &Path, results_dir: &Path, run_id: &RunId, session: &str) {
    let paths = RunPaths::for_run(async_root, results_dir, run_id);
    std::fs::create_dir_all(&paths.run_dir).expect("run dir");
    let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(std::process::id()));
    status.state = RunState::Running;
    status.session_id = SessionId::parse_opt(Some(session));
    std::fs::write(&paths.status, serde_json::to_string(&status).unwrap()).expect("status.json");
}

/// The run's terminal `ResultFile`, so nothing is left running behind the test.
fn settle(async_root: &Path, results_dir: &Path, run_id: &RunId) {
    let paths = RunPaths::for_run(async_root, results_dir, run_id);
    if let Some(parent) = paths.legacy_result_root.parent() {
        std::fs::create_dir_all(parent).expect("results dir");
    }
    let result = ResultFile {
        schedule_origin: None,
        id: run_id.clone(),
        run_id: run_id.clone(),
        agent: "worker".to_string(),
        mode: RunMode::Single,
        state: RunState::Complete,
        success: true,
        cwd: PathBuf::from("/tmp"),
        session_file: None,
        session_id: None,
        completion_owner_id: None,
        results: Vec::new(),
        workflow_children: None,
        workflow_receipt: None,
    };
    std::fs::write(
        &paths.legacy_result_root,
        serde_json::to_string(&result).unwrap(),
    )
    .expect("result file");
}

/// SUBA-232 (the input half) — a child blocked in its own `bg_wait` is released by its PARENT's
/// steer, as upstream's child wait is (`wait-tool.ts:31-35` @ad11b7ab; the parent's steer reaches a
/// pi child as `interactive`, `agent-session.ts:2211-2232` @f1b2e77f5), with the non-error
/// `user_input` yield naming the run still active.
///
/// The steer travels the production path: the parent's `request_async_steer` writes the request,
/// the runner's hop moves it onto the child's `steer-targets/0` with its target pinned (driven here
/// through `consume_steer_requests` + `enqueue_step_steer`, the pair `route_steer_requests` calls;
/// that function's run- and step-state gating is tested in `runner_main/control_watcher.rs`), the
/// child runtime's steering inbox polls it and delivers it with `send_user_message(…,
/// Some(Steer))`, and the host emits it as `input` while the child streams.
///
/// **Proven red at HEAD** (before SUBA-232): the child has no `bg_wait` to block in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parents_steer_through_the_inbox_ends_the_childs_open_bg_wait() {
    let p = project();
    let parent_run_dir = p.home.join("parent-run");
    // The child's own inbox, `steer-targets/0`, as the runner hands it (`steer_inbox_for`).
    let inbox = cyrup_ext_subagents::background::control::step_steer_inbox_dir(&parent_run_dir, 0);
    std::fs::create_dir_all(&inbox).expect("mkdir inbox");
    let runtime = child_runtime(&child_env(
        &p,
        false,
        &[(STEER_INBOX_ENV, inbox.display().to_string())],
    ));
    let faux = scripted(vec![
        tool_step("bg_wait", serde_json::json!({})),
        text_step("read the steer; the run keeps going"),
        text_step("done"),
    ]);
    let session = child_session(&p, faux, vec![runtime.clone() as Arc<dyn NativeExtension>]).await;
    let child_session_id = session.session_id().as_str().to_string();
    let roots = run_artifact_roots_in(&Roots::sandboxed(&p.home), &p.cwd);
    let run_id = RunId::from_token("childwait00001");
    seed_running(
        &roots.async_root,
        &roots.results_dir,
        &run_id,
        &child_session_id,
    );

    let waits = runtime
        .child_user_input_waits()
        .expect("a real child carries its wait's user-input set")
        .clone();
    let mut stream = session
        .prompt("wait for the run".to_string())
        .await
        .expect("the prompt is accepted");
    let mut events = Vec::new();
    let mut steered = false;
    let mut settled = false;
    let drive = async {
        while let Some(event) = stream.next().await {
            if !steered
                && matches!(
                    &event,
                    AgentSessionEvent::ToolExecutionStart { tool_name, .. } if tool_name == "bg_wait"
                )
            {
                tokio::time::timeout(Duration::from_secs(10), async {
                    while waits.open_count() == 0 {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .expect(
                    "the child's bg_wait never held a wait open: is the seeded run where it looks?",
                );
                cyrup_ext_subagents::background::control::request_async_steer(
                    &parent_run_dir,
                    "also check the docs while it runs",
                    None,
                    Some("parent"),
                )
                .await
                .expect("the parent writes the steer");
                // The runner's hop, by its public pair (what `route_steer_requests` does): consume
                // the run-level queue and pin each request to step 0's inbox.
                for request in cyrup_ext_subagents::background::control::consume_steer_requests(
                    &parent_run_dir,
                )
                .await
                {
                    cyrup_ext_subagents::background::control::enqueue_step_steer(
                        &parent_run_dir,
                        0,
                        &request,
                    )
                    .await
                    .expect("the runner routes the steer to the child's inbox");
                }
                steered = true;
            }
            if !settled
                && matches!(
                    &event,
                    AgentSessionEvent::ToolExecutionEnd { tool_name, .. } if tool_name == "bg_wait"
                )
            {
                settle(&roots.async_root, &roots.results_dir, &run_id);
                settled = true;
            }
            events.push(event);
        }
    };
    tokio::time::timeout(Duration::from_secs(60), drive)
        .await
        .expect("the parent's steer must end the child's wait; it may not sit out the window");
    session.wait_for_idle().await;

    assert!(steered, "the child's bg_wait never started: {events:#?}");
    let (is_error, result) = tool_end(&events, "bg_wait");
    assert!(!is_error, "a user_input yield is not an error: {result:#}");
    assert_eq!(
        result["details"]["wait"],
        serde_json::json!({
            "reason": "user_input",
            "timedOut": false,
            "activeRunIds": [run_id.as_str()],
            "activeProviderItems": [],
        }),
        "{result:#}"
    );
    assert!(
        result.to_string().contains(USER_INPUT_YIELD_TEXT),
        "{result:#}"
    );
    assert_eq!(waits.open_count(), 0, "the call closed its entry on return");
}
