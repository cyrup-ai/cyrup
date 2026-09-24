//! Turning a typed machine name into a [`HerdrMachineReference`] — `resolveHerdrMachinePlacement`
//! and its validators (`src/runs/shared/herdr-machine.ts:19-235` @v0.68.0) — plus the two
//! launch-time refusals every entry path shares: [`format_herdr_machine_runner_unsupported`] and
//! the config validator [`validate_optional_machine`] (`src/agents/agents.ts:979-986`).
//!
//! Every sentence is upstream's, with two adaptations and no others: the product name (`Pi` →
//! `cyrup`, `pi-subagents` → `cyrup`) and the settings file a root is configured in
//! (`.pi/settings.json` → `.cyrup/agents/settings.json`, where this port keeps every
//! `subagents.*` key — [`crate::discovery::project_settings_path`]).

use std::path::{Path, PathBuf};

use cyrup_herdr::machine::{MachineProfile, parse_machine_catalog, read_machine_catalog};

use super::{HerdrMachineReference, MachineProvider};
use crate::runner::AgentRunnerConfig;

/// `MAX_MACHINE_NAME_LENGTH` (`herdr-machine.ts:19`).
pub const MAX_MACHINE_NAME_LENGTH: usize = 128;

fn has_control(value: &str) -> bool {
    value.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
}

/// pi `validateOptionalMachine(value, label)` (`agents.ts:979-986`): the one validator every
/// CONFIG surface — frontmatter, `agentOverrides`, management `config.machine`, a runtime
/// definition — runs a machine through. `None`/`false` is "no machine"; anything else must be a
/// non-empty string, at most 128 characters once trimmed, with no control characters.
///
/// # Errors
/// `"<label> must be a non-empty string or false."`, `"<label> must be 128 characters or
/// fewer."`, or `"<label> contains control characters."`.
pub fn validate_optional_machine(
    value: Option<&serde_json::Value>,
    label: &str,
) -> Result<Option<String>, String> {
    match value {
        None | Some(serde_json::Value::Bool(false)) => Ok(None),
        Some(serde_json::Value::String(raw)) if !raw.trim().is_empty() => {
            let machine = raw.trim();
            if machine.chars().count() > MAX_MACHINE_NAME_LENGTH {
                return Err(format!("{label} must be 128 characters or fewer."));
            }
            if has_control(machine) {
                return Err(format!("{label} contains control characters."));
            }
            Ok(Some(machine.to_string()))
        }
        Some(_) => Err(format!("{label} must be a non-empty string or false.")),
    }
}

/// `validateMachineName(value)` (`herdr-machine.ts:66-72`) — the launch-time check on the typed
/// selector (a call parameter never passed through [`validate_optional_machine`]).
///
/// # Errors
/// pi's three sentences.
pub fn validate_machine_name(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("Herdr machine id or label is required.".to_string());
    }
    if trimmed.chars().count() > MAX_MACHINE_NAME_LENGTH {
        let head: String = trimmed.chars().take(24).collect();
        return Err(format!(
            "Herdr machine '{head}…' exceeds {MAX_MACHINE_NAME_LENGTH} characters."
        ));
    }
    if has_control(trimmed) {
        return Err("Herdr machine id or label contains control characters.".to_string());
    }
    Ok(trimmed.to_string())
}

/// `validateTarget(value, requested)` (`herdr-machine.ts:74-80`): the ssh target becomes ONE argv
/// element after every option, so it must not be empty, must not start with `-` (an option
/// injection), and must hold no whitespace or control character.
///
/// # Errors
/// `"Herdr machine '<requested>' has an ssh target that cannot be passed safely: <json>."`
pub fn validate_target(value: &str, requested: &str) -> Result<String, String> {
    let target = value.trim();
    if target.is_empty()
        || target.starts_with('-')
        || target
            .chars()
            .any(|c| c.is_whitespace() || c <= '\u{1f}' || c == '\u{7f}')
    {
        return Err(format!(
            "Herdr machine '{requested}' has an ssh target that cannot be passed safely: {}.",
            serde_json::to_string(target).unwrap_or_default()
        ));
    }
    Ok(target.to_string())
}

/// `isRemoteAbsolute(value)` (`herdr-machine.ts:82-84`).
#[must_use]
pub fn is_remote_absolute(value: &str) -> bool {
    value.starts_with('/') || value == "~" || value.starts_with("~/")
}

/// `validateRemoteCwd(value, requested)` (`herdr-machine.ts:86-91`): absolute or `~`-relative, no
/// control characters, trailing slashes dropped (but `/` stays `/`).
///
/// # Errors
/// pi's two sentences.
pub fn validate_remote_cwd(value: &str, requested: &str) -> Result<String, String> {
    let cwd = value.trim();
    if !is_remote_absolute(cwd) {
        return Err(format!(
            "Herdr machine '{requested}' cwd must be an absolute POSIX path or start with '~': {}.",
            serde_json::to_string(cwd).unwrap_or_default()
        ));
    }
    if has_control(cwd) {
        return Err(format!(
            "Herdr machine '{requested}' cwd contains control characters."
        ));
    }
    if cwd.chars().count() > 1 {
        let stripped = cwd.trim_end_matches('/');
        Ok(if stripped.is_empty() { "/" } else { stripped }.to_string())
    } else {
        Ok(cwd.to_string())
    }
}

/// `path.posix.join(root, relative)` — join, then normalize `.`/`..`/empty segments the way node
/// does (a `..` above an absolute root is dropped; above a relative one it is kept).
#[must_use]
pub fn posix_join(root: &str, relative: &str) -> String {
    let joined = if root.is_empty() {
        relative.to_string()
    } else if relative.is_empty() {
        root.to_string()
    } else {
        format!("{root}/{relative}")
    };
    let absolute = joined.starts_with('/');
    let trailing = joined.ends_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for segment in joined.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|last| *last != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let mut out = parts.join("/");
    if absolute {
        out.insert(0, '/');
    }
    if out.is_empty() {
        return if absolute { "/" } else { "." }.to_string();
    }
    if trailing && !out.ends_with('/') {
        out.push('/');
    }
    out
}

/// One `subagents.machines.<name>` entry (`MachineSettingsEntry`, `herdr-machine.ts:38-41`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MachineSettingsEntry {
    /// The machine's root directory for this repo.
    pub cwd: Option<String>,
    /// Opt-in environment. Upstream refuses any non-empty map — saved-machine runs use the remote
    /// environment — so a present value here is only ever the refusal's input.
    pub env: Option<std::collections::BTreeMap<String, String>>,
}

/// Where [`PlacementResolver::resolve`] reads `subagents.machines` from.
#[derive(Clone, Debug)]
pub enum MachineSettingsSource {
    /// The two settings files, project over user, field by field (`readMachineSettings`,
    /// `herdr-machine.ts:176-183`) — the same pair discovery reads `agentOverrides` from.
    Files {
        /// The user-scope `settings.json`.
        user: PathBuf,
        /// The project-scope `settings.json`, when a project root was found.
        project: Option<PathBuf>,
    },
    /// Upstream's `settings` test seam: this entry instead of reading any file.
    Entry(Option<MachineSettingsEntry>),
}

/// Where the herdr catalog comes from.
#[derive(Clone, Debug)]
pub enum MachineCatalogSource {
    /// `herdr machine list --json`, through `bin` or the `HERDR_BIN` ladder.
    Herdr {
        /// An explicit binary (`herdrBin`), else `HERDR_BIN`, else `herdr`.
        bin: Option<String>,
    },
    /// Upstream's `catalogJson` test seam.
    Json(String),
}

/// `selectMachine(catalog, requested)` (`herdr-machine.ts:135-146`): profile id first, then a
/// unique case-sensitive label; a disabled machine fails closed.
///
/// # Errors
/// pi's ambiguous / not-found / disabled sentences.
pub fn select_machine<'a>(
    catalog: &'a [MachineProfile],
    requested: &str,
) -> Result<&'a MachineProfile, String> {
    let by_id = catalog.iter().find(|entry| entry.id == requested);
    let matches: Vec<&MachineProfile> = match by_id {
        Some(entry) => vec![entry],
        None => catalog
            .iter()
            .filter(|entry| entry.label.as_deref() == Some(requested))
            .collect(),
    };
    if matches.len() > 1 {
        return Err(format!(
            "Machine label '{requested}' is ambiguous; use its profile ID."
        ));
    }
    let Some(machine) = matches.first().copied() else {
        let saved: Vec<&str> = catalog
            .iter()
            .filter(|entry| entry.enabled)
            .map(|entry| entry.label.as_deref().unwrap_or(&entry.id))
            .collect();
        return Err(format!(
            "Herdr machine '{requested}' was not found. Saved machines: {}. Add one with herdr machine add <target> --label <name>.",
            if saved.is_empty() {
                "none".to_string()
            } else {
                saved.join(", ")
            }
        ));
    };
    if !machine.enabled {
        return Err(format!(
            "Machine '{requested}' is disabled. Run herdr machine enable {}.",
            machine.id
        ));
    }
    Ok(machine)
}

/// `readJsonObject(filePath)` (`herdr-machine.ts:148-157`).
fn read_json_object(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(serde_json::Map::new());
        }
        Err(error) => {
            return Err(format!(
                "Failed to read settings file '{}': {error}",
                path.display()
            ));
        }
    };
    let parsed: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("Failed to read settings file '{}': {error}", path.display()))?;
    Ok(match parsed {
        serde_json::Value::Object(object) => object,
        _ => serde_json::Map::new(),
    })
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `machineSettingsFrom(settings, keys, filePath)` (`herdr-machine.ts:159-181`).
fn machine_settings_from(
    settings: &serde_json::Map<String, serde_json::Value>,
    keys: &[String],
    path: &Path,
) -> Result<Option<MachineSettingsEntry>, String> {
    let file = path.display();
    let Some(subagents) = settings
        .get("subagents")
        .and_then(|value| value.as_object())
    else {
        return Ok(None);
    };
    let Some(machines) = subagents.get("machines") else {
        return Ok(None);
    };
    let Some(machines) = machines.as_object() else {
        return Err(format!(
            "Subagent settings in '{file}' have invalid 'machines'; expected an object keyed by machine label or id."
        ));
    };
    let Some(key) = keys
        .iter()
        .find(|candidate| machines.contains_key(*candidate))
    else {
        return Ok(None);
    };
    let Some(record) = machines.get(key).and_then(|value| value.as_object()) else {
        return Err(format!(
            "Subagent settings in '{file}' have invalid 'machines.{key}'; expected an object with 'cwd' and optional 'env'."
        ));
    };
    let mut entry = MachineSettingsEntry::default();
    if let Some(cwd) = record.get("cwd") {
        match cwd.as_str() {
            Some(text) if !text.trim().is_empty() => {
                entry.cwd = Some(validate_remote_cwd(text, key)?);
            }
            _ => {
                return Err(format!(
                    "Subagent settings in '{file}' have invalid 'machines.{key}.cwd'; expected a non-empty string."
                ));
            }
        }
    }
    if let Some(env) = record.get("env") {
        let valid = env.as_object().filter(|object| {
            object
                .iter()
                .all(|(name, item)| valid_env_name(name) && item.is_string())
        });
        let Some(object) = valid else {
            return Err(format!(
                "Subagent settings in '{file}' have invalid 'machines.{key}.env'; expected an object of string values keyed by variable name."
            ));
        };
        if !object.is_empty() {
            return Err(format!(
                "Subagent settings in '{file}' set 'machines.{key}.env'. Saved-machine runs use the remote Herdr/cyrup environment; configure credentials and environment on '{key}' and remove the local env map."
            ));
        }
        entry.env = Some(std::collections::BTreeMap::new());
    }
    Ok(Some(entry))
}

/// `readMachineSettings(cwd, keys)` (`herdr-machine.ts:183-191`): project beats user, field by
/// field.
fn read_machine_settings(
    user: &Path,
    project: Option<&Path>,
    keys: &[String],
) -> Result<Option<MachineSettingsEntry>, String> {
    let user_entry = machine_settings_from(&read_json_object(user)?, keys, user)?;
    let project_entry = match project {
        Some(path) if path != user => machine_settings_from(&read_json_object(path)?, keys, path)?,
        _ => None,
    };
    if user_entry.is_none() && project_entry.is_none() {
        return Ok(None);
    }
    let user_entry = user_entry.unwrap_or_default();
    let project_entry = project_entry.unwrap_or_default();
    Ok(Some(MachineSettingsEntry {
        cwd: project_entry.cwd.or(user_entry.cwd),
        env: project_entry.env.or(user_entry.env),
    }))
}

async fn read_catalog(source: &MachineCatalogSource) -> Result<Vec<MachineProfile>, String> {
    match source {
        MachineCatalogSource::Json(json) => {
            parse_machine_catalog(json).map_err(|error| error.to_string())
        }
        MachineCatalogSource::Herdr { bin } => {
            read_machine_catalog(bin.as_deref(), &cyrup_herdr::env::ProcessEnv)
                .await
                .map_err(|error| error.to_string())
        }
    }
}

/// One launch's placement resolver: reads the catalog at most ONCE however many steps name a
/// machine (upstream re-runs `herdr machine list --json` per step; the answer cannot change
/// within one launch, and a chain of twenty placed steps should not pay twenty process spawns).
#[derive(Debug)]
pub struct PlacementResolver {
    settings: MachineSettingsSource,
    catalog_source: MachineCatalogSource,
    catalog: Option<Vec<MachineProfile>>,
    transport: Option<cyrup_herdr::remote::SshTransport>,
    agent_dir: Option<PathBuf>,
}

impl PlacementResolver {
    /// A resolver over `settings`, reading the catalog from herdr.
    #[must_use]
    pub fn new(settings: MachineSettingsSource) -> Self {
        Self {
            settings,
            catalog_source: MachineCatalogSource::Herdr { bin: None },
            catalog: None,
            transport: None,
            agent_dir: None,
        }
    }

    /// Stamp every resolved reference with the launching extension's agent dir.
    #[must_use]
    pub fn with_agent_dir(mut self, agent_dir: PathBuf) -> Self {
        self.agent_dir = Some(agent_dir);
        self
    }

    /// Stamp every resolved reference with this local ssh transport.
    #[must_use]
    pub fn with_transport(mut self, transport: cyrup_herdr::remote::SshTransport) -> Self {
        self.transport = Some(transport);
        self
    }

    /// Upstream's `catalogJson` seam.
    #[must_use]
    pub fn with_catalog(mut self, catalog: MachineCatalogSource) -> Self {
        self.catalog_source = catalog;
        self
    }

    /// `resolveHerdrMachinePlacement(input)` (`herdr-machine.ts:203-229` @v0.68.0): validate the
    /// selector, read the catalog (once per resolver), select the machine, read its settings
    /// (keyed by the typed selector, its label, or its id), refuse a local env map, and resolve
    /// `machine`'s remote cwd from `step_cwd` and the configured root.
    ///
    /// # Errors
    /// Every upstream sentence along the way.
    pub async fn resolve(
        &mut self,
        machine: &str,
        step_cwd: Option<&str>,
    ) -> Result<HerdrMachineReference, String> {
        let requested = validate_machine_name(machine)?;
        if self.catalog.is_none() {
            self.catalog = Some(read_catalog(&self.catalog_source).await?);
        }
        let catalog = self.catalog.as_deref().unwrap_or_default();
        resolve_with_catalog(&requested, catalog, &self.settings, step_cwd).map(|placement| {
            HerdrMachineReference {
                transport: self.transport.clone(),
                agent_dir: self.agent_dir.clone(),
                ..placement
            }
        })
    }
}

/// What a launch knows about one step's agent when folding its placement.
#[derive(Clone, Copy, Debug)]
pub struct PlacementAgent<'a> {
    /// The agent's name, for the refusal sentences.
    pub name: &'a str,
    /// The agent rung (`a.machine`).
    pub machine: Option<&'a str>,
    /// Its runner, for [`format_herdr_machine_runner_unsupported`].
    pub runner: Option<&'a AgentRunnerConfig>,
}

/// Fold one step's placement at launch — pi `buildSeqStep`'s machine block
/// (`async-execution.ts:990-1001` @v0.68.0): `s.machine ?? launchMachine ?? a.machine`, the
/// runner/worktree refusal, then `resolveHerdrMachinePlacement({ machine, stepCwd: s.cwd ??
/// params.machineCwd })`. A placed step's `cwd` moves INTO the resolved reference and is cleared
/// from the step, so nothing downstream resolves it locally.
///
/// # Errors
/// The refusal or the resolution's sentence.
pub async fn fold_step_placement(
    spec: &mut crate::spawn::chain_graph::SingleStepSpec,
    agent: PlacementAgent<'_>,
    call_machine: Option<&str>,
    call_machine_cwd: Option<&str>,
    worktree: bool,
    resolver: &mut PlacementResolver,
) -> Result<(), String> {
    if spec
        .machine
        .as_ref()
        .is_some_and(|placement| placement.resolved.is_some())
    {
        return Ok(());
    }
    let requested = spec
        .machine
        .as_ref()
        .map(|placement| placement.requested.clone())
        .or_else(|| call_machine.map(str::to_string))
        .or_else(|| agent.machine.map(str::to_string));
    let Some(requested) = requested else {
        return Ok(());
    };
    if let Some(refusal) = format_herdr_machine_runner_unsupported(
        Some(&requested),
        agent.name,
        agent.runner,
        worktree,
    ) {
        return Err(refusal);
    }
    let step_cwd = spec
        .cwd
        .as_ref()
        .map(|cwd| cwd.to_string_lossy().into_owned())
        .or_else(|| call_machine_cwd.map(str::to_string));
    let resolved = resolver.resolve(&requested, step_cwd.as_deref()).await?;
    spec.cwd = None;
    spec.machine = Some(super::StepPlacement {
        requested,
        resolved: Some(resolved),
    });
    Ok(())
}

/// The CALL rung of `s.machine ?? params.machine ?? a.machine`, applied at the tool boundary the
/// way `fast`'s is (`crate::extension::host::slash_render::apply_call_fast`): every step that does
/// not name its own machine takes the call's, and — because every step of such a call is then
/// placed — a step with no `cwd` of its own takes the call's `machineCwd` (pi `stepCwd: s.cwd ??
/// params.machineCwd`, `async-execution.ts:999`). A call without a machine changes nothing.
pub fn apply_call_machine(
    graph: &mut [crate::spawn::chain_graph::RunnerStep],
    call_machine: Option<&str>,
    call_machine_cwd: Option<&str>,
) {
    use crate::spawn::chain_graph::RunnerStep;
    let Some(machine) = call_machine.map(str::trim).filter(|m| !m.is_empty()) else {
        return;
    };
    let mut apply = |spec: &mut crate::spawn::chain_graph::SingleStepSpec| {
        if spec.machine.is_none() {
            spec.machine = Some(super::StepPlacement::requested(machine));
        }
        if spec.cwd.is_none()
            && let Some(cwd) = call_machine_cwd
        {
            spec.cwd = Some(PathBuf::from(cwd));
        }
    };
    for step in graph.iter_mut() {
        match step {
            RunnerStep::SingleStep(spec) => apply(spec),
            RunnerStep::ParallelGroup(group) => group.steps.iter_mut().for_each(&mut apply),
            RunnerStep::DynamicGroup(group) => apply(group.template.as_mut()),
            RunnerStep::ImportAsyncRoot(_) => {}
        }
    }
}

/// Fold every step of a launch graph ([`fold_step_placement`] per single step, parallel member —
/// with its group's `worktree` — and dynamic template). `agent_for` answers a step's agent by
/// the name the step carries.
///
/// # Errors
/// The first step's refusal or resolution failure; the graph is then not launched.
pub async fn resolve_graph_placements<'a>(
    graph: &mut [crate::spawn::chain_graph::RunnerStep],
    agent_for: impl Fn(&str) -> Option<PlacementAgent<'a>>,
    call_machine: Option<&str>,
    call_machine_cwd: Option<&str>,
    resolver: &mut PlacementResolver,
) -> Result<(), String> {
    use crate::spawn::chain_graph::RunnerStep;
    for step in graph.iter_mut() {
        let (specs, worktree): (Vec<&mut crate::spawn::chain_graph::SingleStepSpec>, bool) =
            match step {
                RunnerStep::SingleStep(spec) => (vec![spec], false),
                RunnerStep::ParallelGroup(group) => {
                    let worktree = group.worktree;
                    (group.steps.iter_mut().collect(), worktree)
                }
                RunnerStep::DynamicGroup(group) => (vec![group.template.as_mut()], false),
                // An imported async root launches no child of its own.
                RunnerStep::ImportAsyncRoot(_) => (Vec::new(), false),
            };
        for spec in specs {
            let name = spec.agent.clone();
            let agent = agent_for(&name).unwrap_or(PlacementAgent {
                name: "",
                machine: None,
                runner: None,
            });
            let agent = PlacementAgent {
                name: if agent.name.is_empty() {
                    name.as_str()
                } else {
                    agent.name
                },
                ..agent
            };
            fold_step_placement(
                spec,
                agent,
                call_machine,
                call_machine_cwd,
                worktree,
                resolver,
            )
            .await?;
        }
    }
    Ok(())
}

/// The sync half of [`PlacementResolver::resolve`], over an already-read catalog.
fn resolve_with_catalog(
    requested: &str,
    catalog: &[MachineProfile],
    settings_source: &MachineSettingsSource,
    step_cwd: Option<&str>,
) -> Result<HerdrMachineReference, String> {
    let requested = requested.to_string();
    let selected = select_machine(catalog, &requested)?;
    let name = selected
        .label
        .clone()
        .unwrap_or_else(|| selected.id.clone());
    let mut keys = vec![requested.clone()];
    if let Some(label) = &selected.label
        && !keys.contains(label)
    {
        keys.push(label.clone());
    }
    if !keys.contains(&selected.id) {
        keys.push(selected.id.clone());
    }
    let settings = match settings_source {
        MachineSettingsSource::Entry(entry) => entry.clone(),
        MachineSettingsSource::Files { user, project } => {
            read_machine_settings(user, project.as_deref(), &keys)?
        }
    };
    if settings
        .as_ref()
        .and_then(|entry| entry.env.as_ref())
        .is_some_and(|env| !env.is_empty())
    {
        return Err(format!(
            "Saved-machine environment for '{name}' must be configured remotely. Remove machines.{name}.env and configure the remote Herdr/cyrup session instead."
        ));
    }
    let step_cwd = step_cwd.map(str::trim).filter(|cwd| !cwd.is_empty());
    let configured = settings.as_ref().and_then(|entry| entry.cwd.as_deref());
    let cwd = match (step_cwd, configured) {
        (Some(step), _) if is_remote_absolute(step) => step.to_string(),
        (Some(step), Some(root)) => posix_join(root, step),
        (None, Some(root)) => root.to_string(),
        _ => {
            return Err(format!(
                "No root for {name} in this repo. Set subagents.machines.{name}.cwd in .cyrup/agents/settings.json or pass an absolute cwd on that machine."
            ));
        }
    };
    // pi's `HerdrMachinePlacement` (`herdr-machine.ts:59-63`) pairs the reference with an `env`
    // that is always absent on success (a non-empty map is refused above), so only the reference
    // remains.
    Ok(HerdrMachineReference {
        provider: MachineProvider::Herdr,
        id: selected.id.clone(),
        label: selected.label.clone(),
        target: validate_target(&selected.target, &requested)?,
        session: selected
            .session
            .clone()
            .filter(|session| session != "default"),
        cwd: validate_remote_cwd(&cwd, &requested)?,
        transport: None,
        agent_dir: None,
    })
}

/// `formatHerdrMachineRunnerUnsupported(input)` (`herdr-machine.ts:231-248`): the refusal a launch
/// raises BEFORE a run exists when a machine is requested for a combination pane-native placement
/// cannot honour. `None` when no machine is requested, or the combination is supported.
///
/// Upstream's `win32` arm is compiled only on Windows, where StreamLocal forwarding is the gap.
#[must_use]
pub fn format_herdr_machine_runner_unsupported(
    machine: Option<&str>,
    agent_name: &str,
    runner: Option<&AgentRunnerConfig>,
    worktree: bool,
) -> Option<String> {
    let machine = machine?;
    match runner {
        Some(AgentRunnerConfig::ExternalJob(_)) => {
            return Some(format!(
                "Agent '{agent_name}' requested machine '{machine}', but this runner cannot use pane-native Herdr placement. Use native cyrup or a built-in Claude, Codex, or Cursor profile."
            ));
        }
        Some(AgentRunnerConfig::ExternalCli(cli)) if cli.adapter.is_none() => {
            return Some(format!(
                "Agent '{agent_name}' requested machine '{machine}', but generic external-cli commands cannot be remote-wrapped safely. Use claude-code, claude-code-writer, codex-exec, codex-exec-writer, cursor-agent, or cursor-agent-writer."
            ));
        }
        None | Some(AgentRunnerConfig::Pi | AgentRunnerConfig::ExternalCli(_)) => {}
    }
    if worktree {
        return Some(format!(
            "Agent '{agent_name}' requested machine '{machine}', but managed worktrees are local git operations and cannot be combined with a Herdr saved machine."
        ));
    }
    if cfg!(windows) {
        return Some(
            "Herdr saved-machine pane transport requires hardened OpenSSH StreamLocal forwarding, which is not supported from a Windows host yet."
                .to_string(),
        );
    }
    None
}

/// `resolveExternalCliBinaryAvailability("ssh", env)` for a placed agent (`agent-management.ts:
/// 691-704` @v0.68.0): a placed agent checks only LOCAL ssh at list time. The binary is the one
/// [`cyrup_herdr::remote::SshTransport::with_env`] would run, looked up the way it would be — an
/// absolute path as given, a bare name on the hardened ssh `PATH`.
#[must_use]
pub fn ssh_transport_available() -> bool {
    let transport = cyrup_herdr::remote::SshTransport::with_env(&cyrup_herdr::env::ProcessEnv);
    let bin = transport.bin();
    let executable = |path: &Path| {
        std::fs::metadata(path).is_ok_and(|meta| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                meta.is_file() && meta.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                meta.is_file()
            }
        })
    };
    if bin.contains('/') {
        return executable(Path::new(bin));
    }
    cyrup_herdr::remote::HARDENED_SSH_PATH
        .split(':')
        .any(|dir| executable(&Path::new(dir).join(bin)))
}

/// `runnerListBadge(agent, …)`'s placed arms (`agent-management.ts:709-718` @v0.68.0).
#[must_use]
pub fn placement_list_badge(machine: &str, runner: Option<&AgentRunnerConfig>) -> String {
    match runner {
        Some(AgentRunnerConfig::ExternalCli(cli)) => format!(
            "external-cli:{} @ {machine} saved Herdr placement; transport {}; machine not preflighted",
            cli.command,
            if ssh_transport_available() {
                "✓"
            } else {
                "missing"
            }
        ),
        _ => format!("machine: {machine} (saved Herdr placement)"),
    }
}

/// The launch-time settings pair for [`MachineSettingsSource::Files`], from the discovery config
/// every executor already built — so placement reads `subagents.machines` from exactly the files
/// `agentOverrides` came from.
#[must_use]
pub fn settings_source_for(cfg: &crate::discovery::AgentDiscoveryConfig) -> MachineSettingsSource {
    MachineSettingsSource::Files {
        user: cfg.override_settings.user_settings_path.clone(),
        project: cfg.override_settings.project_settings_path.clone(),
    }
}

/// The launch-time resolver for a launch rooted at `cwd`: `subagents.machines` is read from the
/// SAME two settings files discovery reads `agentOverrides` from for that cwd, and the catalog
/// from `herdr machine list --json` (through `HERDR_BIN`).
///
/// # Errors
/// A malformed settings file, as discovery itself refuses it.
pub(crate) fn launch_resolver(
    cwd: &Path,
    ext: &crate::registration::SubagentExtensionConfig,
) -> Result<PlacementResolver, crate::error::SubagentError> {
    let cfg = crate::extension::SubagentExecutor::discovery_config_on_disk(cwd, &ext.roots)?;
    let env = PlacementEnv::new(&ext.env_overrides);
    Ok(PlacementResolver::new(settings_source_for(&cfg))
        .with_catalog(MachineCatalogSource::Herdr {
            bin: cyrup_herdr::env::EnvSource::var(&env, cyrup_herdr::cli::HERDR_BIN)
                .filter(|bin| !bin.trim().is_empty()),
        })
        .with_transport(cyrup_herdr::remote::SshTransport::with_env(&env))
        .with_agent_dir(ext.roots.agent_dir().to_path_buf()))
}

/// The environment keys placement reads: the ssh binary, the ssh agent socket, and the herdr
/// binary the catalog comes from.
pub const PLACEMENT_ENV_KEYS: &[&str] = &[
    cyrup_herdr::remote::SSH_BIN_ENV,
    "SSH_AUTH_SOCK",
    cyrup_herdr::cli::HERDR_BIN,
];

/// `SubagentExtensionConfig::env_overrides` over the process environment — the same layering
/// every other environment read in this extension takes (`SubagentsExtension::env_lookup`):
/// `Some(value)` pins, `None` scrubs, an absent key falls through to the process.
#[derive(Debug)]
pub struct PlacementEnv<'a> {
    overrides: &'a std::collections::BTreeMap<String, Option<String>>,
}

impl<'a> PlacementEnv<'a> {
    /// Layer `overrides` over the process environment.
    #[must_use]
    pub fn new(overrides: &'a std::collections::BTreeMap<String, Option<String>>) -> Self {
        Self { overrides }
    }
}

impl cyrup_herdr::env::EnvSource for PlacementEnv<'_> {
    fn var(&self, key: &str) -> Option<String> {
        match self.overrides.get(key) {
            Some(value) => value.clone(),
            None => std::env::var(key).ok(),
        }
    }
}

/// A SINGLE run's placement — pi `runSinglePath`'s `params.machine ?? agentConfig.machine`
/// (`subagent-executor.ts:3824-3827` @v0.68.0) behind `canonicalizeExecutionParams`' runner /
/// worktree refusal (`:2453`). A blank call `machine` never gets here upstream (the schema's
/// `minLength: 1`); here it reaches [`PlacementResolver::resolve`]'s `validateMachineName` and is
/// refused with its sentence rather than read as "no machine".
///
/// # Errors
/// The refusal or the resolution's sentence, before any run exists.
pub async fn resolve_single_placement(
    agent: PlacementAgent<'_>,
    call_machine: Option<&str>,
    machine_cwd: Option<&str>,
    resolver: &mut PlacementResolver,
) -> Result<Option<super::StepPlacement>, String> {
    let requested = match call_machine {
        Some(machine) => machine.to_string(),
        None => match agent.machine {
            Some(machine) => machine.to_string(),
            None => return Ok(None),
        },
    };
    if let Some(refusal) =
        format_herdr_machine_runner_unsupported(Some(&requested), agent.name, agent.runner, false)
    {
        return Err(refusal);
    }
    let resolved = resolver.resolve(&requested, machine_cwd).await?;
    Ok(Some(super::StepPlacement {
        requested,
        resolved: Some(resolved),
    }))
}
