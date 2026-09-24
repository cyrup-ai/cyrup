//! SUBA-100 — Herdr saved-machine placement: run a subagent on a machine the operator saved in
//! herdr (`herdr machine add me@gpu-box --label gpu-box`), not on this one.
//!
//! Ported from pi-subagents @v0.68.0: `src/runs/shared/herdr-machine.ts` (the catalog, the
//! `subagents.machines` settings, the cwd rules, the runner refusals, the failure hints),
//! `herdr-placed-run.ts` (the owned pane), `herdr-external-adapters.ts` (Claude/Codex/Cursor in a
//! pane) and `herdr-connection.ts` — whose transport half lives in the workspace's one herdr
//! client, [`cyrup_herdr::remote`], alongside the catalog reader ([`cyrup_herdr::machine`]) and
//! the four placement verbs this feature added there.
//!
//! # The one rule everything here keeps
//!
//! **herdr owns which machines exist and how ssh reaches them; this crate owns what runs there;
//! ssh is bounded transport and never owns the agent** (`herdr-machine.ts:10-17`). A placed child
//! is launched into a fresh herdr-owned, visible pane on the machine. Every ssh process this
//! feature starts either finishes under a deadline or only relays bytes the pane's child already
//! wrote to disk, so losing ssh never kills the agent — it loses the view of it, which is
//! reconnected (bounded) or reported as unknown with the pane retained for inspection.
//!
//! # Where placement comes from, and where it lands
//!
//! `machine` is read from agent frontmatter, `agentOverrides.<name>.machine` (`false` clears), a
//! runtime agent definition, management `create`/`update` `config.machine`, and the tool call —
//! top level, and per chain step / parallel task / dynamic template — with upstream's precedence
//! `step.machine ?? call.machine ?? agent.machine` (`async-execution.ts:990` @v0.68.0). A launch
//! resolves it ONCE, before any run exists ([`resolve::PlacementResolver::resolve`]), into a
//! [`HerdrMachineReference`] carried on the step and on [`crate::exec::RunOptions::machine`], and
//! [`crate::exec::run_sync`] runs the child there: [`native`] for a native cyrup child,
//! [`external`] for the six code-owned external profiles. Everything upstream refuses is refused
//! at the same point with upstream's words ([`resolve::format_herdr_machine_runner_unsupported`],
//! [`native::serialize_placed_launch`]).
//!
//! `cwd` means **the directory on that machine** (`herdr-machine.ts:16`): a launch with a machine
//! never resolves its `cwd` locally, and the remote `cd` is the existence check.

pub mod external;
pub mod hints;
pub mod lock;
pub mod native;
pub mod remote_resources;
pub mod resolve;

#[cfg(test)]
mod tests;

pub use resolve::{
    MachineCatalogSource, MachineSettingsEntry, MachineSettingsSource, PlacementResolver,
    format_herdr_machine_runner_unsupported, resolve_graph_placements, validate_optional_machine,
};

/// The provider discriminant of a [`HerdrMachineReference`] — pi's literal `provider: "herdr"`
/// (`shared/types.ts:1730` @v0.68.0). A one-variant enum rather than a string, so a reference
/// read back from a runner config or a recovery descriptor is refused unless it says `"herdr"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MachineProvider {
    /// herdr's saved SSH machines.
    #[default]
    Herdr,
}

/// A Herdr saved machine resolved for one launch — pi `HerdrMachineReference`
/// (`shared/types.ts:1728-1736` @v0.68.0). `cwd` is the directory ON THAT MACHINE.
///
/// Serialized camelCase exactly as upstream spells it, because it crosses the hop-2 detached
/// runner boundary on the step and is reported on the external runner status.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrMachineReference {
    /// Always [`MachineProvider::Herdr`].
    pub provider: MachineProvider,
    /// herdr's profile id.
    pub id: String,
    /// herdr's label, when the profile has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The ssh target, validated safe to pass as one argv element.
    pub target: String,
    /// The explicit remote herdr session; `None` for the default session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The launch directory on the machine: absolute POSIX, or `~`-relative.
    pub cwd: String,
    /// The LOCAL ssh transport the launch that resolved this reference reaches the machine
    /// through — its ssh binary and agent socket, read from the launching extension's environment
    /// (`SubagentExtensionConfig::env_overrides` over the process). Never serialized: it names
    /// local state, and a detached runner reads its own from the environment it was started with,
    /// into which the launch forwards the same keys.
    #[serde(skip)]
    pub transport: Option<cyrup_herdr::remote::SshTransport>,
    /// The launching extension's agent dir — where the cross-process pane-allocation locks and
    /// the placed-run journal live (`getAgentDir()` upstream). Never serialized, for the same
    /// reason as `transport`; a detached runner falls back to its own resolved agent dir.
    #[serde(skip)]
    pub agent_dir: Option<std::path::PathBuf>,
}

/// One chain/parallel/dynamic step's placement, carried on
/// [`crate::spawn::chain_graph::SingleStepSpec::machine`] across the hop-2 runner boundary.
///
/// `requested` is the selector as the step (or, folded in at launch, the call or the agent) named
/// it — pi's `s.machine ?? launchMachine ?? a.machine` (`async-execution.ts:990` @v0.68.0).
/// `resolved` is filled ONCE, at launch, before any run exists
/// ([`resolve::resolve_graph_placements`]); a step that reaches execution with `requested` but no
/// `resolved` was never launched through a placement-resolving path and is refused rather than run
/// locally.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepPlacement {
    /// The machine id or label as requested.
    pub requested: String,
    /// The launch-resolved machine, cwd included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<HerdrMachineReference>,
}

impl StepPlacement {
    /// A step that names `machine` and has not been resolved yet.
    #[must_use]
    pub fn requested(machine: &str) -> Self {
        Self {
            requested: machine.to_string(),
            resolved: None,
        }
    }
}

impl HerdrMachineReference {
    /// The agent dir this run's locks and journal live under: the launch's own, else this
    /// process's resolved one.
    #[must_use]
    pub fn agent_dir(&self) -> std::path::PathBuf {
        self.agent_dir
            .clone()
            .unwrap_or_else(crate::paths::agent_dir)
    }

    /// The transport this run reaches the machine through: the launch's own, else one read from
    /// this process's environment.
    #[must_use]
    pub fn ssh_transport(&self) -> cyrup_herdr::remote::SshTransport {
        self.transport.clone().unwrap_or_else(|| {
            cyrup_herdr::remote::SshTransport::with_env(&cyrup_herdr::env::ProcessEnv)
        })
    }

    /// `machine.label ?? machine.id` — the name every upstream sentence uses for a machine.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.id)
    }
}
