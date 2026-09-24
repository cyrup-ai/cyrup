//! A placed native child's agent, skills, reads and memory, resolved ON THE MACHINE —
//! `resolveRemoteHerdrResources` and the bridge's `before_agent_start` override
//! (`src/extension/herdr-pi-bridge.ts:34-45,119` @v0.68.0).
//!
//! Upstream never ships a placed child its system prompt. The launch names LOGICAL resources —
//! the agent, its skill names, the tool ceiling, an explicit `reads` list —
//! (`child-launch.ts:309`, `serializeHerdrPiLaunch`), and the remote Pi's bridge resolves them
//! against the machine's own checkout: the agent file, its memory, its skills and its read files
//! are the ones THAT machine has, and a missing one refuses the launch. cyrup's placed child is the
//! `cyrup` on the machine, whose subagents runtime reads the same hand-off from
//! [`RESOURCES_ENV`] ([`resources_from_env`], called while the child's native extensions are built,
//! so a refusal fails the child's launch before its first turn exactly as upstream's `configure`
//! throw fails the placed session) and applies it at `before_agent_start`
//! ([`ResolvedRemoteResources::system_prompt`], applied by the prompt runtime).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// The env key carrying the launch's logical resources to the placed child (the cyrup analog of
/// the bridge's `configure` frame).
pub const RESOURCES_ENV: &str = "CYRUP_SUBAGENTS_HERDR_RESOURCES";

/// `/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u` — an agent or skill name.
static LOGICAL_NAME: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$").ok());
/// `/^[A-Za-z0-9_-]{1,64}$/u` — a tool name in the ceiling.
static TOOL_NAME: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,64}$").ok());

fn logical(name: &str) -> bool {
    LOGICAL_NAME.as_ref().is_some_and(|re| re.is_match(name))
}

fn tool(name: &str) -> bool {
    TOOL_NAME.as_ref().is_some_and(|re| re.is_match(name))
}

/// `serializeHerdrPiLaunch`'s resource refusal (`herdr-placed-run.ts:96`).
pub const RESOURCE_NAMES_REFUSAL: &str =
    "Pane-native remote resources require bounded logical agent, skill, tool, and read names.";

/// The bridge's refusal of a malformed `configure` (`herdr-pi-bridge.ts:35,112`).
pub const INVALID_RESOURCES: &str = "Remote logical resource names are invalid.";

/// `remoteResources` (`child-session.ts:56` @v0.68.0): what a placed launch names instead of
/// shipping a prompt.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteResources {
    /// The child agent's name (`childAgentName`).
    pub agent: String,
    /// `remoteSkillNames` — the launch's skill names (`options.skills ?? agent.skills`); absent
    /// means the remote agent's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    /// The explicit tool allowlist (`toolPlan.effectiveToolAllowlist`): the remote agent's tools
    /// are narrowed to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_ceiling: Option<Vec<String>>,
    /// `remoteReads` — an explicit call-level `reads` (`false` = none); absent means the remote
    /// agent's own `defaultReads`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reads: Option<RemoteReads>,
}

/// `reads?: string[] | false`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum RemoteReads {
    /// `false`: no read instruction.
    Off(bool),
    /// The paths, resolved on the machine.
    Paths(Vec<String>),
}

impl RemoteResources {
    /// `serializeHerdrPiLaunch`'s bound check (`herdr-placed-run.ts:95-96`): every name is a
    /// bounded logical name and every read at most 4096 bytes.
    ///
    /// # Errors
    /// [`RESOURCE_NAMES_REFUSAL`].
    pub fn validate(&self) -> Result<(), String> {
        let skills_ok = self
            .skills
            .as_ref()
            .is_none_or(|skills| skills.iter().all(|name| logical(name)));
        let tools_ok = self
            .tool_ceiling
            .as_ref()
            .is_none_or(|tools| tools.iter().all(|name| tool(name)));
        let reads_ok = match &self.reads {
            None | Some(RemoteReads::Off(false)) => true,
            Some(RemoteReads::Off(true)) => false,
            Some(RemoteReads::Paths(paths)) => paths.iter().all(|read| read.len() <= 4096),
        };
        if logical(&self.agent) && skills_ok && tools_ok && reads_ok {
            Ok(())
        } else {
            Err(RESOURCE_NAMES_REFUSAL.to_string())
        }
    }
}

/// What the machine resolved (`resolveRemoteHerdrResources`' return value).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedRemoteResources {
    /// The agent's canonical name on the machine.
    pub agent: String,
    /// The skill names that were resolved.
    pub skills: Vec<String>,
    /// The active tools to restrict to, when the agent declares `tools` (`None` keeps the child's
    /// default set, which the launch's own `--tools` already bounds by the ceiling).
    pub tools: Option<Vec<String>>,
    /// The agent's prompt as the MACHINE composes it: body, memory, skills, reads, refinement.
    pub system_prompt: String,
    /// The remote agent's `inheritProjectContext`.
    pub inherit_project_context: bool,
    /// The remote agent's `inheritGlobalContext`.
    pub inherit_global_context: bool,
    /// The remote agent's `inheritSkills`.
    pub inherit_skills: bool,
}

/// `resolveRemoteHerdrResources(cwd, resources, remoteDefaultTools)` (`herdr-pi-bridge.ts:34-45`)
/// against `cwd` on the machine, discovering agents from `roots` (the machine's own).
///
/// # Errors
/// Upstream's sentences: invalid names, an agent that is missing or ambiguous, missing skills, an
/// agent whose default tools fall wholly outside the ceiling.
pub fn resolve_remote_resources(
    cwd: &Path,
    resources: &RemoteResources,
    roots: &crate::paths::Roots,
) -> Result<ResolvedRemoteResources, String> {
    if resources.validate().is_err() {
        return Err(INVALID_RESOURCES.to_string());
    }
    let cfg = crate::extension::SubagentExecutor::discovery_config_on_disk(cwd, roots)
        .map_err(|error| error.to_string())?;
    let discovered = crate::discovery::discover_agents(&cfg, None)
        .map_err(|error| error.to_string())?
        .agents;
    let matching: Vec<_> = discovered
        .iter()
        .filter(|agent| {
            agent.name == resources.agent
                || agent.aliases.iter().any(|alias| alias == &resources.agent)
        })
        .collect();
    let [agent] = matching.as_slice() else {
        return Err(format!(
            "Remote agent '{}' was not found unambiguously in {}.",
            resources.agent,
            cwd.display()
        ));
    };
    let names = resources
        .skills
        .clone()
        .unwrap_or_else(|| agent.skills.clone());
    let skills = resolve_skills_blocking(names.clone(), cwd.to_path_buf());
    if !skills.missing.is_empty() {
        return Err(format!(
            "Remote cyrup skills not found: {}. Configure them on the saved machine.",
            skills.missing.join(", ")
        ));
    }
    let skill_prompt = crate::discovery::skills::build_skill_injection(&skills.resolved);
    let reads: Option<Vec<PathBuf>> = match &resources.reads {
        None => agent.default_reads.clone(),
        Some(RemoteReads::Off(_)) => None,
        Some(RemoteReads::Paths(paths)) => Some(paths.iter().map(PathBuf::from).collect()),
    };
    let read_paths = reads
        .as_deref()
        .map(|reads| crate::spawn::chain_graph::resolve_existing_read_paths(reads, cwd))
        .unwrap_or_default();
    let mut prompt = agent.system_prompt_body.clone();
    prompt.push_str(&crate::discovery::agent_memory::build_agent_memory_injection_for(agent, cwd));
    if !skill_prompt.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(&skill_prompt);
    }
    if !read_paths.is_empty() {
        prompt.push_str(&format!("\n\n[Read from: {}]", read_paths.join(", ")));
    }
    let system_prompt =
        crate::exec::agent_refinements::append_agent_refinement_overlay(&prompt, cwd, &agent.name);
    let denied: Vec<&str> = agent
        .exclude_tools
        .iter()
        .flatten()
        .map(String::as_str)
        .collect();
    let within_ceiling = |name: &str| {
        resources
            .tool_ceiling
            .as_ref()
            .is_none_or(|ceiling| ceiling.iter().any(|allowed| allowed == name))
    };
    let tools = match &agent.tools {
        Some(declared) => Some(
            declared
                .iter()
                .filter_map(|reference| match reference {
                    crate::discovery::types::ToolRef::Builtin(name)
                    | crate::discovery::types::ToolRef::ExtensionPath(name) => Some(name.clone()),
                    // A direct-MCP selection is a contract the placed child cannot carry
                    // (`serializeHerdrPiLaunch` refuses MCP direct tools).
                    crate::discovery::types::ToolRef::Mcp(_) => None,
                })
                .filter(|name| !denied.contains(&name.as_str()) && within_ceiling(name))
                .collect(),
        ),
        None => {
            // `agent.tools === undefined ? remoteDefaultTools : …`: the child's default set is
            // already the ceiling (its launch `--tools`); an EMPTY ceiling leaves nothing.
            if resources
                .tool_ceiling
                .as_ref()
                .is_some_and(|ceiling| ceiling.iter().all(|name| denied.contains(&name.as_str())))
            {
                return Err(format!(
                    "Remote agent '{}' resolved no default active tools within the inherited ceiling.",
                    agent.name
                ));
            }
            None
        }
    };
    Ok(ResolvedRemoteResources {
        agent: agent.name.clone(),
        skills: names,
        tools,
        system_prompt,
        inherit_project_context: agent.inherit_project_context,
        inherit_global_context: agent.inherit_global_context,
        inherit_skills: agent.inherit_skills,
    })
}

/// Skill resolution is asynchronous (a `cyrup-resources` discovery pass) and this resolution runs
/// while the child's extensions are BUILT, which is synchronous: it runs to completion on a
/// dedicated thread with its own runtime, so it never blocks — or needs — the caller's.
fn resolve_skills_blocking(
    names: Vec<String>,
    cwd: PathBuf,
) -> crate::discovery::skills::SkillResolution {
    let resolution = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map(|runtime| runtime.block_on(crate::discovery::skills::resolve_skills(&names, &cwd)))
            .ok()
    })
    .join()
    .ok()
    .flatten();
    resolution.unwrap_or(crate::discovery::skills::SkillResolution {
        resolved: Vec::new(),
        missing: vec!["(skill discovery failed)".to_string()],
    })
}

impl ResolvedRemoteResources {
    /// The bridge's `before_agent_start` (`herdr-pi-bridge.ts:119`):
    /// `${rewriteSubagentPrompt(systemPrompt, remote policy)}\n\n<active_agent name=…/>\n\n
    /// ${resolved prompt}` — the REMOTE agent's context policy, never the launching side's.
    ///
    /// `fanout_child` and `structured_output` are the launch's own (the child's role and its
    /// capture file are not the agent's to decide).
    #[must_use]
    pub fn system_prompt(
        &self,
        base: &str,
        global_agent_dir: Option<PathBuf>,
        fanout_child: bool,
        structured_output: bool,
    ) -> String {
        let rewritten = crate::prompt_runtime::rewrite_subagent_prompt(
            base,
            &crate::prompt_runtime::PromptRewriteOptions {
                inherit_project_context: self.inherit_project_context,
                inherit_global_context: self.inherit_global_context,
                global_agent_dir,
                inherit_skills: self.inherit_skills,
                fanout_child,
                structured_output,
            },
        );
        format!(
            "{rewritten}\n\n<active_agent name={}/>\n\n{}",
            serde_json::to_string(&self.agent).unwrap_or_default(),
            self.system_prompt
        )
    }

    /// The configured acknowledgement a placed child writes into its runtime directory — the
    /// bridge's `configured` frame (`herdr-pi-bridge.ts:121`): the agent, skills and tools it
    /// actually applied.
    #[must_use]
    pub fn configured_record(&self) -> serde_json::Value {
        serde_json::json!({
            "agent": self.agent,
            "skills": self.skills,
            "tools": self.tools,
        })
    }
}

/// Read [`RESOURCES_ENV`] and resolve it in `cwd` against the machine's own roots. `Ok(None)`
/// when this child is not a placed one.
///
/// # Errors
/// A malformed hand-off, or [`resolve_remote_resources`]'s refusal.
pub fn resources_from_env(
    get: &dyn Fn(&str) -> Option<String>,
    cwd: &Path,
) -> Result<Option<ResolvedRemoteResources>, String> {
    let Some(raw) = get(RESOURCES_ENV).filter(|raw| !raw.trim().is_empty()) else {
        return Ok(None);
    };
    let resources: RemoteResources =
        serde_json::from_str(&raw).map_err(|_| INVALID_RESOURCES.to_string())?;
    let lookup = |key: &str| get(key).map(std::ffi::OsString::from);
    let roots = crate::paths::Roots::from_lookup(&lookup);
    let resolved = resolve_remote_resources(cwd, &resources, &roots)?;
    // The `configured` acknowledgement, beside the child's other runtime files.
    if let Some(dir) = get(super::native::RUNTIME_DIR_ENV).filter(|dir| dir.starts_with('/')) {
        let _ = std::fs::write(
            Path::new(&dir).join("configured.json"),
            resolved.configured_record().to_string(),
        );
    }
    Ok(Some(resolved))
}
