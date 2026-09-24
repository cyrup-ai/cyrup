//! SUBA-100 placement tests: the catalog/selector/settings resolution with upstream's sentences,
//! the step > call > agent precedence, the launch-time refusals, the native launch serialization
//! refusals, and the placed native run END TO END — a real `run_sync` whose child runs in a pane a
//! fake herdr opened on a "machine" reached through a fake ssh, with its events relayed back.
//!
//! The fakes are real processes and real sockets, not mocks: the ssh is a `/bin/sh` script that
//! parses the hardened argv and runs the remote command under a separate `$HOME` (or, for `-L`,
//! links the forwarded socket); the remote `herdr` is a script answering `status server --json`;
//! the herdr API is a `UnixListener` speaking herdr's one-line-per-connection framing, whose
//! `pane.send_input` runs the typed command in a real `sh` inside the "remote" home.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::resolve::{
    MachineCatalogSource, MachineSettingsEntry, MachineSettingsSource, PlacementAgent,
    PlacementResolver, apply_call_machine, fold_step_placement,
    format_herdr_machine_runner_unsupported, resolve_graph_placements, resolve_single_placement,
    validate_optional_machine,
};
use super::{HerdrMachineReference, MachineProvider, StepPlacement};
use crate::runner::contract::AdapterId;
use crate::runner::{AgentRunnerConfig, ExternalCliRunner};

const CATALOG: &str = r#"{"machines":[
    {"id":"m-1","label":"gpu-box","target":"me@gpu","enabled":true},
    {"id":"m-2","label":"twin","target":"me@twin-a"},
    {"id":"m-3","label":"twin","target":"me@twin-b"},
    {"id":"m-4","label":"cold","target":"me@cold","enabled":false},
    {"id":"m-5","target":"-oProxyCommand=x"},
    {"id":"m-6","label":"sessioned","target":"me@s","session":"work"}
]}"#;

fn entry(cwd: Option<&str>) -> MachineSettingsSource {
    MachineSettingsSource::Entry(cwd.map(|cwd| MachineSettingsEntry {
        cwd: Some(cwd.to_string()),
        env: None,
    }))
}

async fn resolve(
    machine: &str,
    step_cwd: Option<&str>,
    settings: MachineSettingsSource,
) -> Result<HerdrMachineReference, String> {
    PlacementResolver::new(settings)
        .with_catalog(MachineCatalogSource::Json(CATALOG.to_string()))
        .resolve(machine, step_cwd)
        .await
}

fn external(adapter: Option<AdapterId>) -> AgentRunnerConfig {
    AgentRunnerConfig::ExternalCli(ExternalCliRunner {
        adapter,
        command: "claude".to_string(),
        args: Vec::new(),
        prompt_delivery_stdin: false,
        capabilities: None,
    })
}

fn job_runner() -> AgentRunnerConfig {
    AgentRunnerConfig::ExternalJob(crate::runner::ExternalJobRunner {
        provider: "remote-queue".to_string(),
        options: None,
    })
}

// ------------------------------------------------------------------------------------------------
// Validation + catalog selection (upstream's sentences)
// ------------------------------------------------------------------------------------------------

/// `validateOptionalMachine` (`agents.ts:979` @v0.68.0): `false`/absent clear, a string is trimmed,
/// anything else refused with the label.
#[test]
fn the_optional_machine_validator_speaks_upstreams_sentences() {
    let label = "Agent 'w' frontmatter 'machine'";
    assert_eq!(validate_optional_machine(None, label), Ok(None));
    assert_eq!(
        validate_optional_machine(Some(&json!(false)), label),
        Ok(None)
    );
    assert_eq!(
        validate_optional_machine(Some(&json!("  gpu-box ")), label),
        Ok(Some("gpu-box".to_string()))
    );
    assert_eq!(
        validate_optional_machine(Some(&json!("")), label),
        Err(format!("{label} must be a non-empty string or false."))
    );
    assert_eq!(
        validate_optional_machine(Some(&json!(true)), label),
        Err(format!("{label} must be a non-empty string or false."))
    );
    assert_eq!(
        validate_optional_machine(Some(&json!("x".repeat(129))), label),
        Err(format!("{label} must be 128 characters or fewer."))
    );
    assert_eq!(
        validate_optional_machine(Some(&json!("a\u{7}b")), label),
        Err(format!("{label} contains control characters."))
    );
}

/// `selectMachine` (`herdr-machine.ts:135-146`): id first, then a unique label; ambiguous,
/// missing and disabled are refused with upstream's sentences, and an unsafe ssh target is never
/// handed to ssh.
#[tokio::test]
async fn the_catalog_selects_by_id_then_unique_label_and_refuses_the_rest() {
    let by_label = resolve("gpu-box", None, entry(Some("/srv/repo")))
        .await
        .unwrap();
    assert_eq!(
        by_label,
        HerdrMachineReference {
            provider: MachineProvider::Herdr,
            id: "m-1".to_string(),
            label: Some("gpu-box".to_string()),
            target: "me@gpu".to_string(),
            session: None,
            cwd: "/srv/repo".to_string(),
            transport: None,
            agent_dir: None,
        }
    );
    assert_eq!(
        resolve("m-2", None, entry(Some("/srv")))
            .await
            .unwrap()
            .target,
        "me@twin-a",
        "an id wins even when its label is ambiguous"
    );
    assert_eq!(
        resolve("twin", None, entry(Some("/srv")))
            .await
            .unwrap_err(),
        "Machine label 'twin' is ambiguous; use its profile ID."
    );
    assert_eq!(
        resolve("nope", None, entry(Some("/srv")))
            .await
            .unwrap_err(),
        "Herdr machine 'nope' was not found. Saved machines: gpu-box, twin, twin, m-5, sessioned. Add one with herdr machine add <target> --label <name>."
    );
    assert_eq!(
        resolve("cold", None, entry(Some("/srv")))
            .await
            .unwrap_err(),
        "Machine 'cold' is disabled. Run herdr machine enable m-4."
    );
    let unsafe_target = resolve("m-5", None, entry(Some("/srv"))).await.unwrap_err();
    assert!(
        unsafe_target.contains("m-5"),
        "an option-shaped ssh target is refused by name: {unsafe_target}"
    );
    assert_eq!(
        resolve("sessioned", None, entry(Some("/srv")))
            .await
            .unwrap()
            .session
            .as_deref(),
        Some("work")
    );
    assert_eq!(
        resolve("  ", None, entry(Some("/srv"))).await.unwrap_err(),
        "Herdr machine id or label is required."
    );
}

/// The cwd rules (`herdr-machine.ts:213-227`): an absolute or `~` step cwd is a path on the
/// machine as given; a relative one joins the configured root; no root and no absolute cwd is
/// refused, and so is any local env map.
#[tokio::test]
async fn the_remote_cwd_is_resolved_from_the_step_cwd_and_the_configured_root() {
    assert_eq!(
        resolve("gpu-box", Some("/abs/elsewhere"), entry(None))
            .await
            .unwrap()
            .cwd,
        "/abs/elsewhere"
    );
    assert_eq!(
        resolve("gpu-box", Some("~/code"), entry(None))
            .await
            .unwrap()
            .cwd,
        "~/code"
    );
    assert_eq!(
        resolve("gpu-box", Some("pkg/sub"), entry(Some("/srv/repo")))
            .await
            .unwrap()
            .cwd,
        "/srv/repo/pkg/sub"
    );
    assert_eq!(
        resolve("gpu-box", Some("pkg"), entry(None))
            .await
            .unwrap_err(),
        "No root for gpu-box in this repo. Set subagents.machines.gpu-box.cwd in .cyrup/agents/settings.json or pass an absolute cwd on that machine."
    );
    let mut env = BTreeMap::new();
    env.insert("TOKEN".to_string(), "x".to_string());
    assert_eq!(
        resolve(
            "gpu-box",
            None,
            MachineSettingsSource::Entry(Some(MachineSettingsEntry {
                cwd: Some("/srv".to_string()),
                env: Some(env)
            }))
        )
        .await
        .unwrap_err(),
        "Saved-machine environment for 'gpu-box' must be configured remotely. Remove machines.gpu-box.env and configure the remote Herdr/cyrup session instead."
    );
}

/// `readMachineSettings` over the two real settings files: project beats user field by field,
/// keyed by the typed selector, the label, or the id; a malformed entry is refused with the
/// file named; a non-empty env map is refused before anything runs.
#[tokio::test]
async fn machine_settings_are_read_from_both_scopes_project_first() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("user.json");
    let project = dir.path().join("project.json");
    std::fs::write(
        &user,
        json!({"subagents":{"machines":{"m-1":{"cwd":"/home/me/repo"}}}}).to_string(),
    )
    .unwrap();
    let files = || MachineSettingsSource::Files {
        user: user.clone(),
        project: Some(project.clone()),
    };
    // Only the user scope names it — by id, while the call typed the label.
    assert_eq!(
        resolve("gpu-box", None, files()).await.unwrap().cwd,
        "/home/me/repo"
    );
    // The project scope wins.
    std::fs::write(
        &project,
        json!({"subagents":{"machines":{"gpu-box":{"cwd":"/work/repo"}}}}).to_string(),
    )
    .unwrap();
    assert_eq!(
        resolve("gpu-box", None, files()).await.unwrap().cwd,
        "/work/repo"
    );
    // A malformed record names its file and key.
    std::fs::write(
        &project,
        json!({"subagents":{"machines":{"gpu-box":{"cwd":""}}}}).to_string(),
    )
    .unwrap();
    assert_eq!(
        resolve("gpu-box", None, files()).await.unwrap_err(),
        format!(
            "Subagent settings in '{}' have invalid 'machines.gpu-box.cwd'; expected a non-empty string.",
            project.display()
        )
    );
    std::fs::write(
        &project,
        json!({"subagents":{"machines":{"gpu-box":{"cwd":"/w","env":{"K":"v"}}}}}).to_string(),
    )
    .unwrap();
    assert!(
        resolve("gpu-box", None, files())
            .await
            .unwrap_err()
            .contains("set 'machines.gpu-box.env'. Saved-machine runs use the remote Herdr/cyrup environment"),
    );
    std::fs::write(&project, json!({"subagents":{"machines":[]}}).to_string()).unwrap();
    assert_eq!(
        resolve("gpu-box", None, files()).await.unwrap_err(),
        format!(
            "Subagent settings in '{}' have invalid 'machines'; expected an object keyed by machine label or id.",
            project.display()
        )
    );
}

/// The catalog is read from the herdr binary itself (`herdr machine list --json`) through the
/// ladder the launch uses, and its failures are upstream's sentences.
#[tokio::test]
async fn the_catalog_is_read_from_the_herdr_binary() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("herdr");
    write_executable(
        &bin,
        &format!(
            "#!/bin/sh\n[ \"$1 $2 $3\" = 'machine list --json' ] || exit 9\ncat <<'EOF'\n{CATALOG}\nEOF\n"
        ),
    );
    let mut resolver = PlacementResolver::new(entry(Some("/srv/repo"))).with_catalog(
        MachineCatalogSource::Herdr {
            bin: Some(bin.display().to_string()),
        },
    );
    assert_eq!(resolver.resolve("gpu-box", None).await.unwrap().id, "m-1");
    let missing = dir.path().join("no-herdr");
    let mut resolver =
        PlacementResolver::new(entry(Some("/srv"))).with_catalog(MachineCatalogSource::Herdr {
            bin: Some(missing.display().to_string()),
        });
    assert_eq!(
        resolver.resolve("gpu-box", None).await.unwrap_err(),
        format!(
            "Herdr CLI '{}' was not found on PATH. Saved-machine placement needs Herdr installed locally.",
            missing.display()
        )
    );
}

// ------------------------------------------------------------------------------------------------
// Launch refusals + precedence
// ------------------------------------------------------------------------------------------------

/// `formatHerdrMachineRunnerUnsupported` (`herdr-machine.ts:231-248`): job runners, generic
/// external commands and managed worktrees are refused before any run exists; native and the
/// owned external profiles are placeable.
#[test]
fn unsupported_runners_and_worktrees_are_refused_before_launch() {
    let job = job_runner();
    assert_eq!(
        format_herdr_machine_runner_unsupported(Some("gpu-box"), "w", Some(&job), false).as_deref(),
        Some(
            "Agent 'w' requested machine 'gpu-box', but this runner cannot use pane-native Herdr placement. Use native cyrup or a built-in Claude, Codex, or Cursor profile."
        )
    );
    assert_eq!(
        format_herdr_machine_runner_unsupported(Some("gpu-box"), "w", Some(&external(None)), false)
            .as_deref(),
        Some(
            "Agent 'w' requested machine 'gpu-box', but generic external-cli commands cannot be remote-wrapped safely. Use claude-code, claude-code-writer, codex-exec, codex-exec-writer, cursor-agent, or cursor-agent-writer."
        )
    );
    assert_eq!(
        format_herdr_machine_runner_unsupported(Some("gpu-box"), "w", None, true).as_deref(),
        Some(
            "Agent 'w' requested machine 'gpu-box', but managed worktrees are local git operations and cannot be combined with a Herdr saved machine."
        )
    );
    assert_eq!(
        format_herdr_machine_runner_unsupported(Some("gpu-box"), "w", None, false),
        None
    );
    assert_eq!(
        format_herdr_machine_runner_unsupported(
            Some("gpu-box"),
            "w",
            Some(&external(Some(AdapterId::ClaudeCode))),
            false
        ),
        None
    );
    assert_eq!(
        format_herdr_machine_runner_unsupported(None, "w", Some(&job), true),
        None,
        "no machine, no refusal"
    );
}

fn spec(agent: &str) -> crate::spawn::chain_graph::SingleStepSpec {
    crate::extension::testsupport::bare_single_step(agent, "t")
}

fn json_resolver() -> PlacementResolver {
    PlacementResolver::new(entry(Some("/srv/repo")))
        .with_catalog(MachineCatalogSource::Json(CATALOG.to_string()))
}

/// pi `s.machine ?? launchMachine ?? a.machine` (`async-execution.ts:990` @v0.68.0): the step's
/// own machine beats the call's, which beats the agent's; the step cwd (else the call's
/// `machineCwd`) moves INTO the resolved reference and leaves the step.
#[tokio::test]
async fn step_beats_call_beats_agent_and_the_cwd_moves_onto_the_machine() {
    let agent = PlacementAgent {
        name: "w",
        machine: Some("m-6"),
        runner: None,
    };
    let mut resolver = json_resolver();

    let mut by_agent = spec("w");
    fold_step_placement(&mut by_agent, agent, None, None, false, &mut resolver)
        .await
        .unwrap();
    assert_eq!(by_agent.machine.as_ref().unwrap().requested, "m-6");
    assert_eq!(
        by_agent
            .machine
            .as_ref()
            .unwrap()
            .resolved
            .as_ref()
            .unwrap()
            .id,
        "m-6"
    );

    let mut by_call = spec("w");
    fold_step_placement(
        &mut by_call,
        agent,
        Some("m-2"),
        Some("pkg"),
        false,
        &mut resolver,
    )
    .await
    .unwrap();
    let resolved = by_call.machine.as_ref().unwrap().resolved.clone().unwrap();
    assert_eq!(
        (resolved.id.as_str(), resolved.cwd.as_str()),
        ("m-2", "/srv/repo/pkg")
    );

    let mut by_step = spec("w");
    by_step.machine = Some(StepPlacement::requested("gpu-box"));
    by_step.cwd = Some(PathBuf::from("/abs/on/box"));
    fold_step_placement(&mut by_step, agent, Some("m-2"), None, false, &mut resolver)
        .await
        .unwrap();
    let resolved = by_step.machine.as_ref().unwrap().resolved.clone().unwrap();
    assert_eq!(
        (resolved.id.as_str(), resolved.cwd.as_str()),
        ("m-1", "/abs/on/box")
    );
    assert_eq!(
        by_step.cwd, None,
        "a placed step's cwd never reaches a local resolution"
    );

    let mut unplaced = spec("w");
    fold_step_placement(
        &mut unplaced,
        PlacementAgent {
            machine: None,
            ..agent
        },
        None,
        None,
        false,
        &mut resolver,
    )
    .await
    .unwrap();
    assert_eq!(unplaced.machine, None);

    // A worktree group refuses with the agent's name before anything resolves.
    let mut in_worktree = spec("w");
    assert_eq!(
        fold_step_placement(&mut in_worktree, agent, None, None, true, &mut resolver)
            .await
            .unwrap_err(),
        "Agent 'w' requested machine 'm-6', but managed worktrees are local git operations and cannot be combined with a Herdr saved machine."
    );
}

/// The call rung reaches every step of a graph at the tool boundary; the agent rung and the
/// resolution happen at launch over the whole graph, and the first refusal stops the launch.
#[tokio::test]
async fn a_graph_launch_resolves_every_member_and_refuses_the_first_bad_one() {
    use crate::spawn::chain_graph::{ParallelGroupSpec, RunnerStep};
    let mut first = spec("native");
    first.machine = Some(StepPlacement::requested("gpu-box"));
    let group: ParallelGroupSpec = serde_json::from_value(json!({
        "steps": [serde_json::to_value(spec("native")).unwrap(), serde_json::to_value(spec("plain")).unwrap()],
        "concurrency": 2,
        "failFast": false,
        "worktree": false,
    }))
    .unwrap_or_else(|error| panic!("parallel group shape: {error}"));
    let mut graph = vec![
        RunnerStep::SingleStep(first),
        RunnerStep::ParallelGroup(group),
    ];
    apply_call_machine(&mut graph, Some("m-2"), Some("/call/cwd"));
    let RunnerStep::SingleStep(head) = &graph[0] else {
        panic!("shape")
    };
    assert_eq!(
        head.machine.as_ref().unwrap().requested,
        "gpu-box",
        "the step keeps its own"
    );
    assert_eq!(head.cwd.as_deref(), Some(Path::new("/call/cwd")));
    let mut resolver = json_resolver();
    resolve_graph_placements(
        &mut graph,
        |_| {
            Some(PlacementAgent {
                name: "native",
                machine: None,
                runner: None,
            })
        },
        None,
        None,
        &mut resolver,
    )
    .await
    .unwrap();
    let RunnerStep::ParallelGroup(group) = &graph[1] else {
        panic!("shape")
    };
    for member in &group.steps {
        let resolved = member.machine.as_ref().unwrap().resolved.as_ref().unwrap();
        assert_eq!(
            (resolved.id.as_str(), resolved.cwd.as_str()),
            ("m-2", "/call/cwd")
        );
    }

    let job = job_runner();
    let mut refused = vec![RunnerStep::SingleStep(spec("jobber"))];
    let error = resolve_graph_placements(
        &mut refused,
        |_| {
            Some(PlacementAgent {
                name: "jobber",
                machine: Some("gpu-box"),
                runner: Some(&job),
            })
        },
        None,
        None,
        &mut json_resolver(),
    )
    .await
    .unwrap_err();
    assert!(
        error.contains("this runner cannot use pane-native Herdr placement"),
        "{error}"
    );
}

/// A SINGLE run: `params.machine ?? a.machine`. A blank machine never places "nothing": the tool
/// boundary refuses it first (upstream's schema `minLength: 1`), and the resolver refuses it too.
#[tokio::test]
async fn a_single_run_takes_the_call_machine_over_the_agents() {
    let agent = PlacementAgent {
        name: "w",
        machine: Some("gpu-box"),
        runner: None,
    };
    let placed = resolve_single_placement(agent, None, None, &mut json_resolver())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(placed.resolved.unwrap().id, "m-1");
    let placed = resolve_single_placement(agent, Some("m-2"), Some("~/x"), &mut json_resolver())
        .await
        .unwrap()
        .unwrap();
    let resolved = placed.resolved.unwrap();
    assert_eq!(
        (resolved.id.as_str(), resolved.cwd.as_str()),
        ("m-2", "~/x")
    );
    assert_eq!(
        resolve_single_placement(agent, Some(""), None, &mut json_resolver()).await,
        Err("Herdr machine id or label is required.".to_string())
    );
}

/// The placement survives the hop-2 boundary on the step; the transport (local state) does not.
#[test]
fn a_step_placement_round_trips_the_runner_config_without_its_transport() {
    let mut step = spec("w");
    step.machine = Some(StepPlacement {
        requested: "gpu-box".to_string(),
        resolved: Some(HerdrMachineReference {
            provider: MachineProvider::Herdr,
            id: "m-1".to_string(),
            label: Some("gpu-box".to_string()),
            target: "me@gpu".to_string(),
            session: None,
            cwd: "/srv".to_string(),
            transport: Some(cyrup_herdr::remote::SshTransport::new("/opt/ssh", None)),
            agent_dir: Some(PathBuf::from("/home/me/.cyrup/agent")),
        }),
    });
    let value = serde_json::to_value(&step).unwrap();
    assert_eq!(
        value["machine"],
        json!({"requested":"gpu-box","resolved":{"provider":"herdr","id":"m-1","label":"gpu-box","target":"me@gpu","cwd":"/srv"}})
    );
    let back: crate::spawn::chain_graph::SingleStepSpec = serde_json::from_value(value).unwrap();
    let resolved = back.machine.unwrap().resolved.unwrap();
    assert_eq!((resolved.transport, resolved.agent_dir), (None, None));
    let unplaced = serde_json::to_value(spec("w")).unwrap();
    assert!(
        unplaced.get("machine").is_none(),
        "an unplaced step writes no key"
    );
}

// ------------------------------------------------------------------------------------------------
// Entry paths: frontmatter, management config, serializer, runtime definition, chain files
// ------------------------------------------------------------------------------------------------

#[test]
fn frontmatter_runtime_definitions_and_chain_files_carry_the_machine() {
    use crate::discovery::types::AgentSource;
    let path = Path::new("/agents/w.md");
    let parsed = crate::discovery::frontmatter::parse_agent_file_checked(
        "---\nname: w\ndescription: d\nmachine: gpu-box\n---\nbody\n",
        AgentSource::Project,
        path,
    )
    .unwrap()
    .unwrap();
    assert_eq!(parsed.machine.as_deref(), Some("gpu-box"));
    let refused = crate::discovery::frontmatter::parse_agent_file_checked(
        &format!(
            "---\nname: w\ndescription: d\nmachine: {}\n---\nbody\n",
            "x".repeat(129)
        ),
        AgentSource::Project,
        path,
    )
    .unwrap_err();
    assert_eq!(
        refused.error,
        "Agent 'w' frontmatter 'machine' must be 128 characters or fewer."
    );

    // A runtime definition carries it onto the agent it registers.
    let registry = Arc::new(crate::discovery::runtime_registry::RuntimeAgentRegistry::new());
    registry
        .register_value(
            "remote-worker",
            &json!({"description":"d","systemPrompt":"p","machine":"gpu-box"}),
        )
        .unwrap();
    assert_eq!(registry.list()[0].machine.as_deref(), Some("gpu-box"));
    let refused = registry
        .register_value(
            "bad",
            &json!({"description":"d","systemPrompt":"p","machine":7}),
        )
        .unwrap_err()
        .to_string();
    assert!(refused.contains("machine"), "{refused}");

    // A `.chain.md` step's `machine:` line lands on the runner step.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("placed.chain.md"),
        "---\nname: placed\ndescription: d\n---\n\n## w\nmachine: gpu-box\n\nDo the thing.\n",
    )
    .unwrap();
    let scanned = crate::discovery::chains::scan_chain_dir(dir.path(), AgentSource::Project);
    let chain = scanned
        .chains
        .iter()
        .find(|chain| chain.name == "placed")
        .unwrap_or_else(|| panic!("chain parsed: {:?}", scanned.diagnostics));
    let step = crate::discovery::chains::chain_step_to_runner_step(&chain.steps[0], 4);
    let crate::spawn::chain_graph::RunnerStep::SingleStep(step) = step else {
        panic!("a single step")
    };
    assert_eq!(step.machine, Some(StepPlacement::requested("gpu-box")));
}

/// The management surface: `config.machine` on create writes the frontmatter key, `get` and
/// `list` show the placement, `update` with `false` removes it, and junk is refused with
/// upstream's label.
#[tokio::test]
async fn the_management_surface_creates_shows_and_clears_a_machine() {
    use crate::discovery::management::{ManagementRequest, handle_management_action};
    let tmp = tempfile::tempdir().unwrap();
    let cfg = crate::discovery::AgentDiscoveryConfig {
        user_agent_dirs: vec![tmp.path().join("user/agents")],
        user_chain_dirs: vec![tmp.path().join("user/chains")],
        project_agent_dirs: vec![tmp.path().join("project/agents")],
        project_chain_dirs: vec![tmp.path().join("project/chains")],
        ..crate::discovery::AgentDiscoveryConfig::default()
    };
    let request = |agent: Option<&'static str>, config: Option<&'static serde_json::Value>| {
        ManagementRequest {
            agent,
            chain_name: None,
            agent_scope: None,
            config,
            current_session_model: None,
            proactive_skills: None,
        }
    };
    let leak =
        |value: serde_json::Value| -> &'static serde_json::Value { Box::leak(Box::new(value)) };

    let created = handle_management_action(
        &cfg,
        "create",
        &request(
            None,
            Some(leak(json!({"name":"remote","description":"Remote worker","scope":"project","machine":" gpu-box "}))),
        ),
    )
    .await
    .unwrap();
    assert!(!created.is_error, "{}", created.text);
    let file = tmp.path().join("project/agents/remote.md");
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.contains("\nmachine: gpu-box\n"), "{written}");

    let got = handle_management_action(&cfg, "get", &request(Some("remote"), None))
        .await
        .unwrap();
    assert!(
        got.text
            .contains("Machine: gpu-box (saved Herdr placement)"),
        "{}",
        got.text
    );
    let listed = handle_management_action(&cfg, "list", &request(None, None))
        .await
        .unwrap();
    assert!(
        listed.text.lines().any(|line| line.contains("remote")
            && line.contains("machine: gpu-box (saved Herdr placement)")),
        "{}",
        listed.text
    );

    let cleared = handle_management_action(
        &cfg,
        "update",
        &request(Some("remote"), Some(leak(json!({"machine": false})))),
    )
    .await
    .unwrap();
    assert!(!cleared.is_error, "{}", cleared.text);
    assert!(!std::fs::read_to_string(&file).unwrap().contains("machine:"));

    let refused = handle_management_action(
        &cfg,
        "update",
        &request(Some("remote"), Some(leak(json!({"machine": 7})))),
    )
    .await
    .unwrap();
    assert!(refused.is_error);
    assert!(
        refused
            .text
            .contains("config.machine must be a non-empty string or false."),
        "{}",
        refused.text
    );
}

// ------------------------------------------------------------------------------------------------
// Native launch serialization refusals
// ------------------------------------------------------------------------------------------------

fn child_spec(args: &[&str], env: &[(&str, &str)]) -> crate::spawn::ChildSpawnSpec {
    crate::spawn::ChildSpawnSpec {
        command: crate::spawn::SpawnCommand {
            binary: PathBuf::from("/usr/local/bin/cyrup"),
            base_args: Vec::new(),
        },
        args: args.iter().map(|a| (*a).to_string()).collect(),
        task_arg: "Task: do it".to_string(),
        env_overlay: env
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
        cwd: PathBuf::from("/local"),
        temp_files: Vec::new(),
    }
}

/// `serializeHerdrPiLaunch` (`herdr-placed-run.ts:88-99` @v0.68.0): what the machine cannot honour
/// is refused with upstream's sentence before anything is touched; files the child reads are
/// uploaded, files it writes are fetched back, the persona is NOT shipped (the launch names its
/// logical resources instead, tool ceiling read off `--tools`), and the four live channels point
/// into the run directory and are mirrored.
#[test]
fn the_native_launch_serializes_for_the_machine_or_refuses() {
    use super::native::{
        CONTRACT_REFUSAL, EXTENSION_REFUSAL, FORK_REFUSAL, NESTED_REFUSAL, PlacedLaunchContext,
        serialize_placed_launch,
    };
    use super::remote_resources::{RESOURCE_NAMES_REFUSAL, RESOURCES_ENV, RemoteResources};
    let resources = RemoteResources {
        agent: "worker".to_string(),
        skills: Some(vec!["review".to_string()]),
        tool_ceiling: None,
        reads: None,
    };
    let ctx = PlacedLaunchContext {
        child_depth: 1,
        resources: resources.clone(),
    };
    assert_eq!(
        serialize_placed_launch(&child_spec(&["--session", "/s.jsonl"], &[]), ctx.clone())
            .unwrap_err(),
        FORK_REFUSAL
    );
    assert_eq!(
        serialize_placed_launch(&child_spec(&["--extension", "x"], &[]), ctx.clone()).unwrap_err(),
        EXTENSION_REFUSAL
    );
    assert_eq!(
        serialize_placed_launch(
            &child_spec(&[], &[]),
            PlacedLaunchContext {
                child_depth: 2,
                resources: resources.clone(),
            }
        )
        .unwrap_err(),
        NESTED_REFUSAL
    );
    assert_eq!(
        serialize_placed_launch(
            &child_spec(&[], &[(crate::exec::tool_budget::TOOL_BUDGET_ENV, "{}")]),
            ctx.clone()
        )
        .unwrap_err(),
        CONTRACT_REFUSAL
    );
    assert_eq!(
        serialize_placed_launch(
            &child_spec(&[], &[("MCP_DIRECT_TOOLS", "srv/tool")]),
            ctx.clone()
        )
        .unwrap_err(),
        CONTRACT_REFUSAL
    );
    // A resource name outside upstream's bounds refuses the launch.
    assert_eq!(
        serialize_placed_launch(
            &child_spec(&[], &[]),
            PlacedLaunchContext {
                child_depth: 1,
                resources: RemoteResources {
                    agent: "../escape".to_string(),
                    ..resources.clone()
                },
            }
        )
        .unwrap_err(),
        RESOURCE_NAMES_REFUSAL
    );
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().display().to_string();
    assert!(
        serialize_placed_launch(&child_spec(&[], &[("SOME_DIR", &local)]), ctx.clone())
            .unwrap_err()
            .contains("cannot transfer the local path in child contract 'SOME_DIR'")
    );

    let prompt = dir.path().join("prompt.md");
    std::fs::write(&prompt, "persona").unwrap();
    let capture = dir.path().join("capture.json");
    let supervisor = dir.path().join("supervisor");
    let inbox = dir.path().join("steer-targets");
    let acks = dir.path().join("steer-acks");
    let capability = dir.path().join("capability.json");
    let launch = serialize_placed_launch(
        &child_spec(
            &[
                "--mode",
                "json",
                "--tools",
                "read,bash",
                "--system-prompt",
                &prompt.display().to_string(),
            ],
            &[
                ("MCP_DIRECT_TOOLS", "__none__"),
                (
                    crate::exec::structured::STRUCTURED_OUTPUT_CAPTURE_ENV,
                    &capture.display().to_string(),
                ),
                (
                    crate::exec::spawn_plan::PARENT_SESSION_ENV_VAR,
                    "/local/parent.jsonl",
                ),
                (crate::prompt_runtime::INHERIT_PROJECT_CONTEXT_ENV, "0"),
                (
                    crate::spawn::intercom_target::ENV_ORCHESTRATOR_SESSION_ID,
                    "parent-session",
                ),
                (
                    crate::spawn::intercom_target::ENV_SUPERVISOR_CHANNEL_DIR,
                    &supervisor.display().to_string(),
                ),
                (
                    crate::prompt_runtime::STEER_INBOX_ENV,
                    &inbox.display().to_string(),
                ),
                (
                    crate::prompt_runtime::STEER_ACK_DIR_ENV,
                    &acks.display().to_string(),
                ),
                (
                    crate::prompt_runtime::STEER_CAPABILITY_ENV,
                    &capability.display().to_string(),
                ),
            ],
        ),
        ctx,
    )
    .unwrap();
    // The persona file never crosses: the machine resolves the agent's own prompt.
    assert_eq!(
        launch.argv,
        ["--mode", "json", "--tools", "read,bash", "Task: do it"]
    );
    assert!(launch.uploads.is_empty(), "{:?}", launch.uploads);
    assert_eq!(launch.fetches[0].local, capture);
    let sent: RemoteResources = serde_json::from_str(&launch.env[RESOURCES_ENV]).unwrap();
    assert_eq!(
        sent,
        RemoteResources {
            tool_ceiling: Some(vec!["read".to_string(), "bash".to_string()]),
            ..resources
        }
    );
    assert_eq!(
        launch
            .env
            .get(crate::exec::structured::STRUCTURED_OUTPUT_CAPTURE_ENV)
            .map(String::as_str),
        Some("{{RT}}/structured-output.json")
    );
    assert!(
        !launch
            .env
            .contains_key(crate::exec::spawn_plan::PARENT_SESSION_ENV_VAR)
    );
    // The launching side's context policy does not travel: the machine's agent brings its own.
    assert!(
        !launch
            .env
            .contains_key(crate::prompt_runtime::INHERIT_PROJECT_CONTEXT_ENV)
    );
    assert_eq!(
        launch.env[crate::spawn::intercom_target::ENV_ORCHESTRATOR_SESSION_ID],
        "parent-session"
    );
    // The four channels point into the run dir and are mirrored with the parent's.
    assert_eq!(
        launch.env[crate::spawn::intercom_target::ENV_SUPERVISOR_CHANNEL_DIR],
        "{{RT}}/supervisor"
    );
    assert_eq!(
        launch.env[crate::prompt_runtime::STEER_INBOX_ENV],
        "{{RT}}/steer-inbox"
    );
    assert_eq!(
        launch.env[crate::prompt_runtime::STEER_ACK_DIR_ENV],
        "{{RT}}/steer-acks"
    );
    assert_eq!(
        launch.env[crate::prompt_runtime::STEER_CAPABILITY_ENV],
        "{{RT}}/steer-capability.json"
    );
    let downs: Vec<(&str, &Path)> = launch
        .down
        .iter()
        .map(|mirror| (mirror.remote.as_str(), mirror.local.as_path()))
        .collect();
    assert!(downs.contains(&("supervisor/requests", supervisor.join("requests").as_path())));
    assert!(downs.contains(&("steer-acks", acks.as_path())));
    assert!(downs.contains(&("steer-capability.json", capability.as_path())));
    let ups: Vec<(&Path, &str)> = launch
        .up
        .iter()
        .map(|mirror| (mirror.local.as_path(), mirror.remote.as_str()))
        .collect();
    assert!(ups.contains(&(supervisor.join("replies").as_path(), "supervisor/replies")));
    assert!(ups.contains(&(inbox.as_path(), "steer-inbox")));
    let (_, env) = launch.bind("/tmp/cyrup-subagents-herdr-r-1");
    assert_eq!(
        env[crate::exec::structured::STRUCTURED_OUTPUT_CAPTURE_ENV],
        "/tmp/cyrup-subagents-herdr-r-1/structured-output.json"
    );
    assert_eq!(
        env[crate::prompt_runtime::STEER_INBOX_ENV],
        "/tmp/cyrup-subagents-herdr-r-1/steer-inbox"
    );
}

/// pi's placed-run mutation evidence (`execution.ts:1502-1503` @v0.68.0): remote Git before vs
/// after, never the local checkout.
#[test]
fn placed_mutation_evidence_is_the_remote_git_pair() {
    use super::native::{
        LOCAL_GIT_NOT_AUTHORITATIVE, NativeMachineEvidence, REMOTE_GIT_INCOMPLETE, RemoteGitStatus,
        placed_mutation_evidence, placed_mutation_snapshot,
    };
    let git = |head: &str, dirty: bool| RemoteGitStatus {
        head: Some(head.to_string()),
        branch: Some("main".to_string()),
        dirty: Some(dirty),
    };
    let evidence = |initial, final_git| NativeMachineEvidence {
        initial_git: initial,
        final_git,
        ..NativeMachineEvidence::new("m-1")
    };
    let changed = placed_mutation_evidence(Some(&evidence(
        Some(git("abcd", false)),
        Some(git("abcd", true)),
    )));
    assert!(changed.attempted_mutation && changed.unavailable.is_none());
    let same = placed_mutation_evidence(Some(&evidence(
        Some(git("abcd", false)),
        Some(git("abcd", false)),
    )));
    assert!(!same.attempted_mutation && same.unavailable.is_none());
    let partial = placed_mutation_evidence(Some(&evidence(Some(git("abcd", false)), None)));
    assert_eq!(partial.unavailable.as_deref(), Some(REMOTE_GIT_INCOMPLETE));
    assert_eq!(
        placed_mutation_evidence(None).unavailable.as_deref(),
        Some(LOCAL_GIT_NOT_AUTHORITATIVE)
    );
    assert_eq!(
        placed_mutation_snapshot(Path::new("/local"))
            .unavailable
            .as_deref(),
        Some(LOCAL_GIT_NOT_AUTHORITATIVE)
    );
}

// ------------------------------------------------------------------------------------------------
// End to end: a placed native run through `run_sync`, against a fake machine
// ------------------------------------------------------------------------------------------------

fn write_executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A fake saved machine: an ssh that "reaches" a separate `$HOME`, that home's `herdr`, and the
/// herdr API server its panes run under.
pub(crate) struct FakeMachine {
    pub(crate) dir: tempfile::TempDir,
    pub(crate) ssh: PathBuf,
    pub(crate) home: PathBuf,
    pub(crate) workdir: PathBuf,
    pub(crate) remote_tmp: PathBuf,
    pub(crate) calls: Arc<Mutex<Vec<serde_json::Value>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for FakeMachine {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[derive(Default)]
struct FakeState {
    next: usize,
    workspaces: Vec<serde_json::Value>,
    panes: Vec<serde_json::Value>,
    children: BTreeMap<String, tokio::process::Child>,
    /// The agent `agent.start` started (an external profile), as herdr reports it.
    agent: Option<serde_json::Value>,
    /// When set, the FIRST `agent.prompt` loses the transport: every `-N` forward is torn down
    /// and the request is dropped unanswered.
    drop_first_prompt: bool,
    prompts: usize,
    /// What `pane.read` shows once the external agent has answered.
    pane_text: String,
}

impl FakeMachine {
    /// Start the fake. `remote_cyrup` is the body of the `cyrup` on the machine's `PATH`.
    pub(crate) fn start(remote_cyrup: &str) -> Self {
        Self::start_with(remote_cyrup, FakeState::default())
    }

    /// Start the fake with a pre-seeded herdr state (an external profile's behaviour).
    fn start_with(remote_cyrup: &str, seed: FakeState) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let home = root.join("remote-home");
        let bin = home.join(".local").join("bin");
        let workdir = home.join("repo");
        let remote_tmp = root.join("remote-tmp");
        for path in [&bin, &workdir, &remote_tmp] {
            std::fs::create_dir_all(path).unwrap();
        }
        let socket = root.join("herdr-api.sock");
        write_executable(
            &bin.join("herdr"),
            &format!(
                "#!/bin/sh\n[ \"$1 $2 $3\" = 'status server --json' ] || exit 9\nprintf '%s\\n' '{}'\n",
                json!({
                    "status":"running","running":true,"version":"0.9.1","protocol":7,
                    "compatible":true,"endpoint_compatible":true,
                    "socket": socket.display().to_string(),"session":null
                })
            ),
        );
        write_executable(&bin.join("cyrup"), remote_cyrup);
        // The machine's Claude Code: a canonical binary above the pane-native floor that
        // documents every interactive option the profile's argv uses.
        write_executable(
            &bin.join("claude"),
            "#!/bin/sh\ncase \"$1\" in --version) echo '2.1.300 (Claude Code)';; --help) echo 'Usage: claude --session-id --restricted --permission-mode --tools --strict-mcp-config --mcp-config --disable-slash-commands --no-chrome';; esac\n",
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
  printf '%s %s\n' "$$" "${{fwd%%:*}}" >> '{forwards}'
  exec sleep 3600
fi
cd '{home}' || exit 255
HOME='{home}' TMPDIR='{tmp}' exec /bin/sh -c "$1"
"#,
                home = home.display(),
                tmp = remote_tmp.display(),
                forwards = root.join("forwards").display()
            ),
        );
        let calls = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let state = Arc::new(tokio::sync::Mutex::new(seed));
        let server = {
            let calls = Arc::clone(&calls);
            let home = home.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let calls = Arc::clone(&calls);
                    let state = Arc::clone(&state);
                    let home = home.clone();
                    tokio::spawn(async move {
                        serve_one(stream, calls, state, home).await;
                    });
                }
            })
        };
        Self {
            dir,
            ssh,
            home,
            workdir,
            remote_tmp,
            calls,
            server,
        }
    }

    /// The machine, as a launch would have resolved it (with this fake's ssh as its transport).
    pub(crate) fn reference(&self) -> HerdrMachineReference {
        HerdrMachineReference {
            provider: MachineProvider::Herdr,
            id: "m-fake".to_string(),
            label: Some("fake-box".to_string()),
            target: "me@fake-box".to_string(),
            session: None,
            cwd: self.workdir.display().to_string(),
            transport: Some(cyrup_herdr::remote::SshTransport::new(
                self.ssh.display().to_string(),
                None,
            )),
            agent_dir: Some(self.dir.path().join("agent")),
        }
    }

    pub(crate) fn methods(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call["method"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// Every runtime dir still present on the machine.
    pub(crate) fn runtime_dirs(&self) -> Vec<PathBuf> {
        std::fs::read_dir(&self.remote_tmp)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(super::native::RUNTIME_DIR_PREFIX))
            })
            .collect()
    }
}

fn pane_json(pane_id: &str, workspace_id: &str, tab_id: &str, cwd: &str) -> serde_json::Value {
    json!({"pane_id":pane_id,"terminal_id":format!("t-{pane_id}"),"workspace_id":workspace_id,
        "tab_id":tab_id,"focused":false,"cwd":cwd,"agent_status":"idle","revision":1})
}

async fn serve_one(
    stream: tokio::net::UnixStream,
    calls: Arc<Mutex<Vec<serde_json::Value>>>,
    state: Arc<tokio::sync::Mutex<FakeState>>,
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
    let id = request["id"].clone();
    let params = &request["params"];
    let mut state = state.lock().await;
    let result: Result<serde_json::Value, (&str, String)> = match request["method"]
        .as_str()
        .unwrap_or_default()
    {
        "session.snapshot" => Ok(json!({"type":"session_snapshot","snapshot":{
            "version":"0.9.1","protocol":7,"workspaces":state.workspaces,"tabs":[],
            "panes":state.panes,"layouts":[],"agents":state.agent.iter().collect::<Vec<_>>()}})),
        "agent.start" => {
            let pane_id = params["pane_id"].as_str().unwrap_or_default().to_string();
            let pane = state
                .panes
                .iter()
                .find(|pane| pane["pane_id"] == pane_id.as_str())
                .cloned()
                .unwrap_or_default();
            let agent = json!({"terminal_id":format!("t-{pane_id}"),"name":params["name"],
                "agent":params["kind"],"agent_status":"idle","workspace_id":pane["workspace_id"],
                "tab_id":pane["tab_id"],"pane_id":pane_id,"focused":false,
                "interactive_ready":true,"launch_pending":false,"revision":1});
            state.agent = Some(agent.clone());
            let mut argv = vec![params["kind"].clone()];
            argv.extend(params["args"].as_array().cloned().unwrap_or_default());
            Ok(json!({"type":"agent_started","agent":agent,"argv":argv}))
        }
        "agent.prompt" => {
            state.prompts += 1;
            if state.drop_first_prompt && state.prompts == 1 {
                // The transport dies under the prompt: every forward goes, and this request is
                // never answered.
                let forwards = home.parent().unwrap().join("forwards");
                for line in std::fs::read_to_string(&forwards)
                    .unwrap_or_default()
                    .lines()
                {
                    if let Some((pid, link)) = line.split_once(' ') {
                        let _ = std::fs::remove_file(link);
                        let _ = std::process::Command::new("kill")
                            .arg("-9")
                            .arg(pid)
                            .status();
                    }
                }
                return;
            }
            state
                .agent
                .clone()
                .map(|agent| json!({"type":"agent_prompted","agent":agent}))
                .ok_or(("agent_not_found", "no agent".to_string()))
        }
        "agent.get" => state
            .agent
            .clone()
            .map(|agent| json!({"type":"agent_info","agent":agent}))
            .ok_or(("agent_not_found", "no agent".to_string())),
        "pane.process_info" => {
            let pane_id = params["pane_id"].as_str().unwrap_or_default();
            Ok(
                json!({"type":"pane_process_info","process_info":{"pane_id":pane_id,
                "foreground_processes":[{"pid":4242,"name":"claude","argv0":"claude",
                    "cwd":home.join("repo").display().to_string()}]}}),
            )
        }
        "pane.read" => {
            let pane_id = params["pane_id"].as_str().unwrap_or_default();
            let pane = state
                .panes
                .iter()
                .find(|pane| pane["pane_id"] == pane_id)
                .cloned()
                .unwrap_or_default();
            Ok(json!({"type":"pane_read","read":{"pane_id":pane_id,
                "workspace_id":pane["workspace_id"],"tab_id":pane["tab_id"],
                "source":params["source"],"format":"text","text":state.pane_text,
                "revision":0,"truncated":false}}))
        }
        "workspace.create" => {
            state.next += 1;
            let n = state.next;
            let (ws, tab, pane) = (format!("w{n}"), format!("w{n}:t1"), format!("w{n}:p1"));
            let cwd = params["cwd"].as_str().unwrap_or_default().to_string();
            let workspace = json!({"workspace_id":ws,"number":n,"label":params["label"],
                "focused":false,"pane_count":1,"tab_count":1,"active_tab_id":tab,"agent_status":"idle"});
            let tab_json = json!({"tab_id":tab,"workspace_id":ws,"number":1,"label":"1",
                "focused":false,"pane_count":1,"agent_status":"idle"});
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
            let cwd = state
                .panes
                .iter()
                .find(|pane| pane["pane_id"] == pane_id.as_str())
                .and_then(|pane| pane["cwd"].as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.clone());
            let child = tokio::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(params["text"].as_str().unwrap_or_default())
                .current_dir(cwd)
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
                .ok_or(("pane_not_found", format!("pane {pane_id} not found")))
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
        other => Err(("unknown_method", format!("unexpected method {other}"))),
    };
    let reply = match result {
        Ok(result) => json!({"id":id,"result":result}),
        Err((code, message)) => json!({"id":id,"error":{"code":code,"message":message}}),
    };
    let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
    let _ = write.flush().await;
}

/// The remote `cyrup`: records where it ran and with what, then speaks the child protocol.
fn recording_cyrup(text: &str, exit: i32) -> String {
    format!(
        r#"#!/bin/sh
pwd > "$HOME/ran-in"
for a in "$@"; do printf '%s\n' "$a"; done > "$HOME/argv"
printf '%s\n' "${{CYRUP_SUBAGENT_DEPTH-unset}}" > "$HOME/depth"
printf '%s\n' '{{"type":"message_end","message":{{"role":"assistant","content":[{{"type":"text","text":"{text}"}}]}}}}'
printf '%s\n' '{{"type":"agent_settled"}}'
echo "remote diagnostics" >&2
exit {exit}
"#
    )
}

fn placed_opts(cwd: &Path, machine: &FakeMachine) -> crate::exec::RunOptions {
    let mut opts = crate::exec::testsupport::base_opts(cwd, &["m1"]);
    opts.machine = Some(machine.reference());
    // The local binary is never run for a placed child; naming one that does not exist proves it.
    opts.spawn_command = Some(crate::spawn::SpawnCommand {
        binary: PathBuf::from("/nonexistent/local-cyrup"),
        base_args: Vec::new(),
    });
    opts
}

/// THE placement contract, end to end: the child runs in a fresh Herdr-owned pane ON the
/// machine, in the machine's directory, with the native child argv; its event stream comes back
/// through the ssh relay and settles the run exactly as a local child's would; the pane is closed
/// and the run-private runtime dir removed afterwards; nothing is spawned locally.
#[tokio::test(flavor = "multi_thread")]
async fn a_placed_native_run_executes_in_a_herdr_pane_on_the_machine() {
    let machine = FakeMachine::start(&recording_cyrup("remote hello", 0));
    let local = tempfile::tempdir().unwrap();
    let agent = crate::exec::testsupport::sample_agent_config("m1", &[]);
    let result =
        crate::exec::run_sync(&agent, "Say hello", &placed_opts(local.path(), &machine)).await;

    assert_eq!(result.exit_code, 0, "{result:?}");
    assert_eq!(
        result.final_output.as_deref(),
        Some("remote hello"),
        "{result:?}"
    );
    // pi `SingleResult.nativeMachine` (`types.ts:1268`, `execution.ts:1293` @v0.68.0).
    assert_eq!(
        result
            .native_machine
            .as_ref()
            .map(|evidence| evidence.machine_id.as_str()),
        Some("m-fake"),
        "the result carries the machine's evidence"
    );
    assert_eq!(
        std::fs::read_to_string(machine.home.join("ran-in"))
            .unwrap()
            .trim(),
        machine.workdir.display().to_string(),
        "the child ran in the machine's directory"
    );
    let argv = std::fs::read_to_string(machine.home.join("argv")).unwrap();
    assert!(
        argv.lines().any(|arg| arg == "json"),
        "the native child argv crossed: {argv}"
    );
    assert!(argv.contains("Say hello"), "the task crossed: {argv}");
    assert_eq!(
        std::fs::read_to_string(machine.home.join("depth"))
            .unwrap()
            .trim(),
        "1",
        "the child's depth contract crossed"
    );
    let methods = machine.methods();
    assert_eq!(
        methods,
        [
            "session.snapshot",
            "workspace.create",
            "pane.send_input",
            "pane.get",
            "pane.close"
        ],
        "a fresh owned pane, the child typed into it, the pane closed after"
    );
    let calls = machine.calls.lock().unwrap().clone();
    assert_eq!(
        calls[1]["params"]["label"],
        super::native::owned_workspace_label(&machine.workdir.display().to_string())
    );
    assert_eq!(calls[1]["params"]["focus"], false);
    assert!(
        calls[2]["params"]["text"]
            .as_str()
            .unwrap()
            .starts_with(&format!(
                "sh {}/{}",
                machine.remote_tmp.display(),
                super::native::RUNTIME_DIR_PREFIX
            )),
        "{calls:?}"
    );
    assert_eq!(
        machine.runtime_dirs(),
        Vec::<PathBuf>::new(),
        "the runtime dir is removed"
    );
    let journal = machine.dir.path().join("agent/subagents/herdr-run-journal");
    let entries: Vec<_> = std::fs::read_dir(&journal).unwrap().collect();
    assert_eq!(
        entries.len(),
        1,
        "one placed run journaled under the launch's agent dir"
    );
    assert!(
        machine
            .dir
            .path()
            .join("agent/subagents/herdr-allocation-locks")
            .is_dir()
    );
}

/// A placed child that fails is a failure with the remote stderr and upstream's remote-failure
/// hint; its pane is still closed and its runtime dir removed.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_placed_child_reports_the_remote_error_with_the_machine_hint() {
    let machine = FakeMachine::start("#!/bin/sh\necho 'sh: 1: claude: not found' >&2\nexit 127\n");
    let local = tempfile::tempdir().unwrap();
    let agent = crate::exec::testsupport::sample_agent_config("m1", &[]);
    let result =
        crate::exec::run_sync(&agent, "Say hello", &placed_opts(local.path(), &machine)).await;
    assert_ne!(result.exit_code, 0, "{result:?}");
    let error = result.error.unwrap_or_default();
    assert!(
        error.contains("The agent CLI was not found on fake-box."),
        "the remote-failure hint names the machine: {error}"
    );
    assert!(machine.methods().contains(&"pane.close".to_string()));
    assert_eq!(machine.runtime_dirs(), Vec::<PathBuf>::new());
}

/// A launch the machine cannot honour is refused before anything is provisioned on it.
#[tokio::test(flavor = "multi_thread")]
async fn a_placed_run_with_a_local_session_is_refused_before_touching_the_machine() {
    let machine = FakeMachine::start(&recording_cyrup("unused", 0));
    let local = tempfile::tempdir().unwrap();
    let session = local.path().join("fork.jsonl");
    std::fs::write(&session, "{}\n").unwrap();
    let agent = crate::exec::testsupport::sample_agent_config("m1", &[]);
    let mut opts = placed_opts(local.path(), &machine);
    opts.fork_context = crate::fork_context::ForkContext {
        mode: crate::fork_context::ContextMode::Fork,
        session_file_path: Some(session),
        thinking_override: None,
    };
    let result = crate::exec::run_sync(&agent, "Say hello", &opts).await;
    assert_ne!(result.exit_code, 0);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains(super::native::FORK_REFUSAL),
        "{result:?}"
    );
    assert!(machine.methods().is_empty(), "nothing reached herdr");
    assert_eq!(machine.runtime_dirs(), Vec::<PathBuf>::new());
}

/// A run that times out while its placed child is still running closes the pane (which ends the
/// child) and removes the runtime dir.
#[tokio::test(flavor = "multi_thread")]
async fn a_timed_out_placed_run_closes_its_pane() {
    let machine = FakeMachine::start("#!/bin/sh\nexec sleep 30\n");
    let local = tempfile::tempdir().unwrap();
    let agent = crate::exec::testsupport::sample_agent_config("m1", &[]);
    let mut opts = placed_opts(local.path(), &machine);
    opts.timeout_ms = Some(3_000);
    opts.deadline_at = Some(std::time::Instant::now() + std::time::Duration::from_millis(3_000));
    let started = std::time::Instant::now();
    let result = crate::exec::run_sync(&agent, "Say hello", &opts).await;
    assert!(result.timed_out, "{result:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(25));
    assert!(machine.methods().contains(&"pane.close".to_string()));
    assert_eq!(machine.runtime_dirs(), Vec::<PathBuf>::new());
}

/// The live channels of a placed child (pi's bridge `supervisor-request` / `supervisor-reply` /
/// `steer` / `follow-up` frames, `herdr-pi-bridge.ts` @v0.68.0), end to end through `run_sync`:
/// the child's `contact_supervisor` request lands in the parent's LOCAL supervisor channel, the
/// parent's reply is delivered to the child ON THE MACHINE, a steer request the parent drops into
/// the child's local inbox reaches the child there, and the child's acknowledgement and capability
/// record come back to the parent's local paths.
#[tokio::test(flavor = "multi_thread")]
async fn a_placed_childs_supervisor_and_steer_channels_reach_the_parent() {
    let machine = FakeMachine::start(
        r#"#!/bin/sh
d="$CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR"
printf '{"type":"subagent.supervisor.request","id":"q1","reason":"need_decision","message":"which way?"}' > "$d/requests/q1.json"
i=0; while [ ! -f "$d/replies/q1.json" ]; do i=$((i+1)); [ $i -gt 300 ] && exit 9; sleep 0.1; done
reply=$(tr -d '"{}' < "$d/replies/q1.json")
inbox="$CYRUP_SUBAGENT_STEER_INBOX"
i=0; while ! ls "$inbox"/*.json >/dev/null 2>&1; do i=$((i+1)); [ $i -gt 300 ] && exit 8; sleep 0.1; done
f=$(ls "$inbox"/*.json | head -n 1); steer=$(tr -d '"{}' < "$f"); rm -f "$f"
printf '{"requestId":"s1","state":"delivered"}' > "$CYRUP_SUBAGENT_STEER_ACK_DIR/s1.json"
printf '{"steerable":true}' > "$CYRUP_SUBAGENT_STEER_CAPABILITY"
printf '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"reply=%s steer=%s"}]}}
' "$reply" "$steer"
printf '{"type":"agent_settled"}
'
"#,
    );
    let local = tempfile::tempdir().unwrap();
    let agent = crate::exec::testsupport::sample_agent_config("m1", &[]);
    let mut opts = placed_opts(local.path(), &machine);
    // The supervisor channel lives under the shared per-user root, keyed by run id: a run id of
    // its own keeps a concurrent or earlier run of this test out of it.
    let run_id = crate::background::RunId::from_token(format!(
        "placedchannels{}x{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    opts.run_id = Some(run_id.clone());
    opts.child_index = Some(0);
    opts.orchestrator_intercom_target = Some("orchestrator".to_string());
    opts.parent_session_id = Some("parent-session".to_string());
    let inbox = local.path().join("steer-targets/0");
    let acks = local.path().join("steer-acks/0");
    let capability = local.path().join("steer-capabilities/0.json");
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::create_dir_all(&acks).unwrap();
    std::fs::create_dir_all(capability.parent().unwrap()).unwrap();
    opts.steer_inbox_dir = Some(inbox.clone());
    opts.steer_ack_dir = Some(acks.clone());
    opts.steer_capability_path = Some(capability.clone());
    let channel =
        crate::native_supervisor::resolve_supervisor_channel_dir(run_id.as_str(), &agent.name, 0);
    // The parent: answer the request once it arrives, then steer.
    let parent = {
        let channel = channel.clone();
        let inbox = inbox.clone();
        tokio::spawn(async move {
            for _ in 0..600 {
                if let Ok(request) = std::fs::read_to_string(channel.join("requests/q1.json")) {
                    std::fs::write(channel.join("replies/q1.json"), "\"left\"").unwrap();
                    std::fs::write(inbox.join("s1.json"), "\"hurry\"").unwrap();
                    return Some(request);
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            None
        })
    };
    let result = crate::exec::run_sync(&agent, "Ask and wait", &opts).await;
    let request = parent.await.unwrap();
    assert_eq!(result.exit_code, 0, "{result:?}");
    assert_eq!(
        result.final_output.as_deref(),
        Some("reply=left steer=hurry"),
        "the reply and the steer reached the child on the machine: {result:?}"
    );
    assert!(
        request
            .as_deref()
            .is_some_and(|text| text.contains("which way?")),
        "the child's request reached the parent's local channel: {request:?}"
    );
    assert_eq!(
        std::fs::read_to_string(acks.join("s1.json")).unwrap(),
        "{\"requestId\":\"s1\",\"state\":\"delivered\"}"
    );
    assert_eq!(
        std::fs::read_to_string(&capability).unwrap(),
        "{\"steerable\":true}"
    );
    assert!(
        !inbox.join("s1.json").exists(),
        "the steer request was delivered out of the parent's inbox"
    );
    let _ = std::fs::remove_dir_all(&channel);
}

/// `resolveRemoteHerdrResources` (`herdr-pi-bridge.ts:34-45` @v0.68.0) and the bridge's
/// `before_agent_start` (`:119`), through the placed child's production entry
/// (`prompt_runtime_extension_from`, what `cyrup` builds its child runtime from): the agent is the
/// MACHINE's — its body, its `defaultReads` that exist on the machine, its tools narrowed to the
/// launch's ceiling — composed under the machine agent's own context policy with the
/// `<active_agent>` tag; the `configured` record lands in the run dir; a missing agent or skill
/// refuses the child's launch with upstream's sentence.
#[tokio::test]
async fn a_placed_child_resolves_its_agent_skills_and_reads_on_the_machine() {
    use cyrup_ext::native::{ExtMode, HostCtx, InitApi};
    use cyrup_ext::{EventKind, EventPatch, HookOutcome, HostEvent};

    use super::remote_resources::RESOURCES_ENV;

    let home = tempfile::tempdir().unwrap();
    let agents = home.path().join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("machine-worker.md"),
        "---\nname: machine-worker\ndescription: the machine's own worker\ntools: read, bash\ndefaultReads: notes.md, absent.md\n---\n\nYou are the MACHINE's worker.\n",
    )
    .unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("notes.md"), "notes").unwrap();
    let rt = tempfile::tempdir().unwrap();
    let env = |resources: &str| {
        let pairs: Vec<(String, String)> = vec![
            ("HOME".to_string(), home.path().display().to_string()),
            (RESOURCES_ENV.to_string(), resources.to_string()),
            (
                super::native::RUNTIME_DIR_ENV.to_string(),
                rt.path().display().to_string(),
            ),
        ];
        move |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    };

    let resolved = super::remote_resources::resources_from_env(
        &env(r#"{"agent":"machine-worker","toolCeiling":["read","grep"]}"#),
        cwd.path(),
    )
    .unwrap()
    .expect("a placed child resolves its resources");
    assert_eq!(resolved.agent, "machine-worker");
    assert_eq!(resolved.tools.as_deref(), Some(&["read".to_string()][..]));
    assert!(
        resolved
            .system_prompt
            .starts_with("You are the MACHINE's worker."),
        "{}",
        resolved.system_prompt
    );
    let read_line = format!("[Read from: {}]", cwd.path().join("notes.md").display());
    assert!(
        resolved.system_prompt.contains(&read_line),
        "the machine's existing defaultReads: {}",
        resolved.system_prompt
    );
    assert!(!resolved.system_prompt.contains("absent.md"));
    let configured: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(rt.path().join("configured.json")).unwrap())
            .unwrap();
    assert_eq!(configured["agent"], "machine-worker");
    assert_eq!(configured["tools"], serde_json::json!(["read"]));
    let composed = resolved.system_prompt("BASE PROMPT", None, false, false);
    assert!(
        composed.contains("BASE PROMPT")
            && composed.contains("You are a child subagent, not the parent orchestrator.")
            && composed.ends_with(&format!(
                "<active_agent name=\"machine-worker\"/>\n\n{}",
                resolved.system_prompt
            )),
        "{composed}"
    );

    // A missing agent refuses the child's launch before its first turn.
    let missing =
        crate::prompt_runtime::prompt_runtime_extension_from(&env(r#"{"agent":"ghost"}"#))
            .err()
            .expect("an unknown remote agent refuses the launch");
    assert!(
        missing.starts_with("Remote agent 'ghost' was not found unambiguously in "),
        "{missing}"
    );
    let skills = super::remote_resources::resources_from_env(
        &env(r#"{"agent":"machine-worker","skills":["nosuch-skill"]}"#),
        cwd.path(),
    )
    .unwrap_err();
    assert_eq!(
        skills,
        "Remote cyrup skills not found: nosuch-skill. Configure them on the saved machine."
    );
    assert_eq!(
        super::remote_resources::resources_from_env(&env(r#"{"agent":"../x"}"#), cwd.path())
            .unwrap_err(),
        super::remote_resources::INVALID_RESOURCES
    );

    // The production child runtime applies it at `before_agent_start`.
    let runtime =
        crate::prompt_runtime::prompt_runtime_extension_from(&env(r#"{"agent":"machine-worker"}"#))
            .unwrap()
            .expect("a placed child arms the runtime");
    let mut api = InitApi::new();
    runtime.init(&mut api).await.unwrap();
    assert!(api.subscriptions().contains(EventKind::BeforeAgentStart));
    let outcome = runtime
        .on_event(
            &HostEvent::BeforeAgentStart {
                prompt: "task".to_string(),
                images: serde_json::Value::Null,
                system_prompt: "BASE PROMPT".to_string(),
                options: serde_json::Value::Null,
                injected: Vec::new(),
            },
            &HostCtx::event(ExtMode::Json, false, PathBuf::from("/tmp")),
        )
        .await;
    let HookOutcome::Mutate(EventPatch::SystemPromptAndInject {
        system: Some(system),
        ..
    }) = outcome
    else {
        panic!("the placed child replaces its prompt: {outcome:?}");
    };
    assert!(
        system.contains("<active_agent name=\"machine-worker\"/>")
            && system.contains("You are the MACHINE's worker."),
        "{system}"
    );
}

/// A placed Claude Code run end to end through `run_sync`, with the herdr transport LOST under
/// its prompt: the run reconnects (`HerdrExternalSession.reconnect`, `herdr-external-adapters.ts:
/// 160-178` @v0.68.0) — a fresh forward, the same session, the same terminal in the same pane, the
/// same native pid — reads the agent's settled state, and settles from the pane as upstream's
/// placed external run settles: `[best-effort/unverified]` terminal evidence at exit 1, the pane
/// closed afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn a_placed_external_run_reconnects_a_lost_transport_and_settles() {
    let machine = FakeMachine::start_with(
        "#!/bin/sh\nexit 99\n",
        FakeState {
            drop_first_prompt: true,
            pane_text: "Claude answered FORTY-TWO".to_string(),
            ..FakeState::default()
        },
    );
    let local = tempfile::tempdir().unwrap();
    let mut agent = crate::exec::testsupport::sample_agent_config("m1", &[]);
    agent.runner = Some(crate::runner::AgentRunnerConfig::ExternalCli(
        crate::runner::ExternalCliRunner {
            adapter: Some(AdapterId::ClaudeCode),
            command: "claude".to_string(),
            args: Vec::new(),
            prompt_delivery_stdin: false,
            capabilities: None,
        },
    ));
    let result =
        crate::exec::run_sync(&agent, "Answer", &placed_opts(local.path(), &machine)).await;
    assert_eq!(
        result.exit_code, 1,
        "a placed external run settles partial: {result:?}"
    );
    assert_eq!(result.error, None, "{result:?}");
    assert_eq!(
        result.execution,
        Some(crate::exec::run_result::ExecutionOutcome::partial()),
        "upstream's `execution: {{ status: \"partial\" }}` rides the result"
    );
    assert!(crate::exec::run_result::partial_evidence(
        result.execution.as_ref()
    ));
    assert_eq!(
        result.final_output.as_deref(),
        Some("[best-effort/unverified]\nClaude answered FORTY-TWO"),
        "{result:?}"
    );
    let methods = machine.methods();
    assert_eq!(
        methods.iter().filter(|m| *m == "agent.prompt").count(),
        1,
        "the prompt is not re-sent after the reconnect: {methods:?}"
    );
    let reconnect_at = methods
        .iter()
        .rposition(|m| m == "session.snapshot")
        .expect("the reconnect proves the session");
    assert!(
        methods[reconnect_at..].starts_with(&[
            "session.snapshot".to_string(),
            "pane.process_info".to_string(),
            "agent.get".to_string(),
        ]),
        "reconnect: session and pid proven, then the settled agent read: {methods:?}"
    );
    assert!(methods.ends_with(&["pane.get".to_string(), "pane.close".to_string()]));
    assert_eq!(
        std::fs::read_to_string(machine.dir.path().join("forwards"))
            .unwrap()
            .lines()
            .count(),
        2,
        "one forward lost, one reconnected"
    );
}

// ------------------------------------------------------------------------------------------------
// Pane-native external profiles: preflight, launch argv, terminal settlement
// ------------------------------------------------------------------------------------------------

fn evidence(binary: &str, version: &str, help: &str) -> super::external::PreflightEvidence {
    let record = |args: &[&str], stdout: &str| super::external::CommandEvidence {
        binary: format!("/usr/local/bin/{binary}"),
        args: args.iter().map(|a| (*a).to_string()).collect(),
        status: 0,
        stdout: stdout.to_string(),
        stderr: String::new(),
    };
    super::external::PreflightEvidence {
        version: record(&["--version"], version),
        help: record(&["--help"], help),
    }
}

/// `validateHerdrExternalPreflight` (`herdr-external-adapters.ts:57-64`): the remote binary's
/// identity, version floor and documented interactive options, with upstream's sentences.
#[test]
fn the_remote_external_preflight_holds_upstreams_floor_and_options() {
    use super::external::validate_external_preflight;
    let codex_help =
        "Usage: codex [OPTIONS]\n  --sandbox <MODE>\n  --ask-for-approval <P>\n  --no-alt-screen\n";
    assert_eq!(
        validate_external_preflight(
            AdapterId::CodexExec,
            &evidence("codex", "codex-cli 0.160.2\n", codex_help)
        ),
        Ok(())
    );
    assert_eq!(
        validate_external_preflight(
            AdapterId::CodexExec,
            &evidence("codex", "codex-cli 0.100.0", codex_help)
        ),
        Err("Remote codex is below the tested pane-native capability floor.".to_string())
    );
    assert_eq!(
        validate_external_preflight(
            AdapterId::CodexExec,
            &evidence("codex", "codex 1", codex_help)
        ),
        Err("Unsupported codex version response: \"codex 1\".".to_string())
    );
    assert_eq!(
        validate_external_preflight(
            AdapterId::CodexExec,
            &evidence(
                "codex",
                "codex-cli 0.160.2",
                "Usage: codex\n  --sandbox\n  --no-alt-screen\n"
            )
        ),
        Err(
            "codex help does not document required interactive option \"--ask-for-approval\"."
                .to_string()
        )
    );
    let mut wrong_binary = evidence("codex", "codex-cli 0.160.2", codex_help);
    wrong_binary.help.binary = "/opt/other/codex".to_string();
    assert_eq!(
        validate_external_preflight(AdapterId::CodexExec, &wrong_binary),
        Err("External preflight canonical binary identity is inconsistent.".to_string())
    );
    let mut failed = evidence("claude", "2.1.300 (Claude Code)", "Claude Code");
    failed.version.status = 1;
    failed.version.stderr = "boom".to_string();
    assert_eq!(
        validate_external_preflight(AdapterId::ClaudeCode, &failed),
        Err("claude preflight exited 1: boom".to_string())
    );
}

/// `createHerdrExternalAdapterLaunch`: each owned profile's sandbox argv, and Cursor's refusal of
/// a non-absolute workspace.
#[test]
fn the_pane_native_launch_argv_is_the_profiles_sandbox() {
    use super::external::external_launch;
    let rt = "/tmp/cyrup-subagents-herdr-run_abcdefgh-12345678";
    let claude =
        external_launch(AdapterId::ClaudeCode, rt, "/srv", Some("sid".to_string())).unwrap();
    assert_eq!(
        claude.args,
        [
            "--session-id",
            "sid",
            "--restricted",
            "--permission-mode",
            "plan",
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
            "--disable-slash-commands",
            "--no-chrome"
        ]
    );
    let writer = external_launch(
        AdapterId::ClaudeCodeWriter,
        rt,
        "/srv",
        Some("sid".to_string()),
    )
    .unwrap();
    assert!(writer.args.contains(&"acceptEdits".to_string()));
    assert_eq!(
        external_launch(AdapterId::CodexExecWriter, rt, "/srv", None)
            .unwrap()
            .args,
        [
            "--sandbox",
            "workspace-write",
            "--ask-for-approval",
            "never",
            "--no-alt-screen"
        ]
    );
    assert_eq!(
        external_launch(AdapterId::CursorAgent, rt, "/srv", None)
            .unwrap()
            .args,
        [
            "--mode",
            "ask",
            "--sandbox",
            "enabled",
            "--workspace",
            "/srv"
        ]
    );
    assert_eq!(
        external_launch(AdapterId::CursorAgent, rt, "~/srv", None).unwrap_err(),
        "Pane-native Cursor requires the exact remote-absolute owned workspace."
    );
    assert_eq!(
        external_launch(AdapterId::CodexExec, "relative", "/srv", None).unwrap_err(),
        "External adapter requires the accepted run-private remote runtime root."
    );
}

/// The terminal evidence is sanitized and tagged `[best-effort/unverified]`; Codex's answer is the
/// text after the echoed task, minus prompt chrome.
#[test]
fn placed_terminal_evidence_is_sanitized_and_tagged_unverified() {
    use super::external::{codex_assistant_output, normalize_placed_terminal, sanitize_terminal};
    assert_eq!(
        sanitize_terminal("\u{1b}[31mred\u{1b}[0m  \r\nnext\u{7}"),
        "red\nnext"
    );
    assert_eq!(
        normalize_placed_terminal("\u{1b}[1mdone\u{1b}[0m"),
        Ok("[best-effort/unverified]\ndone".to_string())
    );
    assert_eq!(
        normalize_placed_terminal("\u{1b}[0m \n"),
        Err("sanitized terminal output is missing.".to_string())
    );
    let screen = "banner\n› Fix the bug\nline two\n\nThe fix is in parser.rs.\n\n› Ask Codex anything\nTokens used: 12";
    assert_eq!(
        codex_assistant_output(screen, "Fix the bug\nline two"),
        "The fix is in parser.rs."
    );
    assert_eq!(codex_assistant_output("no echo here", "Fix the bug"), "");
}

/// `monitorHerdrCodex`: settles only on stable, ready, non-working output past the startup grace;
/// identity drift and attention prompts retain the pane with upstream's sentences.
#[tokio::test(start_paused = true)]
async fn the_codex_monitor_settles_on_stable_ready_output_and_retains_on_drift() {
    use super::external::{CodexSnapshot, ExternalError, monitor_codex};
    use cyrup_herdr::schema::ReadSource;
    let snap = |text: &str, pid: u32| CodexSnapshot {
        workspace_id: "w1".to_string(),
        pane_id: "w1:p1".to_string(),
        terminal_id: "t1".to_string(),
        pid,
        text: text.to_string(),
        source: ReadSource::RecentUnwrapped,
    };
    let done = "› Fix it\n\nFixed parser.rs.\n\n› Ask Codex anything";
    let stopped = Arc::new(Mutex::new(false));
    let settled = monitor_codex(
        "Fix it",
        "› Ask Codex anything",
        ("w1", "w1:p1", "t1", 7),
        || async { Ok(snap(done, 7)) },
        || {
            let stopped = Arc::clone(&stopped);
            async move { *stopped.lock().unwrap() = true }
        },
        std::time::Duration::from_secs(120),
    )
    .await;
    assert_eq!(settled.ok().as_deref(), Some("Fixed parser.rs."));
    assert!(*stopped.lock().unwrap(), "a settled Codex is stopped");

    let drifted = monitor_codex(
        "Fix it",
        "",
        ("w1", "w1:p1", "t1", 7),
        || async { Ok(snap(done, 8)) },
        || async {},
        std::time::Duration::from_secs(120),
    )
    .await;
    assert!(matches!(
        drifted,
        Err(ExternalError::NeedsAttention(ref m)) if m == "Placed Codex identity drifted; truthful pane retained for inspection."
    ));
    let attention = monitor_codex(
        "Fix it",
        "",
        ("w1", "w1:p1", "t1", 7),
        || async { Ok(snap("Do you trust the contents of this directory?", 7)) },
        || async {},
        std::time::Duration::from_secs(120),
    )
    .await;
    assert!(matches!(
        attention,
        Err(ExternalError::NeedsAttention(ref m)) if m == "Placed Codex requires attention; truthful pane retained for inspection."
    ));
    let never_ready = monitor_codex(
        "Fix it",
        "",
        ("w1", "w1:p1", "t1", 7),
        || async { Ok(snap("› Fix it\n\nthinking… esc to interrupt", 7)) },
        || async {},
        std::time::Duration::from_secs(60),
    )
    .await;
    assert!(matches!(
        never_ready,
        Err(ExternalError::NeedsAttention(ref m)) if m.starts_with("Placed Codex settlement remained ambiguous until timeout")
    ));
}
