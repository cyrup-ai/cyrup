//! Direct-MCP tool-allowlist resolution — a faithful port of pi-subagents'
//! `runs/shared/mcp-direct-tool-allowlist.ts` (T4).
//!
//! A subagent may declare `mcp:<server>` / `mcp:<server>/<tool>` entries in its `tools` allowlist
//! (parsed at discovery time into [`crate::discovery::types::ToolRef::Mcp`], with the `mcp:` prefix
//! stripped — pi's `splitToolList` / `mcpDirectTools`). Those bare selectors are NOT valid builtin
//! tool names on their own: before they can be handed to the child's `--tools` allowlist, each
//! `mcp:` selector must be **expanded** into the concrete, adapter-visible tool names it selects
//! (per-server tool + resource expansion, server-prefix modes, `excludeTools`, builtin/duplicate
//! collision suppression), consulting the MCP metadata cache the adapter maintains. Before this
//! module existed, `mcp:` refs were passed through to `--tools` literally (the T4 gap) — a name the
//! child's tool registry could never resolve.
//!
//! This is the direct analogue of pi's module: [`resolve_mcp_direct_tool_names`] is
//! `resolveMcpDirectToolNames`; [`compute_mcp_server_hash`] is `computeMcpServerHash` (exported so
//! this crate's tests — and, in production, whichever component writes the metadata cache — stamp
//! the SAME `configHash` the resolver validates against). The one deliberate deviation from a
//! line-for-line port is directory resolution: pi reads `getAgentDir()` / `os.homedir()` from
//! `process.env` directly; this crate forbids `unsafe` env mutation (edition-2024), so the
//! directory context is factored into an injectable [`McpDirs`] — [`McpDirs::from_env`] reproduces
//! pi's env/home resolution for production, while tests inject a hermetic [`McpDirs`] pointing at a
//! tempdir (the faithful equivalent of pi's tests setting `PI_CODING_AGENT_DIR`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The metadata-cache schema version this resolver understands (pi `CACHE_VERSION`). A cache file
/// declaring any other version is treated as absent.
const CACHE_VERSION: i64 = 1;

/// Maximum age (milliseconds) a cached server-metadata entry may have before it is treated as
/// stale and skipped — pi's `CACHE_MAX_AGE_MS` (7 days).
const CACHE_MAX_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Builtin tool names a resolved/prefixed MCP name may never shadow (pi `BUILTIN_TOOL_NAMES`): a
/// formatted MCP name colliding with one of these is dropped rather than emitted.
const BUILTIN_TOOL_NAMES: [&str; 8] = ["read", "bash", "edit", "write", "grep", "find", "ls", "mcp"];

/// The server-name → tool-name prefixing mode (pi `ToolPrefix = "server" | "none" | "short"`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ToolPrefix {
    Server,
    None,
    Short,
}

/// The adapter/host families whose native MCP config a pi `imports` array can pull servers from
/// (pi `IMPORT_PATHS` keys). Ported for completeness (pi resolves these before the resolver runs);
/// none of this crate's required behaviors depend on them, but the module is the "whole"
/// `mcp-direct-tool-allowlist.ts`, imports included.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ImportKind {
    Cursor,
    ClaudeCode,
    ClaudeDesktop,
    Codex,
    Windsurf,
    Vscode,
}

impl ImportKind {
    fn parse(value: &str) -> Option<ImportKind> {
        match value {
            "cursor" => Some(ImportKind::Cursor),
            "claude-code" => Some(ImportKind::ClaudeCode),
            "claude-desktop" => Some(ImportKind::ClaudeDesktop),
            "codex" => Some(ImportKind::Codex),
            "windsurf" => Some(ImportKind::Windsurf),
            "vscode" => Some(ImportKind::Vscode),
            _ => None,
        }
    }

    /// Candidate config paths for this import family, in priority order (pi `IMPORT_PATHS`). A
    /// leading `.`-relative entry (`vscode`) resolves against `cwd`; every other entry is
    /// home-anchored.
    fn candidate_paths(self, cwd: &Path, home: &Path) -> Vec<PathBuf> {
        match self {
            ImportKind::Cursor => vec![home.join(".cursor").join("mcp.json")],
            ImportKind::ClaudeCode => vec![
                home.join(".claude").join("mcp.json"),
                home.join(".claude.json"),
                home.join(".claude").join("claude_desktop_config.json"),
            ],
            ImportKind::ClaudeDesktop => vec![home
                .join("Library")
                .join("Application Support")
                .join("Claude")
                .join("claude_desktop_config.json")],
            ImportKind::Codex => vec![home.join(".codex").join("config.json")],
            ImportKind::Windsurf => vec![home.join(".windsurf").join("mcp.json")],
            ImportKind::Vscode => vec![cwd.join(".vscode").join("mcp.json")],
        }
    }

    /// Whether this family's native config nests its servers under `mcpServers`/`mcp-servers`
    /// (pi: cursor/windsurf/vscode) or only `mcpServers` (the others).
    fn allows_hyphen_key(self) -> bool {
        matches!(self, ImportKind::Cursor | ImportKind::Windsurf | ImportKind::Vscode)
    }
}

/// One MCP server definition as read from an `mcp.json` (pi `ServerEntry`). Only the fields that
/// participate in resolution and the config-identity hash are modeled; every other key (e.g.
/// `directTools`, adapter-private fields) is ignored on deserialize. Public so this crate's tests
/// can build a definition, hash it with [`compute_mcp_server_hash`], and write both the config and
/// a matching cache entry — exactly as pi's own test fixtures do.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServerEntry {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// `Record<string, unknown>` in pi; non-string values are dropped by interpolation, so this is
    /// held as raw JSON and filtered to strings at hash time (matching `interpolateEnvRecord`).
    #[serde(default)]
    pub env: Option<BTreeMap<String, Value>>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, Value>>,
    /// `"oauth" | "bearer" | false` in pi — held raw and folded verbatim into the identity hash.
    #[serde(default)]
    pub auth: Option<Value>,
    #[serde(default, rename = "bearerToken")]
    pub bearer_token: Option<String>,
    #[serde(default, rename = "bearerTokenEnv")]
    pub bearer_token_env: Option<String>,
    #[serde(default, rename = "exposeResources")]
    pub expose_resources: Option<bool>,
    #[serde(default, rename = "excludeTools")]
    pub exclude_tools: Option<Vec<String>>,
}

/// Optional `settings` block of an `mcp.json` (pi `McpConfig.settings`).
#[derive(Clone, Debug, Default, Deserialize)]
struct McpSettings {
    #[serde(default, rename = "toolPrefix")]
    tool_prefix: Option<String>,
}

/// A merged view of every `mcp.json` source (pi `McpConfig`).
#[derive(Clone, Debug, Default)]
struct McpConfig {
    mcp_servers: BTreeMap<String, ServerEntry>,
    imports: Vec<ImportKind>,
    settings: Option<McpSettings>,
}

/// One cached tool descriptor (pi `CachedTool`).
#[derive(Clone, Debug, Default, Deserialize)]
struct CachedTool {
    #[serde(default)]
    name: Option<String>,
}

/// One cached resource descriptor (pi `CachedResource`).
#[derive(Clone, Debug, Default, Deserialize)]
struct CachedResource {
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// One server's cached metadata entry (pi `ServerCacheEntry`).
#[derive(Clone, Debug, Default, Deserialize)]
struct ServerCacheEntry {
    #[serde(default, rename = "configHash")]
    config_hash: Option<String>,
    #[serde(default)]
    tools: Option<Vec<CachedTool>>,
    #[serde(default)]
    resources: Option<Vec<CachedResource>>,
    #[serde(default, rename = "cachedAt")]
    cached_at: Option<i64>,
}

/// The whole on-disk metadata cache (pi `MetadataCache`).
#[derive(Clone, Debug, Deserialize)]
struct MetadataCache {
    version: i64,
    servers: BTreeMap<String, ServerCacheEntry>,
}

/// The directory context the resolver reads config/cache from. Factored out of the bare
/// `process.env` reads pi performs inline (`getAgentDir()` / `os.homedir()`) so tests can inject a
/// hermetic layout without mutating real (edition-2024 `unsafe`) process env — mirroring
/// [`crate::spawn::depth::resolve_effective_depth_from`]'s injectable-lookup pattern.
#[derive(Clone, Debug)]
pub struct McpDirs {
    /// pi `getAgentDir()` — holds `mcp.json` (global pi config) and `mcp-cache.json`.
    pub agent_dir: PathBuf,
    /// pi `GENERIC_GLOBAL_CONFIG_PATH` — `~/.config/mcp/mcp.json`.
    pub generic_global_config_path: PathBuf,
    /// The user home dir, for `~`-expansion of a server's `cwd` and for import-path resolution.
    pub home: PathBuf,
}

impl McpDirs {
    /// Resolve the production directory context from the process environment, reproducing pi's
    /// `getAgentDir()` (`CYRUP_AGENT_DIR`/`PI_CODING_AGENT_DIR` with `~` expansion, else
    /// `<home>/.cyrup/agent`) and `GENERIC_GLOBAL_CONFIG_PATH` (`~/.config/mcp/mcp.json`).
    #[must_use]
    pub fn from_env() -> McpDirs {
        let home = home_dir();
        let agent_dir = resolve_agent_dir(&home);
        let generic_global_config_path = home.join(".config").join("mcp").join("mcp.json");
        McpDirs {
            agent_dir,
            generic_global_config_path,
            home,
        }
    }
}

/// Resolve the concrete, adapter-visible builtin tool names selected by a subagent's `mcp:` direct
/// selectors (pi `resolveMcpDirectToolNames`). `mcp_direct_tools` is the list of `mcp:`-stripped
/// selectors (`<server>` or `<server>/<tool>`); `cwd` is the child's working directory (project
/// `.mcp.json` / `.cyrup/mcp.json` are resolved against it). Any missing/invalid config or cache,
/// or a stale/mismatched cache entry, yields an empty list — never an error (pi returns `[]` from
/// its `try/catch`), so the caller simply omits those names from `--tools`.
#[must_use]
pub fn resolve_mcp_direct_tool_names(mcp_direct_tools: &[String], cwd: &Path) -> Vec<String> {
    resolve_mcp_direct_tool_names_in(mcp_direct_tools, cwd, &McpDirs::from_env())
}

/// The injectable core of [`resolve_mcp_direct_tool_names`] — identical behavior, but reading its
/// config/cache from an explicit [`McpDirs`] instead of the process environment (so tests can be
/// hermetic without `unsafe` env mutation).
#[must_use]
pub fn resolve_mcp_direct_tool_names_in(
    mcp_direct_tools: &[String],
    cwd: &Path,
    dirs: &McpDirs,
) -> Vec<String> {
    if mcp_direct_tools.is_empty() {
        return Vec::new();
    }
    let config = load_mcp_config(cwd, dirs);
    let Some(cache) = load_metadata_cache(dirs) else {
        return Vec::new();
    };
    let prefix = get_tool_prefix(config.settings.as_ref().and_then(|s| s.tool_prefix.as_deref()));
    resolve_direct_tool_names(&config, &cache, prefix, mcp_direct_tools)
}

fn load_metadata_cache(dirs: &McpDirs) -> Option<MetadataCache> {
    let cache_path = dirs.agent_dir.join("mcp-cache.json");
    let text = std::fs::read_to_string(&cache_path).ok()?;
    let parsed: MetadataCache = serde_json::from_str(&text).ok()?;
    if parsed.version != CACHE_VERSION {
        return None;
    }
    Some(parsed)
}

fn load_mcp_config(cwd: &Path, dirs: &McpDirs) -> McpConfig {
    let mut config = McpConfig::default();
    for source_path in get_config_paths(cwd, dirs) {
        let Some(loaded) = read_config(&source_path) else {
            continue;
        };
        config = merge_configs(config, expand_imports(loaded, cwd, dirs));
    }
    config
}

fn get_config_paths(cwd: &Path, dirs: &McpDirs) -> Vec<PathBuf> {
    let pi_global_path = dirs.agent_dir.join("mcp.json");
    let project_path = cwd.join(".mcp.json");
    let project_pi_path = cwd.join(".cyrup").join("mcp.json");
    let mut sources = Vec::new();
    if dirs.generic_global_config_path != pi_global_path {
        sources.push(dirs.generic_global_config_path.clone());
    }
    sources.push(pi_global_path.clone());
    if project_path != pi_global_path {
        sources.push(project_path.clone());
    }
    if project_pi_path != pi_global_path && project_pi_path != project_path {
        sources.push(project_pi_path);
    }
    sources
}

fn read_config(config_path: &Path) -> Option<McpConfig> {
    let text = std::fs::read_to_string(config_path).ok()?;
    let parsed: Value = serde_json::from_str(&text).ok()?;
    Some(validate_config(&parsed))
}

fn validate_config(raw: &Value) -> McpConfig {
    let Some(obj) = raw.as_object() else {
        return McpConfig::default();
    };
    let servers_value = obj
        .get("mcpServers")
        .or_else(|| obj.get("mcp-servers"))
        .cloned()
        .unwrap_or(Value::Null);
    McpConfig {
        mcp_servers: extract_server_map(&servers_value),
        imports: obj
            .get("imports")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().and_then(ImportKind::parse))
                    .collect()
            })
            .unwrap_or_default(),
        settings: obj
            .get("settings")
            .filter(|s| s.is_object())
            .and_then(|s| serde_json::from_value::<McpSettings>(s.clone()).ok()),
    }
}

/// Deserialize a `Record<string, ServerEntry>` leniently: a single malformed server entry is
/// dropped rather than failing the whole map (matching pi's dynamic `as` cast, which never throws
/// on a per-entry shape mismatch).
fn extract_server_map(value: &Value) -> BTreeMap<String, ServerEntry> {
    let Some(map) = value.as_object() else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for (name, def) in map {
        if let Ok(entry) = serde_json::from_value::<ServerEntry>(def.clone()) {
            out.insert(name.clone(), entry);
        }
    }
    out
}

fn merge_configs(base: McpConfig, next: McpConfig) -> McpConfig {
    let mut mcp_servers = base.mcp_servers;
    for (name, entry) in next.mcp_servers {
        mcp_servers.insert(name, entry);
    }
    let mut imports = base.imports;
    for kind in next.imports {
        if !imports.contains(&kind) {
            imports.push(kind);
        }
    }
    let settings = next.settings.or(base.settings);
    McpConfig {
        mcp_servers,
        imports,
        settings,
    }
}

fn expand_imports(config: McpConfig, cwd: &Path, dirs: &McpDirs) -> McpConfig {
    if config.imports.is_empty() {
        return config;
    }
    let mut imported_servers: BTreeMap<String, ServerEntry> = BTreeMap::new();
    for import_kind in &config.imports {
        let Some(import_path) = resolve_import_path(*import_kind, cwd, &dirs.home) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&import_path) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        for (name, definition) in extract_import_servers(&parsed, *import_kind) {
            imported_servers.entry(name).or_insert(definition);
        }
    }
    // Project/global servers win over imported ones of the same name (pi spreads
    // `...importedServers` first, then `...config.mcpServers`).
    let mut mcp_servers = imported_servers;
    for (name, entry) in config.mcp_servers {
        mcp_servers.insert(name, entry);
    }
    McpConfig {
        mcp_servers,
        imports: config.imports,
        settings: config.settings,
    }
}

fn resolve_import_path(import_kind: ImportKind, cwd: &Path, home: &Path) -> Option<PathBuf> {
    import_kind
        .candidate_paths(cwd, home)
        .into_iter()
        .find(|candidate| candidate.exists())
}

fn extract_import_servers(config: &Value, kind: ImportKind) -> BTreeMap<String, ServerEntry> {
    let Some(obj) = config.as_object() else {
        return BTreeMap::new();
    };
    let servers = if kind.allows_hyphen_key() {
        obj.get("mcpServers").or_else(|| obj.get("mcp-servers"))
    } else {
        obj.get("mcpServers")
    };
    servers.map(extract_server_map).unwrap_or_default()
}

fn resolve_direct_tool_names(
    config: &McpConfig,
    cache: &MetadataCache,
    prefix: ToolPrefix,
    env_override: &[String],
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let (selected_servers, selected_tools) = parse_selections(env_override);

    for (server_name, definition) in &config.mcp_servers {
        let Some(server_cache) = cache.servers.get(server_name) else {
            continue;
        };
        if !is_server_cache_valid(server_cache, definition) {
            continue;
        }

        // `true` selects the whole server; `Some(set)` selects only named tools; `None` skips it.
        let whole_server = selected_servers.contains(server_name);
        let tool_filter = selected_tools.get(server_name);
        if !whole_server && tool_filter.is_none() {
            continue;
        }

        for tool in server_cache.tools.iter().flatten() {
            let Some(tool_name) = tool.name.as_deref().filter(|n| !n.is_empty()) else {
                continue;
            };
            if !whole_server && !tool_filter.map(|f| f.contains(tool_name)).unwrap_or(false) {
                continue;
            }
            if is_tool_excluded(tool_name, server_name, prefix, definition.exclude_tools.as_deref()) {
                continue;
            }
            let prefixed = format_tool_name(tool_name, server_name, prefix);
            if is_builtin_name(&prefixed) || seen_names.contains(&prefixed) {
                continue;
            }
            seen_names.insert(prefixed.clone());
            names.push(prefixed);
        }

        if definition.expose_resources == Some(false) {
            continue;
        }
        for resource in server_cache.resources.iter().flatten() {
            let (Some(name), Some(uri)) = (
                resource.name.as_deref().filter(|n| !n.is_empty()),
                resource.uri.as_deref().filter(|u| !u.is_empty()),
            ) else {
                continue;
            };
            let _ = uri; // presence-checked above, exactly as pi requires a non-empty uri
            let base_name = format!("get_{}", resource_name_to_tool_name(name));
            if !whole_server && !tool_filter.map(|f| f.contains(&base_name)).unwrap_or(false) {
                continue;
            }
            if is_tool_excluded(&base_name, server_name, prefix, definition.exclude_tools.as_deref())
            {
                continue;
            }
            let prefixed = format_tool_name(&base_name, server_name, prefix);
            if is_builtin_name(&prefixed) || seen_names.contains(&prefixed) {
                continue;
            }
            seen_names.insert(prefixed.clone());
            names.push(prefixed);
        }
    }

    names
}

fn is_builtin_name(name: &str) -> bool {
    BUILTIN_TOOL_NAMES.contains(&name)
}

/// Parse the `mcp:` selectors into whole-server selections and per-server named-tool selections
/// (pi `parseSelections`). Mirrors JS `item.split("/", 2)` (which discards a third segment).
fn parse_selections(selections: &[String]) -> (HashSet<String>, HashMap<String, HashSet<String>>) {
    let mut servers: HashSet<String> = HashSet::new();
    let mut tools: HashMap<String, HashSet<String>> = HashMap::new();
    for raw in selections {
        let item = raw.trim_end_matches('/');
        if item.contains('/') {
            let mut parts = item.split('/');
            let server = parts.next().unwrap_or("");
            let tool = parts.next().unwrap_or("");
            if !server.is_empty() && !tool.is_empty() {
                tools
                    .entry(server.to_string())
                    .or_default()
                    .insert(tool.to_string());
            } else if !server.is_empty() {
                servers.insert(server.to_string());
            }
        } else if !item.is_empty() {
            servers.insert(item.to_string());
        }
    }
    (servers, tools)
}

fn is_server_cache_valid(entry: &ServerCacheEntry, definition: &ServerEntry) -> bool {
    if entry.config_hash.as_deref() != Some(compute_mcp_server_hash(definition).as_str()) {
        return false;
    }
    let Some(cached_at) = entry.cached_at else {
        return false;
    };
    now_ms().saturating_sub(cached_at) <= CACHE_MAX_AGE_MS
}

/// Compute the stable config-identity hash for a server definition (pi `computeMcpServerHash`):
/// a SHA-256 over the sorted-key JSON of the resolution-relevant fields (with env/headers/cwd/
/// bearer-token interpolated against the process environment, exactly as pi does). Exported so a
/// cache writer (and this crate's tests) stamp the same `configHash` the resolver validates.
#[must_use]
pub fn compute_mcp_server_hash(definition: &ServerEntry) -> String {
    let home = home_dir();
    let env = |name: &str| std::env::var(name).ok();
    let mut identity = serde_json::Map::new();
    identity.insert("command".to_string(), opt_str_value(definition.command.as_deref()));
    identity.insert(
        "args".to_string(),
        definition
            .args
            .as_ref()
            .map(|a| Value::Array(a.iter().map(|s| Value::String(s.clone())).collect()))
            .unwrap_or(Value::Null),
    );
    identity.insert("env".to_string(), interpolate_env_record(definition.env.as_ref(), &env));
    identity.insert(
        "cwd".to_string(),
        opt_str_value(resolve_config_path(definition.cwd.as_deref(), &home, &env).as_deref()),
    );
    identity.insert("url".to_string(), opt_str_value(definition.url.as_deref()));
    identity.insert(
        "headers".to_string(),
        interpolate_env_record(definition.headers.as_ref(), &env),
    );
    identity.insert("auth".to_string(), definition.auth.clone().unwrap_or(Value::Null));
    identity.insert(
        "bearerToken".to_string(),
        opt_str_value(resolve_bearer_token(definition, &env).as_deref()),
    );
    identity.insert(
        "bearerTokenEnv".to_string(),
        opt_str_value(definition.bearer_token_env.as_deref()),
    );
    identity.insert(
        "exposeResources".to_string(),
        definition.expose_resources.map(Value::Bool).unwrap_or(Value::Null),
    );
    identity.insert(
        "excludeTools".to_string(),
        definition
            .exclude_tools
            .as_ref()
            .map(|a| Value::Array(a.iter().map(|s| Value::String(s.clone())).collect()))
            .unwrap_or(Value::Null),
    );

    let serialized = stable_stringify(&Value::Object(identity));
    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

fn opt_str_value(value: Option<&str>) -> Value {
    value.map(|s| Value::String(s.to_string())).unwrap_or(Value::Null)
}

fn get_tool_prefix(value: Option<&str>) -> ToolPrefix {
    match value {
        Some("none") => ToolPrefix::None,
        Some("short") => ToolPrefix::Short,
        _ => ToolPrefix::Server,
    }
}

fn get_server_prefix(server_name: &str, mode: ToolPrefix) -> String {
    match mode {
        ToolPrefix::None => String::new(),
        ToolPrefix::Short => {
            let short = strip_mcp_suffix(server_name).replace('-', "_");
            if short.is_empty() {
                "mcp".to_string()
            } else {
                short
            }
        }
        ToolPrefix::Server => server_name.replace('-', "_"),
    }
}

/// Port of pi's `serverName.replace(/-?mcp$/i, "")`: strip a trailing case-insensitive `mcp`, plus
/// one optional preceding `-`.
fn strip_mcp_suffix(server_name: &str) -> &str {
    let lower = server_name.to_ascii_lowercase();
    let trimmed = lower
        .strip_suffix("mcp")
        .map(|p| p.strip_suffix('-').unwrap_or(p));
    match trimmed {
        Some(prefix) => server_name.get(..prefix.len()).unwrap_or(server_name),
        None => server_name,
    }
}

fn format_tool_name(tool_name: &str, server_name: &str, prefix: ToolPrefix) -> String {
    let server_prefix = get_server_prefix(server_name, prefix);
    if server_prefix.is_empty() {
        tool_name.to_string()
    } else {
        format!("{server_prefix}_{tool_name}")
    }
}

fn is_tool_excluded(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    exclude_tools: Option<&[String]>,
) -> bool {
    let Some(exclude_tools) = exclude_tools.filter(|e| !e.is_empty()) else {
        return false;
    };
    let candidates: HashSet<String> = [
        normalize_tool_name(tool_name),
        normalize_tool_name(&format_tool_name(tool_name, server_name, prefix)),
        normalize_tool_name(&format_tool_name(tool_name, server_name, ToolPrefix::Server)),
        normalize_tool_name(&format_tool_name(tool_name, server_name, ToolPrefix::Short)),
    ]
    .into_iter()
    .collect();
    exclude_tools
        .iter()
        .any(|excluded| candidates.contains(&normalize_tool_name(excluded)))
}

fn normalize_tool_name(value: &str) -> String {
    value.replace('-', "_")
}

/// Port of pi `resourceNameToToolName`: non-alphanumerics → `_`, collapse runs, trim edge `_`,
/// lowercase; empty or leading-digit results are prefixed with `resource`.
fn resource_name_to_tool_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let mut collapsed = String::with_capacity(replaced.len());
    let mut prev_underscore = false;
    for c in replaced.chars() {
        if c == '_' {
            if !prev_underscore {
                collapsed.push('_');
            }
            prev_underscore = true;
        } else {
            collapsed.push(c);
            prev_underscore = false;
        }
    }
    let mut result = collapsed.trim_matches('_').to_ascii_lowercase();
    let starts_with_digit = result.chars().next().is_some_and(|c| c.is_ascii_digit());
    if result.is_empty() || starts_with_digit {
        result = if result.is_empty() {
            "resource".to_string()
        } else {
            format!("resource_{result}")
        };
    }
    result
}

/// Interpolate a `Record<string, unknown>`, keeping only string-typed values and expanding env
/// references in each (pi `interpolateEnvRecord`). Absent/empty maps yield JSON `null`.
fn interpolate_env_record(
    values: Option<&BTreeMap<String, Value>>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Value {
    let Some(values) = values else {
        return Value::Null;
    };
    let mut resolved = serde_json::Map::new();
    for (key, value) in values {
        if let Some(text) = value.as_str() {
            resolved.insert(key.clone(), Value::String(interpolate_env_vars(text, env)));
        }
    }
    Value::Object(resolved)
}

/// Expand `${NAME}` then `$env:NAME` references against `env` (pi `interpolateEnvVars`). An absent
/// variable expands to the empty string.
fn interpolate_env_vars(value: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    let after_braces = expand_pattern(value, "${", Some("}"), env);
    expand_pattern(&after_braces, "$env:", None, env)
}

/// Expand `<open><NAME><close?>` references, where `NAME` is `[A-Za-z0-9_]+`. When `close` is
/// `Some`, the name runs up to the closing delimiter (`${NAME}`); when `None`, the name runs to the
/// first non-word char (`$env:NAME`). A malformed/empty reference is emitted verbatim.
fn expand_pattern(
    input: &str,
    open: &str,
    close: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find(open) {
        out.push_str(rest.get(..start).unwrap_or(""));
        let after = rest.get(start + open.len()..).unwrap_or("");
        match close {
            Some(close_delim) => {
                if let Some(end) = after.find(close_delim) {
                    let name = after.get(..end).unwrap_or("");
                    if !name.is_empty() && name.chars().all(is_word_char) {
                        out.push_str(&env(name).unwrap_or_default());
                        rest = after.get(end + close_delim.len()..).unwrap_or("");
                        continue;
                    }
                }
                out.push_str(open);
                rest = after;
            }
            None => {
                let name_len = after.find(|c: char| !is_word_char(c)).unwrap_or(after.len());
                let name = after.get(..name_len).unwrap_or("");
                if name.is_empty() {
                    out.push_str(open);
                    rest = after;
                } else {
                    out.push_str(&env(name).unwrap_or_default());
                    rest = after.get(name_len..).unwrap_or("");
                }
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn resolve_config_path(
    value: Option<&str>,
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let value = value?;
    let resolved = interpolate_env_vars(value, env);
    if resolved == "~" {
        return Some(home.to_string_lossy().into_owned());
    }
    if let Some(rest) = resolved.strip_prefix("~/").or_else(|| resolved.strip_prefix("~\\")) {
        return Some(home.join(rest).to_string_lossy().into_owned());
    }
    Some(resolved)
}

fn resolve_bearer_token(
    definition: &ServerEntry,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    if let Some(token) = definition.bearer_token.as_deref() {
        return Some(interpolate_env_vars(token, env));
    }
    definition.bearer_token_env.as_deref().and_then(env)
}

/// Stable, key-sorted JSON stringification (pi `stableStringify`) — the pre-image the config hash
/// is taken over. Deterministic across runs (object keys sorted), so a cache writer and this
/// resolver agree on `configHash`.
fn stable_stringify(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(_) => serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string()),
        Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(stable_stringify).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|key| {
                    let key_json = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
                    let val_json = map.get(key).map(stable_stringify).unwrap_or_else(|| "null".to_string());
                    format!("{key_json}:{val_json}")
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The user home dir, mirroring this crate's existing `extension.rs::dirs_home` convention
/// (`CYRUP_HOME` → `HOME` → tempdir) so the resolver anchors identically to the rest of the crate.
fn home_dir() -> PathBuf {
    std::env::var_os("CYRUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
}

/// Resolve pi's `getAgentDir()`: `CYRUP_AGENT_DIR`/`PI_CODING_AGENT_DIR` (with `~` expansion), else
/// `<home>/.cyrup/agent`.
fn resolve_agent_dir(home: &Path) -> PathBuf {
    let configured = std::env::var("CYRUP_AGENT_DIR")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("PI_CODING_AGENT_DIR").ok().filter(|v| !v.is_empty()));
    match configured {
        Some(v) if v == "~" => home.to_path_buf(),
        Some(v) if v.starts_with("~/") => home.join(v.get(2..).unwrap_or("")),
        Some(v) => PathBuf::from(v),
        None => home.join(".cyrup").join("agent"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::too_many_arguments
    )]

    use super::*;

    struct McpFixture {
        _root: tempfile::TempDir,
        agent_dir: PathBuf,
        project_dir: PathBuf,
        dirs: McpDirs,
    }

    fn make_fixture() -> McpFixture {
        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path().join("home");
        let agent_dir = home.join(".cyrup").join("agent");
        let project_dir = root.path().join("project");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        std::fs::create_dir_all(&project_dir).expect("project dir");
        let dirs = McpDirs {
            agent_dir: agent_dir.clone(),
            generic_global_config_path: home.join(".config").join("mcp").join("mcp.json"),
            home,
        };
        McpFixture {
            _root: root,
            agent_dir,
            project_dir,
            dirs,
        }
    }

    fn write_json(path: &Path, value: &Value) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent dir");
        }
        std::fs::write(path, serde_json::to_string_pretty(value).expect("json")).expect("write");
    }

    /// Mirror of pi's `writeMcpFixture`: write an `mcp.json` (server config) and a matching
    /// `mcp-cache.json` (tools/resources + the identity hash) under the given config path + agent
    /// dir.
    fn write_mcp_fixture(
        fixture: &McpFixture,
        server_name: &str,
        definition_extra: Value,
        settings: Option<Value>,
        tools: Vec<&str>,
        resources: Vec<(&str, &str)>,
        config_path: Option<PathBuf>,
        cached_at: Option<i64>,
    ) {
        let mut definition = serde_json::json!({ "command": "npx", "args": ["chrome-devtools-mcp"] });
        if let Value::Object(extra) = definition_extra
            && let Value::Object(base) = &mut definition
        {
            for (k, v) in extra {
                base.insert(k, v);
            }
        }
        let mut config = serde_json::Map::new();
        if let Some(settings) = settings {
            config.insert("settings".to_string(), settings);
        }
        config.insert(
            "mcpServers".to_string(),
            serde_json::json!({ server_name: definition.clone() }),
        );
        let config_path = config_path.unwrap_or_else(|| fixture.agent_dir.join("mcp.json"));
        write_json(&config_path, &Value::Object(config));

        let entry: ServerEntry = serde_json::from_value(definition.clone()).expect("server entry");
        let tools_json: Vec<Value> = tools
            .iter()
            .map(|name| serde_json::json!({ "name": name }))
            .collect();
        let resources_json: Vec<Value> = resources
            .iter()
            .map(|(name, uri)| serde_json::json!({ "name": name, "uri": uri }))
            .collect();
        write_json(
            &fixture.agent_dir.join("mcp-cache.json"),
            &serde_json::json!({
                "version": 1,
                "servers": {
                    server_name: {
                        "configHash": compute_mcp_server_hash(&entry),
                        "cachedAt": cached_at.unwrap_or_else(now_ms),
                        "tools": tools_json,
                        "resources": resources_json,
                    }
                }
            }),
        );
    }

    fn resolve(fixture: &McpFixture, selectors: &[&str]) -> Vec<String> {
        let owned: Vec<String> = selectors.iter().map(|s| s.to_string()).collect();
        resolve_mcp_direct_tool_names_in(&owned, &fixture.project_dir, &fixture.dirs)
    }

    #[test]
    fn selects_direct_mcp_tool_names_for_a_whole_server() {
        // pi: "augments explicit builtin allowlists with selected direct MCP tool names".
        let fixture = make_fixture();
        write_mcp_fixture(
            &fixture,
            "chrome-devtools",
            Value::Null,
            None,
            vec!["take_screenshot", "click"],
            vec![],
            None,
            None,
        );
        assert_eq!(
            resolve(&fixture, &["chrome-devtools"]),
            vec![
                "chrome_devtools_take_screenshot".to_string(),
                "chrome_devtools_click".to_string()
            ]
        );
    }

    #[test]
    fn supports_direct_mcp_server_slash_tool_filters() {
        // pi: "supports direct MCP server/tool filters".
        let fixture = make_fixture();
        write_mcp_fixture(
            &fixture,
            "github",
            serde_json::json!({ "command": "github-mcp" }),
            None,
            vec!["search_repositories", "create_issue"],
            vec![],
            None,
            None,
        );
        assert_eq!(
            resolve(&fixture, &["github/search_repositories"]),
            vec!["github_search_repositories".to_string()]
        );
    }

    #[test]
    fn matches_adapter_prefix_modes_for_direct_mcp_names() {
        // pi: "matches adapter prefix modes for direct MCP names".
        for (prefix, expected) in [
            ("server", "linear_mcp_list_issues"),
            ("short", "linear_list_issues"),
            ("none", "list_issues"),
        ] {
            let fixture = make_fixture();
            write_mcp_fixture(
                &fixture,
                "linear-mcp",
                Value::Null,
                Some(serde_json::json!({ "toolPrefix": prefix })),
                vec!["list_issues"],
                vec![],
                None,
                None,
            );
            assert_eq!(resolve(&fixture, &["linear-mcp"]), vec![expected.to_string()]);
        }
    }

    #[test]
    fn includes_resource_tools_and_respects_exclude_tools() {
        // pi: "includes resource tools and respects excludeTools".
        let fixture = make_fixture();
        write_mcp_fixture(
            &fixture,
            "browser-mcp",
            serde_json::json!({ "excludeTools": ["browser_click"] }),
            None,
            vec!["click", "navigate"],
            vec![("Console Logs", "resource://console")],
            None,
            None,
        );
        assert_eq!(
            resolve(&fixture, &["browser-mcp"]),
            vec![
                "browser_mcp_navigate".to_string(),
                "browser_mcp_get_console_logs".to_string()
            ]
        );
    }

    #[test]
    fn falls_back_to_empty_when_cache_missing_or_stale() {
        // pi: "falls back to explicit builtins when direct MCP cache or config is missing or invalid".
        let missing = make_fixture();
        write_json(
            &missing.agent_dir.join("mcp.json"),
            &serde_json::json!({
                "mcpServers": { "chrome-devtools": { "command": "npx", "args": ["chrome-devtools-mcp"] } }
            }),
        );
        // No cache file at all -> empty.
        assert!(resolve(&missing, &["chrome-devtools"]).is_empty());

        // Stale cache (8 days old) -> the server is skipped, empty.
        let stale = make_fixture();
        write_mcp_fixture(
            &stale,
            "chrome-devtools",
            Value::Null,
            None,
            vec!["take_screenshot", "click"],
            vec![],
            None,
            Some(now_ms() - 8 * 24 * 60 * 60 * 1000),
        );
        assert!(resolve(&stale, &["chrome-devtools"]).is_empty());
    }

    #[test]
    fn resolves_project_mcp_config_from_the_child_cwd() {
        // pi: "resolves project MCP config from the child cwd".
        let fixture = make_fixture();
        write_mcp_fixture(
            &fixture,
            "project-mcp",
            Value::Null,
            None,
            vec!["inspect"],
            vec![],
            Some(fixture.project_dir.join(".mcp.json")),
            None,
        );
        assert_eq!(
            resolve(&fixture, &["project-mcp"]),
            vec!["project_mcp_inspect".to_string()]
        );
    }

    #[test]
    fn empty_selector_list_resolves_to_empty() {
        let fixture = make_fixture();
        assert!(resolve_mcp_direct_tool_names_in(&[], &fixture.project_dir, &fixture.dirs).is_empty());
    }

    #[test]
    fn resource_name_to_tool_name_matches_pi_rules() {
        assert_eq!(resource_name_to_tool_name("Console Logs"), "console_logs");
        assert_eq!(resource_name_to_tool_name("123"), "resource_123");
        assert_eq!(resource_name_to_tool_name("!!!"), "resource");
    }

    #[test]
    fn strip_mcp_suffix_matches_pi_short_prefix_rule() {
        assert_eq!(strip_mcp_suffix("linear-mcp"), "linear");
        assert_eq!(strip_mcp_suffix("chrome-devtools"), "chrome-devtools");
        assert_eq!(get_server_prefix("linear-mcp", ToolPrefix::Short), "linear");
        assert_eq!(get_server_prefix("mcp", ToolPrefix::Short), "mcp");
    }

    #[test]
    fn interpolate_env_vars_expands_both_forms() {
        let env = |name: &str| match name {
            "FOO" => Some("bar".to_string()),
            _ => None,
        };
        assert_eq!(interpolate_env_vars("a-${FOO}-b", &env), "a-bar-b");
        assert_eq!(interpolate_env_vars("a-$env:FOO-b", &env), "a-bar-b");
        assert_eq!(interpolate_env_vars("a-${MISSING}-b", &env), "a--b");
    }
}
