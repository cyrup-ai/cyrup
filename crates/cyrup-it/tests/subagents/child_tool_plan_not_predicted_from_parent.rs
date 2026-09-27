//! SUBA-114 — a child's tools are NOT predicted from the launching session (pi `b12496b8`, #2289,
//! v0.70.0), proven end to end through a REAL parent `AgentSession` started as a narrow
//! dispatcher.
//!
//! The parent here is a narrow dispatcher on BOTH of cyrup's narrowing axes: the CLI's
//! `--tools subagent,read` (`SessionConfig::tools`, which bounds the ACTIVE set) and the SDK's
//! `allowedToolNames` (`SessionConfig::tool_availability`, which bounds the tool REGISTRY itself —
//! the `_toolRegistry` `HostServices::all_tools` reads). Before SUBA-114 the subagent extension
//! read that registry through `HostServices::all_tools`, saw a host with `read` and nothing else
//! (the registry had no `bash`/`edit` definition at all), and (a) stripped `bash` and
//! `edit` from every child it launched — foreground, detached background and chain step alike, the
//! background case via `RunnerConfig::hostAvailableBuiltins` — and (b) refused a `reviewer` or
//! `scout` launch outright as a "lane infrastructure failure". Now:
//!
//! * the child's REAL argv (`--tools`, echoed by the scripted `cyrup-subagent-fixture` child and
//!   read back from the per-attempt stdout tee) carries every tool its agent declares, on all three
//!   launch paths, and `reviewer`/`scout` launch;
//! * the one remaining guard is the child's own: a real child session whose registry lacks a
//!   required tool refuses its run at `agent_start`, before its first model call, with upstream's
//!   `formatChildToolDiagnostic` text — and the parent reports that text as the run's error.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cyrup_core::StopReason;
use cyrup_ext::NativeExtension;
use cyrup_ext_subagents::background::RunPaths;
use cyrup_ext_subagents::background::runner_main::{RunnerConfig, RunnerOverrides, run_with};
use cyrup_ext_subagents::exec::tool_availability::{
    CHILD_TOOL_DIAGNOSTIC_PATH_ENV, read_child_tool_diagnostic_error,
};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::native_supervisor::ENV_REQUIRED_CHILD_TOOLS;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::spawn::intercom_target::ENV_CHILD_AGENT;
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use cyrup_session_svc::{AgentSession, AgentSessionEvent, SessionBuilder, SessionConfig};
use futures::StreamExt;

/// Upstream's `formatChildToolDiagnostic` (`src/runs/shared/tool-availability.ts:15-37` @v0.71.0,
/// runner arm) for a `worker` whose registry lacks `bash` and `edit` — the full five lines, since
/// the remedy lines are the operator-facing half of the refusal.
const WORKER_MISSING_BASH_EDIT: &str = "Agent 'worker' requested unavailable child tools: bash, \
     edit.\nThe `tools` field is a strict allowlist; it does not load extension code.\nFor \
     extension tools, add the provider path to `subagentOnlyExtensions` (child-only), \
     `extensions`, or as a path-like entry in `tools`, while keeping each registered tool name in \
     `tools`.\nFor MCP tools, verify the MCP adapter configuration and selected tool names. For \
     builtin tools, verify the name against the installed Pi version.";

/// A parent project: a cwd with three project personas (the names the deleted lane contract
/// matched, plus a `worker`), an empty agent dir, and a sandboxed subagent home.
struct Project {
    _tmp: tempfile::TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
    home: PathBuf,
}

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
    for (name, tools) in [
        ("worker", "read, bash, edit"),
        ("reviewer", "read, grep, find, ls, bash"),
        ("scout", "read, grep, find, ls, bash"),
    ] {
        std::fs::write(
            agents.join(format!("{name}.md")),
            format!(
                "---\nname: {name}\ndescription: SUBA-114 fixture persona\nmodel: \
                 fixture/model\ntools: {tools}\n---\n\nYou are the {name}.\n"
            ),
        )
        .expect("write persona");
    }
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

fn message_end(text: &str, stop_reason: &str) -> String {
    serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {
                "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
            },
            "stopReason": stop_reason
        }
    })
    .to_string()
}

/// A child that echoes its argv and finishes cleanly.
fn echo_script(dir: &Path) -> PathBuf {
    write_script(
        dir,
        "echo-child.json",
        &serde_json::json!({
            "echo_argv": true,
            "steps": [{"kind": "emit", "line": message_end("child done", "stop")}],
            "exit_code": 0
        }),
    )
}

/// The subagent extension as the parent session loads it, with the child binary named in config.
fn extension(p: &Project, spawn: SpawnCommand) -> Arc<SubagentsExtension> {
    Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            // Foreground unless a call asks for `async: true` (pi `config.ts:222-224`).
            async_by_default: false,
            spawn_command: Some(spawn),
            roots: Roots::sandboxed(&p.home),
            ..SubagentExtensionConfig::default()
        },
        p.cwd.clone(),
    ))
}

/// A REAL session started with `cyrup --tools <cli_tools>` in a runtime whose tool registry holds
/// only `registry` (the SDK's `allowedToolNames`, `Availability::Allow`).
async fn session_with_tools(
    p: &Project,
    cli_tools: &[&str],
    registry: &[&str],
    faux: Arc<FauxProvider>,
    extensions: Vec<Arc<dyn NativeExtension>>,
) -> Arc<AgentSession> {
    let mut cfg = SessionConfig::new(p.cwd.clone(), p.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.tools = Some(cli_tools.iter().map(|t| (*t).to_string()).collect());
    cfg.tool_availability =
        cyrup_tools::Availability::Allow(registry.iter().map(|t| (*t).to_string()).collect());
    let mut builder = SessionBuilder::new(faux as Arc<dyn Provider>, cfg);
    for ext in extensions {
        builder = builder.with_native_extension(ext);
    }
    // `into_shared` exactly as the production runtime does (`cyrup-session-svc/src/runtime.rs`):
    // it is what binds the live session's activity, i.e. what makes `ctx.abort()` interrupt the
    // run in flight rather than queue for a turn boundary.
    builder
        .build()
        .await
        .expect("a real session builds")
        .into_shared()
}

/// The parent's one turn: call `subagent` with `args`, then acknowledge.
fn delegate_once(args: serde_json::Value) -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        FauxResponseStep::from(faux_assistant_message(
            vec![faux_tool_call("subagent", args)],
            StopReason::ToolUse,
        )),
        FauxResponseStep::from(faux_assistant_message(
            vec![faux_text("acknowledged")],
            StopReason::Stop,
        )),
    ]);
    faux
}

async fn run_turn(session: &AgentSession, text: &str) -> Vec<AgentSessionEvent> {
    let mut stream = session
        .prompt(text.to_string())
        .await
        .expect("the prompt is accepted");
    let mut events = Vec::new();
    while let Some(ev) = stream.next().await {
        events.push(ev);
    }
    session.wait_for_idle().await;
    events
}

/// The single `subagent` tool result of a turn: `(is_error, result)`.
fn subagent_result(events: &[AgentSessionEvent]) -> (bool, serde_json::Value) {
    let ends: Vec<(bool, serde_json::Value)> = events
        .iter()
        .filter_map(|e| match e {
            AgentSessionEvent::ToolExecutionEnd {
                tool_name,
                result,
                is_error,
                ..
            } if tool_name == "subagent" => Some((*is_error, result.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(ends.len(), 1, "exactly one subagent call: {events:#?}");
    ends.into_iter().next().expect("one result")
}

/// The value after `--tools` in the argv the scripted child received for the LAST attempt run in
/// `cwd` (the fixture echoes each argv entry as `{"type":"unknown","arg":…}`).
fn child_tools_arg(cwd: &Path) -> Option<String> {
    let tee = std::fs::read_to_string(
        cyrup_ext_subagents::background::attempt_scratch_dir(cwd).join("attempt-0.jsonl"),
    )
    .unwrap_or_default();
    let args: Vec<String> = tee
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|v| v.get("arg").and_then(|a| a.as_str()).map(str::to_string))
        .collect();
    let idx = args.iter().position(|a| a == "--tools")?;
    args.get(idx + 1).cloned()
}

fn clear_tee(cwd: &Path) {
    let _ = std::fs::remove_file(
        cyrup_ext_subagents::background::attempt_scratch_dir(cwd).join("attempt-0.jsonl"),
    );
}

/// The parent really is the narrow dispatcher the bug needed: `read` and `subagent` are active,
/// and the tools the children declare are neither active nor even DEFINED in its registry — the
/// registry being exactly what the deleted `host_builtin_tool_names` read.
fn assert_parent_is_narrow(session: &AgentSession) {
    let active = session.active_tool_names();
    for present in ["read", "subagent"] {
        assert!(
            active.iter().any(|t| t == present),
            "{present} missing: {active:?}"
        );
    }
    for absent in ["bash", "edit", "grep", "find", "ls"] {
        assert!(
            !active.iter().any(|t| t == absent) && session.tool_definition(absent).is_none(),
            "the parent must NOT hold `{absent}` for this to prove anything: {active:?}"
        );
    }
}

/// Foreground single runs: a `worker` declaring `read, bash, edit` gets all three on its REAL
/// argv, and a `reviewer` and a `scout` — refused outright before SUBA-114 whenever the parent
/// lacked a repository tool — launch and succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_narrow_parent_launches_foreground_children_with_every_declared_tool() {
    let p = project();
    let script = echo_script(&p.cwd);
    for (agent, expected) in [
        ("worker", "read,bash,edit"),
        ("reviewer", "read,grep,find,ls,bash"),
        ("scout", "read,grep,find,ls,bash"),
    ] {
        clear_tee(&p.cwd);
        let faux = delegate_once(serde_json::json!({"agent": agent, "task": "do the thing"}));
        let session = session_with_tools(
            &p,
            &["subagent", "read"],
            &["subagent", "read"],
            faux,
            vec![extension(&p, fixture_cmd(&script)) as Arc<dyn NativeExtension>],
        )
        .await;
        assert_parent_is_narrow(&session);

        let events = run_turn(&session, &format!("delegate to the {agent}")).await;
        let (is_error, result) = subagent_result(&events);
        assert!(!is_error, "{agent} must launch and succeed: {result:#}");
        assert!(
            result.to_string().contains("child done"),
            "{agent}: the child's own output is the result: {result:#}"
        );
        assert_eq!(
            child_tools_arg(&p.cwd).as_deref(),
            Some(expected),
            "{agent}: the child's `--tools` must carry what its agent declares, not what the \
             parent session holds"
        );
    }
}

/// A foreground `/chain` step (`ExecSingleStepExecutor::foreground`) — the path that used to take
/// its own host observation as a constructor argument.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_narrow_parent_launches_a_chain_step_with_every_declared_tool() {
    let p = project();
    let script = echo_script(&p.cwd);
    let faux = delegate_once(serde_json::json!({
        "chain": [{"agent": "worker", "task": "do the thing"}]
    }));
    let session = session_with_tools(
        &p,
        &["subagent", "read"],
        &["subagent", "read"],
        faux,
        vec![extension(&p, fixture_cmd(&script)) as Arc<dyn NativeExtension>],
    )
    .await;
    assert_parent_is_narrow(&session);

    let events = run_turn(&session, "run the chain").await;
    let (is_error, result) = subagent_result(&events);
    assert!(!is_error, "the chain step must succeed: {result:#}");
    assert_eq!(child_tools_arg(&p.cwd).as_deref(), Some("read,bash,edit"));
}

/// Find the one `runner-config.json` the async launch wrote under the sandboxed home.
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

/// The detached background path: the orchestrator plans the run from the narrow parent and writes
/// `runner-config.json` (which used to carry `hostAvailableBuiltins: ["read"]`); the hop-2 runner
/// then launches the child from that config alone. Hop 1 is a long-sleeping stand-in so nothing
/// races the in-process hop 2 this test drives through the production `run_with`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_narrow_parent_launches_a_detached_background_child_with_every_declared_tool() {
    let p = project();
    let sleeper = write_script(
        &p.cwd,
        "hop1-sleeper.json",
        &serde_json::json!({"steps": [{"kind": "sleep_ms", "ms": 20000}], "exit_code": 0}),
    );
    let faux = delegate_once(serde_json::json!({
        "agent": "worker", "task": "do the thing", "async": true
    }));
    let session = session_with_tools(
        &p,
        &["subagent", "read"],
        &["subagent", "read"],
        faux,
        vec![extension(&p, fixture_cmd(&sleeper)) as Arc<dyn NativeExtension>],
    )
    .await;
    assert_parent_is_narrow(&session);

    let events = run_turn(&session, "run it in the background").await;
    let (is_error, result) = subagent_result(&events);
    assert!(!is_error, "the async launch must be accepted: {result:#}");

    let cfg_path = find_runner_config(&p.home).unwrap_or_else(|| {
        panic!(
            "the async launch must write runner-config.json under {}: {result:#}",
            p.home.display()
        )
    });
    let raw = std::fs::read(&cfg_path).expect("read runner-config.json");
    let on_disk: serde_json::Value = serde_json::from_slice(&raw).expect("config is JSON");
    assert!(
        on_disk.get("hostAvailableBuiltins").is_none(),
        "the orchestrator no longer ships its own registry to hop 2: {on_disk:#}"
    );
    let config: RunnerConfig = serde_json::from_slice(&raw).expect("a RunnerConfig");
    let run_paths = RunPaths::for_run(&config.async_root, &config.results_dir, &config.run_id);

    clear_tee(&config.cwd);
    let echo = echo_script(&p.cwd);
    run_with(
        &cfg_path,
        &run_paths,
        RunnerOverrides {
            spawn_command: Some(fixture_cmd(&echo)),
            roots: Some(Roots::sandboxed(&p.home)),
            ..Default::default()
        },
    )
    .await
    .expect("run_with never returns Err");

    assert_eq!(
        child_tools_arg(&config.cwd).as_deref(),
        Some("read,bash,edit"),
        "the detached runner's child must carry every declared tool"
    );
    // Let hop 2's own terminal writes settle before the tempdir goes.
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// The child-side guard SUBA-114 relies on, in a REAL child session: the child is started exactly
/// as its spawn starts it (`--tools read,bash,edit`, the declared list), but in a runtime whose
/// builtin menu differs — its registry has only `read` (upstream's "fork host" case) — with the
/// prompt runtime assembled by the production `prompt_runtime_from_env` from the variables the
/// parent writes. At `agent_start` it refuses: the run is aborted BEFORE the model request is
/// ever made, and the diagnostic carries upstream's message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_whose_registry_lacks_a_required_tool_is_refused_at_agent_start() {
    let p = project();
    let diagnostic = p.home.join("tool-diagnostic.json");
    let raw_path = diagnostic.display().to_string();
    let env = move |key: &str| -> Option<String> {
        match key {
            k if k == CHILD_TOOL_DIAGNOSTIC_PATH_ENV => Some(raw_path.clone()),
            k if k == ENV_REQUIRED_CHILD_TOOLS => Some(r#"["read","bash","edit"]"#.to_string()),
            k if k == ENV_CHILD_AGENT => Some("worker".to_string()),
            _ => None,
        }
    };
    let runtime = Arc::new(
        cyrup_ext_subagents::prompt_runtime::prompt_runtime_from_env(&env)
            .expect("builds")
            .expect("a child with a required-tools contract has a prompt runtime"),
    );
    // An ASYNC factory is resolved lazily, inside the returned stream (`faux.rs`, pi
    // `queueMicrotask`), so this flag flips only if the agent loop actually POLLS the model
    // request — `call_count` alone would also count a request object built and then dropped
    // unpolled because the run was already aborted.
    let asked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let asked_in_step = Arc::clone(&asked);
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![FauxResponseStep::async_factory(
        move |_ctx, _opts, _state, _model| {
            let asked = Arc::clone(&asked_in_step);
            async move {
                asked.store(true, std::sync::atomic::Ordering::SeqCst);
                faux_assistant_message(
                    vec![faux_text("the refused child must never be asked")],
                    StopReason::Stop,
                )
            }
        },
    )]);
    let child = session_with_tools(
        &p,
        &["read", "bash", "edit"],
        &["read"],
        faux.clone(),
        vec![runtime as Arc<dyn NativeExtension>],
    )
    .await;
    let active = child.active_tool_names();
    assert!(
        !active.iter().any(|t| t == "bash" || t == "edit"),
        "{active:?}"
    );

    let events = run_turn(&child, "do the thing").await;

    assert_eq!(
        read_child_tool_diagnostic_error(Some(&diagnostic)).as_deref(),
        Some(WORKER_MISSING_BASH_EDIT),
        "the refusal names exactly what the child's own registry lacks, in upstream's words"
    );
    let assistant_stop_reasons: Vec<StopReason> = events
        .iter()
        .filter_map(|e| match e {
            AgentSessionEvent::MessageEnd {
                message: cyrup_agent::AgentMessage::Assistant(a),
            } => Some(a.stop_reason),
            _ => None,
        })
        .collect();
    assert_eq!(
        assistant_stop_reasons,
        vec![StopReason::Aborted],
        "the child's one turn ends ABORTED: {events:#?}"
    );
    assert!(
        !asked.load(std::sync::atomic::Ordering::SeqCst),
        "the refusal happens at agent_start, before the child's first model call is made"
    );
}

/// The parent half of the same refusal, through the narrow parent session: a child that refused at
/// `agent_start` (it wrote the diagnostic, its run ended aborted, and it exited 1 — what a real
/// `cyrup --mode json` child does after aborting) surfaces upstream's message as the subagent
/// run's error, outranking the aborted assistant turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_parent_reports_the_childs_agent_start_refusal_as_the_run_error() {
    let p = project();
    let refusal = write_script(
        &p.cwd,
        "refusing-child.json",
        &serde_json::json!({
            "steps": [
                {"kind": "write_tool_diagnostic", "value": {
                    "agent": "worker",
                    "required": ["read", "bash", "edit"],
                    "available": ["read"],
                    "missing": ["bash", "edit"]
                }},
                {"kind": "emit", "line": message_end("", "aborted")},
                {"kind": "emit_stderr", "line": "Request aborted"}
            ],
            "exit_code": 1
        }),
    );
    let faux = delegate_once(serde_json::json!({"agent": "worker", "task": "do the thing"}));
    let session = session_with_tools(
        &p,
        &["subagent", "read"],
        &["subagent", "read"],
        faux,
        vec![extension(&p, fixture_cmd(&refusal)) as Arc<dyn NativeExtension>],
    )
    .await;

    let events = run_turn(&session, "delegate to the worker").await;
    let (is_error, result) = subagent_result(&events);
    assert!(is_error, "a refused child is a failed run: {result:#}");
    let text = result.to_string();
    assert!(
        text.contains("Agent 'worker' requested unavailable child tools: bash, edit."),
        "the run's error is the child's own refusal: {text}"
    );
}
