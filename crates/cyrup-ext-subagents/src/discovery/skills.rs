//! Skills association for subagent runs (T5, C4) — a port of pi-subagents'
//! `agents/skills.ts:608-682` (`resolveSkills`/`resolveSkillsWithFallback`/`buildSkillInjection`/
//! `discoverAvailableSkills`) and the whole of `agents/proactive-skills.ts`.
//!
//! # What this module owns
//!
//! - **Skill resolution** ([`resolve_skills`]/[`resolve_skills_with_fallback`]): map a list of
//!   skill NAMES to on-disk skills against an execution cwd (with an optional runtime-cwd fallback,
//!   pi `resolveSkillsWithFallback`, `skills.ts:640-654`), returning the resolved pointers and the
//!   names that could not be found.
//! - **Lazy `<available_skills>` injection** ([`build_skill_injection`]): render the resolved skills
//!   as a POINTER block (name + description + `SKILL.md` location + a "read the file" invocation
//!   hint) for the child's system prompt — NEVER the full `SKILL.md` body (pi `buildSkillInjection`,
//!   `skills.ts:656-675`; the "lazy references instead of inlining full skill bodies" contract of
//!   `skills-fallback.test.ts`).
//! - **Orchestration-skill exclusion** ([`SUBAGENT_ORCHESTRATION_SKILL`]): the `pi-subagents`
//!   operational skill is the PARENT orchestration skill and is NEVER child-injectable — it always
//!   resolves to `missing` and never appears in the available-skills listing (pi's
//!   `SUBAGENT_ORCHESTRATION_SKILL` rule, `skills.ts:54,618,717`).
//! - **Proactive skill-subagent suggestions** ([`recommend_proactive_skill_subagents`] et al.): a
//!   1:1 port of `proactive-skills.ts` — from the configured agents/chains, suggest skills that are
//!   referenced by at least `min_references` enabled configs and are actually available.
//!
//! # Reuse of `cyrup-resources`
//!
//! The filesystem WALK (which roots to scan, `SKILL.md` anchoring, `node_modules`/dot-dir skipping,
//! description-required drop, same-name precedence) is NOT re-implemented here — it is
//! [`cyrup_resources::discover`], the crate that already owns the Agent Skills standard discovery
//! (arch-09). This module builds a skills-only [`cyrup_resources::DiscoveryConfig`] for the target
//! cwd and looks names up in the resulting [`cyrup_resources::ResourceSet`]. cyrup's project config
//! dir is `.cyrup` (pi's `.pi`); user skills live under `~/.cyrup/skills` + `~/.agents/skills`.
//!
//! The child skill injection itself is wired in [`crate::exec`] (`build_task_text` composes
//! the [`build_skill_injection`] block into the task text handed to the spawned child — pi folds it
//! into the persona system prompt instead, `execution.ts:1054-1056`, but cyrup keeps it in the task
//! text so a `Replace`-mode persona cannot suppress it), honoring the
//! agent's own `skills` list and leaving `inherit_skills` (the `--no-skills` child flag) orthogonal:
//! an `inherit_skills: false` agent still receives its EXPLICITLY-listed skills as pointers, exactly
//! like pi (`execution.ts:935-952` builds the injection from `options.skills ?? agent.skills`
//! regardless of the inherit flag, which only governs `pi-args.ts:139` `--no-skills`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use cyrup_core::CancelToken;
use cyrup_resources::{DiscoveryConfig, ResourceSet, Skill, discover};

use crate::discovery::types::{AgentDefinition, ChainDefinition, ChainListBinding, ChainStepConfig};

/// The parent orchestration skill (pi `SUBAGENT_ORCHESTRATION_SKILL`, `skills.ts:54`). It is NEVER
/// injectable into a child: an explicit request for it always reports it as `missing`, and it is
/// filtered out of [`discover_available_skills`] and every proactive suggestion. The name is kept
/// verbatim from pi (the operational skill ships as `skills/pi-subagents/SKILL.md`) so a chain/agent
/// authored against pi resolves identically.
pub const SUBAGENT_ORCHESTRATION_SKILL: &str = "pi-subagents";

// ================================================================================================
// Skill resolution
// ================================================================================================

/// One resolved skill — the lazy POINTER shape (pi `ResolvedSkill` minus `content`/`source`, since
/// the injection carries no body). `path` is the skill's `SKILL.md` (pi's `ResolvedSkill.path`,
/// which `buildSkillInjection` renders as `<location>`); the child opens it on demand with `read`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedSkill {
    pub name: String,
    pub path: PathBuf,
    pub description: Option<String>,
}

/// The outcome of resolving a list of skill names: the pointers that resolved, and the names that
/// did not (pi `{ resolved, missing }`, `skills.ts:611`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillResolution {
    pub resolved: Vec<ResolvedSkill>,
    pub missing: Vec<String>,
}

/// A name+description available-skill row (pi `AvailableSkill`, `proactive-skills.ts:27`; the
/// `source` field pi's `discoverAvailableSkills` also carries is a doctor/UX concern folded into a
/// later tier — this crate's proactive path needs only name+description).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableSkill {
    pub name: String,
    pub description: Option<String>,
}

/// The user/global roots skill discovery scans in addition to the per-run cwd. Resolved from env in
/// production ([`SkillDiscoveryDirs::from_env`]); supplied explicitly by tests for isolation
/// (mirroring `cyrup-resources`' own `tests/resources.rs`, which roots `global` under a temp dir).
struct SkillDiscoveryDirs {
    /// cyrup's user config dir (`~/.cyrup`) — `<global_dir>/skills` is the user loose-skill root.
    global_dir: PathBuf,
    /// The cross-tool user skills base (`~/.agents`) — `<user_agents_dir>/skills` (pi's
    /// `userAgentsSkillsDir`). `None` keeps only the `<global_dir>/skills` user root.
    user_agents_dir: Option<PathBuf>,
}

impl SkillDiscoveryDirs {
    /// Production roots, derived from `CYRUP_HOME`/`HOME` exactly like `extension.rs::dirs_home`
    /// (kept as this module's own small copy per the crate's per-module-private-helper convention).
    fn from_env() -> Self {
        let home = std::env::var_os("CYRUP_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(std::env::temp_dir);
        Self {
            global_dir: home.join(".cyrup"),
            user_agents_dir: Some(home.join(".agents")),
        }
    }
}

/// Run a skills-only `cyrup-resources` discovery pass rooted at `cwd`, returning the winning skills.
/// Prompts/themes are disabled (this crate resolves only skills), and the project cwd is trusted so
/// project `.cyrup/skills` + `.agents/skills` surface (pi resolves the subagent's project skills
/// unconditionally, `skills.ts:321-343`). A discovery error degrades to an empty set — the caller
/// then reports every requested name as `missing` rather than panicking (this crate forbids panic).
async fn discover_skills(cwd: &Path, dirs: &SkillDiscoveryDirs) -> ResourceSet<Skill> {
    let mut cfg = DiscoveryConfig::new(cwd, &dirs.global_dir);
    cfg.enable_prompts = false;
    cfg.enable_themes = false;
    cfg.enable_skills = true;
    cfg.trusted_project = true;
    cfg.user_agents_dir = dirs.user_agents_dir.clone();
    match discover(&cfg, CancelToken::new()).await {
        Ok(report) => report.registry.skills,
        Err(_) => ResourceSet::default(),
    }
}

/// Look one skill name up in a discovered set, projecting the winning [`Skill`] to a lazy pointer.
fn resolve_one(skills: &ResourceSet<Skill>, name: &str) -> Option<ResolvedSkill> {
    let skill = skills.get_name(name)?;
    Some(ResolvedSkill {
        name: skill.name.clone(),
        path: skill.skill_md.clone(),
        description: skill.front.description.clone(),
    })
}

/// Resolve skill NAMES against a cwd (pi `resolveSkills`, `skills.ts:608-638`). Each name is
/// trimmed; empty names are skipped; [`SUBAGENT_ORCHESTRATION_SKILL`] is ALWAYS reported missing
/// (never resolved); any other name that has no on-disk skill is reported missing.
///
/// Discovery is only run when at least one requested name is a real (non-orchestration) skill —
/// mirroring pi, whose `getCachedSkills` is reached lazily inside `resolveSkillPath` and thus never
/// touched for an all-empty / all-`pi-subagents` request.
pub async fn resolve_skills(skill_names: &[String], cwd: &Path) -> SkillResolution {
    resolve_skills_in(skill_names, cwd, &SkillDiscoveryDirs::from_env()).await
}

async fn resolve_skills_in(
    skill_names: &[String],
    cwd: &Path,
    dirs: &SkillDiscoveryDirs,
) -> SkillResolution {
    let needs_discovery = skill_names.iter().any(|n| {
        let trimmed = n.trim();
        !trimmed.is_empty() && trimmed != SUBAGENT_ORCHESTRATION_SKILL
    });
    let skills = if needs_discovery {
        Some(discover_skills(cwd, dirs).await)
    } else {
        None
    };

    let mut resolved = Vec::new();
    let mut missing = Vec::new();
    for name in skill_names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == SUBAGENT_ORCHESTRATION_SKILL {
            missing.push(trimmed.to_string());
            continue;
        }
        match skills.as_ref().and_then(|set| resolve_one(set, trimmed)) {
            Some(skill) => resolved.push(skill),
            None => missing.push(trimmed.to_string()),
        }
    }
    SkillResolution { resolved, missing }
}

/// Resolve against `primary_cwd`, then re-resolve only the still-missing names against
/// `fallback_cwd` (pi `resolveSkillsWithFallback`, `skills.ts:640-654`). The fallback is skipped
/// when there is no fallback cwd, when nothing was missing, or when the two cwds are the same
/// directory (pi's `path.resolve(a) === path.resolve(b)` guard) — the resulting `missing` is exactly
/// the fallback pass's `missing` (a name still absent from BOTH cwds).
pub async fn resolve_skills_with_fallback(
    skill_names: &[String],
    primary_cwd: &Path,
    fallback_cwd: Option<&Path>,
) -> SkillResolution {
    resolve_skills_with_fallback_in(
        skill_names,
        primary_cwd,
        fallback_cwd,
        &SkillDiscoveryDirs::from_env(),
    )
    .await
}

async fn resolve_skills_with_fallback_in(
    skill_names: &[String],
    primary_cwd: &Path,
    fallback_cwd: Option<&Path>,
    dirs: &SkillDiscoveryDirs,
) -> SkillResolution {
    let primary = resolve_skills_in(skill_names, primary_cwd, dirs).await;
    let Some(fallback_cwd) = fallback_cwd else {
        return primary;
    };
    if primary.missing.is_empty() || same_directory(primary_cwd, fallback_cwd) {
        return primary;
    }
    let fallback = resolve_skills_in(&primary.missing, fallback_cwd, dirs).await;
    let mut resolved = primary.resolved;
    resolved.extend(fallback.resolved);
    SkillResolution {
        resolved,
        missing: fallback.missing,
    }
}

/// Lexical absolute-path equality (pi's `path.resolve(a) === path.resolve(b)`): no symlink
/// resolution, no filesystem access, so it matches pi's `path.resolve` byte-for-byte on
/// already-absolute inputs. Falls back to the raw path if lexical absolutization fails.
fn same_directory(a: &Path, b: &Path) -> bool {
    let abs_a = std::path::absolute(a).unwrap_or_else(|_| a.to_path_buf());
    let abs_b = std::path::absolute(b).unwrap_or_else(|_| b.to_path_buf());
    abs_a == abs_b
}

/// Every child-injectable skill discoverable from `cwd`, sorted by name and EXCLUDING the
/// orchestration skill (pi `discoverAvailableSkills`, `skills.ts:710-724`).
pub async fn discover_available_skills(cwd: &Path) -> Vec<AvailableSkill> {
    discover_available_skills_in(cwd, &SkillDiscoveryDirs::from_env()).await
}

async fn discover_available_skills_in(cwd: &Path, dirs: &SkillDiscoveryDirs) -> Vec<AvailableSkill> {
    let skills = discover_skills(cwd, dirs).await;
    let mut out: Vec<AvailableSkill> = skills
        .winners()
        .filter(|skill| skill.name != SUBAGENT_ORCHESTRATION_SKILL)
        .map(|skill| AvailableSkill {
            name: skill.name.clone(),
            description: skill.front.description.clone(),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

// ================================================================================================
// Lazy `<available_skills>` injection
// ================================================================================================

/// Render the resolved skills as the lazy `<available_skills>` POINTER block for a child's system
/// prompt (pi `buildSkillInjection`, `skills.ts:656-675`). Each entry carries only the skill's
/// name, description, and `SKILL.md` `<location>` plus a "use the read tool to load it" hint — the
/// full body is deliberately EXCLUDED so it never bloats the system prompt. Returns an empty string
/// for an empty input (no block emitted).
#[must_use]
pub fn build_skill_injection(skills: &[ResolvedSkill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        "The following configured skills are available to this subagent.".to_string(),
        "Use the read tool to load a skill's file when the task matches its description.".to_string(),
        "When a skill file references a relative path, resolve it against the skill directory \
         (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands."
            .to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];
    for skill in skills {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml_text(&skill.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml_text(skill.description.as_deref().unwrap_or(""))
        ));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml_text(&skill.path.display().to_string())
        ));
        lines.push("  </skill>".to_string());
    }
    lines.push("</available_skills>".to_string());
    lines.join("\n")
}

/// Escape the three XML-significant characters in skill metadata (pi `escapeXmlText`,
/// `skills.ts:677-682`). `&` MUST be replaced first so an already-escaped entity is not
/// double-escaped.
fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ================================================================================================
// Proactive skill-subagent suggestions (port of proactive-skills.ts)
// ================================================================================================

const DEFAULT_MIN_REFERENCES: i64 = 2;
const DEFAULT_MAX_RECOMMENDATIONS: i64 = 3;
const DEFAULT_PREFERRED_AGENT: &str = "reviewer";
const FALLBACK_AGENT_ORDER: [&str; 3] = ["reviewer", "context-builder", "delegate"];
const MAX_RECOMMENDATION_CAP: i64 = 5;

/// The `proactiveSkillSubagents` extension-config block (pi `ProactiveSkillSubagentsConfig`). Every
/// field is optional; absence falls through to the defaults resolved by
/// [`resolve_proactive_skill_subagents_config`].
#[derive(Clone, Debug, Default)]
pub struct ProactiveSkillSubagentsConfig {
    pub enabled: Option<bool>,
    pub min_references: Option<i64>,
    pub max_recommendations: Option<i64>,
    pub preferred_agent: Option<String>,
}

/// The three-state input pi models as `ProactiveSkillSubagentsConfig | false | undefined`: passing
/// [`None`] to the resolver is pi's `undefined` (defaults-on), [`ProactiveSkillSubagentsSetting::Disabled`]
/// is pi's `false` (fully off), and [`ProactiveSkillSubagentsSetting::Config`] carries an explicit
/// block.
#[derive(Clone, Debug)]
pub enum ProactiveSkillSubagentsSetting {
    /// pi's `config === false`.
    Disabled,
    Config(ProactiveSkillSubagentsConfig),
}

/// The resolved, defaults-applied config (pi `ResolvedProactiveSkillSubagentsConfig`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedProactiveSkillSubagentsConfig {
    pub enabled: bool,
    pub min_references: i64,
    pub max_recommendations: i64,
    pub preferred_agent: String,
}

/// One recommendation row (pi `ProactiveSkillSubagentRecommendation`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProactiveSkillSubagentRecommendation {
    pub skill: String,
    pub agent: String,
    pub references: usize,
    pub sources: Vec<String>,
    pub description: Option<String>,
    pub reason: String,
}

/// The minimal agent shape the recommender consults (pi reads only `name`/`disabled`/`skills` off
/// `AgentConfig`). Bridged from a full [`AgentDefinition`] via [`proactive_agent_input`].
#[derive(Clone, Debug)]
pub struct ProactiveAgentInput {
    pub name: String,
    pub disabled: bool,
    pub skills: Vec<String>,
}

/// The minimal chain shape the recommender consults: the chain name plus the union of every step's
/// skills (pi collects these per chain via `collectStepSkills`). Bridged from a full
/// [`ChainDefinition`] via [`proactive_chain_input`].
#[derive(Clone, Debug)]
pub struct ProactiveChainInput {
    pub name: String,
    pub skills: Vec<String>,
}

/// pi `positiveInteger` (`proactive-skills.ts:32-36`): keep only an integer `>= 1`.
fn positive_integer(value: Option<i64>) -> Option<i64> {
    value.filter(|v| *v >= 1)
}

/// Resolve the proactive config, applying pi's defaults + the recommendation cap
/// (`proactive-skills.ts:38-59`).
#[must_use]
pub fn resolve_proactive_skill_subagents_config(
    setting: Option<&ProactiveSkillSubagentsSetting>,
) -> ResolvedProactiveSkillSubagentsConfig {
    if matches!(setting, Some(ProactiveSkillSubagentsSetting::Disabled)) {
        return ResolvedProactiveSkillSubagentsConfig {
            enabled: false,
            min_references: DEFAULT_MIN_REFERENCES,
            max_recommendations: DEFAULT_MAX_RECOMMENDATIONS,
            preferred_agent: DEFAULT_PREFERRED_AGENT.to_string(),
        };
    }
    let config = match setting {
        Some(ProactiveSkillSubagentsSetting::Config(config)) => Some(config),
        _ => None,
    };
    let max_recommendations = config
        .and_then(|c| positive_integer(c.max_recommendations))
        .unwrap_or(DEFAULT_MAX_RECOMMENDATIONS);
    ResolvedProactiveSkillSubagentsConfig {
        enabled: config.and_then(|c| c.enabled).unwrap_or(true),
        min_references: config
            .and_then(|c| positive_integer(c.min_references))
            .unwrap_or(DEFAULT_MIN_REFERENCES),
        max_recommendations: max_recommendations.min(MAX_RECOMMENDATION_CAP),
        preferred_agent: config
            .and_then(|c| c.preferred_agent.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_PREFERRED_AGENT.to_string()),
    }
}

/// Choose the agent that carries every recommendation (pi `chooseRecommendationAgent`): the
/// preferred agent if enabled, else the first available fallback in order, else the first enabled
/// agent.
fn choose_recommendation_agent(agents: &[ProactiveAgentInput], preferred_agent: &str) -> Option<String> {
    let enabled: Vec<&ProactiveAgentInput> = agents.iter().filter(|a| !a.disabled).collect();
    if enabled.iter().any(|a| a.name == preferred_agent) {
        return Some(preferred_agent.to_string());
    }
    for name in FALLBACK_AGENT_ORDER {
        if enabled.iter().any(|a| a.name == name) {
            return Some(name.to_string());
        }
    }
    enabled.first().map(|a| a.name.clone())
}

/// Record `source` as a reference to `skill`, skipping the orchestration skill (pi `addSource`).
fn add_source(counts: &mut BTreeMap<String, BTreeSet<String>>, skill: &str, source: String) {
    if skill == SUBAGENT_ORCHESTRATION_SKILL {
        return;
    }
    counts.entry(skill.to_string()).or_default().insert(source);
}

/// Recommend proactive skill-subagents (pi `recommendProactiveSkillSubagents`,
/// `proactive-skills.ts:108-154`): count the enabled agents/chains referencing each skill, keep
/// those referenced at least `min_references` times AND (when an availability list is given) actually
/// available, sort by references desc then name asc, and cap at `max_recommendations`.
#[must_use]
pub fn recommend_proactive_skill_subagents(
    agents: &[ProactiveAgentInput],
    chains: &[ProactiveChainInput],
    available_skills: Option<&[AvailableSkill]>,
    setting: Option<&ProactiveSkillSubagentsSetting>,
) -> Vec<ProactiveSkillSubagentRecommendation> {
    let config = resolve_proactive_skill_subagents_config(setting);
    if !config.enabled {
        return Vec::new();
    }
    let Some(agent) = choose_recommendation_agent(agents, &config.preferred_agent) else {
        return Vec::new();
    };

    let available_by_name: Option<HashMap<&str, &AvailableSkill>> = available_skills
        .map(|list| list.iter().map(|s| (s.name.as_str(), s)).collect());

    let mut counts: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for candidate in agents {
        if candidate.disabled {
            continue;
        }
        for skill in &candidate.skills {
            add_source(&mut counts, skill, format!("agent:{}", candidate.name));
        }
    }
    for chain in chains {
        // pi collects each chain's skills into a Set before adding, so one chain contributes at most
        // one reference per skill regardless of how many of its steps name that skill.
        let chain_skills: BTreeSet<&String> = chain.skills.iter().collect();
        for skill in chain_skills {
            add_source(&mut counts, skill, format!("chain:{}", chain.name));
        }
    }

    let mut recommendations: Vec<ProactiveSkillSubagentRecommendation> = counts
        .into_iter()
        .filter(|(skill, sources)| {
            (sources.len() as i64) >= config.min_references
                && available_by_name
                    .as_ref()
                    .is_none_or(|map| map.contains_key(skill.as_str()))
        })
        .map(|(skill, sources)| {
            let references = sources.len();
            // `BTreeSet` already yields sources in sorted order (pi sorts them with `localeCompare`).
            let sources: Vec<String> = sources.into_iter().collect();
            let description = available_by_name
                .as_ref()
                .and_then(|map| map.get(skill.as_str()))
                .and_then(|entry| entry.description.clone());
            ProactiveSkillSubagentRecommendation {
                agent: agent.clone(),
                references,
                sources,
                description,
                reason: format!("referenced by {references} configured agents/chains"),
                skill,
            }
        })
        .collect();

    recommendations.sort_by(|a, b| {
        b.references
            .cmp(&a.references)
            .then_with(|| a.skill.cmp(&b.skill))
    });
    let cap = config.max_recommendations.max(0) as usize;
    recommendations.truncate(cap);
    recommendations
}

/// Format recommendations as human-readable transcript lines with the guardrails footer (pi
/// `formatProactiveSkillSubagentRecommendations`, `proactive-skills.ts:156-170`). Returns an empty
/// vec for no recommendations.
#[must_use]
pub fn format_proactive_skill_subagent_recommendations(
    recommendations: &[ProactiveSkillSubagentRecommendation],
) -> Vec<String> {
    if recommendations.is_empty() {
        return Vec::new();
    }
    let mut lines = vec!["Proactive skill subagent suggestions:".to_string()];
    for recommendation in recommendations {
        let sample_sources = recommendation
            .sources
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let extra = if recommendation.sources.len() > 3 {
            format!(", +{} more", recommendation.sources.len() - 3)
        } else {
            String::new()
        };
        let description = recommendation
            .description
            .as_deref()
            .map(|d| format!(" - {d}"))
            .unwrap_or_default();
        lines.push(format!(
            "- {} via {} ({}; {sample_sources}{extra}){description}",
            recommendation.skill, recommendation.agent, recommendation.reason,
        ));
    }
    lines.push(
        "Guardrails: use these for broad tasks where a skill-specialist pass is useful; keep \
         fanout small, use fresh context unless private/session context is explicitly needed, and \
         skip when the user asks for a direct answer."
            .to_string(),
    );
    lines
}

/// Build the proactive recommendation transcript lines (pi
/// `buildProactiveSkillSubagentRecommendationLines`, `proactive-skills.ts:172-191`). When proactive
/// suggestions are disabled the `discover_available_skills` closure is NOT invoked; a closure error
/// (discovery failure) degrades to an empty availability list, which yields no suggestions.
pub fn build_proactive_skill_subagent_recommendation_lines<F, E>(
    agents: &[ProactiveAgentInput],
    chains: &[ProactiveChainInput],
    setting: Option<&ProactiveSkillSubagentsSetting>,
    discover_available_skills: F,
) -> Vec<String>
where
    F: FnOnce() -> Result<Vec<AvailableSkill>, E>,
{
    if !resolve_proactive_skill_subagents_config(setting).enabled {
        return Vec::new();
    }
    let available_skills = discover_available_skills().unwrap_or_default();
    format_proactive_skill_subagent_recommendations(&recommend_proactive_skill_subagents(
        agents,
        chains,
        Some(&available_skills),
        setting,
    ))
}

// ---- bridges from the crate's real discovery types --------------------------------------------

/// Project a discovered [`AgentDefinition`] onto the minimal proactive input.
#[must_use]
pub fn proactive_agent_input(agent: &AgentDefinition) -> ProactiveAgentInput {
    ProactiveAgentInput {
        name: agent.name.clone(),
        disabled: agent.disabled.unwrap_or(false),
        skills: agent.skills.clone(),
    }
}

/// Project a discovered [`ChainDefinition`] onto the minimal proactive input, collecting the union
/// of every step's skills (including nested parallel steps).
#[must_use]
pub fn proactive_chain_input(chain: &ChainDefinition) -> ProactiveChainInput {
    ProactiveChainInput {
        name: chain.name.clone(),
        skills: collect_chain_step_skills(&chain.steps),
    }
}

/// The de-duplicated union of every step's skills across a chain, recursing into nested parallel
/// steps (pi `collectStepSkills`, `proactive-skills.ts:72-90`).
#[must_use]
pub fn collect_chain_step_skills(steps: &[ChainStepConfig]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for step in steps {
        collect_step_skills(step, &mut out, &mut seen);
    }
    out
}

fn collect_step_skills(step: &ChainStepConfig, out: &mut Vec<String>, seen: &mut HashSet<String>) {
    for skill in step_skill_names(step) {
        if seen.insert(skill.clone()) {
            out.push(skill);
        }
    }
    if let Some(parallel) = &step.parallel {
        collect_parallel_skills(parallel, out, seen);
    }
}

/// pi `normalizeSkillNames(step.skills ?? step.skill)`: the typed `skills` binding wins when
/// present (even `skills: false` → no skills); otherwise the raw `skill`/`skills` extra key is
/// normalized.
fn step_skill_names(step: &ChainStepConfig) -> Vec<String> {
    match &step.skills {
        Some(ChainListBinding::List(list)) => dedup_trimmed(list.iter().cloned()),
        Some(ChainListBinding::Toggle(_)) => Vec::new(),
        None => normalize_skill_names_json(step.extra.get("skill")),
    }
}

/// pi `normalizeSkillNames` over a raw JSON value (`proactive-skills.ts:61-70`): `false`/`true`/
/// `null`/absent → none; an array → its de-duplicated trimmed string entries; a string → its
/// comma-split de-duplicated trimmed parts.
fn normalize_skill_names_json(value: Option<&serde_json::Value>) -> Vec<String> {
    match value {
        Some(serde_json::Value::Array(items)) => {
            dedup_trimmed(items.iter().filter_map(|v| v.as_str().map(str::to_string)))
        }
        Some(serde_json::Value::String(raw)) => {
            dedup_trimmed(raw.split(',').map(str::to_string))
        }
        _ => Vec::new(),
    }
}

fn collect_parallel_skills(
    parallel: &serde_json::Value,
    out: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    match parallel {
        serde_json::Value::Array(children) => {
            for child in children {
                if child.is_object()
                    && let Ok(step) = serde_json::from_value::<ChainStepConfig>(child.clone())
                {
                    collect_step_skills(&step, out, seen);
                }
            }
        }
        serde_json::Value::Object(_) => {
            if let Ok(step) = serde_json::from_value::<ChainStepConfig>(parallel.clone()) {
                collect_step_skills(&step, out, seen);
            }
        }
        _ => {}
    }
}

/// Trim, drop empties, and de-duplicate preserving first-occurrence order (pi's
/// `[...new Set(input.map(trim).filter(nonEmpty))]`).
fn dedup_trimmed(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for value in values {
        let trimmed = value.trim().to_string();
        if !trimmed.is_empty() && seen.insert(trimmed.clone()) {
            out.push(trimmed);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

    use super::*;

    // --- skill-resolution fixtures (mirror skills-fallback.test.ts, cyrup `.cyrup` config dir) ---

    /// A skills-only discovery-dirs whose user/global roots live under a temp `global` dir, so a
    /// test's assertions never see the developer's real `~/.cyrup/skills` (mirrors
    /// `cyrup-resources` `tests/resources.rs`'s `root/global` isolation).
    fn isolated_dirs(root: &Path) -> SkillDiscoveryDirs {
        SkillDiscoveryDirs {
            global_dir: root.join("global"),
            user_agents_dir: Some(root.join("user-agents")),
        }
    }

    /// Write a project skill under `<cwd>/.cyrup/skills/<name>/SKILL.md` (cyrup's project config
    /// dir; pi's fixtures use `.pi/skills`).
    fn make_project_skill(cwd: &Path, name: &str, body: &str, description: &str) {
        let skill_dir = cwd.join(".cyrup").join("skills").join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\ndescription: {description}\n---\n\n{body}\n"),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn discovers_project_skills_from_filesystem_paths() {
        let tmp = tempfile::tempdir().unwrap();
        make_project_skill(tmp.path(), "fallback-skill", "Use fallback mode.", "Test description");

        let skills = discover_available_skills_in(tmp.path(), &isolated_dirs(tmp.path())).await;
        let discovered = skills
            .iter()
            .find(|s| s.name == "fallback-skill")
            .expect("expected fallback-skill to be discovered");
        assert_eq!(discovered.description.as_deref(), Some("Test description"));
    }

    #[tokio::test]
    async fn resolves_and_reads_skill_pointer_via_filesystem() {
        let tmp = tempfile::tempdir().unwrap();
        make_project_skill(tmp.path(), "resolve-skill", "Run local fallback checks.", "Test description");

        let resolution =
            resolve_skills_in(&["resolve-skill".to_string()], tmp.path(), &isolated_dirs(tmp.path()))
                .await;
        assert_eq!(resolution.missing, Vec::<String>::new());
        assert_eq!(resolution.resolved.len(), 1);
        assert_eq!(resolution.resolved[0].name, "resolve-skill");
        assert!(resolution.resolved[0].path.ends_with("SKILL.md"));
    }

    #[tokio::test]
    async fn builds_lazy_skill_references_instead_of_inlining_bodies() {
        let tmp = tempfile::tempdir().unwrap();
        make_project_skill(
            tmp.path(),
            "lazy-skill",
            "This body should stay out of the system prompt.",
            "Test description",
        );

        let resolution =
            resolve_skills_in(&["lazy-skill".to_string()], tmp.path(), &isolated_dirs(tmp.path()))
                .await;
        assert_eq!(resolution.missing, Vec::<String>::new());

        let injection = build_skill_injection(&resolution.resolved);
        assert!(injection.contains("The following configured skills are available to this subagent"));
        assert!(injection.contains("Use the read tool to load a skill's file"));
        assert!(injection.contains("<available_skills>"));
        assert!(injection.contains("<name>lazy-skill</name>"));
        assert!(injection.contains("<description>Test description</description>"));
        assert!(injection.contains("SKILL.md</location>"));
        // The lazy pointer NEVER inlines the body, and never uses the attribute form pi rejects.
        assert!(!injection.contains("This body should stay out"));
        assert!(!injection.contains("<skill name="));
    }

    #[tokio::test]
    async fn reports_a_missing_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let resolution = resolve_skills_in(
            &["does-not-exist".to_string()],
            tmp.path(),
            &isolated_dirs(tmp.path()),
        )
        .await;
        assert_eq!(resolution.resolved, Vec::new());
        assert_eq!(resolution.missing, vec!["does-not-exist".to_string()]);
    }

    #[tokio::test]
    async fn does_not_expose_pi_subagents_as_a_child_injectable_skill() {
        let tmp = tempfile::tempdir().unwrap();
        make_project_skill(tmp.path(), "pi-subagents", "Parent orchestration only.", "Orchestration");
        make_project_skill(tmp.path(), "safe-bash", "Use safe bash.", "Safe bash");
        let dirs = isolated_dirs(tmp.path());

        let available: Vec<String> = discover_available_skills_in(tmp.path(), &dirs)
            .await
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert!(!available.contains(&"pi-subagents".to_string()));
        assert!(available.contains(&"safe-bash".to_string()));

        let resolution = resolve_skills_in(
            &["pi-subagents".to_string(), "safe-bash".to_string()],
            tmp.path(),
            &dirs,
        )
        .await;
        assert_eq!(resolution.missing, vec!["pi-subagents".to_string()]);
        assert_eq!(
            resolution.resolved.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
            vec!["safe-bash".to_string()]
        );
    }

    #[tokio::test]
    async fn falls_back_to_the_runtime_cwd_when_the_execution_cwd_lacks_the_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        // The skill lives at the runtime (fallback) cwd only.
        make_project_skill(tmp.path(), "runtime-fallback-skill", "Runtime fallback skill.", "Fallback");
        let dirs = isolated_dirs(tmp.path());

        // Primary (nested) misses it; fallback (tmp) resolves it.
        let resolution = resolve_skills_with_fallback_in(
            &["runtime-fallback-skill".to_string()],
            &nested,
            Some(tmp.path()),
            &dirs,
        )
        .await;
        assert_eq!(resolution.missing, Vec::<String>::new());
        assert_eq!(resolution.resolved.len(), 1);
        assert_eq!(resolution.resolved[0].name, "runtime-fallback-skill");
    }

    #[tokio::test]
    async fn empty_skill_list_resolves_to_nothing_without_discovery() {
        let tmp = tempfile::tempdir().unwrap();
        let resolution = resolve_skills_in(&[], tmp.path(), &isolated_dirs(tmp.path())).await;
        assert_eq!(resolution, SkillResolution::default());
    }

    #[test]
    fn build_skill_injection_is_empty_for_no_skills() {
        assert_eq!(build_skill_injection(&[]), "");
    }

    #[test]
    fn build_skill_injection_escapes_xml_sensitive_metadata() {
        // Direct unit test of the escaping (pi "escapes XML-sensitive skill metadata"), independent
        // of on-disk discovery.
        let skills = [ResolvedSkill {
            name: "amp&skill".to_string(),
            path: PathBuf::from("/skills/amp&skill/SKILL.md"),
            description: Some("Use A & B <carefully>".to_string()),
        }];
        let injection = build_skill_injection(&skills);
        assert!(injection.contains("<name>amp&amp;skill</name>"));
        assert!(injection.contains("<description>Use A &amp; B &lt;carefully&gt;</description>"));
        assert!(injection.contains("amp&amp;skill/SKILL.md"));
    }

    // --- proactive-skills.test.ts port ---------------------------------------------------------

    fn agent(name: &str, skills: &[&str], disabled: bool) -> ProactiveAgentInput {
        ProactiveAgentInput {
            name: name.to_string(),
            disabled,
            skills: skills.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn chain(name: &str, skills: &[&str]) -> ProactiveChainInput {
        ProactiveChainInput {
            name: name.to_string(),
            skills: skills.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn available(name: &str, description: Option<&str>) -> AvailableSkill {
        AvailableSkill {
            name: name.to_string(),
            description: description.map(str::to_string),
        }
    }

    #[test]
    fn recommends_available_skills_referenced_by_multiple_enabled_configs() {
        let recommendations = recommend_proactive_skill_subagents(
            &[
                agent("reviewer", &[], false),
                agent("ui-reviewer", &["accessibility"], false),
                agent("disabled-reviewer", &["accessibility"], true),
            ],
            &[chain("ui-check", &["accessibility"]), chain("cleanup", &["deslop"])],
            Some(&[
                available("accessibility", Some("Accessibility review.")),
                available("deslop", Some("Cleanup review.")),
            ]),
            None,
        );
        assert_eq!(recommendations.len(), 1);
        assert_eq!(recommendations[0].skill, "accessibility");
        assert_eq!(recommendations[0].agent, "reviewer");
        assert_eq!(recommendations[0].references, 2);
        assert_eq!(
            recommendations[0].sources,
            vec!["agent:ui-reviewer".to_string(), "chain:ui-check".to_string()]
        );
    }

    #[test]
    fn filters_unavailable_orchestration_skills_and_honors_config_bounds() {
        let recommendations = recommend_proactive_skill_subagents(
            &[
                agent("delegate", &["pi-subagents", "alpha", "beta"], false),
                agent("one", &["alpha", "beta"], false),
                agent("two", &["gamma"], false),
                agent("three", &["gamma"], false),
            ],
            &[],
            Some(&[available("alpha", None), available("beta", None), available("gamma", None)]),
            Some(&ProactiveSkillSubagentsSetting::Config(ProactiveSkillSubagentsConfig {
                preferred_agent: Some("delegate".to_string()),
                max_recommendations: Some(2),
                ..Default::default()
            })),
        );
        assert_eq!(
            recommendations.iter().map(|r| r.skill.clone()).collect::<Vec<_>>(),
            vec!["alpha".to_string(), "beta".to_string()]
        );
        assert!(recommendations.iter().all(|r| r.agent == "delegate"));
    }

    #[test]
    fn can_be_disabled_and_formats_guardrails_for_visible_suggestions() {
        assert!(!resolve_proactive_skill_subagents_config(Some(&ProactiveSkillSubagentsSetting::Disabled)).enabled);
        assert!(recommend_proactive_skill_subagents(
            &[agent("reviewer", &["deslop"], false), agent("cleanup", &["deslop"], false)],
            &[],
            Some(&[available("deslop", None)]),
            Some(&ProactiveSkillSubagentsSetting::Disabled),
        )
        .is_empty());

        let lines = format_proactive_skill_subagent_recommendations(&[
            ProactiveSkillSubagentRecommendation {
                skill: "deslop".to_string(),
                agent: "reviewer".to_string(),
                references: 2,
                sources: vec!["agent:a".to_string(), "chain:b".to_string()],
                description: None,
                reason: "referenced by 2 configured agents/chains".to_string(),
            },
        ]);
        let joined = lines.join("\n");
        assert!(joined.contains("Proactive skill subagent suggestions:"));
        assert!(joined.contains("fresh context"));
    }

    #[test]
    fn does_not_discover_skills_when_disabled_and_treats_failures_as_no_suggestions() {
        let discovery_calls = std::cell::Cell::new(0usize);

        let disabled_lines = build_proactive_skill_subagent_recommendation_lines(
            &[agent("reviewer", &["deslop"], false), agent("cleanup", &["deslop"], false)],
            &[],
            Some(&ProactiveSkillSubagentsSetting::Disabled),
            || {
                discovery_calls.set(discovery_calls.get() + 1);
                Err::<Vec<AvailableSkill>, String>("should not discover when disabled".to_string())
            },
        );
        assert!(disabled_lines.is_empty());
        assert_eq!(discovery_calls.get(), 0);

        let failed_lines = build_proactive_skill_subagent_recommendation_lines(
            &[agent("reviewer", &["deslop"], false), agent("cleanup", &["deslop"], false)],
            &[],
            None,
            || {
                discovery_calls.set(discovery_calls.get() + 1);
                Err::<Vec<AvailableSkill>, String>("skill scan failed".to_string())
            },
        );
        assert!(failed_lines.is_empty());
        assert_eq!(discovery_calls.get(), 1);
    }

    #[test]
    fn collect_chain_step_skills_unions_sequential_and_nested_parallel_skills() {
        let steps = vec![
            ChainStepConfig {
                agent: Some("worker".to_string()),
                skills: Some(ChainListBinding::List(vec!["alpha".to_string(), "beta".to_string()])),
                ..Default::default()
            },
            ChainStepConfig {
                parallel: Some(serde_json::json!([
                    { "agent": "a", "skills": ["beta", "gamma"] },
                    { "agent": "b", "skills": false },
                ])),
                ..Default::default()
            },
        ];
        let collected = collect_chain_step_skills(&steps);
        // Union, de-duplicated, first-occurrence order; `skills: false` contributes nothing.
        assert_eq!(collected, vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()]);
    }
}
