//! A native cyrup child placed on a Herdr saved machine — the cyrup equivalent of pi-subagents'
//! pane-native remote Pi (`src/runs/shared/herdr-placed-run.ts` @v0.68.0,
//! `createHerdrPiSession`/`HerdrPlacedRunOwner`/`serializeHerdrPiLaunch`, and the remote half,
//! `src/extension/herdr-pi-bridge.ts`).
//!
//! # What upstream does, and what the cyrup equivalent is
//!
//! Upstream: make a run-private remote runtime directory (`createRemoteRuntimeDir`), forward the
//! remote herdr socket (`connectHerdrMachine`), open a herdr-owned pane in an owned workspace under
//! an allocation lock (`provisionHerdrPane`), `agent.start` Pi in it, and talk to that Pi through a
//! bridge extension over a second forwarded socket: `configure` hands it LOGICAL resources (agent,
//! skills, tool ceiling, reads) which it resolves against the machine's own checkout; events,
//! `contact_supervisor` requests and replies, steering, follow-ups and aborts cross the bridge;
//! losing ssh is reconnected (bounded). SSH is transport; the pane owns the agent.
//!
//! cyrup keeps every one of those steps. Its child contract is argv + environment in, NDJSON on
//! stdout out, and its live channels are files (the supervisor channel, the steer inbox and its
//! acknowledgements), so the placed child is the SAME child a local run would spawn, with its
//! launch serialized for the machine ([`serialize_placed_launch`]):
//!
//! 1. its task file and structured-output schema are uploaded into the runtime dir, and every
//!    argv/env reference to them is rewritten to the remote path. Its persona is NOT shipped: the
//!    launch names its logical resources in
//!    [`crate::placement::remote_resources::RESOURCES_ENV`], and the `cyrup` on the machine
//!    resolves the agent, its memory, skills and reads against the machine's own files, refusing
//!    the launch when they are not there (`resolveRemoteHerdrResources`);
//! 2. its supervisor channel and steer inbox / acknowledgements / capability point INTO the
//!    runtime dir, and the relay mirrors them with the parent's local ones — the bridge's
//!    `supervisor-request` / `supervisor-reply` / `steer` / `follow-up`;
//! 3. a `run.sh` is uploaded that records its pid, `cd`s to the machine cwd (the directory check),
//!    records remote git before/after, and runs `cyrup` with that argv/env — stdout to
//!    `events.ndjson`, stderr to the pane (visible) and to `stderr`, the exit code to an `exit`
//!    marker written only after both are flushed;
//! 4. the pane is opened exactly as upstream opens it and `sh <runtime>/run.sh` is typed into it
//!    (`pane.send_input` — herdr's `agent.start` only knows the agent kinds it detects, and
//!    `cyrup` is not one of them);
//! 5. the attempt runner drives the child through the in-process relay
//!    ([`cyrup_herdr::relay::RunRelay`], the one herdr client's ssh transport) exactly as it
//!    drives a local child's stdout: [`PlacedNativeRun::start_relay`] hands it a
//!    [`crate::spawn::RelayedChild`]. A dropped ssh is reconnected from the delivered byte offset
//!    (three attempts inside fifteen seconds per loss, upstream's `boundedHerdrReconnect`), and a
//!    child whose pane was closed is reported lost; past the budget the run's state is unknown.
//!
//! Everything downstream of the relay — progress, transcript, the fallback ladder, deadlines,
//! cancellation, acceptance — is the unchanged local machinery. [`PlacedNativeRun::finish`] then
//! fetches the child's result files back, reads the remote git evidence, and closes the pane (or
//! retains it when the state is unknown). Stop/abort closes the pane, which ends the child
//! (upstream's `abort` then `cleanup(true)`).
//!
//! # What cannot be placed, refused with upstream's words
//!
//! [`serialize_placed_launch`] refuses before anything touches the network, exactly where
//! `serializeHerdrPiLaunch` throws (`herdr-placed-run.ts:88-99`): fork/resume (a local session
//! file), local extension paths, nested delegation, exclusions / permission rules / tool budgets /
//! MCP direct tools / required-tool contracts, and resource names outside upstream's bounds. Any
//! other launch input that is a LOCAL PATH is refused by name rather than shipped to a machine
//! where it means nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cyrup_herdr::relay::{DownMirror, MirrorKind, RelayEnd, RunRelay, UpMirror};
use cyrup_herdr::remote::{
    MachineConnection, REMOTE_COMMAND_TIMEOUT, SshTransport, remote_shell_command,
    shell_quote_remote,
};
use cyrup_herdr::schema::{PaneSendInputParams, TabCreateParams, WorkspaceCreateParams};
use sha2::Digest;

use super::HerdrMachineReference;
use super::remote_resources::{RESOURCES_ENV, RemoteResources};
use crate::spawn::{ChildSpawnSpec, RelayedChild};

/// The runtime-directory name prefix; the remove script refuses anything else.
pub const RUNTIME_DIR_PREFIX: &str = "cyrup-subagents-herdr-";
/// The owned-workspace label prefix (`pi-subagents-<sha>`, `herdr-placed-run.ts:140`).
pub const OWNED_WORKSPACE_PREFIX: &str = "cyrup-subagents-";
/// `HERDR_MAX_OWNED_PANES` (`herdr-placed-run.ts:56`).
pub const HERDR_MAX_OWNED_PANES: usize = 20;
/// The pane-env key carrying the placed run's identity (`HERDR_PI_RUN_ENV`).
pub const RUN_ID_ENV: &str = "CYRUP_SUBAGENTS_HERDR_RUN_ID";
/// The pane-env key carrying the runtime directory (`HERDR_PI_RUNTIME_DIR_ENV`).
pub const RUNTIME_DIR_ENV: &str = "CYRUP_SUBAGENTS_HERDR_RUNTIME_DIR";
/// The binary a placed native child runs, resolved on the machine's own `PATH`
/// ([`cyrup_herdr::remote::HERDR_REMOTE_PATH`] first) — the cyrup analog of herdr's `pi` kind.
pub const REMOTE_BINARY: &str = "cyrup";
/// The exit code a relayed child reports when its state is unknown (the relay lost the machine
/// past its reconnect budget, or the child's pane ended without an exit record) — ssh's own
/// failure code, so the attempt's exit classification reads it as a transport failure.
pub const RELAY_UNKNOWN_EXIT: i32 = 255;
/// The run dir's supervisor channel (`requests/`, `replies/`), mirrored with the parent's.
pub const REMOTE_SUPERVISOR_DIR: &str = "supervisor";
/// The run dir's steer inbox, which the parent's requests are delivered into.
pub const REMOTE_STEER_INBOX: &str = "steer-inbox";
/// The run dir's steer acknowledgements, mirrored back to the parent.
pub const REMOTE_STEER_ACKS: &str = "steer-acks";
/// The run dir's steer capability record, mirrored back to the parent.
pub const REMOTE_STEER_CAPABILITY: &str = "steer-capability.json";

/// pi `HerdrRemoteGitStatus` (`shared/types.ts:1720-1726` @v0.68.0).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteGitStatus {
    /// `git rev-parse HEAD` on the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// The checked-out branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Whether `git status --porcelain` printed anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
}

/// pi `result.nativeMachine` (`shared/types.ts:1268`): the authoritative before/after git
/// evidence a placed native child produced ON THE MACHINE —
/// `{ provider: "herdr", machineId, initialGit?, finalGit? }`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeMachineEvidence {
    /// Always [`super::MachineProvider::Herdr`].
    #[serde(default)]
    pub provider: super::MachineProvider,
    /// herdr's profile id.
    pub machine_id: String,
    /// Before the child ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_git: Option<RemoteGitStatus>,
    /// After it exited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_git: Option<RemoteGitStatus>,
}

impl NativeMachineEvidence {
    /// Evidence for `machine_id` with neither endpoint captured yet.
    #[must_use]
    pub fn new(machine_id: impl Into<String>) -> Self {
        Self {
            provider: super::MachineProvider::Herdr,
            machine_id: machine_id.into(),
            initial_git: None,
            final_git: None,
        }
    }

    /// pi `remoteGitChanged` (`execution.ts:1502` @v0.68.0): `Some(head or dirty changed)` when
    /// both endpoints were captured, `None` when either is missing.
    #[must_use]
    pub fn remote_git_changed(&self) -> Option<bool> {
        match (&self.initial_git, &self.final_git) {
            (Some(initial), Some(final_git)) => {
                Some(initial.head != final_git.head || initial.dirty != final_git.dirty)
            }
            _ => None,
        }
    }
}

/// pi's placed-run snapshot stand-in (`foreground/execution.ts:554` @v0.68.0): the LOCAL checkout
/// is not where a placed child works, so no local Git state is taken and every later collect
/// short-circuits on the reason.
pub const LOCAL_GIT_NOT_AUTHORITATIVE: &str =
    "Local Git evidence is not authoritative for a pane-native remote run.";

/// pi's placed-run evidence when the remote before/after pair is incomplete
/// (`foreground/execution.ts:1503` @v0.68.0).
pub const REMOTE_GIT_INCOMPLETE: &str = "Remote Git before/after evidence was incomplete.";

/// The mutation snapshot a placed run takes in place of the local one.
#[must_use]
pub fn placed_mutation_snapshot(
    cwd: &Path,
) -> crate::exec::mutation_evidence::TrackedMutationSnapshot {
    crate::exec::mutation_evidence::TrackedMutationSnapshot {
        source: Default::default(),
        tracked_only: true,
        cwd: cwd.to_path_buf(),
        git_root: None,
        dirty_files: Vec::new(),
        fingerprints: BTreeMap::new(),
        truncated: false,
        unavailable: Some(LOCAL_GIT_NOT_AUTHORITATIVE.to_string()),
    }
}

/// pi `result.nativeMachine ? { …, changedFiles: [], attemptedMutation: remoteGitChanged === true,
/// …(remoteGitChanged === undefined ? { unavailable } : {}) } : collect(snapshot)`
/// (`foreground/execution.ts:1502-1503` @v0.68.0). With no machine evidence at all the collect runs
/// over [`placed_mutation_snapshot`], whose reason it carries forward.
#[must_use]
pub fn placed_mutation_evidence(
    evidence: Option<&NativeMachineEvidence>,
) -> crate::exec::mutation_evidence::TrackedMutationEvidence {
    let (attempted_mutation, unavailable) = match evidence {
        Some(evidence) => match evidence.remote_git_changed() {
            Some(changed) => (changed, None),
            None => (false, Some(REMOTE_GIT_INCOMPLETE.to_string())),
        },
        None => (false, Some(LOCAL_GIT_NOT_AUTHORITATIVE.to_string())),
    };
    crate::exec::mutation_evidence::TrackedMutationEvidence {
        source: Default::default(),
        tracked_only: true,
        changed_files: Vec::new(),
        attempted_mutation,
        truncated: false,
        unavailable,
    }
}

/// `validateHerdrPiRunId` (`herdr-pi-protocol.ts:8-11`): `^[A-Za-z0-9_-]{8,96}$`.
///
/// # Errors
/// `"Herdr placed run identity is invalid."`
pub fn validate_run_id(value: &str) -> Result<String, String> {
    let ok = (8..=96).contains(&value.len())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(value.to_string())
    } else {
        Err("Herdr placed run identity is invalid.".to_string())
    }
}

/// `safeRunId(value)` (`herdr-placed-run.ts:47`): the run id sanitized to `[A-Za-z0-9_-]`, cut to
/// 48 characters, plus 16 random hex digits — unique per placed attempt.
#[must_use]
pub fn safe_run_id(value: Option<&str>) -> String {
    let prefix: String = value
        .map(|raw| {
            raw.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .take(48)
                .collect()
        })
        .unwrap_or_else(|| "run".to_string());
    let random = uuid::Uuid::new_v4().simple().to_string();
    let suffix: String = random.chars().take(16).collect();
    format!("{prefix}_{suffix}")
}

/// One file that crosses to (upload) or from (fetch) the runtime directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteFile {
    /// The file name inside the runtime directory.
    pub name: String,
    /// The local path it comes from or returns to.
    pub local: PathBuf,
}

/// A native launch serialized for a machine (`serializeHerdrPiLaunch`'s result, cyrup's shape).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteLaunch {
    /// argv after [`REMOTE_BINARY`]; runtime-dir references are `{{RT}}`-prefixed until
    /// [`RemoteLaunch::bind`] substitutes the real directory.
    pub argv: Vec<String>,
    /// The child environment, same placeholder rule.
    pub env: BTreeMap<String, String>,
    /// Files uploaded before the child starts.
    pub uploads: Vec<RemoteFile>,
    /// Files fetched back after it exits.
    pub fetches: Vec<RemoteFile>,
    /// Channel paths mirrored machine → parent while the child runs.
    pub down: Vec<DownMirror>,
    /// Parent directories delivered parent → machine while the child runs.
    pub up: Vec<UpMirror>,
}

const RT: &str = "{{RT}}";

impl RemoteLaunch {
    /// Substitute the runtime directory into every placeholder.
    #[must_use]
    pub fn bind(&self, runtime_dir: &str) -> (Vec<String>, BTreeMap<String, String>) {
        let bind = |value: &str| value.replace(RT, runtime_dir);
        (
            self.argv.iter().map(|arg| bind(arg)).collect(),
            self.env
                .iter()
                .map(|(key, value)| (key.clone(), bind(value)))
                .collect(),
        )
    }
}

/// What [`serialize_placed_launch`] needs beyond the spec.
#[derive(Clone, Debug, Default)]
pub struct PlacedLaunchContext {
    /// The depth the child would run at (`launch.runtime.depth`).
    pub child_depth: u32,
    /// The launch's logical resources (`remoteResources`, `child-launch.ts:309`): the agent,
    /// its skill names and an explicit `reads`. The tool ceiling is read off the spec's own
    /// `--tools`/`--no-tools` (`toolPlan.effectiveToolAllowlist`).
    pub resources: RemoteResources,
}

/// `serializeHerdrPiLaunch` refusal: a local session file.
pub const FORK_REFUSAL: &str =
    "Pane-native remote cyrup does not support fork, resume, or revival; use fresh context.";
/// `serializeHerdrPiLaunch` refusal: local extension paths.
pub const EXTENSION_REFUSAL: &str = "Pane-native remote cyrup cannot transfer local extension paths; install and configure extensions on the remote machine.";
/// `serializeHerdrPiLaunch` refusal: nested delegation.
pub const NESTED_REFUSAL: &str =
    "Nested delegation from a pane-native remote cyrup is not supported.";
/// `serializeHerdrPiLaunch` refusal: the contracts the remote cannot represent.
pub const CONTRACT_REFUSAL: &str = "Pane-native remote cyrup cannot yet represent exclusions, permission rules, tool budgets, MCP direct tools, or required-tool/extension contracts; remove machine placement.";

/// How one child-environment key crosses to the machine.
enum EnvCrossing {
    /// A value — sent as is.
    Pass,
    /// A local path whose FILE the child reads — uploaded under this name.
    Upload(&'static str),
    /// A local path the child WRITES — pointed at the runtime dir, fetched back under this name.
    Fetch(&'static str),
    /// A local-only channel or identity with no meaning on the machine — not sent.
    Drop,
    /// A live channel the parent and the child share through files: pointed into the run's remote
    /// directory and mirrored by the relay (the bridge's supervisor / steer frames).
    Channel(Channel),
    /// A contract the machine cannot represent — the launch is refused.
    Refuse(&'static str),
}

/// The four file channels a placed child shares with its parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Channel {
    /// `contact_supervisor`'s `requests/` (machine → parent) and `replies/` (parent → machine).
    Supervisor,
    /// Steer / follow-up requests (parent → machine).
    SteerInbox,
    /// Steer acknowledgements (machine → parent).
    SteerAcks,
    /// The steering capability record (machine → parent).
    SteerCapability,
}

fn env_crossing(key: &str, value: &str) -> EnvCrossing {
    use crate::spawn::intercom_target as intercom;
    use crate::spawn::nested_events as nested;
    match key {
        k if k == crate::exec::structured::STRUCTURED_OUTPUT_SCHEMA_ENV => {
            EnvCrossing::Upload("structured-output-schema.json")
        }
        k if k == crate::exec::structured::STRUCTURED_OUTPUT_CAPTURE_ENV => {
            EnvCrossing::Fetch("structured-output.json")
        }
        k if k == crate::exec::tool_availability::CHILD_TOOL_DIAGNOSTIC_PATH_ENV => {
            EnvCrossing::Fetch("tool-diagnostic.json")
        }
        k if k == crate::exec::runtime_acknowledged_extensions::RUNTIME_EXTENSION_ACK_PATH_ENV => {
            EnvCrossing::Fetch("runtime-acknowledged-extensions.json")
        }
        k if k == crate::watchdog::permission_arbiter::PERMISSION_POLICY_ENV
            || k == crate::watchdog::permission_arbiter::PERMISSION_AUDIT_PATH_ENV
            || k == crate::exec::tool_budget::TOOL_BUDGET_ENV
            || k == crate::native_supervisor::ENV_REQUIRED_CHILD_TOOLS
            || k == crate::exec::tool_availability::MCP_DIRECT_CHILD_TOOLS_ENV =>
        {
            EnvCrossing::Refuse(CONTRACT_REFUSAL)
        }
        "MCP_DIRECT_TOOLS" if value != "__none__" => EnvCrossing::Refuse(CONTRACT_REFUSAL),
        k if k == nested::PARENT_EVENT_SINK_ENV
            || k == nested::PARENT_CONTROL_INBOX_ENV
            || k == nested::PARENT_ROOT_RUN_ID_ENV
            || k == nested::PARENT_RUN_ID_ENV
            || k == nested::PARENT_CHILD_INDEX_ENV
            || k == nested::PARENT_DEPTH_ENV
            || k == nested::PARENT_PATH_ENV
            || k == nested::PARENT_CAPABILITY_TOKEN_ENV =>
        {
            EnvCrossing::Refuse(NESTED_REFUSAL)
        }
        k if k == intercom::ENV_SUPERVISOR_CHANNEL_DIR => EnvCrossing::Channel(Channel::Supervisor),
        k if k == crate::prompt_runtime::STEER_INBOX_ENV => {
            EnvCrossing::Channel(Channel::SteerInbox)
        }
        k if k == crate::prompt_runtime::STEER_ACK_DIR_ENV => {
            EnvCrossing::Channel(Channel::SteerAcks)
        }
        k if k == crate::prompt_runtime::STEER_CAPABILITY_ENV => {
            EnvCrossing::Channel(Channel::SteerCapability)
        }
        // The launching side's context policy is not the placed child's: the machine's agent
        // brings its own (`resolveRemoteHerdrResources` returns `inheritProjectContext` /
        // `inheritGlobalContext` / `inheritSkills` from the REMOTE definition).
        k if k == crate::prompt_runtime::INHERIT_PROJECT_CONTEXT_ENV
            || k == crate::prompt_runtime::INHERIT_GLOBAL_CONTEXT_ENV
            || k == crate::prompt_runtime::INHERIT_SKILLS_ENV =>
        {
            EnvCrossing::Drop
        }
        k if k == crate::exec::spawn_plan::PARENT_SESSION_ENV_VAR
            || k == intercom::ENV_ORCHESTRATOR_TARGET
            || k == intercom::ENV_INTERCOM_SESSION_ID
            || k == intercom::ENV_INTERCOM_SESSION_NAME
            || k == crate::spawn::SUBAGENT_BINARY_ENV_VAR
            || k == crate::spawn::SUBAGENT_BINARY_ARGS_ENV_VAR
            || k == nested::TEMP_ROOT_ENV =>
        {
            EnvCrossing::Drop
        }
        _ => EnvCrossing::Pass,
    }
}

/// Whether `value` names something on THIS machine — an absolute path that exists here.
fn is_local_path(value: &str) -> bool {
    value.starts_with('/') && Path::new(value).exists()
}

/// `serializeHerdrPiLaunch(launch)` (`herdr-placed-run.ts:112-128`), for cyrup's argv + env child
/// contract. Pure: reads the spec and the local files it names, touches no network.
///
/// # Errors
/// Upstream's refusal for every input the machine cannot honour; any other input that names a
/// local path is refused by name.
pub fn serialize_placed_launch(
    spec: &ChildSpawnSpec,
    ctx: PlacedLaunchContext,
) -> Result<RemoteLaunch, String> {
    if ctx.child_depth > 1 {
        return Err(NESTED_REFUSAL.to_string());
    }
    let mut launch = RemoteLaunch::default();
    let mut resources = ctx.resources.clone();
    let mut args = spec.args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--session" => return Err(FORK_REFUSAL.to_string()),
            // The child's session store is the MACHINE's (upstream's remote Pi persists its own
            // native session); a local directory means nothing there.
            "--session-dir" => {
                let _ = args.next();
            }
            "--extension" => return Err(EXTENSION_REFUSAL.to_string()),
            "--exclude-tools" => return Err(CONTRACT_REFUSAL.to_string()),
            // The persona is not shipped: the machine resolves the agent's own prompt from its
            // checkout ([`super::remote_resources`]), exactly as upstream's launch carries no
            // system prompt for a placed Pi.
            flag @ ("--system-prompt" | "--append-system-prompt") => {
                if args.next().is_none() {
                    return Err(format!("{flag} carried no prompt file."));
                }
            }
            // `toolCeiling: toolPlan.effectiveToolAllowlist` (`child-launch.ts:309`): the remote
            // agent's tools are narrowed to the launch's pinned allowlist, which the child's own
            // `--tools` also carries.
            "--tools" => {
                let Some(list) = args.next() else {
                    return Err("--tools carried no tool list.".to_string());
                };
                resources.tool_ceiling = Some(
                    list.split(',')
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                        .collect(),
                );
                launch.argv.push("--tools".to_string());
                launch.argv.push(list.clone());
            }
            "--no-tools" => {
                resources.tool_ceiling = Some(Vec::new());
                launch.argv.push("--no-tools".to_string());
            }
            other if is_local_path(other) => {
                return Err(format!(
                    "Pane-native remote cyrup cannot transfer the local path argument '{other}'; remove machine placement."
                ));
            }
            other => launch.argv.push(other.to_string()),
        }
    }
    match spec.task_arg.strip_prefix('@') {
        Some(path) if Path::new(path).is_file() => {
            launch.uploads.push(RemoteFile {
                name: "task.md".to_string(),
                local: PathBuf::from(path),
            });
            launch.argv.push(format!("@{RT}/task.md"));
        }
        _ => launch.argv.push(spec.task_arg.clone()),
    }
    for (key, value) in &spec.env_overlay {
        let assignable = key
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !assignable {
            return Err(format!(
                "Pane-native remote cyrup cannot export the child contract '{key}'; remove machine placement."
            ));
        }
        match env_crossing(key, value) {
            EnvCrossing::Pass => {
                if is_local_path(value) {
                    return Err(format!(
                        "Pane-native remote cyrup cannot transfer the local path in child contract '{key}'; remove machine placement."
                    ));
                }
                launch.env.insert(key.clone(), value.clone());
            }
            EnvCrossing::Upload(name) => {
                launch.uploads.push(RemoteFile {
                    name: name.to_string(),
                    local: PathBuf::from(value),
                });
                launch.env.insert(key.clone(), format!("{RT}/{name}"));
            }
            EnvCrossing::Fetch(name) => {
                launch.fetches.push(RemoteFile {
                    name: name.to_string(),
                    local: PathBuf::from(value),
                });
                launch.env.insert(key.clone(), format!("{RT}/{name}"));
            }
            EnvCrossing::Drop => {}
            EnvCrossing::Channel(channel) => {
                let local = PathBuf::from(value);
                let remote = match channel {
                    Channel::Supervisor => {
                        launch.down.push(DownMirror {
                            key: "supervisor-request".to_string(),
                            remote: format!("{REMOTE_SUPERVISOR_DIR}/requests"),
                            local: local.join("requests"),
                            kind: MirrorKind::Dir,
                        });
                        launch.up.push(UpMirror {
                            local: local.join("replies"),
                            remote: format!("{REMOTE_SUPERVISOR_DIR}/replies"),
                        });
                        REMOTE_SUPERVISOR_DIR
                    }
                    Channel::SteerInbox => {
                        launch.up.push(UpMirror {
                            local,
                            remote: REMOTE_STEER_INBOX.to_string(),
                        });
                        REMOTE_STEER_INBOX
                    }
                    Channel::SteerAcks => {
                        launch.down.push(DownMirror {
                            key: "steer-ack".to_string(),
                            remote: REMOTE_STEER_ACKS.to_string(),
                            local,
                            kind: MirrorKind::Dir,
                        });
                        REMOTE_STEER_ACKS
                    }
                    Channel::SteerCapability => {
                        launch.down.push(DownMirror {
                            key: "steer-capability".to_string(),
                            remote: REMOTE_STEER_CAPABILITY.to_string(),
                            local,
                            kind: MirrorKind::File,
                        });
                        REMOTE_STEER_CAPABILITY
                    }
                };
                launch.env.insert(key.clone(), format!("{RT}/{remote}"));
            }
            EnvCrossing::Refuse(reason) => return Err(reason.to_string()),
        }
    }
    resources.validate()?;
    launch.env.insert(
        RESOURCES_ENV.to_string(),
        serde_json::to_string(&resources).map_err(|error| error.to_string())?,
    );
    launch.uploads.sort_by(|a, b| a.name.cmp(&b.name));
    launch.fetches.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(launch)
}

/// `cd` target for a remote cwd: `~`-relative paths expand against the remote `$HOME`, everything
/// else is single-quoted.
fn cd_target(cwd: &str) -> String {
    if cwd == "~" {
        "\"$HOME\"".to_string()
    } else if let Some(rest) = cwd.strip_prefix("~/") {
        format!("\"$HOME\"/{}", shell_quote_remote(rest))
    } else {
        shell_quote_remote(cwd)
    }
}

/// The remote git evidence helper `run.sh` defines — pi's bridge `gitEvidence(cwd)`
/// (`herdr-pi-bridge.ts:26-32`) in POSIX sh, emitting only values that are safe JSON as-is.
const GIT_EVIDENCE_FN: &str = r#"cyrup_git_evidence() {
  git rev-parse --is-inside-work-tree >/dev/null 2>&1 || return 0
  h=$(git rev-parse HEAD 2>/dev/null); b=$(git symbolic-ref --quiet --short HEAD 2>/dev/null)
  if [ -n "$(git status --porcelain 2>/dev/null)" ]; then d=true; else d=false; fi
  j=""
  case "$h" in ''|*[!0-9a-fA-F]*) ;; *) j="\"head\":\"$h\",";; esac
  case "$b" in ''|*[\"\\]*) ;; *) j="$j\"branch\":\"$b\",";; esac
  printf '{%s"dirty":%s}\n' "$j" "$d" > "$1"
}"#;

/// Render the uploaded `run.sh`.
#[must_use]
pub fn render_run_script(
    runtime_dir: &str,
    cwd: &str,
    argv: &[String],
    env: &BTreeMap<String, String>,
    banner: &str,
) -> String {
    let rt = shell_quote_remote(runtime_dir);
    let mut script = String::new();
    script.push_str("#!/bin/sh\n");
    script.push_str("umask 077\n");
    script.push_str(&format!("rt={rt}\n"));
    // The relay reads the pid to tell a child whose pane was closed (no `exit` will ever come)
    // from one that is still working; it is written BEFORE `started`.
    script.push_str("echo $$ > \"$rt/pid\"\n");
    script.push_str(&format!(
        "mkdir -p \"$rt/{REMOTE_SUPERVISOR_DIR}/requests\" \"$rt/{REMOTE_SUPERVISOR_DIR}/replies\" \"$rt/{REMOTE_STEER_INBOX}\" \"$rt/{REMOTE_STEER_ACKS}\"\n"
    ));
    script.push_str(": > \"$rt/started\"\n");
    script.push_str(&format!(
        "PATH=\"{}:$PATH\"; export PATH\n",
        cyrup_herdr::remote::HERDR_REMOTE_PATH
    ));
    script.push_str(&format!("printf '%s\\n' {}\n", shell_quote_remote(banner)));
    script.push_str(GIT_EVIDENCE_FN);
    script.push('\n');
    script.push_str(&format!(
        "if ! cd -- {}; then echo 125 > \"$rt/exit.tmp\"; mv \"$rt/exit.tmp\" \"$rt/exit\"; exit 125; fi\n",
        cd_target(cwd)
    ));
    script.push_str("cyrup_git_evidence \"$rt/git-initial.json\"\n");
    for (key, value) in env {
        script.push_str(&format!(
            "{key}={}; export {key}\n",
            shell_quote_remote(value)
        ));
    }
    let mut command = shell_quote_remote(REMOTE_BINARY);
    for arg in argv {
        command.push(' ');
        command.push_str(&shell_quote_remote(arg));
    }
    // stdout → events (fd 3), stderr → the pane AND `stderr`; the exit code is written inside the
    // group, and published as `exit` only once the pipeline has drained, so a relay that sees the
    // marker has every event byte already on disk.
    script.push_str("exec 3>\"$rt/events.ndjson\"\n");
    script.push_str(&format!(
        "{{ {command} </dev/null 2>&1 1>&3 3>&-; echo $? > \"$rt/exit.tmp\"; }} | tee -a \"$rt/stderr\"\n"
    ));
    script.push_str("exec 3>&-\n");
    script.push_str("cyrup_git_evidence \"$rt/git-final.json\"\n");
    script.push_str("mv \"$rt/exit.tmp\" \"$rt/exit\"\n");
    script
}

/// `createRemoteRuntimeDir` (`herdr-placed-run.ts:130`), plus the run's identity inside it — the
/// relay refuses a directory whose `run-id` is not its run's.
const MKTEMP_SCRIPT: &str = "umask 077; d=$(mktemp -d \"${TMPDIR:-/tmp}/cyrup-subagents-herdr-$1-XXXXXXXX\") && printf '%s' \"$1\" > \"$d/run-id\" && printf '%s\\n' \"$d\"";
const UPLOAD_SCRIPT: &str = "umask 077; cat > \"$1/$2\"";
const STDERR_TAIL_SCRIPT: &str =
    "p=\"$1/stderr\"; if test -f \"$p\" && test ! -L \"$p\"; then tail -c 4096 \"$p\"; fi";
const FETCH_SCRIPT: &str = "p=\"$1/$2\"; test -f \"$p\" && test ! -L \"$p\" && cat \"$p\"";
const REMOVE_SCRIPT: &str = "p=$1; case \"${p##*/}\" in cyrup-subagents-herdr-*) test -d \"$p\" && test ! -L \"$p\" && rm -rf -- \"$p\";; *) exit 64;; esac";
const MAX_FETCH_BYTES: usize = 16 * 1024 * 1024;

/// A runtime directory safe to type into a pane unquoted.
fn safe_runtime_dir(value: &str) -> bool {
    value.starts_with('/')
        && value.contains(RUNTIME_DIR_PREFIX)
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+'))
}

/// What [`PlacedNativeRun::finish`] brings back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedFinish {
    /// pi `result.nativeMachine`.
    pub evidence: NativeMachineEvidence,
    /// The child's stderr tail (last 4 KiB), when it settled.
    pub stderr_tail: Option<String>,
}

/// The pane a placed run owns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedPane {
    /// Its workspace.
    pub workspace_id: String,
    /// Its tab.
    pub tab_id: String,
    /// The pane.
    pub pane_id: String,
}

/// How a placed attempt ended, for [`PlacedNativeRun::finish`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacedDisposition {
    /// The child exited and its exit marker was read — fetch its files, close the pane.
    Settled,
    /// Stopped, cancelled or timed out locally — close the pane, which ends the child.
    Aborted,
    /// Transport lost past the reconnect budget — state unknown: retain pane and runtime dir.
    Unknown,
}

/// The single owner of one placed native attempt (`HerdrPlacedRunOwner`,
/// `herdr-placed-run.ts:155-196`).
#[derive(Debug)]
pub struct PlacedNativeRun {
    machine: HerdrMachineReference,
    transport: SshTransport,
    connection: Option<MachineConnection>,
    run_id: String,
    runtime_dir: String,
    owned: Option<OwnedPane>,
    fetches: Vec<RemoteFile>,
    down: Vec<DownMirror>,
    up: Vec<UpMirror>,
}

/// Everything [`prepare_placed_native_attempt`] needs besides the spec.
#[derive(Clone, Debug)]
pub struct PlacedAttemptInput<'a> {
    /// The resolved machine.
    pub machine: &'a HerdrMachineReference,
    /// The run id, for the placed run's identity.
    pub run_id: Option<&'a str>,
    /// The agent, for the pane banner.
    pub agent_name: &'a str,
    /// The child's depth.
    pub child_depth: u32,
    /// The launch's logical resources, resolved on the machine.
    pub resources: RemoteResources,
    /// The agent dir (allocation locks, run journal).
    pub agent_dir: &'a Path,
    /// The ssh transport.
    pub transport: SshTransport,
}

impl PlacedNativeRun {
    /// Start the relay that carries this run's child back to the attempt runner — its NDJSON
    /// stdout, its stderr tail and its exit code — and its live channels both ways
    /// ([`cyrup_herdr::relay::RunRelay`]). The exit resolves to [`RELAY_UNKNOWN_EXIT`] when the
    /// child's state is unknown: the machine was lost past the reconnect budget, or the child's
    /// pane ended without an exit record.
    #[must_use]
    pub fn start_relay(&self) -> RelayedChild {
        let mut relay = RunRelay::new(
            self.transport.clone(),
            self.machine.target.clone(),
            self.runtime_dir.clone(),
            self.run_id.clone(),
        );
        for mirror in &self.down {
            relay = relay.with_down(mirror.clone());
        }
        for mirror in &self.up {
            relay = relay.with_up(mirror.clone());
        }
        let (stdout, events) = tokio::io::duplex(1024 * 1024);
        let (stderr, diagnostics) = tokio::io::duplex(64 * 1024);
        let exit = tokio::spawn(async move {
            match relay.run(events, diagnostics).await {
                RelayEnd::Exited(code) => code,
                RelayEnd::Lost(_) | RelayEnd::Unknown(_) => RELAY_UNKNOWN_EXIT,
            }
        });
        RelayedChild {
            stdout,
            stderr,
            exit,
        }
    }

    async fn remote(
        &self,
        script: &str,
        args: &[&str],
        stdin: Option<&[u8]>,
    ) -> cyrup_herdr::remote::RemoteOutput {
        self.transport
            .run(
                &self.machine.target,
                &remote_shell_command(script, args),
                REMOTE_COMMAND_TIMEOUT,
                MAX_FETCH_BYTES,
                stdin,
            )
            .await
    }

    async fn fetch(&self, name: &str) -> Option<Vec<u8>> {
        let output = self
            .remote(FETCH_SCRIPT, &[&self.runtime_dir, name], None)
            .await;
        (output.error.is_none() && output.status == Some(0)).then(|| output.stdout.into_bytes())
    }

    /// Close the owned pane if it is still ours — same workspace and tab
    /// (`#performCleanup`'s ownership proof, `herdr-placed-run.ts:190-193`).
    async fn close_pane(&self) -> Result<(), String> {
        let (Some(owned), Some(connection)) = (&self.owned, &self.connection) else {
            return Ok(());
        };
        match connection.client.pane_get(owned.pane_id.clone()).await {
            Ok(pane) => {
                if pane.workspace_id != owned.workspace_id || pane.tab_id != owned.tab_id {
                    return Err(
                        "Refusing cleanup because startup pane ownership is uncertain.".to_string(),
                    );
                }
                connection
                    .client
                    .pane_close(owned.pane_id.clone())
                    .await
                    .map_err(|error| error.to_string())
            }
            // Already gone: nothing of ours to close.
            Err(_) => Ok(()),
        }
    }

    /// `cleanup(closeStartedPane)` (`herdr-placed-run.ts:187-196`) plus the settle half of the
    /// bridge: fetch the child's result files, git evidence and stderr tail back (when it
    /// settled), close the pane unless the state is unknown, remove the runtime dir, stop the
    /// forwards.
    pub async fn finish(mut self, disposition: PlacedDisposition) -> PlacedFinish {
        let mut evidence = NativeMachineEvidence::new(self.machine.id.clone());
        let mut stderr_tail = None;
        if disposition == PlacedDisposition::Settled {
            // `decorateHerdrMachineResult`'s `stderr.slice(-4096)` (`subagent-runner.ts:716-719`
            // @v0.68.0): the remote failure hint matches the child's own stderr, not only the
            // error the attempt settled on.
            let tail = self
                .remote(STDERR_TAIL_SCRIPT, &[&self.runtime_dir], None)
                .await;
            if tail.error.is_none() && tail.status == Some(0) {
                stderr_tail = Some(tail.stdout);
            }
            for file in &self.fetches {
                if let Some(bytes) = self.fetch(&file.name).await {
                    let _ = std::fs::write(&file.local, bytes);
                }
            }
            let parse = |bytes: Option<Vec<u8>>| {
                bytes
                    .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
                    .and_then(|value| normalize_git(&value))
            };
            evidence.initial_git = parse(self.fetch("git-initial.json").await);
            evidence.final_git = parse(self.fetch("git-final.json").await);
        }
        if disposition != PlacedDisposition::Unknown {
            let _ = self.close_pane().await;
            let _ = self.remote(REMOVE_SCRIPT, &[&self.runtime_dir], None).await;
        }
        if let Some(connection) = self.connection.take() {
            connection.close().await;
        }
        PlacedFinish {
            evidence,
            stderr_tail,
        }
    }

    /// Tear down a run that never reached its relay (a failure inside
    /// [`prepare_placed_native_attempt`]).
    async fn abandon(self) {
        let _ = self.finish(PlacedDisposition::Aborted).await;
    }
}

/// `normalizeGit(value)` (`herdr-external-adapters.ts:35-43`): only `head`/`branch`/`dirty`, each
/// well-formed, or the whole record is dropped.
#[must_use]
pub fn normalize_git(value: &serde_json::Value) -> Option<RemoteGitStatus> {
    let data = value.as_object()?;
    if data
        .keys()
        .any(|key| !matches!(key.as_str(), "head" | "branch" | "dirty"))
        || value.to_string().len() > 2048
    {
        return None;
    }
    let head = match data.get("head") {
        None => None,
        Some(serde_json::Value::String(head))
            if (4..=64).contains(&head.len()) && head.chars().all(|c| c.is_ascii_hexdigit()) =>
        {
            Some(head.clone())
        }
        Some(_) => return None,
    };
    let branch = match data.get("branch") {
        None => None,
        Some(serde_json::Value::String(branch))
            if !branch.trim().is_empty()
                && branch.len() <= 256
                && !branch.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}') =>
        {
            Some(branch.clone())
        }
        Some(_) => return None,
    };
    let dirty = match data.get("dirty") {
        None => None,
        Some(serde_json::Value::Bool(dirty)) => Some(*dirty),
        Some(_) => return None,
    };
    Some(RemoteGitStatus {
        head,
        branch,
        dirty,
    })
}

/// The deterministic owned-workspace label for a cwd (`herdr-placed-run.ts:140`).
#[must_use]
pub fn owned_workspace_label(cwd: &str) -> String {
    let digest = sha2::Sha256::digest(cwd.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{OWNED_WORKSPACE_PREFIX}{}", hex.get(..10).unwrap_or(&hex))
}

/// `provisionHerdrPane(client, cwd, runId, runtimeDir, env, allocationKey)`
/// (`herdr-placed-run.ts:132-151`): under the allocation lock, reuse the deterministic owned
/// workspace (or the one workspace already holding a pane at this cwd) and open a tab in it, or
/// create the owned workspace; refuse past [`HERDR_MAX_OWNED_PANES`].
///
/// # Errors
/// Upstream's ambiguity and pane-limit sentences, the lock timeout, herdr's refusals.
pub async fn provision_pane(
    client: &cyrup_herdr::HerdrClient,
    cwd: &str,
    run_id: &str,
    env: BTreeMap<String, String>,
    agent_dir: &Path,
    allocation_key: &str,
) -> Result<OwnedPane, String> {
    let _lock = super::lock::acquire_pane_allocation_lock(agent_dir, allocation_key).await?;
    let snapshot = client
        .session_snapshot()
        .await
        .map_err(|error| error.to_string())?;
    let owned_label = owned_workspace_label(cwd);
    let owned: Vec<_> = snapshot
        .workspaces
        .iter()
        .filter(|workspace| workspace.label == owned_label)
        .collect();
    if owned.len() > 1 {
        return Err("Herdr deterministic owned workspace identity is ambiguous.".to_string());
    }
    let cwd_workspace_ids: std::collections::BTreeSet<&str> = snapshot
        .panes
        .iter()
        .filter(|pane| {
            pane.cwd.as_deref() == Some(cwd) || pane.foreground_cwd.as_deref() == Some(cwd)
        })
        .map(|pane| pane.workspace_id.as_str())
        .collect();
    let cwd_workspaces: Vec<_> = snapshot
        .workspaces
        .iter()
        .filter(|workspace| cwd_workspace_ids.contains(workspace.workspace_id.as_str()))
        .collect();
    let selected = owned.first().copied().or_else(|| {
        if cwd_workspaces.len() == 1 {
            cwd_workspaces.first().copied()
        } else {
            None
        }
    });
    if let Some(workspace) = selected {
        let actual = snapshot
            .panes
            .iter()
            .filter(|pane| pane.workspace_id == workspace.workspace_id)
            .count();
        if actual >= HERDR_MAX_OWNED_PANES {
            return Err(format!(
                "Herdr workspace pane limit of {HERDR_MAX_OWNED_PANES} is reached; close an existing pane before launching another placed run."
            ));
        }
        let tail: String = {
            let chars: Vec<char> = run_id.chars().collect();
            chars
                .get(chars.len().saturating_sub(8)..)
                .map(|slice| slice.iter().collect())
                .unwrap_or_default()
        };
        let (tab, pane) = client
            .tab_create(TabCreateParams {
                workspace_id: Some(workspace.workspace_id.clone()),
                cwd: Some(cwd.to_string()),
                focus: false,
                label: Some(format!("subagent-{tail}")),
                env,
            })
            .await
            .map_err(|error| error.to_string())?;
        return Ok(OwnedPane {
            workspace_id: workspace.workspace_id.clone(),
            tab_id: tab.tab_id,
            pane_id: pane.pane_id,
        });
    }
    let (workspace, tab, pane) = client
        .workspace_create(WorkspaceCreateParams {
            source_workspace_id: None,
            cwd: Some(cwd.to_string()),
            focus: false,
            label: Some(owned_label),
            env,
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(OwnedPane {
        workspace_id: workspace.workspace_id,
        tab_id: tab.tab_id,
        pane_id: pane.pane_id,
    })
}

/// `journal(identity)` (`herdr-placed-run.ts:183`): a mode-0600 record of the placed run's
/// identity, written once, so an operator can find a retained pane after the fact.
fn write_journal(agent_dir: &Path, run: &PlacedNativeRun, agent_name: &str) {
    let Some(owned) = &run.owned else {
        return;
    };
    let dir = agent_dir.join("subagents").join("herdr-run-journal");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let record = serde_json::json!({
        "version": 1,
        "identity": {
            "runId": run.run_id,
            "machineId": run.machine.id,
            "target": run.machine.target,
            "session": run.machine.session,
            "workspaceId": owned.workspace_id,
            "tabId": owned.tab_id,
            "paneId": owned.pane_id,
            "agentName": agent_name,
            "cwd": run.machine.cwd,
            "runtimeDir": run.runtime_dir,
        },
        "state": "ready",
        "updatedAt": crate::time::now_epoch_millis(),
    });
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut file) = options.open(dir.join(format!("{}.json", run.run_id))) {
        use std::io::Write;
        let _ = file.write_all(record.to_string().as_bytes());
    }
}

/// `createHerdrPiSession(launch)` (`herdr-placed-run.ts:245-262`) for a native cyrup child:
/// serialize, provision the runtime dir, upload, connect, provision the pane, start the child,
/// journal — and hand back the local relay the attempt runner drives in the child's place.
///
/// Every failure after the runtime dir exists tears down what was built (upstream's
/// `catch (error) { await owner.cleanup(); throw error; }`), and the local spec's temp files are
/// removed once uploaded — the relay owns none.
///
/// # Errors
/// [`serialize_placed_launch`]'s refusals first (nothing touched), then each step's failure.
pub async fn prepare_placed_native_attempt(
    mut spec: ChildSpawnSpec,
    input: PlacedAttemptInput<'_>,
) -> Result<PlacedNativeRun, String> {
    let launch = serialize_placed_launch(
        &spec,
        PlacedLaunchContext {
            child_depth: input.child_depth,
            resources: input.resources.clone(),
        },
    )?;
    let temp_files = std::mem::take(&mut spec.temp_files);
    let result = prepare_inner(&launch, &input).await;
    crate::spawn::cleanup_temp_files(&temp_files);
    result
}

async fn prepare_inner(
    launch: &RemoteLaunch,
    input: &PlacedAttemptInput<'_>,
) -> Result<PlacedNativeRun, String> {
    let machine = input.machine;
    let run_id = validate_run_id(&safe_run_id(input.run_id))?;
    let transport = input.transport.clone();
    let made = transport
        .run(
            &machine.target,
            &remote_shell_command(MKTEMP_SCRIPT, &[&run_id]),
            Duration::from_secs(10),
            4096,
            None,
        )
        .await;
    if let Some(error) = &made.error {
        return Err(format!(
            "Could not provision a run-private remote runtime directory: {error}"
        ));
    }
    let runtime_dir = made.stdout.trim().to_string();
    if made.status != Some(0) || !safe_runtime_dir(&runtime_dir) || runtime_dir.contains('\n') {
        return Err("Could not provision a run-private remote runtime directory.".to_string());
    }
    let mut run = PlacedNativeRun {
        machine: machine.clone(),
        transport: transport.clone(),
        connection: None,
        run_id: run_id.clone(),
        runtime_dir: runtime_dir.clone(),
        owned: None,
        fetches: launch.fetches.clone(),
        down: launch.down.clone(),
        up: launch.up.clone(),
    };
    match provision(&mut run, launch, input).await {
        Ok(()) => {
            write_journal(input.agent_dir, &run, input.agent_name);
            Ok(run)
        }
        Err(error) => {
            run.abandon().await;
            Err(error)
        }
    }
}

async fn provision(
    run: &mut PlacedNativeRun,
    launch: &RemoteLaunch,
    input: &PlacedAttemptInput<'_>,
) -> Result<(), String> {
    let (argv, env) = launch.bind(&run.runtime_dir);
    for file in &launch.uploads {
        let bytes = std::fs::read(&file.local).map_err(|error| {
            format!(
                "Could not read '{}' for upload to {}: {error}",
                file.local.display(),
                run.machine.display_name()
            )
        })?;
        let sent = run
            .remote(UPLOAD_SCRIPT, &[&run.runtime_dir, &file.name], Some(&bytes))
            .await;
        if sent.error.is_some() || sent.status != Some(0) {
            return Err(format!(
                "Could not upload '{}' to the placed run on {}: {}",
                file.name,
                run.machine.display_name(),
                sent.error.unwrap_or_else(|| sent.stderr.trim().to_string())
            ));
        }
    }
    let banner = format!(
        "cyrup subagent '{}' (placed run {}) on {} — events: {}/events.ndjson",
        input.agent_name,
        run.run_id,
        run.machine.display_name(),
        run.runtime_dir
    );
    let script = render_run_script(&run.runtime_dir, &run.machine.cwd, &argv, &env, &banner);
    let sent = run
        .remote(
            UPLOAD_SCRIPT,
            &[&run.runtime_dir, "run.sh"],
            Some(script.as_bytes()),
        )
        .await;
    if sent.error.is_some() || sent.status != Some(0) {
        return Err(format!(
            "Could not upload the placed run script to {}: {}",
            run.machine.display_name(),
            sent.error.unwrap_or_else(|| sent.stderr.trim().to_string())
        ));
    }
    let connection = run
        .transport
        .connect(&run.machine.target, run.machine.session.as_deref())
        .await
        .map_err(|error| error.to_string())?;
    let session = connection.endpoint.session.clone();
    run.connection = Some(connection);
    let mut pane_env = BTreeMap::new();
    pane_env.insert(RUN_ID_ENV.to_string(), run.run_id.clone());
    pane_env.insert(RUNTIME_DIR_ENV.to_string(), run.runtime_dir.clone());
    let key =
        super::lock::pane_allocation_key(&run.machine.target, session.as_deref(), &run.machine.cwd);
    let client = run
        .connection
        .as_ref()
        .map(|connection| connection.client.clone())
        .ok_or_else(|| "Herdr placement owner has no connection.".to_string())?;
    let owned = provision_pane(
        &client,
        &run.machine.cwd,
        &run.run_id,
        pane_env,
        input.agent_dir,
        &key,
    )
    .await?;
    run.owned = Some(owned.clone());
    client
        .pane_send_input(PaneSendInputParams::run(
            owned.pane_id.clone(),
            format!("sh {}/run.sh", run.runtime_dir),
        ))
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}
