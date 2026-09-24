//! SUBA-100 — Herdr saved-machine placement, end to end through the REAL `subagent` tool and the
//! REAL detached runner.
//!
//! Upstream reference: `pi-subagents` @v0.68.0 — `src/shared/herdr-machine.ts` (catalog,
//! settings, cwd rules, refusals), `src/runs/shared/herdr-connection.ts` (hardened ssh + socket
//! forward), `src/runs/shared/herdr-placed-run.ts` (the owned pane), and the launch folds in
//! `runs/foreground/subagent-executor.ts:3823-3827` / `runs/background/async-execution.ts:990-1001`.
//!
//! No mocks. The saved machine is reached through a real `/bin/sh` ssh stand-in that runs the
//! remote command under a separate `$HOME` (and, for `-L`, links the forwarded API socket); that
//! home holds the machine's own `herdr` (answering `status server --json`) and its `cyrup` — the
//! real `cyrup-subagent-fixture` binary behind a two-line wrapper. The herdr API is a real
//! `UnixListener` speaking herdr's one-line framing whose `pane.send_input` runs the typed command
//! in a real `sh`. The local `herdr machine list --json` is a real script. Everything this
//! extension reads from the environment is named through `SubagentExtensionConfig::env_overrides`
//! (the ssh binary, the herdr binary) — no process environment is mutated.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{CancelToken, Content, Tool, ToolCallId};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use serde_json::json;

fn write_executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn message_end_line(text: &str) -> String {
    json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {"input": 3, "output": 2, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 5,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}},
            "stopReason": "stop"
        }
    })
    .to_string()
}

/// The saved machine `fake-box`: its home, its herdr API, and the local ssh + herdr that reach it.
struct Machine {
    dir: tempfile::TempDir,
    home: PathBuf,
    workdir: PathBuf,
    remote_tmp: PathBuf,
    ssh: PathBuf,
    local_herdr: PathBuf,
    calls: Arc<Mutex<Vec<serde_json::Value>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[derive(Default)]
struct HerdrState {
    next: usize,
    workspaces: Vec<serde_json::Value>,
    panes: Vec<serde_json::Value>,
    children: BTreeMap<String, tokio::process::Child>,
}

impl Machine {
    fn start(child_output: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let home = root.join("remote-home");
        let bin = home.join(".local").join("bin");
        let workdir = home.join("repo");
        let remote_tmp = root.join("remote-tmp");
        for path in [&bin, &workdir.join("sub"), &remote_tmp] {
            std::fs::create_dir_all(path).unwrap();
        }
        let socket = root.join("herdr-api.sock");
        write_executable(
            &bin.join("herdr"),
            &format!(
                "#!/bin/sh\n[ \"$1 $2 $3\" = 'status server --json' ] || exit 9\nprintf '%s\\n' '{}'\n",
                json!({"status":"running","running":true,"version":"0.9.1","protocol":7,
                    "compatible":true,"endpoint_compatible":true,
                    "socket":socket.display().to_string(),"session":null})
            ),
        );
        // The machine's `cyrup` IS the scripted fixture: the same child contract, run over there.
        let script = root.join("fixture-script.json");
        std::fs::write(
            &script,
            json!({"steps":[{"kind":"emit","line":message_end_line(child_output)}],"exit_code":0})
                .to_string(),
        )
        .unwrap();
        write_executable(
            &bin.join("cyrup"),
            &format!(
                "#!/bin/sh\npwd >> \"$HOME/ran-in\"\nexec '{}' --fixture-script '{}' \"$@\"\n",
                crate::support::bins::subagent_fixture().display(),
                script.display()
            ),
        );
        let ssh = root.join("fake-ssh");
        write_executable(
            &ssh,
            &format!(
                r#"#!/bin/sh
fwd=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) shift 2 ;;
    -T|-N) shift ;;
    -L) fwd=$2; shift 2 ;;
    *) break ;;
  esac
done
[ "$1" = "me@fake-box" ] || {{ echo "ssh: Could not resolve hostname $1" >&2; exit 255; }}
shift
if [ -n "$fwd" ]; then
  ln -s "${{fwd#*:}}" "${{fwd%%:*}}" || exit 255
  exec sleep 3600
fi
cd '{home}' || exit 255
HOME='{home}' TMPDIR='{tmp}' exec /bin/sh -c "$1"
"#,
                home = home.display(),
                tmp = remote_tmp.display()
            ),
        );
        let local_herdr = root.join("local-herdr");
        write_executable(
            &local_herdr,
            &format!(
                "#!/bin/sh\n[ \"$1 $2 $3\" = 'machine list --json' ] || exit 9\nprintf '%s\\n' '{}'\n",
                json!({"machines":[
                    {"id":"m-fake","label":"fake-box","target":"me@fake-box","enabled":true},
                    {"id":"m-cold","label":"cold-box","target":"me@cold","enabled":false}
                ]})
            ),
        );
        let calls = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let state = Arc::new(tokio::sync::Mutex::new(HerdrState::default()));
        let server = {
            let calls = Arc::clone(&calls);
            let home = home.clone();
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let calls = Arc::clone(&calls);
                    let state = Arc::clone(&state);
                    let home = home.clone();
                    tokio::spawn(async move { serve(stream, calls, state, home).await });
                }
            })
        };
        Self {
            dir,
            home,
            workdir,
            remote_tmp,
            ssh,
            local_herdr,
            calls,
            server,
        }
    }

    fn methods(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call["method"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    fn ran_in(&self) -> Vec<String> {
        std::fs::read_to_string(self.home.join("ran-in"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn runtime_dirs_left(&self) -> usize {
        std::fs::read_dir(&self.remote_tmp)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("cyrup-subagents-herdr-"))
            })
            .count()
    }

    /// The env this extension reads placement from, pinned rather than exported.
    fn env_overrides(&self) -> BTreeMap<String, Option<String>> {
        BTreeMap::from([
            (
                "CYRUP_HERDR_SSH_BIN".to_string(),
                Some(self.ssh.display().to_string()),
            ),
            (
                "HERDR_BIN".to_string(),
                Some(self.local_herdr.display().to_string()),
            ),
            ("SSH_AUTH_SOCK".to_string(), None),
        ])
    }
}

fn pane_json(pane_id: &str, workspace_id: &str, tab_id: &str, cwd: &str) -> serde_json::Value {
    json!({"pane_id":pane_id,"terminal_id":format!("t-{pane_id}"),"workspace_id":workspace_id,
        "tab_id":tab_id,"focused":false,"cwd":cwd,"agent_status":"idle","revision":1})
}

async fn serve(
    stream: tokio::net::UnixStream,
    calls: Arc<Mutex<Vec<serde_json::Value>>>,
    state: Arc<tokio::sync::Mutex<HerdrState>>,
    home: PathBuf,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    if BufReader::new(read).read_line(&mut line).await.is_err() {
        return;
    }
    let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) else {
        return;
    };
    calls.lock().unwrap().push(request.clone());
    let params = &request["params"];
    let mut state = state.lock().await;
    let result: Result<serde_json::Value, String> = match request["method"]
        .as_str()
        .unwrap_or_default()
    {
        "session.snapshot" => Ok(
            json!({"type":"session_snapshot","snapshot":{"version":"0.9.1",
            "protocol":7,"workspaces":state.workspaces,"tabs":[],"panes":state.panes,"layouts":[],"agents":[]}}),
        ),
        "workspace.create" => {
            state.next += 1;
            let n = state.next;
            let (ws, tab, pane) = (format!("w{n}"), format!("w{n}:t1"), format!("w{n}:p1"));
            let cwd = params["cwd"].as_str().unwrap_or_default().to_string();
            let workspace = json!({"workspace_id":ws,"number":n,"label":params["label"],"focused":false,
                "pane_count":1,"tab_count":1,"active_tab_id":tab,"agent_status":"idle"});
            let tab_json = json!({"tab_id":tab,"workspace_id":ws,"number":1,"label":"1","focused":false,
                "pane_count":1,"agent_status":"idle"});
            let pane_info = pane_json(&pane, &ws, &tab, &cwd);
            state.workspaces.push(workspace.clone());
            state.panes.push(pane_info.clone());
            Ok(
                json!({"type":"workspace_created","workspace":workspace,"tab":tab_json,"root_pane":pane_info}),
            )
        }
        "tab.create" => {
            state.next += 1;
            let n = state.next;
            let ws = params["workspace_id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let (tab, pane) = (format!("{ws}:t{n}"), format!("{ws}:p{n}"));
            let cwd = params["cwd"].as_str().unwrap_or_default().to_string();
            let tab_json = json!({"tab_id":tab,"workspace_id":ws,"number":n,"label":params["label"],
                "focused":false,"pane_count":1,"agent_status":"idle"});
            let pane_info = pane_json(&pane, &ws, &tab, &cwd);
            state.panes.push(pane_info.clone());
            Ok(json!({"type":"tab_created","tab":tab_json,"root_pane":pane_info}))
        }
        "pane.send_input" => {
            let pane_id = params["pane_id"].as_str().unwrap_or_default().to_string();
            let child = tokio::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(params["text"].as_str().unwrap_or_default())
                .current_dir(&home)
                .env("HOME", &home)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            state.children.insert(pane_id, child);
            Ok(json!({"type":"ok"}))
        }
        "pane.get" => {
            let pane_id = params["pane_id"].as_str().unwrap_or_default();
            state
                .panes
                .iter()
                .find(|pane| pane["pane_id"] == pane_id)
                .map(|pane| json!({"type":"pane_info","pane":pane}))
                .ok_or_else(|| format!("pane {pane_id} not found"))
        }
        "pane.close" => {
            let pane_id = params["pane_id"].as_str().unwrap_or_default().to_string();
            state
                .panes
                .retain(|pane| pane["pane_id"] != pane_id.as_str());
            if let Some(mut child) = state.children.remove(&pane_id) {
                let _ = child.start_kill();
            }
            Ok(json!({"type":"ok"}))
        }
        other => Err(format!("unexpected method {other}")),
    };
    let reply = match result {
        Ok(result) => json!({"id":request["id"],"result":result}),
        Err(message) => json!({"id":request["id"],"error":{"code":"not_found","message":message}}),
    };
    let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
}

/// A project whose settings map `fake-box` to the repo on the machine.
fn project(machine: &Machine) -> tempfile::TempDir {
    let work = tempfile::tempdir().unwrap();
    let agents = work.path().join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("settings.json"),
        json!({"subagents":{"machines":{"fake-box":{"cwd":machine.workdir.display().to_string()}}}})
            .to_string(),
    )
    .unwrap();
    std::fs::write(
        agents.join("remote.md"),
        "---\nname: remote\ndescription: a worker placed on the saved machine\nmodel: fixture/model\nmachine: fake-box\n---\n\nYou work remotely.\n",
    )
    .unwrap();
    std::fs::write(
        agents.join("plain.md"),
        "---\nname: plain\ndescription: an unplaced worker\nmodel: fixture/model\n---\n\nYou work.\n",
    )
    .unwrap();
    std::fs::write(
        agents.join("generic.md"),
        "---\nname: generic\ndescription: a generic foreign command\nmachine: fake-box\nrunner: {\"type\": \"external-cli\", \"command\": \"my-cli\"}\n---\n\nbody\n",
    )
    .unwrap();
    work
}

fn extension(machine: &Machine, work: &Path, home: &Path) -> SubagentsExtension {
    SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            async_by_default: false,
            roots: Roots::sandboxed(home),
            env_overrides: machine.env_overrides(),
            ..SubagentExtensionConfig::default()
        },
        work.to_path_buf(),
    )
}

async fn call(ext: &SubagentsExtension, params: serde_json::Value) -> Result<String, String> {
    let text = |content: &[Content]| {
        content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    match ext
        .subagent_tool()
        .execute(
            ToolCallId::from("suba100"),
            params,
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
    {
        Ok(result) => Ok(text(&result.content)),
        Err(error) => Err(error.to_string()),
    }
}

/// An agent whose frontmatter names a saved machine runs its child ON that machine — in a fresh
/// Herdr-owned pane, in the directory `subagents.machines.<name>.cwd` configures — and the child's
/// answer comes back through the tool exactly as a local child's would.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_with_a_saved_machine_runs_on_that_machine() {
    let machine = Machine::start("SUBA100_REMOTE_ANSWER");
    let work = project(&machine);
    let home = tempfile::tempdir().unwrap();
    let ext = extension(&machine, work.path(), home.path());

    let text = call(&ext, json!({"agent":"remote","task":"answer from the box"}))
        .await
        .expect("the placed run completes");
    assert!(text.contains("SUBA100_REMOTE_ANSWER"), "{text}");
    assert_eq!(machine.ran_in(), [machine.workdir.display().to_string()]);
    assert_eq!(
        machine.methods(),
        [
            "session.snapshot",
            "workspace.create",
            "pane.send_input",
            "pane.get",
            "pane.close"
        ]
    );
    assert_eq!(machine.runtime_dirs_left(), 0);
    // The allocation lock and run journal live under THIS extension's agent dir, never the
    // process's real home.
    let journal = Roots::sandboxed(home.path())
        .agent_dir()
        .join("subagents/herdr-run-journal");
    assert_eq!(std::fs::read_dir(&journal).unwrap().count(), 1);
    let _ = &machine.dir;
}

/// A call-level `machine` places every chain step, and the call's `cwd` is a path ON the machine
/// (relative → joined onto the configured root), never resolved locally.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_machine_places_every_chain_step_in_the_machine_cwd() {
    let machine = Machine::start("SUBA100_CHAIN_STEP");
    let work = project(&machine);
    let home = tempfile::tempdir().unwrap();
    let ext = extension(&machine, work.path(), home.path());

    let text = call(
        &ext,
        json!({"chain":[{"agent":"plain","task":"one"},{"agent":"plain","task":"two {previous}"}],
            "machine":"fake-box","cwd":"sub"}),
    )
    .await
    .expect("the placed chain completes");
    assert!(text.contains("SUBA100_CHAIN_STEP"), "{text}");
    let sub = machine.workdir.join("sub").display().to_string();
    assert_eq!(machine.ran_in(), [sub.clone(), sub]);
    let methods = machine.methods();
    assert_eq!(
        methods.iter().filter(|m| *m == "workspace.create").count(),
        1,
        "{methods:?}"
    );
    assert_eq!(
        methods.iter().filter(|m| *m == "tab.create").count(),
        1,
        "the second step reuses the owned workspace: {methods:?}"
    );
    assert!(
        !work.path().join("sub").exists(),
        "the machine cwd never becomes a local directory"
    );
}

/// A top-level `tasks[]` fan-out with a call machine places every task.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_machine_places_every_parallel_task() {
    let machine = Machine::start("SUBA100_PARALLEL_TASK");
    let work = project(&machine);
    let home = tempfile::tempdir().unwrap();
    let ext = extension(&machine, work.path(), home.path());

    let text = call(
        &ext,
        json!({"tasks":[{"agent":"plain","task":"a"},{"agent":"plain","task":"b"}],"machine":"fake-box"}),
    )
    .await
    .expect("the placed fan-out completes");
    assert!(text.contains("SUBA100_PARALLEL_TASK"), "{text}");
    let root = machine.workdir.display().to_string();
    assert_eq!(machine.ran_in(), [root.clone(), root]);
    assert_eq!(machine.runtime_dirs_left(), 0);
}

/// Upstream's launch refusals, before anything reaches the machine: an unknown or disabled
/// machine, and a generic external command that cannot be remote-wrapped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unplaceable_launches_are_refused_before_anything_runs() {
    let machine = Machine::start("never");
    let work = project(&machine);
    let home = tempfile::tempdir().unwrap();
    let ext = extension(&machine, work.path(), home.path());

    let unknown = call(&ext, json!({"agent":"plain","task":"x","machine":"nope"}))
        .await
        .expect_err("an unknown machine is refused");
    assert!(
        unknown.contains("Herdr machine 'nope' was not found. Saved machines: fake-box. Add one with herdr machine add <target> --label <name>."),
        "{unknown}"
    );
    let disabled = call(
        &ext,
        json!({"agent":"plain","task":"x","machine":"cold-box"}),
    )
    .await
    .expect_err("a disabled machine is refused");
    assert!(
        disabled.contains("Machine 'cold-box' is disabled. Run herdr machine enable m-cold."),
        "{disabled}"
    );
    // Upstream's schema `machine: { minLength: 1 }`: a blank machine is refused, never read as
    // "a machine" that silently moves the call's cwd onto nothing. The schema check runs before
    // anything is resolved, so it wins over the unknown-agent refusal a later stage would raise.
    let blank = call(
        &ext,
        json!({"agent":"no-such-agent","task":"x","machine":"  "}),
    )
    .await
    .expect_err("a blank machine is refused");
    assert!(
        blank.contains("Herdr machine id or label is required."),
        "{blank}"
    );
    let generic = call(&ext, json!({"agent":"generic","task":"x"}))
        .await
        .expect_err("a generic external command is refused placement");
    assert!(
        generic.contains("Agent 'generic' requested machine 'fake-box', but generic external-cli commands cannot be remote-wrapped safely."),
        "{generic}"
    );
    assert!(machine.methods().is_empty(), "nothing reached herdr");
    assert!(machine.ran_in().is_empty(), "nothing ran on the machine");
}

/// The detached runner (hop 2) places a step it was handed already resolved: the runner is a real
/// separate process, started with the ssh binary in its environment the way a launch forwards it,
/// and its step's child runs on the machine.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_detached_runner_runs_a_placed_step_on_the_machine() {
    use cyrup_ext_subagents::background::atomic::write_atomic_json;
    use cyrup_ext_subagents::background::runner_main::RunnerConfig;
    use cyrup_ext_subagents::background::{RunId, RunMode, RunPaths, RunState};
    use cyrup_ext_subagents::placement::{HerdrMachineReference, MachineProvider, StepPlacement};
    use cyrup_ext_subagents::spawn::chain_graph::{RunnerStep, SingleStepSpec};

    let machine = Machine::start("SUBA100_BACKGROUND_ANSWER");
    let dir = tempfile::tempdir().unwrap();
    let run_id = RunId::from_token("placedbackground00000000000001");
    let async_root = dir.path().join("async");
    let results_dir = dir.path().join("results");
    std::fs::create_dir_all(&async_root).unwrap();
    std::fs::create_dir_all(&results_dir).unwrap();
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    std::fs::create_dir_all(&run_paths.run_dir).unwrap();

    let mut step: SingleStepSpec =
        serde_json::from_value(json!({"agent":"worker","task":"answer"}))
            .unwrap_or_else(|error| panic!("minimal step: {error}"));
    step.machine = Some(StepPlacement {
        requested: "fake-box".to_string(),
        resolved: Some(HerdrMachineReference {
            provider: MachineProvider::Herdr,
            id: "m-fake".to_string(),
            label: Some("fake-box".to_string()),
            target: "me@fake-box".to_string(),
            session: None,
            cwd: machine.workdir.display().to_string(),
            transport: None,
            agent_dir: None,
        }),
    });
    let persona: cyrup_ext_subagents::exec::ResolvedAgentPersona = serde_json::from_value(json!({
        "name":"worker","model":"fixture-model","fallbackModels":[],"systemPromptMode":"replace",
        "systemPromptBody":"","subagentOnlyExtensions":[],"excludeTools":[],"inheritProjectContext":false,
        "inheritSkills":true,"skills":[],"completionGuard":false
    }))
    .unwrap_or_else(|error| panic!("minimal persona: {error}"));
    let config: RunnerConfig = serde_json::from_value(json!({
        "runId": run_id,
        "mode": "single",
        "steps": [serde_json::to_value(RunnerStep::SingleStep(step)).unwrap()],
        "cwd": dir.path(),
        "sessionId": "it-session",
        "globalConcurrencyLimit": 4,
        "maxSubagentDepth": 2,
        "asyncRoot": async_root,
        "resultsDir": results_dir,
        "resolvedAgents": {"worker": persona},
        "originalTask": "answer",
    }))
    .unwrap_or_else(|error| panic!("minimal runner config: {error}"));
    assert_eq!(config.mode, RunMode::Single);
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config).await.unwrap();

    let mut orchestrator =
        std::process::Command::new(crate::support::bins::subagent_orchestrator_sim())
            .arg(&cfg_path)
            .arg(&run_paths.runner_stdout_log)
            .arg(&run_paths.runner_stderr_log)
            .env("CYRUP_SUBAGENT_STEP_BINARY", "/nonexistent/local-cyrup")
            .env("CYRUP_HERDR_SSH_BIN", &machine.ssh)
            .env("CYRUP_HOME", dir.path().join("cyrup-home"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("orchestrator-sim spawns");
    let mut first = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(orchestrator.stdout.take().unwrap()),
        &mut first,
    )
    .unwrap();
    assert!(!first.starts_with("SPAWN_FAILED"), "{first}");
    assert!(orchestrator.wait().unwrap().success());

    let session = cyrup_ext_subagents::identity::SessionId::parse("it-session").unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while run_paths.resolve_result(&session, &run_id).await.is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no terminal result.\nrunner stderr:\n{}\nstatus:\n{}",
            std::fs::read_to_string(&run_paths.runner_stderr_log).unwrap_or_default(),
            std::fs::read_to_string(&run_paths.status).unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let status: cyrup_ext_subagents::background::RunStatus =
        serde_json::from_slice(&std::fs::read(&run_paths.status).unwrap()).unwrap();
    let result_path = run_paths.resolve_result(&session, &run_id).await.unwrap();
    let result = std::fs::read_to_string(&result_path).unwrap();
    assert_eq!(
        status.state,
        RunState::Complete,
        "{result}\nrunner stderr:\n{}",
        std::fs::read_to_string(&run_paths.runner_stderr_log).unwrap_or_default()
    );
    assert!(result.contains("SUBA100_BACKGROUND_ANSWER"), "{result}");
    // pi `StepResult.nativeMachine` (`subagent-runner.ts:1549,1579` @v0.68.0): the step's
    // placed-run evidence names the machine it ran on, on the step status and in the result.
    assert_eq!(
        status.steps[0]
            .native_machine
            .as_ref()
            .map(|evidence| evidence.machine_id.as_str()),
        Some("m-fake"),
        "{result}"
    );
    assert!(result.contains("\"nativeMachine\""), "{result}");
    assert_eq!(machine.ran_in(), [machine.workdir.display().to_string()]);
    assert!(machine.methods().contains(&"pane.close".to_string()));
    assert_eq!(machine.runtime_dirs_left(), 0);
}
