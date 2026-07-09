//! The permission decision engine (port of pi `permission-manager.ts`). Entirely host-independent:
//! four fixed config layers (`global(trusted) → project(untrusted) → agent(trusted) →
//! projectAgent(untrusted)`), last-match-wins with a **trusted-floor** (an untrusted layer cannot
//! relax a trusted `deny`), wildcard + per-action/per-resource + bash-command + mcp-target
//! matching, and mtime-stamped resolution caching.
//!
//! The primary wired entry point is [`PermissionManager::check_permission`], called by the gate
//! (`gate.rs`) on every tool call. The three tool-shaping query methods
//! [`PermissionManager::get_tool_permission`] / [`PermissionManager::has_allowed_skills`] /
//! [`PermissionManager::get_bash_permissions`] (pi `permission-manager.ts:834-915`) drive the wired
//! `before_agent_start` prompt-sanitization + active-tools shaping (pi `shouldExposeTool`,
//! `index.ts:2049-2075`) — reached from `extension.rs`'s `should_expose_tool` at the live
//! `before_agent_start` seam (§9). None is a callerless primitive.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use crate::common::{self, get_non_empty_string, to_record};
use crate::jsonc;
use crate::ordered::OrderedValue;
use crate::types::{
    AgentPermissions, Category, CheckSource, DefaultCategory, DefaultPolicy, GlobalPermissionConfig,
    OrderedRules, PartialDefaultPolicy, PermissionCheckResult, PermissionState,
};
use crate::wildcard::{self, CompiledWildcard};

const BUILT_IN_TOOL_NAMES: [&str; 7] = ["bash", "read", "write", "edit", "grep", "find", "ls"];

/// pi `onWarning` ctor option's callback shape (`permission-manager.ts:620,631,643`): notified with a
/// human-readable message whenever a policy file exists but fails to load/parse.
type WarningCallback = Arc<dyn Fn(&str) + Send + Sync>;
const SPECIAL_KEYS: [&str; 2] = ["doom_loop", "external_directory"];
const MCP_BASELINE_TARGETS: [&str; 5] =
    ["mcp_status", "mcp_list", "mcp_search", "mcp_describe", "mcp_connect"];

fn is_built_in_tool(name: &str) -> bool {
    BUILT_IN_TOOL_NAMES.contains(&name)
}
fn is_special(name: &str) -> bool {
    SPECIAL_KEYS.contains(&name)
}
fn is_mcp_baseline(name: &str) -> bool {
    MCP_BASELINE_TARGETS.contains(&name)
}

/// The wildcard-state a compiled pattern carries: its resolved permission plus whether its layer is
/// trusted (system-owned). pi `LayeredPermissionState`, `permission-manager.ts:307-311`.
#[derive(Debug, Clone, Copy)]
struct LayeredState {
    state: PermissionState,
    trusted: bool,
}

type CompiledPatterns = Vec<CompiledWildcard<LayeredState>>;

/// A resolved layered match (pi `LayeredPermissionMatch`, `permission-manager.ts:315-319`).
#[derive(Debug, Clone)]
struct LayeredMatch {
    state: PermissionState,
    matched_pattern: String,
    matched_name: String,
}

/// A resolved scalar (default/record) value with its trust (pi `LayeredPermissionResolution`).
#[derive(Debug, Clone, Copy)]
struct LayeredResolution {
    state: PermissionState,
    trusted: bool,
}

/// One config layer (pi `PermissionLayer`, `permission-manager.ts:301-305`).
struct Layer {
    permissions: AgentPermissions,
    trusted: bool,
}

/// The compiled, resolved permission bundle for one agent (pi `ResolvedPermissions`,
/// `permission-manager.ts:325-335`).
struct ResolvedPermissions {
    /// The shallow-merged record view (only `merged.mcp.any_allow()` is read at runtime — the mcp
    /// baseline check, pi `index.ts:998`).
    merged: AgentPermissions,
    layers: Vec<Layer>,
    compiled_tools: CompiledPatterns,
    compiled_special: CompiledPatterns,
    compiled_skills: CompiledPatterns,
    compiled_mcp: CompiledPatterns,
    compiled_bash: CompiledPatterns,
}

/// Paths the manager reads its four policy layers from (plus the two mcp-server-name sources).
#[derive(Debug, Clone)]
pub struct ManagerPaths {
    pub global_config_path: PathBuf,
    pub agents_dir: PathBuf,
    pub project_global_config_path: Option<PathBuf>,
    pub project_agents_dir: Option<PathBuf>,
    pub legacy_global_settings_path: PathBuf,
    pub global_mcp_config_path: PathBuf,
    /// An explicit mcp-server-name override (tests); `None` reads them from the two config paths.
    pub mcp_server_names_override: Option<Vec<String>>,
}

/// The permission decision engine.
pub struct PermissionManager {
    paths: ManagerPaths,
    resolved_cache: HashMap<String, (String, Arc<ResolvedPermissions>)>,
    mcp_names_cache: Option<(String, Vec<String>)>,
    /// pi `onWarning` ctor option (`permission-manager.ts:620,631,643`): notified with a human-
    /// readable message whenever a policy file exists but fails to load/parse (NOT when it is
    /// simply absent — see [`notify_config_load_warning`]).
    on_warning: Option<WarningCallback>,
}

impl PermissionManager {
    #[must_use]
    pub fn new(paths: ManagerPaths) -> Self {
        Self { paths, resolved_cache: HashMap::new(), mcp_names_cache: None, on_warning: None }
    }

    /// Register a warning callback (pi ctor's `onWarning` option, `permission-manager.ts:631,643`),
    /// invoked by [`Self::load_global_config`]/[`Self::load_project_global_config`] when an existing
    /// policy file fails to read or parse.
    #[must_use]
    pub fn with_on_warning(mut self, callback: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.on_warning = Some(Arc::new(callback));
        self
    }

    /// pi `notifyWarning` (`permission-manager.ts:646-648`).
    fn notify_warning(&self, message: &str) {
        if let Some(cb) = &self.on_warning {
            cb(message);
        }
    }

    /// The single wired entry point (pi `checkPermission`, `permission-manager.ts:917-1047`).
    /// Resolves `tool_name` + `input` against the layered policy for `agent_name`.
    pub fn check_permission(
        &mut self,
        tool_name: &str,
        input: &Value,
        agent_name: Option<&str>,
    ) -> PermissionCheckResult {
        let resolved = self.resolve_permissions(agent_name);
        let normalized = tool_name.trim();
        let tool_match = find_compiled_match(&resolved.compiled_tools, normalized);

        // special (doom_loop / external_directory)
        if is_special(normalized) {
            let mut targets = create_action_resource_targets(normalized, input);
            targets.push(normalized.to_string());
            let result = find_by_pattern_order_for_names(&resolved.compiled_special, &targets);
            let state = result
                .as_ref()
                .map(|r| r.state)
                .or_else(|| default_state(&resolved.layers, DefaultCategory::Special))
                .unwrap_or(PermissionState::Ask);
            return PermissionCheckResult {
                tool_name: tool_name.to_string(),
                state,
                matched_pattern: result.as_ref().map(|r| r.matched_pattern.clone()),
                command: None,
                target: result.map(|r| r.matched_name),
                source: CheckSource::Special,
            };
        }

        // skill — pi checks `typeof skillName === "string"` (`permission-manager.ts:934-951`), i.e.
        // ANY string value (including `""`/whitespace-only) enters the pattern-match branch
        // untrimmed; only a non-string `name` (missing/number/etc.) falls back to the plain
        // layered default. Do NOT require non-emptiness here (that's a divergence from pi).
        if normalized == "skill" {
            let skill_name = to_record(input).get("name").and_then(Value::as_str);
            if let Some(sn) = skill_name {
                let result = find_compiled_match(&resolved.compiled_skills, sn);
                let state = result
                    .as_ref()
                    .map(|r| r.state)
                    .or_else(|| default_state(&resolved.layers, DefaultCategory::Skills))
                    .unwrap_or(PermissionState::Ask);
                return PermissionCheckResult {
                    tool_name: tool_name.to_string(),
                    state,
                    matched_pattern: result.map(|r| r.matched_pattern),
                    command: None,
                    target: None,
                    source: CheckSource::Skill,
                };
            }
            return PermissionCheckResult {
                tool_name: tool_name.to_string(),
                state: default_state(&resolved.layers, DefaultCategory::Skills)
                    .unwrap_or(PermissionState::Ask),
                matched_pattern: None,
                command: None,
                target: None,
                source: CheckSource::Skill,
            };
        }

        // bash — command rules OUTRANK the tool-level `bash` fallback (pi `:953-968`).
        if normalized == "bash" {
            let command = to_record(input)
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let result = find_compiled_match(&resolved.compiled_bash, &command);
            let state = result
                .as_ref()
                .map(|r| r.state)
                .or_else(|| tool_match.as_ref().map(|r| r.state))
                .or_else(|| default_state(&resolved.layers, DefaultCategory::Bash))
                .unwrap_or(PermissionState::Ask);
            return PermissionCheckResult {
                tool_name: tool_name.to_string(),
                state,
                matched_pattern: result.map(|r| r.matched_pattern),
                command: Some(command),
                target: None,
                source: CheckSource::Bash,
            };
        }

        // mcp
        if normalized == "mcp" {
            let server_names = self.configured_mcp_server_names();
            let mut mcp_targets = create_mcp_permission_targets(input, &server_names);
            mcp_targets.push("mcp".to_string());
            let fallback_target =
                mcp_targets.first().cloned().unwrap_or_else(|| "mcp".to_string());
            let default_mcp = default_state(&resolved.layers, DefaultCategory::Mcp)
                .unwrap_or(PermissionState::Ask);

            if let Some(m) = find_match_for_names(&resolved.compiled_mcp, &mcp_targets) {
                return PermissionCheckResult {
                    tool_name: tool_name.to_string(),
                    state: m.state,
                    matched_pattern: Some(m.matched_pattern),
                    command: None,
                    target: Some(m.matched_name),
                    source: CheckSource::Mcp,
                };
            }
            if let Some(tm) = &tool_match {
                return PermissionCheckResult {
                    tool_name: tool_name.to_string(),
                    state: tm.state,
                    matched_pattern: Some(tm.matched_pattern.clone()),
                    command: None,
                    target: Some(fallback_target),
                    source: CheckSource::Tool,
                };
            }
            let baseline = mcp_targets.iter().find(|t| is_mcp_baseline(t));
            if let Some(bt) = baseline
                && (resolved.merged.mcp.any_allow() || default_mcp == PermissionState::Allow)
            {
                return PermissionCheckResult {
                    tool_name: tool_name.to_string(),
                    state: PermissionState::Allow,
                    matched_pattern: None,
                    command: None,
                    target: Some(bt.clone()),
                    source: CheckSource::Mcp,
                };
            }
            return PermissionCheckResult {
                tool_name: tool_name.to_string(),
                state: default_mcp,
                matched_pattern: None,
                command: None,
                target: Some(fallback_target),
                source: CheckSource::Default,
            };
        }

        // built-in path tools (read/write/edit/grep/find/ls)
        if is_built_in_tool(normalized) {
            let mut targets = create_action_resource_targets(normalized, input);
            targets.push(normalized.to_string());
            let result = find_by_pattern_order_for_names(&resolved.compiled_tools, &targets);
            let state = result
                .as_ref()
                .map(|r| r.state)
                .or_else(|| default_state(&resolved.layers, DefaultCategory::Tools))
                .unwrap_or(PermissionState::Ask);
            return PermissionCheckResult {
                tool_name: tool_name.to_string(),
                state,
                matched_pattern: result.as_ref().map(|r| r.matched_pattern.clone()),
                command: None,
                target: result.map(|r| r.matched_name),
                source: CheckSource::Tool,
            };
        }

        // arbitrary extension tools
        if let Some(tm) = tool_match {
            return PermissionCheckResult {
                tool_name: tool_name.to_string(),
                state: tm.state,
                matched_pattern: Some(tm.matched_pattern),
                command: None,
                target: None,
                source: CheckSource::Tool,
            };
        }
        PermissionCheckResult {
            tool_name: tool_name.to_string(),
            state: default_state(&resolved.layers, DefaultCategory::Tools)
                .unwrap_or(PermissionState::Ask),
            matched_pattern: None,
            command: None,
            target: None,
            source: CheckSource::Default,
        }
    }

    // -------------------------------------------------------------------- tool-shaping query API

    /// pi `getToolPermission` (`permission-manager.ts:874-915`): the TOOL-LEVEL permission state for
    /// `tool_name` WITHOUT command/resource rules — the input to the `before_agent_start` active-tools
    /// shaping (`shouldExposeTool`, pi `index.ts:2049-2075`, wired in `extension.rs`). A special key →
    /// the layered `special` default; `skill` → the layered `skills` default; `bash`/`mcp`/other → the
    /// compiled `tools` match, else that category's layered default (trusted-floor, pi
    /// `resolveLayeredDefaultPermission`).
    pub fn get_tool_permission(
        &mut self,
        tool_name: &str,
        agent_name: Option<&str>,
    ) -> PermissionState {
        let resolved = self.resolve_permissions(agent_name);
        let normalized = tool_name.trim();

        if is_special(normalized) {
            return default_state(&resolved.layers, DefaultCategory::Special)
                .unwrap_or(PermissionState::Ask);
        }
        if normalized == "skill" {
            return default_state(&resolved.layers, DefaultCategory::Skills)
                .unwrap_or(PermissionState::Ask);
        }

        let tool_match = find_compiled_match(&resolved.compiled_tools, normalized);
        let category = match normalized {
            "bash" => DefaultCategory::Bash,
            "mcp" => DefaultCategory::Mcp,
            _ => DefaultCategory::Tools,
        };
        tool_match
            .map(|m| m.state)
            .or_else(|| default_state(&resolved.layers, category))
            .unwrap_or(PermissionState::Ask)
    }

    /// pi `hasAllowedSkills` (`permission-manager.ts:848-856`): whether the resolved policy exposes ANY
    /// allowed skill — true when the merged default `skills` policy is not `deny`, OR any explicit
    /// `skills` entry is `allow`. Drives the `read`-tool exposure bypass (pi `index.ts:2070`) so a
    /// `read`-denied agent can still reach its skill files.
    pub fn has_allowed_skills(&mut self, agent_name: Option<&str>) -> bool {
        let resolved = self.resolve_permissions(agent_name);
        // pi `merged.defaultPolicy.skills`: the shallow-merged (last-defined-wins across the four
        // layers in build order, NO trusted-floor) default skills policy. The global layer always
        // defines it (its `default_policy` is the complete `full_to_partial(global.default_policy)`),
        // so the fallback is only a safety net.
        let default_skills = resolved
            .layers
            .iter()
            .rev()
            .find_map(|layer| layer.permissions.default_policy.skills)
            .unwrap_or(PermissionState::Ask);
        if default_skills != PermissionState::Deny {
            return true;
        }
        resolved.merged.skills.any_allow()
    }

    /// pi `getBashPermissions` (`permission-manager.ts:834-837`): the merged `bash` command rules for
    /// `agent_name` (pi `merged.bash || {}`). Drives the `bash`-tool exposure bypass in the
    /// `before_agent_start` shaping — a `bash`-denied agent that still has an explicitly `allow`ed bash
    /// command keeps `bash` exposed (the gate re-checks each command), mirroring the `read`+skills
    /// bypass.
    pub fn get_bash_permissions(&mut self, agent_name: Option<&str>) -> OrderedRules {
        self.resolve_permissions(agent_name).merged.bash.clone()
    }

    // ------------------------------------------------------------------------------------- resolve

    fn resolve_permissions(&mut self, agent_name: Option<&str>) -> Arc<ResolvedPermissions> {
        let cache_key = agent_name.filter(|n| !n.is_empty()).unwrap_or("__global__").to_string();
        let stamp = self.policy_cache_stamp(agent_name);
        if let Some((s, v)) = self.resolved_cache.get(&cache_key)
            && *s == stamp
        {
            return v.clone();
        }

        let global = self.load_global_config();
        let project = self.load_project_global_config();
        let agent = self.load_agent_permissions(agent_name);
        let project_agent = self.load_project_agent_permissions(agent_name);

        // Shallow merged record view (pi `mergePermissions`, only `merged.mcp` is read at runtime).
        let mut merged = global.permissions.clone();
        for layer in [&project, &agent, &project_agent] {
            merge_into(&mut merged, layer);
        }

        let layers = build_layers(&global, project, agent, project_agent);
        let resolved = Arc::new(ResolvedPermissions {
            compiled_tools: compile_from_layers(Category::Tools, &layers),
            compiled_special: compile_from_layers(Category::Special, &layers),
            compiled_skills: compile_from_layers(Category::Skills, &layers),
            compiled_mcp: compile_from_layers(Category::Mcp, &layers),
            compiled_bash: compile_from_layers(Category::Bash, &layers),
            merged,
            layers,
        });
        self.resolved_cache.insert(cache_key, (stamp, resolved.clone()));
        resolved
    }

    /// pi `getPolicyCacheStamp` (`permission-manager.ts:790-798`): the four policy files' mtimes.
    fn policy_cache_stamp(&self, agent_name: Option<&str>) -> String {
        let agent_path = resolve_agent_markdown_path(Some(&self.paths.agents_dir), agent_name);
        let project_agent_path = self
            .paths
            .project_agents_dir
            .as_deref()
            .and_then(|d| resolve_agent_markdown_path(Some(d), agent_name));
        let global = file_stamp(&self.paths.global_config_path);
        let project = match &self.paths.project_global_config_path {
            Some(p) => file_stamp(p),
            None => "none".to_string(),
        };
        let agent = match agent_path {
            Some(p) => file_stamp(&p),
            None => "missing".to_string(),
        };
        let project_agent = match project_agent_path {
            Some(p) => file_stamp(&p),
            None => "none".to_string(),
        };
        format!("{global}|{project}|{agent}|{project_agent}")
    }

    /// pi `loadGlobalConfig` (`permission-manager.ts:650-685`): on a read/parse failure of an
    /// EXISTING file, warns (`formatJsoncConfigLoadWarning` + `notifyWarning`) before falling back
    /// to the empty/ask config; a simply-absent file (`ENOENT`) is silent.
    fn load_global_config(&self) -> GlobalPermissionConfig {
        let path = &self.paths.global_config_path;
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                notify_config_load_error(self, &e, path, "using ask fallback");
                return GlobalPermissionConfig::default();
            }
        };
        let path_str = path.display().to_string();
        let value = match jsonc::parse_ordered_config(&text, &path_str, "permission config") {
            Ok(v) => v,
            Err(err) => {
                self.notify_warning(&format!("{err}; using ask fallback."));
                return GlobalPermissionConfig::default();
            }
        };
        let permissions = normalize_raw_permission(&value);
        let default_policy = normalize_policy(value.get("defaultPolicy"));
        GlobalPermissionConfig { default_policy, permissions }
    }

    /// pi `loadProjectGlobalConfig` (`permission-manager.ts:687-717`): same read/parse-warning
    /// contract as [`Self::load_global_config`], but the fallback message + empty value differ.
    fn load_project_global_config(&self) -> AgentPermissions {
        let Some(path) = &self.paths.project_global_config_path else {
            return AgentPermissions::default();
        };
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                notify_config_load_error(self, &e, path, "ignoring project permission overrides");
                return AgentPermissions::default();
            }
        };
        let path_str = path.display().to_string();
        match jsonc::parse_ordered_config(&text, &path_str, "permission config") {
            Ok(value) => normalize_raw_permission(&value),
            Err(err) => {
                self.notify_warning(&format!("{err}; ignoring project permission overrides."));
                AgentPermissions::default()
            }
        }
    }

    fn load_agent_permissions(&self, agent_name: Option<&str>) -> AgentPermissions {
        load_agent_permissions_from(Some(&self.paths.agents_dir), agent_name)
    }

    fn load_project_agent_permissions(&self, agent_name: Option<&str>) -> AgentPermissions {
        load_agent_permissions_from(self.paths.project_agents_dir.as_deref(), agent_name)
    }

    /// pi `getConfiguredMcpServerNames` (`permission-manager.ts:858-872`).
    fn configured_mcp_server_names(&mut self) -> Vec<String> {
        if let Some(over) = &self.paths.mcp_server_names_override {
            return over.clone();
        }
        let paths = [
            self.paths.global_mcp_config_path.clone(),
            self.paths.legacy_global_settings_path.clone(),
        ];
        let stamp = paths
            .iter()
            .map(|p| format!("{}:{}", p.display(), file_stamp(p)))
            .collect::<Vec<_>>()
            .join("|");
        if let Some((s, v)) = &self.mcp_names_cache
            && *s == stamp
        {
            return v.clone();
        }
        let mut seen: Vec<String> = Vec::new();
        for path in &paths {
            for name in read_configured_mcp_server_names(path) {
                if !seen.contains(&name) {
                    seen.push(name);
                }
            }
        }
        // pi sort: length desc, then lexicographic (`permission-manager.ts:134`).
        seen.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        self.mcp_names_cache = Some((stamp, seen.clone()));
        seen
    }
}

// ================================================================================ pure engine fns

fn read_configured_mcp_server_names(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = jsonc::parse(&text) else {
        return Vec::new();
    };
    let root = to_record(&value);
    let servers = root
        .get("mcpServers")
        .or_else(|| root.get("mcp-servers"))
        .map(to_record)
        .unwrap_or_else(|| to_record(&Value::Null));
    servers
        .keys()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .collect()
}

fn load_agent_permissions_from(dir: Option<&Path>, agent_name: Option<&str>) -> AgentPermissions {
    let Some(file_path) = resolve_agent_markdown_path(dir, agent_name) else {
        return AgentPermissions::default();
    };
    let Ok(markdown) = std::fs::read_to_string(&file_path) else {
        return AgentPermissions::default();
    };
    let frontmatter = common::extract_frontmatter(&markdown);
    if frontmatter.is_empty() {
        return AgentPermissions::default();
    }
    let parsed = common::parse_simple_yaml_map(&frontmatter);
    match parsed.get("permission") {
        Some(perm) => normalize_raw_permission(perm),
        None => AgentPermissions::default(),
    }
}

/// pi `resolveAgentMarkdownPath` (`permission-manager.ts:591-604`) — the agent-name traversal guard:
/// a name that resolves OUTSIDE the agents dir (a `../outside` escape or an absolute path) yields
/// `None`, so no out-of-tree file is ever read (verified by `edge-red-agent-name-traversal.test.ts`).
fn resolve_agent_markdown_path(dir: Option<&Path>, agent_name: Option<&str>) -> Option<PathBuf> {
    let dir = dir?;
    let name = agent_name.filter(|n| !n.is_empty())?;

    let root = common::lexical_normalize(&dir.display().to_string());
    let leaf = format!("{name}.md");
    let candidate = if leaf.starts_with('/') {
        common::lexical_normalize(&leaf)
    } else {
        common::lexical_normalize(&common::join_paths(&root, &leaf))
    };
    let prefix = format!("{}/", root.trim_end_matches('/'));
    if candidate.starts_with(&prefix) {
        Some(PathBuf::from(candidate))
    } else {
        None
    }
}

/// pi `formatJsoncConfigLoadWarning`'s `isNodeErrorWithCode(error, "ENOENT")` branch
/// (`jsonc-config.ts:43-45`) applied to a file-read failure: a simply-absent file is silent (no
/// warning, matching a fresh install with no config yet); any other read error (permissions
/// denied, I/O failure, ...) is warn-worthy, mirroring `loadGlobalConfig`/`loadProjectGlobalConfig`
/// catch blocks (`permission-manager.ts:670-681`, `:702-711`).
fn notify_config_load_error(
    manager: &PermissionManager,
    error: &std::io::Error,
    path: &Path,
    fallback_message: &str,
) {
    if error.kind() == std::io::ErrorKind::NotFound {
        return;
    }
    manager.notify_warning(&format!(
        "Failed to load permission config at '{}': {error}; {fallback_message}.",
        path.display()
    ));
}

fn file_stamp(path: &Path) -> String {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_else(|| "missing".to_string())
}

fn build_layers(
    global: &GlobalPermissionConfig,
    project: AgentPermissions,
    agent: AgentPermissions,
    project_agent: AgentPermissions,
) -> Vec<Layer> {
    let mut global_perms = global.permissions.clone();
    // The global layer's default policy is COMPLETE (every category), expressed as a full partial so
    // `default_state` reads it uniformly across layers (pi's global layer is `GlobalPermissionConfig`).
    global_perms.default_policy = full_to_partial(global.default_policy);
    vec![
        Layer { permissions: global_perms, trusted: true },
        Layer { permissions: project, trusted: false },
        Layer { permissions: agent, trusted: true },
        Layer { permissions: project_agent, trusted: false },
    ]
}

/// pi `compilePermissionPatternsFromLayers` (`permission-manager.ts:351-373`): concatenate every
/// layer's entries IN LAYER ORDER (later layers = higher last-match-wins priority), preserving each
/// entry's insertion order within its layer.
fn compile_from_layers(category: Category, layers: &[Layer]) -> CompiledPatterns {
    let mut entries: Vec<(String, LayeredState)> = Vec::new();
    for layer in layers {
        for (pattern, state) in layer.permissions.category(category).iter() {
            entries.push((pattern.to_string(), LayeredState { state, trusted: layer.trusted }));
        }
    }
    if entries.is_empty() {
        return Vec::new();
    }
    wildcard::compile_entries(entries)
}

fn merge_into(base: &mut AgentPermissions, other: &AgentPermissions) {
    for cat in [Category::Tools, Category::Bash, Category::Mcp, Category::Skills, Category::Special] {
        let src: Vec<(String, PermissionState)> =
            other.category(cat).iter().map(|(p, s)| (p.to_string(), s)).collect();
        let dst = base_category_mut(base, cat);
        for (p, s) in src {
            dst.insert(p, s);
        }
    }
}

fn base_category_mut(perms: &mut AgentPermissions, cat: Category) -> &mut OrderedRules {
    match cat {
        Category::Tools => &mut perms.tools,
        Category::Bash => &mut perms.bash,
        Category::Mcp => &mut perms.mcp,
        Category::Skills => &mut perms.skills,
        Category::Special => &mut perms.special,
    }
}

fn full_to_partial(p: DefaultPolicy) -> PartialDefaultPolicy {
    PartialDefaultPolicy {
        tools: Some(p.tools),
        bash: Some(p.bash),
        mcp: Some(p.mcp),
        skills: Some(p.skills),
        special: Some(p.special),
    }
}

// ---- layered matchers (pi permission-manager.ts:387-511) ----

/// pi `findLatestTrustedPermissionMatch` (`:387-405`): the last TRUSTED entry matching `name` (raw).
fn find_latest_trusted_match(patterns: &CompiledPatterns, name: &str) -> Option<LayeredMatch> {
    for index in (0..patterns.len()).rev() {
        let p = patterns.get(index)?;
        if p.state.trusted && p.is_match(name) {
            return Some(LayeredMatch {
                state: p.state.state,
                matched_pattern: p.pattern.clone(),
                matched_name: name.to_string(),
            });
        }
    }
    None
}

/// pi `findCompiledPermissionMatch` (`:407-428`): last match; but if it is not `deny` and not
/// trusted, a trusted `deny` floor wins (the trusted-floor rule).
fn find_compiled_match(patterns: &CompiledPatterns, name: &str) -> Option<LayeredMatch> {
    if patterns.is_empty() {
        return None;
    }
    let index = wildcard::find_match_index(patterns, name)?;
    let m = patterns.get(index)?;
    if m.state.state != PermissionState::Deny
        && !m.state.trusted
        && let Some(floor) = find_latest_trusted_match(patterns, name)
        && floor.state == PermissionState::Deny
    {
        return Some(floor);
    }
    Some(LayeredMatch {
        state: m.state.state,
        matched_pattern: m.pattern.clone(),
        matched_name: name.to_string(),
    })
}

/// pi `findCompiledPermissionMatchForNames` (`:430-447`): first name (in order) that matches.
fn find_match_for_names(patterns: &CompiledPatterns, names: &[String]) -> Option<LayeredMatch> {
    if patterns.is_empty() {
        return None;
    }
    for name in names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(m) = find_compiled_match(patterns, trimmed) {
            return Some(m);
        }
    }
    None
}

/// pi `findLatestTrustedPermissionMatchForNames` (`:449-473`): last trusted entry matching any name
/// (name normalized `\\`→`/` per test).
fn find_latest_trusted_match_for_names(
    patterns: &CompiledPatterns,
    names: &[String],
) -> Option<LayeredMatch> {
    for index in (0..patterns.len()).rev() {
        let p = patterns.get(index)?;
        if !p.state.trusted {
            continue;
        }
        for name in names {
            let normalized = name.replace('\\', "/");
            if p.is_match(&normalized) {
                return Some(LayeredMatch {
                    state: p.state.state,
                    matched_pattern: p.pattern.clone(),
                    matched_name: name.clone(),
                });
            }
        }
    }
    None
}

/// pi `findCompiledPermissionMatchByPatternOrderForNames` (`:475-511`): scan patterns from the end;
/// for each, try each name; on a non-deny untrusted hit, a trusted `deny` floor wins.
fn find_by_pattern_order_for_names(
    patterns: &CompiledPatterns,
    names: &[String],
) -> Option<LayeredMatch> {
    if patterns.is_empty() {
        return None;
    }
    let normalized_names: Vec<String> =
        names.iter().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect();
    if normalized_names.is_empty() {
        return None;
    }
    for index in (0..patterns.len()).rev() {
        let p = patterns.get(index)?;
        for name in &normalized_names {
            if !p.is_match(&name.replace('\\', "/")) {
                continue;
            }
            if p.state.state != PermissionState::Deny
                && !p.state.trusted
                && let Some(floor) =
                    find_latest_trusted_match_for_names(patterns, &normalized_names)
                && floor.state == PermissionState::Deny
            {
                return Some(floor);
            }
            return Some(LayeredMatch {
                state: p.state.state,
                matched_pattern: p.pattern.clone(),
                matched_name: name.clone(),
            });
        }
    }
    None
}

/// pi `resolveLayeredPermissionValue` (`:530-561`): last-set layered scalar with a trusted-`deny`
/// floor.
fn resolve_layered_value(
    layers: &[Layer],
    select: impl Fn(&Layer) -> Option<PermissionState>,
) -> Option<LayeredResolution> {
    let mut current: Option<LayeredResolution> = None;
    let mut trusted_floor: Option<LayeredResolution> = None;
    for layer in layers {
        let Some(state) = select(layer) else { continue };
        let candidate = LayeredResolution { state, trusted: layer.trusted };
        if !candidate.trusted
            && candidate.state != PermissionState::Deny
            && trusted_floor.map(|f| f.state == PermissionState::Deny).unwrap_or(false)
        {
            current = trusted_floor;
            continue;
        }
        current = Some(candidate);
        if candidate.trusted {
            trusted_floor = Some(candidate);
        }
    }
    current
}

fn default_state(layers: &[Layer], category: DefaultCategory) -> Option<PermissionState> {
    resolve_layered_value(layers, |layer| layer.permissions.default_policy.get(category))
        .map(|r| r.state)
}

// ---- input → targets (pi permission-manager.ts:513-528, 240-297) ----

fn path_resource_from_input(input: &Value) -> Option<String> {
    let record = to_record(input);
    let path = get_non_empty_string(record.get("path"))
        .or_else(|| get_non_empty_string(record.get("file_path")))?;
    let cwd = get_non_empty_string(record.get("cwd")).unwrap_or_else(process_cwd);
    let resource = common::normalize_path_resource_for_permission(&path, &cwd);
    if resource.is_empty() { None } else { Some(resource) }
}

fn create_action_resource_targets(action: &str, input: &Value) -> Vec<String> {
    match path_resource_from_input(input) {
        Some(resource) => vec![format!("{action}:{resource}")],
        None => Vec::new(),
    }
}

fn process_cwd() -> String {
    std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default()
}

// ---- mcp targets (pi permission-manager.ts:168-297) ----

fn parse_qualified_mcp_tool_name(value: &str) -> Option<(String, String)> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let colon = trimmed.find(':')?;
    if colon == 0 || colon >= trimmed.len() - 1 {
        return None;
    }
    let server = trimmed.get(..colon)?.trim();
    let tool = trimmed.get(colon + 1..)?.trim();
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server.to_string(), tool.to_string()))
}

fn push_unique(targets: &mut Vec<String>, value: &str) {
    if value.is_empty() {
        return;
    }
    if !targets.iter().any(|t| t == value) {
        targets.push(value.to_string());
    }
}

fn add_derived_mcp_server_targets(
    tool_name: &str,
    server_names: &[String],
    targets: &mut Vec<String>,
) {
    let trimmed_tool = tool_name.trim();
    if trimmed_tool.is_empty() {
        return;
    }
    for server in server_names {
        let s = server.trim();
        if s.is_empty() {
            continue;
        }
        if !trimmed_tool.ends_with(&format!("_{s}")) {
            continue;
        }
        if trimmed_tool.starts_with(&format!("{s}_")) {
            continue;
        }
        push_unique(targets, &format!("{s}_{trimmed_tool}"));
        push_unique(targets, &format!("{s}:{trimmed_tool}"));
        push_unique(targets, s);
    }
}

fn push_mcp_tool_permission_targets(
    raw_reference: &str,
    server_hint: Option<&str>,
    server_names: &[String],
    targets: &mut Vec<String>,
) {
    let qualified = parse_qualified_mcp_tool_name(raw_reference);
    let resolved_server = server_hint
        .map(str::to_string)
        .or_else(|| qualified.as_ref().map(|(s, _)| s.clone()));
    let resolved_tool =
        qualified.as_ref().map(|(_, t)| t.clone()).unwrap_or_else(|| raw_reference.to_string());

    if let Some(server) = &resolved_server {
        push_unique(targets, &format!("{server}_{resolved_tool}"));
        push_unique(targets, &format!("{server}:{resolved_tool}"));
        push_unique(targets, server);
    } else {
        add_derived_mcp_server_targets(&resolved_tool, server_names, targets);
    }
    push_unique(targets, &resolved_tool);
    push_unique(targets, raw_reference);
}

fn create_mcp_permission_targets(input: &Value, server_names: &[String]) -> Vec<String> {
    let record = to_record(input);
    let tool = get_non_empty_string(record.get("tool"));
    let server = get_non_empty_string(record.get("server"));
    let connect = get_non_empty_string(record.get("connect"));
    let describe = get_non_empty_string(record.get("describe"));
    let search = get_non_empty_string(record.get("search"));

    let mut targets: Vec<String> = Vec::new();

    if let Some(tool) = tool {
        push_mcp_tool_permission_targets(&tool, server.as_deref(), server_names, &mut targets);
        push_unique(&mut targets, "mcp_call");
        return targets;
    }
    if let Some(connect) = connect {
        push_unique(&mut targets, &format!("mcp_connect_{connect}"));
        push_unique(&mut targets, &connect);
        push_unique(&mut targets, "mcp_connect");
        return targets;
    }
    if let Some(describe) = describe {
        push_mcp_tool_permission_targets(&describe, server.as_deref(), server_names, &mut targets);
        push_unique(&mut targets, "mcp_describe");
        return targets;
    }
    if let Some(search) = search {
        if let Some(server) = &server {
            push_unique(&mut targets, &format!("mcp_server_{server}"));
            push_unique(&mut targets, server);
        }
        push_unique(&mut targets, &search);
        push_unique(&mut targets, "mcp_search");
        return targets;
    }
    if let Some(server) = &server {
        push_unique(&mut targets, &format!("mcp_server_{server}"));
        push_unique(&mut targets, server);
        push_unique(&mut targets, "mcp_list");
        return targets;
    }
    push_unique(&mut targets, "mcp_status");
    targets
}

// ---- policy normalization (pi permission-manager.ts:61-166) ----

fn normalize_policy(value: Option<&OrderedValue>) -> DefaultPolicy {
    let d = DefaultPolicy::default();
    let get = |k: &str, fallback: PermissionState| {
        value
            .and_then(|v| v.get(k))
            .and_then(OrderedValue::as_str)
            .and_then(PermissionState::parse)
            .unwrap_or(fallback)
    };
    DefaultPolicy {
        tools: get("tools", d.tools),
        bash: get("bash", d.bash),
        mcp: get("mcp", d.mcp),
        skills: get("skills", d.skills),
        special: get("special", d.special),
    }
}

fn normalize_partial_policy(value: Option<&OrderedValue>) -> PartialDefaultPolicy {
    let get = |k: &str| {
        value.and_then(|v| v.get(k)).and_then(OrderedValue::as_str).and_then(PermissionState::parse)
    };
    PartialDefaultPolicy {
        tools: get("tools"),
        bash: get("bash"),
        mcp: get("mcp"),
        skills: get("skills"),
        special: get("special"),
    }
}

fn normalize_permission_record(value: Option<&OrderedValue>) -> OrderedRules {
    let mut rules = OrderedRules::new();
    if let Some(entries) = value.and_then(OrderedValue::as_object) {
        for (key, val) in entries {
            if let Some(state) = val.as_str().and_then(PermissionState::parse) {
                rules.insert(key.clone(), state);
            }
        }
    }
    rules
}

/// pi `normalizeRawPermission` (`permission-manager.ts:137-166`).
fn normalize_raw_permission(raw: &OrderedValue) -> AgentPermissions {
    let mut normalized = AgentPermissions {
        default_policy: normalize_partial_policy(raw.get("defaultPolicy")),
        tools: normalize_permission_record(raw.get("tools")),
        bash: normalize_permission_record(raw.get("bash")),
        mcp: normalize_permission_record(raw.get("mcp")),
        skills: normalize_permission_record(raw.get("skills")),
        special: normalize_permission_record(raw.get("special")),
    };

    // Fold top-level built-in/special state keys into their category (pi `:150-163`).
    if let Some(entries) = raw.as_object() {
        for (key, val) in entries {
            let Some(state) = val.as_str().and_then(PermissionState::parse) else { continue };
            if is_built_in_tool(key) {
                normalized.tools.insert(key.clone(), state);
            } else if is_special(key) {
                normalized.special.insert(key.clone(), state);
            }
        }
    }

    normalized
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    fn manager_with_global(dir: &Path, body: &str) -> PermissionManager {
        let global = dir.join("cyrup-permissions.jsonc");
        write(&global, body);
        PermissionManager::new(ManagerPaths {
            global_config_path: global,
            agents_dir: dir.join("agents"),
            project_global_config_path: None,
            project_agents_dir: None,
            legacy_global_settings_path: dir.join("settings.json"),
            global_mcp_config_path: dir.join("mcp.json"),
            mcp_server_names_override: Some(Vec::new()),
        })
    }

    #[test]
    fn default_policy_is_ask_for_unconfigured_bash() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = manager_with_global(dir.path(), "{}");
        let r = m.check_permission("bash", &serde_json::json!({"command":"ls"}), None);
        assert_eq!(r.state, PermissionState::Ask);
    }

    #[test]
    fn bash_allow_and_deny_by_command() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = manager_with_global(
            dir.path(),
            r#"{ "bash": { "echo *": "allow", "rm -rf *": "deny" } }"#,
        );
        assert_eq!(
            m.check_permission("bash", &serde_json::json!({"command":"echo hi"}), None).state,
            PermissionState::Allow
        );
        assert_eq!(
            m.check_permission("bash", &serde_json::json!({"command":"rm -rf /"}), None).state,
            PermissionState::Deny
        );
    }

    #[test]
    fn trusted_floor_untrusted_project_cannot_relax_global_deny() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("cyrup-permissions.jsonc");
        write(&global, r#"{ "bash": { "curl *": "deny" } }"#);
        let project = dir.path().join("proj.jsonc");
        write(&project, r#"{ "bash": { "curl *": "allow" } }"#);
        let mut m = PermissionManager::new(ManagerPaths {
            global_config_path: global,
            agents_dir: dir.path().join("agents"),
            project_global_config_path: Some(project),
            project_agents_dir: None,
            legacy_global_settings_path: dir.path().join("settings.json"),
            global_mcp_config_path: dir.path().join("mcp.json"),
            mcp_server_names_override: Some(Vec::new()),
        });
        // Untrusted project's allow cannot relax the trusted global deny.
        assert_eq!(
            m.check_permission("bash", &serde_json::json!({"command":"curl x"}), None).state,
            PermissionState::Deny
        );
    }

    #[test]
    fn directory_resource_read_allow_does_not_grant_edit() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = manager_with_global(
            dir.path(),
            r#"{ "tools": { "read:/data/*": "allow" } }"#,
        );
        let read = m.check_permission(
            "read",
            &serde_json::json!({"path":"/data/x","cwd":"/data"}),
            None,
        );
        assert_eq!(read.state, PermissionState::Allow);
        // `edit` on the same path is NOT granted by a `read:` allow → default ask.
        let edit = m.check_permission(
            "edit",
            &serde_json::json!({"path":"/data/x","cwd":"/data"}),
            None,
        );
        assert_eq!(edit.state, PermissionState::Ask);
    }

    #[test]
    fn dotdot_traversal_canonicalizes_out_of_allowed_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = manager_with_global(
            dir.path(),
            r#"{ "tools": { "read:/data/*": "allow" } }"#,
        );
        let r = m.check_permission(
            "read",
            &serde_json::json!({"path":"/data/../etc/passwd","cwd":"/data"}),
            None,
        );
        assert_eq!(r.state, PermissionState::Ask);
    }

    #[test]
    fn agent_name_traversal_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        // A malicious "../secret" name must resolve to None (out of tree).
        assert!(resolve_agent_markdown_path(Some(&agents), Some("../secret")).is_none());
        // A normal name resolves inside the tree.
        assert!(resolve_agent_markdown_path(Some(&agents), Some("coder")).is_some());
    }

    // Regression test for the missing pi `onWarning`/`formatJsoncConfigLoadWarning` port
    // (`jsonc-config.ts:37-52`, `permission-manager.ts:670-681`): pre-fix, `load_global_config`
    // discarded a parse failure via `let Ok(..) else { return default }` with no warning path at
    // all (`PermissionManager` had no `on_warning` field), so a syntactically broken existing
    // config was silently treated exactly like an absent one.
    #[test]
    fn malformed_global_config_notifies_warning_and_falls_back_to_ask() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("cyrup-permissions.jsonc");
        write(&global, "{ not valid json");
        let warnings: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let warnings_clone = warnings.clone();
        let mut m = PermissionManager::new(ManagerPaths {
            global_config_path: global,
            agents_dir: dir.path().join("agents"),
            project_global_config_path: None,
            project_agents_dir: None,
            legacy_global_settings_path: dir.path().join("settings.json"),
            global_mcp_config_path: dir.path().join("mcp.json"),
            mcp_server_names_override: Some(Vec::new()),
        })
        .with_on_warning(move |msg| warnings_clone.lock().unwrap().push(msg.to_string()));

        let r = m.check_permission("bash", &serde_json::json!({"command":"ls"}), None);
        assert_eq!(r.state, PermissionState::Ask);
        let got = warnings.lock().unwrap();
        assert_eq!(got.len(), 1, "expected exactly one warning, got {got:?}");
        assert!(
            got[0].contains("Failed to parse permission config"),
            "unexpected warning text: {}",
            got[0]
        );
    }

    // A simply-absent config file must stay silent (pi's ENOENT suppression,
    // `jsonc-config.ts:43-45`) — only an EXISTING-but-broken file warns.
    #[test]
    fn missing_global_config_does_not_notify_warning() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("does-not-exist.jsonc");
        let warnings: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let warnings_clone = warnings.clone();
        let mut m = PermissionManager::new(ManagerPaths {
            global_config_path: global,
            agents_dir: dir.path().join("agents"),
            project_global_config_path: None,
            project_agents_dir: None,
            legacy_global_settings_path: dir.path().join("settings.json"),
            global_mcp_config_path: dir.path().join("mcp.json"),
            mcp_server_names_override: Some(Vec::new()),
        })
        .with_on_warning(move |msg| warnings_clone.lock().unwrap().push(msg.to_string()));

        let _ = m.check_permission("bash", &serde_json::json!({"command":"ls"}), None);
        assert!(warnings.lock().unwrap().is_empty());
    }

    // Regression test for the skill-name emptiness divergence (pi `permission-manager.ts:934-951`
    // checks `typeof skillName === "string"`, i.e. ANY string including `""`; pre-fix cyrup used
    // `get_non_empty_string`, which trims and rejects empty/whitespace names, so an empty name
    // never reached the pattern-match branch and a `"*": "allow"` skills wildcard was ignored).
    #[test]
    fn skill_empty_name_matches_wildcard_like_pi() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = manager_with_global(dir.path(), r#"{ "skills": { "*": "allow" } }"#);
        let r = m.check_permission("skill", &serde_json::json!({"name": ""}), None);
        assert_eq!(r.state, PermissionState::Allow);
        assert_eq!(r.matched_pattern.as_deref(), Some("*"));
    }
}
