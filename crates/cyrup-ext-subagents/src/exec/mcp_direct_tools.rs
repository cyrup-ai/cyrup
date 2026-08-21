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
//!
//! # The metadata cache is a shared on-disk contract, and `cyrup-mcp` is its writer
//!
//! `<agent_dir>/mcp-cache.json` has exactly two participants: `cyrup_mcp::dirs` writes it, this
//! module reads it. Three things they must agree on byte-for-byte live in the writer:
//! `cyrup_mcp::dirs::server_identity_pre_image` (the 14-key `configHash` pre-image),
//! `cyrup_mcp::dirs::stable_stringify` (the token grammar that pre-image is rendered in) and
//! `cyrup_mcp::registration::{server_prefix, format_tool_name}` (the name a direct tool is actually
//! registered under). The writer was retargeted at upstream v2.26.1 and this reader was not, so the
//! two disagreed **silently**: every cached entry failed hash validation (so `mcp:` selectors
//! resolved to nothing) and every resource-backed tool resolved to a name the adapter never
//! registers. This module now mirrors all three:
//!
//! * MCP-141 — the identity pre-image is the same **14** keys, in the same *resolved* forms, as
//!   `server_identity_pre_image` (upstream `computeServerHash`, `metadata-cache.ts:82` @ v2.26.1).
//! * MCP-142 — `stable_stringify` renders an **absent** field as the bare nine-character token
//!   `undefined` (`metadata-cache.ts:344`), which is why `HashValue` exists instead of
//!   `serde_json::Value`.
//! * MCP-146 — a resource-backed tool is `read_<name>`, never `get_<name>`
//!   (`tool-metadata.ts:46` and `:119`).
//! * MCP-370 — `ToolPrefix` carries upstream's **four** modes and the server prefix goes through
//!   `sanitizeServerPrefix` (`types.ts:667`/`:675`), which **preserves hyphens**.
//!
//! ## What this module is NOT byte-identical to, stated exactly
//!
//! Three separate divergences survive this change. Two are filed and deliberately out of scope:
//! `interpolate_env_vars` is missing upstream's third `{env:NAME}` pattern (MCP-143), and the
//! `!`/`!!` secret grammar is not applied to hashed values (MCP-144).
//!
//! **The third is wider than the other two and is NOT a property of this file.** This reader
//! RESOLVES the six resolvable identity fields, exactly as upstream does — `env` and `headers`
//! through `interpolate_env_record`, `cwd` through `resolve_config_path` (which expands a leading
//! `~`), `url` through `resolve_server_url`, `bearerToken` through `resolve_bearer_token` (which
//! falls back to the *value* of `bearerTokenEnv`). The writer's two production call sites
//! (`cyrup_mcp::ui` :1758/:5050) still pass `ResolvedIdentity::verbatim`, which resolves NOTHING —
//! its own `TODO(MCP-082, MCP-084)` says so. So the two sides agree only for a definition whose
//! `env`/`headers`/`url`/`cwd`/`requestHeadersCommand` contain no `${VAR}` and no `$env:VAR`, whose
//! `cwd` has no leading `~`, and which does not rely on `bearerTokenEnv` to supply `bearerToken`.
//! For anything else the reader is upstream-correct and the WRITER is the lagging side; the fix
//! belongs in MCP-082/MCP-084, not here, and hashing raw here to match `verbatim` would be
//! deliberately un-upstreaming the one side that is currently right.
//!
//! **And neither side matches upstream on `socket`.** `computeServerHash` builds a **15**-key
//! identity whose third key is `socket: resolveConfigPath(definition.socket)`
//! (`metadata-cache.ts:89`), and upstream's `stableStringify` walks `Object.keys()`, so an absent
//! `socket` is still emitted as `"socket":undefined` rather than dropped. `ServerEntry` has no
//! `socket` field at all post-Cut-3, and both Rust pre-images omit the key entirely — a deliberate,
//! documented choice on the writer (`cyrup_mcp::dirs` golden vectors say "with `socket` unset"),
//! but one that contradicts 13c-mcp-servers.md:1753 ("Keep `socket` … in the pre-image despite Cut
//! 3") and puts every cyrup digest one key away from pi's. Measured, not inferred: see
//! `the_socket_key_is_the_one_divergence_from_upstreams_own_digest` below, which pins both digests.

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

/// The server-name → tool-name prefixing mode — upstream `types.ts:460`
/// `ToolPrefix = "server" | "none" | "short" | "mcp"`, and the twin of
/// `cyrup_mcp::config::ToolPrefix`.
///
/// **MCP-370.** This enum had three variants: `"mcp"` — upstream's fourth mode, which names a tool
/// `mcp__<server>_<tool>` — was folded into [`ToolPrefix::Server`] by `get_tool_prefix`'s catch-all
/// arm. A user who set `settings.toolPrefix: "mcp"` therefore got `<server>_<tool>` out of this
/// resolver while `cyrup_mcp::registration::server_prefix` registered `mcp__<server>_<tool>`, and
/// this file was the only `ToolPrefix` in the tree, so upstream's fourth mode had no representation
/// in cyrup at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ToolPrefix {
    Server,
    None,
    Short,
    Mcp,
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

/// The `requestHeadersCommand` block of a server definition (upstream `types.ts:383`, v2.26.0),
/// mirroring `cyrup_mcp::config::HttpRequestHeadersCommand`. Modeled here **only** because it is the
/// fourteenth key of the config-identity pre-image (MCP-141) — this resolver never runs it, and the
/// engine that does lives in `cyrup_mcp::request_headers_command`.
///
/// Every field is optional for the same reason the adapter's copy makes them optional: upstream's
/// `command: string` is a TypeScript type, not a check, and the non-empty-command diagnostic is
/// raised at connect time. `timeoutMs` is `f64` because it arrived from `JSON.parse` and must render
/// through the JS number grammar (`2500`, never `2500.0`).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RequestHeadersCommand {
    /// The executable, `interpolateEnvVars`'d into the pre-image.
    #[serde(default)]
    pub command: Option<String>,
    /// Arguments, each `interpolateEnvVars`'d into the pre-image.
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// Environment overrides. Raw JSON like [`ServerEntry::env`], because `computeServerHash` runs
    /// it through the same `interpolateEnvRecord` (`metadata-cache.ts:94-101`).
    #[serde(default)]
    pub env: Option<BTreeMap<String, Value>>,
    /// Per-invocation timeout in milliseconds.
    #[serde(default, rename = "timeoutMs")]
    pub timeout_ms: Option<f64>,
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
    /// Per-request header-signing command (upstream v2.26.0). Hashed, never executed here — see
    /// [`RequestHeadersCommand`]. The fourteenth identity key (MCP-141).
    #[serde(default, rename = "requestHeadersCommand")]
    pub request_headers_command: Option<RequestHeadersCommand>,
    /// `"oauth" | "bearer" | false` in pi — held raw and folded verbatim into the identity hash.
    #[serde(default)]
    pub auth: Option<Value>,
    /// `"legacy" | "auto" | "2026-07-28"` in pi — held raw and folded verbatim into the identity
    /// hash, exactly as [`Self::auth`] is. Part of the config identity (MCP-141) because pinning a
    /// protocol revision changes what the server advertises; not otherwise used by this resolver.
    #[serde(default, rename = "protocolVersion")]
    pub protocol_version: Option<Value>,
    #[serde(default, rename = "bearerToken")]
    pub bearer_token: Option<String>,
    #[serde(default, rename = "bearerTokenEnv")]
    pub bearer_token_env: Option<String>,
    #[serde(default, rename = "exposeResources")]
    pub expose_resources: Option<bool>,
    /// The glob-or-exact allowlist upstream applies **before** [`Self::exclude_tools`]. Part of the
    /// config identity (MCP-141): editing it changes which tools the adapter registers, so it must
    /// evict the cache. It is deliberately **not** applied by this resolver's own filtering yet —
    /// that, and upstream's 18-expression `getToolNameCandidates` with its glob matching, are the
    /// remaining half of MCP-370 (`isToolAllowed`, `types.ts`).
    #[serde(default, rename = "includeTools")]
    pub include_tools: Option<Vec<String>>,
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
            // `read_`, not `get_` (MCP-146). Upstream builds
            // `` `read_${resourceNameToToolName(resource.name)}` `` at BOTH resource sites of
            // `tool-metadata.ts` (`:46` in the collision-candidate scan, `:119` in the emission
            // loop), and `cyrup_mcp::registration` registers exactly that name. A `get_`-prefixed
            // name matches no registered tool and no `excludeTools` entry a user would ever write —
            // and supporting both is not an option, because the child's `--tools` allowlist is an
            // exact string match and two names would then denote one tool.
            let base_name = format!("read_{}", resource_name_to_tool_name(name));
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

/// The config-identity **pre-image** for a server definition — the exact byte sequence
/// [`compute_mcp_server_hash`] digests, and the shared contract with the cache writer's
/// `cyrup_mcp::dirs::server_identity_pre_image` (upstream `computeServerHash`,
/// `metadata-cache.ts:82` @ v2.26.1).
///
/// # Fourteen keys, and the list is the specification (MCP-141)
///
/// This function built **eleven**: `protocolVersion`, `includeTools` and `requestHeadersCommand`
/// were missing, so editing any of them left a stale tool list looking valid — and, far worse, the
/// eleven-key pre-image is one the writer never produces, so no `cyrup-mcp`-written cache entry
/// could validate here at all.
///
/// Upstream builds **fifteen**: its first key is `socket: resolveConfigPath(definition.socket)`.
/// `socket` is Cut 3 in this tree — `cyrup_mcp::config::ServerEntry` has no such field, the raw
/// framed unix-socket transport does not exist here, and a config that could carry it cannot be
/// written — so both sides omit it. That is a recorded divergence from pi's digests, not an
/// oversight. (13c's MCP-141 text says to keep `socket` in the pre-image "despite Cut 3"; the
/// writer that actually shipped does not, and a reader that disagreed with the in-tree writer in
/// order to agree with a field neither crate can express would break the only contract that exists
/// here. The consequence is that a cache written by pi itself is not hash-compatible with cyrup's,
/// which costs one cold start.)
///
/// # Five of the fourteen are hashed in their *resolved* form
///
/// `env` and `headers` (`interpolateEnvRecord`), `cwd` (`resolveConfigPath`), `url`
/// (`resolveServerUrl` — MCP-141's fourth gap: this took `definition.url` **raw**) and `bearerToken`
/// (`resolveBearerToken`). The digest has to change when `$API_HOST` changes even though the config
/// text did not; that is the entire point of hashing the resolved form.
///
/// Returned separately from the digest for the reason the writer returns it separately: a hash
/// mismatch tells you nothing about *which* field disagreed, and the conformance tests below assert
/// the bytes.
#[must_use]
pub fn server_identity_pre_image(definition: &ServerEntry) -> String {
    let home = home_dir();
    let env = |name: &str| std::env::var(name).ok();
    let identity = HashValue::Object(vec![
        ("command".to_string(), opt_string(definition.command.as_deref())),
        ("args".to_string(), opt_string_list(definition.args.as_ref())),
        ("env".to_string(), interpolate_env_record(definition.env.as_ref(), &env)),
        (
            "cwd".to_string(),
            opt_string(resolve_config_path(definition.cwd.as_deref(), &home, &env).as_deref()),
        ),
        (
            "url".to_string(),
            opt_string(resolve_server_url(definition.url.as_deref(), &env).as_deref()),
        ),
        ("headers".to_string(), interpolate_env_record(definition.headers.as_ref(), &env)),
        (
            "requestHeadersCommand".to_string(),
            request_headers_command_value(definition.request_headers_command.as_ref(), &env),
        ),
        ("auth".to_string(), HashValue::from_optional_json(definition.auth.clone())),
        (
            "protocolVersion".to_string(),
            HashValue::from_optional_json(definition.protocol_version.clone()),
        ),
        (
            "bearerToken".to_string(),
            opt_string(resolve_bearer_token(definition, &env).as_deref()),
        ),
        ("bearerTokenEnv".to_string(), opt_string(definition.bearer_token_env.as_deref())),
        (
            "exposeResources".to_string(),
            definition.expose_resources.map_or(HashValue::Undefined, HashValue::Bool),
        ),
        ("includeTools".to_string(), opt_string_list(definition.include_tools.as_ref())),
        ("excludeTools".to_string(), opt_string_list(definition.exclude_tools.as_ref())),
    ]);
    stable_stringify(&identity)
}

/// Compute the stable config-identity hash for a server definition (pi `computeMcpServerHash`, and
/// the twin of `cyrup_mcp::dirs::compute_server_hash`): SHA-256 over
/// [`server_identity_pre_image`], hex-encoded. Exported so a cache writer (and this crate's tests)
/// stamp the SAME `configHash` the resolver validates.
#[must_use]
pub fn compute_mcp_server_hash(definition: &ServerEntry) -> String {
    let mut hasher = Sha256::new();
    hasher.update(server_identity_pre_image(definition).as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// `undefined` for an absent field, a JSON string otherwise (`cyrup_mcp::dirs::opt_string`).
fn opt_string(value: Option<&str>) -> HashValue {
    value.map_or(HashValue::Undefined, |s| HashValue::String(s.to_string()))
}

/// `undefined` for absent, an array of JSON strings otherwise. An **empty** array is not absent:
/// `excludeTools: []` hashes differently from an omitted `excludeTools`.
fn opt_string_list(value: Option<&Vec<String>>) -> HashValue {
    value.map_or(HashValue::Undefined, |items| {
        HashValue::Array(items.iter().map(|s| HashValue::String(s.clone())).collect())
    })
}

/// The fourteenth identity key (`metadata-cache.ts:94-101`): `undefined` when the field is absent,
/// otherwise the four-key nested object with `command`, every `args` element and every `env` value
/// interpolated and `timeoutMs` copied through. Interpolated for the same reason `headers` is — the
/// digest must change when `$MCP_SIGNER_ACTOR` changes even though the config text did not, which is
/// what upstream's `"hashes the effective per-request header command"` test asserts
/// (`__tests__/direct-tools.test.ts:357-379`).
///
/// Note the nested object is emitted **whole** whenever the field is present: each absent member
/// renders as `undefined` inside it, and [`stable_stringify`] sorts its four keys just as it sorts
/// the outer ones.
fn request_headers_command_value(
    value: Option<&RequestHeadersCommand>,
    env: &dyn Fn(&str) -> Option<String>,
) -> HashValue {
    let Some(command) = value else {
        return HashValue::Undefined;
    };
    HashValue::Object(vec![
        (
            "command".to_string(),
            command
                .command
                .as_deref()
                .map_or(HashValue::Undefined, |c| HashValue::String(interpolate_env_vars(c, env))),
        ),
        (
            "args".to_string(),
            command.args.as_ref().map_or(HashValue::Undefined, |args| {
                HashValue::Array(
                    args.iter().map(|a| HashValue::String(interpolate_env_vars(a, env))).collect(),
                )
            }),
        ),
        ("env".to_string(), interpolate_env_record(command.env.as_ref(), env)),
        (
            "timeoutMs".to_string(),
            command.timeout_ms.map_or(HashValue::Undefined, HashValue::Number),
        ),
    ])
}

/// The value half of `resolveServerUrl` (`utils.ts:167`): an absent `url` stays absent, otherwise
/// the URL is interpolated and the **resolved** string is what the digest covers (MCP-141, which
/// this took raw).
///
/// Upstream also *throws* here — on a missing environment variable, or a URL that no longer parses
/// after interpolation — and `isServerCacheValid` catches that throw into `false`. That arm is
/// MCP-145 and is deliberately not ported: this function has no error channel, and it needs none to
/// be safe, because a definition whose hash the *writer* cannot compute never gets a cache entry
/// written for it, so the worst this reader can do is compute a digest that matches nothing and skip
/// the server — which is the same outcome the throw produces.
fn resolve_server_url(url: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    Some(interpolate_env_vars(url?, env))
}

/// `settings.toolPrefix` → the prefixing mode. Upstream's `validateConfig` never validates this key
/// (it is a bare cast), so an unrecognised value falls through to the `"server"` default exactly as
/// `getServerPrefix`'s final `return` does — but `"mcp"` is a real mode (`types.ts:460`), and
/// folding it into [`ToolPrefix::Server`] renamed every tool of a `toolPrefix: "mcp"` config
/// (MCP-370).
fn get_tool_prefix(value: Option<&str>) -> ToolPrefix {
    match value {
        Some("none") => ToolPrefix::None,
        Some("short") => ToolPrefix::Short,
        Some("mcp") => ToolPrefix::Mcp,
        _ => ToolPrefix::Server,
    }
}

/// Port of `sanitizeServerPrefix` (`types.ts:667`) at its default `preserveProviderValid = true` —
/// the twin of `cyrup_mcp::registration::sanitize_server_prefix(name, true)`. `[A-Za-z0-9_-]`
/// survives verbatim; every other code point becomes `_<lowercase hex code point>_`. Iteration is by
/// `char` because upstream's is by `Array.from`, i.e. by code point.
///
/// **MCP-370.** This module folded `-` into `_` instead, so `chrome-devtools` produced the prefix
/// `chrome_devtools` while `cyrup_mcp::registration` registers `chrome-devtools`: every `mcp:`
/// selector naming a hyphenated server (which is most of them) expanded to names the child's tool
/// registry never had, and the subagent silently started with no MCP tools at all. Folding also
/// **collides** two distinct servers (`a-b` and `a_b`) onto one prefix, which the escape form does
/// not.
///
/// The legacy grammar (`preserve_provider_valid = false`, `github_2d_mcp`) is not ported: upstream
/// uses it only to build the alias candidates of `getToolNameCandidates`, which this module does not
/// implement — see [`is_tool_excluded`].
fn sanitize_server_prefix(server_name: &str) -> String {
    let mut out = String::with_capacity(server_name.len());
    for ch in server_name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
            out.push_str(&format!("{:x}", ch as u32));
            out.push('_');
        }
    }
    out
}

/// Port of `getServerPrefix` (`types.ts:675`), and the twin of
/// `cyrup_mcp::registration::server_prefix`.
fn get_server_prefix(server_name: &str, mode: ToolPrefix) -> String {
    match mode {
        ToolPrefix::None => String::new(),
        ToolPrefix::Short => {
            let short = sanitize_server_prefix(strip_mcp_suffix(server_name));
            if short.is_empty() {
                "mcp".to_string()
            } else {
                short
            }
        }
        // `mcp__<server>_<tool>` — one underscore between server and tool. The double-underscore
        // form belongs to prompt slash commands (`formatPromptCommandName`), not to tool names.
        ToolPrefix::Mcp => format!("mcp__{}", sanitize_server_prefix(server_name)),
        ToolPrefix::Server => sanitize_server_prefix(server_name),
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

/// Port of `formatToolName` (`types.ts:692`), and the twin of
/// `cyrup_mcp::registration::format_tool_name`: the server prefix, an underscore, and the tool name
/// with **dots** replaced by underscores.
///
/// Dots only — a hyphen inside the *tool* name survives, which is exactly why upstream's legacy
/// candidate set exists. This module dropped the `.` → `_` substitution along with the server-prefix
/// grammar (MCP-370): a server-side tool named `fs.read` resolved to `<server>_fs.read` here while
/// the adapter registers `<server>_fs_read`.
fn format_tool_name(tool_name: &str, server_name: &str, prefix: ToolPrefix) -> String {
    let server_prefix = get_server_prefix(server_name, prefix);
    let sanitized = tool_name.replace('.', "_");
    if server_prefix.is_empty() {
        sanitized
    } else {
        format!("{server_prefix}_{sanitized}")
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
    // `getToolNameCandidates` (`types.ts:775`) builds the bare name plus one formatted name per
    // mode; [`ToolPrefix::Mcp`] joins the list here because MCP-370 gave this module upstream's
    // fourth mode. Upstream's other 13 expressions — the legacy `-`-folding grammar and its
    // aliases — stay unported, as does `isToolAllowed`'s glob matching and `includeTools`; that is
    // the remaining half of MCP-370 and is not widened here.
    let candidates: HashSet<String> = [
        normalize_tool_name(tool_name),
        normalize_tool_name(&format_tool_name(tool_name, server_name, prefix)),
        normalize_tool_name(&format_tool_name(tool_name, server_name, ToolPrefix::Server)),
        normalize_tool_name(&format_tool_name(tool_name, server_name, ToolPrefix::Short)),
        normalize_tool_name(&format_tool_name(tool_name, server_name, ToolPrefix::Mcp)),
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
/// references in each (pi `interpolateEnvRecord`, `utils.ts:107`). An **absent** map is `undefined`
/// (MCP-142), never `null`; a present map is an object, even when every value was dropped.
///
/// [`BTreeMap`] iterates in key order, which is the order [`stable_stringify`] would impose anyway.
fn interpolate_env_record(
    values: Option<&BTreeMap<String, Value>>,
    env: &dyn Fn(&str) -> Option<String>,
) -> HashValue {
    let Some(values) = values else {
        return HashValue::Undefined;
    };
    HashValue::Object(
        values
            .iter()
            .filter_map(|(key, value)| {
                value
                    .as_str()
                    .map(|text| (key.clone(), HashValue::String(interpolate_env_vars(text, env))))
            })
            .collect(),
    )
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

/// One node of the config-identity pre-image (`metadata-cache.ts:344`'s input), mirroring
/// `cyrup_mcp::dirs::HashValue` variant for variant.
///
/// This is deliberately **not** `serde_json::Value` and must not become it (MCP-142): JS
/// distinguishes an **absent** property from one explicitly set to `null`, `stableStringify` renders
/// the first as the bare token `undefined` and the second as `null`, and the digest differs.
/// Collapsing absence onto `Value::Null` — which is what this module's `opt_str_value`, its
/// `interpolate_env_record` absent arm and its four `.unwrap_or(Value::Null)` arms did — made every
/// absent key of every server hash as `null`, so nothing the writer stamps could ever match.
#[derive(Clone, Debug, PartialEq)]
enum HashValue {
    /// JS `undefined` — an absent property. Renders as the nine characters `undefined`.
    Undefined,
    /// JS `null` — a property explicitly set to null. Renders as `null`.
    ///
    /// Unreachable from a top-level identity field, and that is not an accident to be fixed here:
    /// `Option<Value>` maps a JSON `null` onto `None`, so `"auth": null` arrives as
    /// [`HashValue::Undefined`] — exactly as it does in `cyrup-mcp`, whose `lenient` reader cannot
    /// preserve explicit-null-vs-absent either. It *is* reachable inside a raw `auth` /
    /// `protocolVersion` value (`"auth": [null]`), and it keeps the token grammar total.
    Null,
    /// A boolean.
    Bool(bool),
    /// A number. `f64` is the faithful model: every number in the pre-image arrived through
    /// `JSON.parse`, which produces IEEE-754 doubles, and Rust's `f64` `Display` is the same
    /// shortest-round-trip form `String(n)` produces — so `timeoutMs: 2500` renders `2500`, where
    /// routing it through `serde_json::Number` would have produced `2500.0`.
    Number(f64),
    /// A string, rendered through `JSON.stringify`'s escaping.
    String(String),
    /// An array. Elements keep their order and an `Undefined` element stays `undefined` — JS
    /// `JSON.stringify` would have written `null` there, which is one more reason this cannot be
    /// routed through a JSON serialiser.
    Array(Vec<HashValue>),
    /// An object. Keys are sorted at render time, never here.
    Object(Vec<(String, HashValue)>),
}

impl HashValue {
    /// Lift a `serde_json::Value` — every variant maps across, and `Value::Null` becomes
    /// [`HashValue::Null`], **not** [`HashValue::Undefined`].
    fn from_json(value: Value) -> HashValue {
        match value {
            Value::Null => HashValue::Null,
            Value::Bool(b) => HashValue::Bool(b),
            Value::Number(n) => n.as_f64().map_or(HashValue::Null, HashValue::Number),
            Value::String(s) => HashValue::String(s),
            Value::Array(items) => {
                HashValue::Array(items.into_iter().map(HashValue::from_json).collect())
            }
            Value::Object(map) => HashValue::Object(
                map.into_iter().map(|(k, v)| (k, HashValue::from_json(v))).collect(),
            ),
        }
    }

    /// `undefined` for `None`, [`from_json`](Self::from_json) for `Some` — the shape every optional
    /// identity field takes.
    fn from_optional_json(value: Option<Value>) -> HashValue {
        value.map_or(HashValue::Undefined, HashValue::from_json)
    }
}

/// Stable, key-sorted stringification (pi `stableStringify`, `metadata-cache.ts:344`) — the
/// pre-image the config hash is taken over, and the byte-for-byte twin of
/// `cyrup_mcp::dirs::stable_stringify`.
///
/// Deliberately **not valid JSON**. Upstream's scalar branch is
/// `const serialized = JSON.stringify(value); return serialized === undefined ? "undefined" : serialized;`
/// and `JSON.stringify(undefined)` returns the JS value `undefined`, so an absent property renders
/// as the literal nine-character token `undefined` (MCP-142) and only an explicit JSON null renders
/// as `null`. Object keys are sorted (`Object.keys(obj).sort()`); arrays keep their order. Strings
/// go through `serde_json`, whose escaping matches `JSON.stringify` byte for byte for every `str`.
///
/// The key sort is byte-wise UTF-8 where JS sorts by UTF-16 code unit. The two agree on every ASCII
/// key — which is every key of the identity object — and can differ only between a `U+E000..U+FFFF`
/// key and an astral-plane one inside a user's `env` or `headers` map.
fn stable_stringify(value: &HashValue) -> String {
    match value {
        HashValue::Undefined => "undefined".to_string(),
        HashValue::Null => "null".to_string(),
        HashValue::Bool(true) => "true".to_string(),
        HashValue::Bool(false) => "false".to_string(),
        HashValue::Number(n) => {
            // `JSON.stringify(NaN) === "null"`, and likewise for ±Infinity.
            if n.is_finite() {
                format!("{n}")
            } else {
                "null".to_string()
            }
        }
        HashValue::String(s) => json_quote(s),
        HashValue::Array(items) => {
            let parts: Vec<String> = items.iter().map(stable_stringify).collect();
            format!("[{}]", parts.join(","))
        }
        HashValue::Object(entries) => {
            let mut sorted: Vec<&(String, HashValue)> = entries.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            let parts: Vec<String> = sorted
                .into_iter()
                .map(|(key, val)| format!("{}:{}", json_quote(key), stable_stringify(val)))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

/// `JSON.stringify(s)` for a string. Serialising a `&str` cannot fail; the fallback exists because
/// this crate denies `unwrap`, and an empty string is the least surprising thing to hash if the
/// impossible happens.
fn json_quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
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
        // `chrome-devtools_…`, NOT `chrome_devtools_…` (MCP-370). The old expectation encoded this
        // module's `server_name.replace('-', "_")`, but `getServerPrefix` → `sanitizeServerPrefix`
        // (`types.ts:667`/`:675`) keeps `-`, and `cyrup_mcp::registration::format_tool_name`
        // registers the hyphenated name — so the name this test used to assert is one no tool
        // registry in the tree ever holds.
        assert_eq!(
            resolve(&fixture, &["chrome-devtools"]),
            vec![
                "chrome-devtools_take_screenshot".to_string(),
                "chrome-devtools_click".to_string()
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
        //
        // Two changes (MCP-370): `server` mode keeps the hyphen (the old `linear_mcp_list_issues`
        // was this module's `-` → `_` folding, which the adapter does not do), and `mcp` — upstream's
        // fourth mode, `types.ts:460` — is asserted at all for the first time. It used to fall
        // through `get_tool_prefix`'s catch-all into `server`, so a `toolPrefix: "mcp"` config
        // resolved to `linear-mcp_list_issues` while the adapter registered
        // `mcp__linear-mcp_list_issues`.
        for (prefix, expected) in [
            ("server", "linear-mcp_list_issues"),
            ("short", "linear_list_issues"),
            ("none", "list_issues"),
            ("mcp", "mcp__linear-mcp_list_issues"),
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
        // Both names changed, for two independent reasons:
        //   * `browser-mcp_…` rather than `browser_mcp_…` — MCP-370, as above.
        //   * `read_console_logs` rather than `get_console_logs` — MCP-146. `get_` appears **zero**
        //     times in upstream's resource-tool naming; `tool-metadata.ts:46` and `:119` both build
        //     `` `read_${resourceNameToToolName(resource.name)}` ``, and that is the name
        //     `cyrup_mcp::registration` registers and that a user's `excludeTools` entry must match.
        //     The old expectation asserted a name nothing in the tree produces or accepts.
        // `excludeTools: ["browser_click"]` still suppresses `click`: it matches the `short`-mode
        // candidate `browser_click` under `normalize_tool_name`.
        assert_eq!(
            resolve(&fixture, &["browser-mcp"]),
            vec![
                "browser-mcp_navigate".to_string(),
                "browser-mcp_read_console_logs".to_string()
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
        // `project-mcp_inspect` — MCP-370's hyphen rule again; the config-source resolution this
        // test actually covers is unchanged.
        assert_eq!(
            resolve(&fixture, &["project-mcp"]),
            vec!["project-mcp_inspect".to_string()]
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

    // ---------------------------------------------------------------------------------------
    // Conformance with the cache WRITER — MCP-141/142/146/370.
    //
    // `cyrup-mcp` is a DEV-dependency (see this crate's `Cargo.toml`): the two crates share an
    // on-disk file, not a type, so only a test can hold them together. These assert against the
    // writer's own functions rather than against constants copied out of its tests, except where a
    // constant is the upstream-generated golden vector — that one is pinned on both sides so a
    // change that "fixes" both implementations at once still fails.
    // ---------------------------------------------------------------------------------------

    use cyrup_mcp::config::{ServerEntry as WriterServerEntry, ToolPrefix as WriterToolPrefix};
    use cyrup_mcp::dirs::{
        ResolvedIdentity, compute_server_hash, server_identity_pre_image as writer_pre_image,
    };

    /// All fourteen identity fields set at once, hashed by BOTH implementations (MCP-141/142).
    ///
    /// Fails on the pre-committed code twice over: this module emitted **eleven** keys (no
    /// `protocolVersion`, no `includeTools`, no `requestHeadersCommand`) and rendered every absent
    /// field as `null` where the writer renders `undefined`. In production that meant
    /// `is_server_cache_valid` rejected every entry `cyrup-mcp` writes, so every `mcp:` selector a
    /// subagent declared resolved to nothing — with no error anywhere.
    ///
    /// The fixture carries no interpolation token and no `!` secret marker on purpose:
    /// `ResolvedIdentity::verbatim` is the writer's placeholder until its own TODO(MCP-082,
    /// MCP-084) lands the real resolvers, while this side already resolves. Where both resolve to
    /// the input they agree today; MCP-143 (`{env:NAME}`) and MCP-144 (`!`/`!!`) are what remain
    /// before they agree for every input.
    #[test]
    fn reader_and_writer_agree_on_the_fourteen_field_pre_image() {
        const DEFINITION: &str = r#"{
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
            "env": { "NODE_ENV": "production", "API_TOKEN": "s3cret" },
            "cwd": "/home/u/work",
            "url": "https://api.example.com/mcp",
            "headers": { "X-Api-Key": "k", "Accept": "application/json" },
            "requestHeadersCommand": {
                "command": "node",
                "args": ["sign.mjs", "actor-one"],
                "env": { "ACTOR": "actor-one" },
                "timeoutMs": 2500
            },
            "auth": "bearer",
            "protocolVersion": "2026-07-28",
            "bearerToken": "tok",
            "bearerTokenEnv": "TOK_ENV",
            "exposeResources": false,
            "includeTools": ["a", "b"],
            "excludeTools": ["danger_*"],
            "lifecycle": "eager",
            "requestTimeoutMs": 30000,
            "debug": true
        }"#;
        let reader: ServerEntry = serde_json::from_str(DEFINITION).expect("reader entry");
        let writer: WriterServerEntry = serde_json::from_str(DEFINITION).expect("writer entry");
        let resolved = ResolvedIdentity::verbatim(&writer);

        let pre_image = server_identity_pre_image(&reader);
        assert_eq!(pre_image, writer_pre_image(&writer, &resolved));
        assert_eq!(compute_mcp_server_hash(&reader), compute_server_hash(&writer, &resolved));

        // Fourteen keys, named — a count alone would not say which one went missing.
        for key in [
            "args",
            "auth",
            "bearerToken",
            "bearerTokenEnv",
            "command",
            "cwd",
            "env",
            "excludeTools",
            "exposeResources",
            "headers",
            "includeTools",
            "protocolVersion",
            "requestHeadersCommand",
            "url",
        ] {
            assert!(pre_image.contains(&format!("\"{key}\":")), "missing {key} in {pre_image}");
        }
        // `timeoutMs` renders as a JS number: `2500`, never `2500.0`.
        assert!(pre_image.contains(r#""timeoutMs":2500}"#), "{pre_image}");
        // The three runtime-only keys are outside the identity, so editing them must NOT evict.
        assert!(!pre_image.contains("lifecycle"), "{pre_image}");
        assert!(!pre_image.contains("debug"), "{pre_image}");
    }

    /// The `socket` divergence, measured against upstream rather than argued about.
    ///
    /// `computeServerHash` (`metadata-cache.ts:86-109` @ v2.26.1) builds a **15**-key identity whose
    /// third key is `socket: resolveConfigPath(definition.socket)`, and `stableStringify` walks
    /// `Object.keys()` — which includes keys holding `undefined` — so upstream emits
    /// `"socket":undefined` even for a definition that never mentions a socket. `ServerEntry` has no
    /// `socket` field post-Cut-3 and neither Rust pre-image emits the key, so **every** cyrup digest
    /// differs from pi's by exactly that one member.
    ///
    /// Both constants below were produced by running upstream's own `stableStringify` +
    /// `computeServerHash` on node 22 against `tmp/pi-mcp-adapter` at tag `v2.26.1` (`fafae21`),
    /// using the same fixture as the vector above. The test exists so the divergence is a pinned,
    /// visible fact rather than a comment: if someone lands `socket` (13c-mcp-servers.md:1753 asks
    /// for it), this test fails and tells them the two digests just converged.
    #[test]
    fn the_socket_key_is_the_one_divergence_from_upstreams_own_digest() {
        let entry: ServerEntry = serde_json::from_str(
            r#"{
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
                "env": { "NODE_ENV": "production", "API_TOKEN": "s3cret" },
                "cwd": "/home/u/work",
                "exposeResources": false,
                "excludeTools": ["danger_*"],
                "lifecycle": "eager",
                "requestTimeoutMs": 30000,
                "debug": true
            }"#,
        )
        .expect("entry");

        // What upstream actually emits for this fixture, verbatim from node 22.
        const UPSTREAM_PRE_IMAGE: &str = concat!(
            r#"{"args":["-y","@modelcontextprotocol/server-filesystem","/tmp"],"#,
            r#""auth":undefined,"bearerToken":undefined,"bearerTokenEnv":undefined,"#,
            r#""command":"npx","cwd":"/home/u/work","#,
            r#""env":{"API_TOKEN":"s3cret","NODE_ENV":"production"},"#,
            r#""excludeTools":["danger_*"],"exposeResources":false,"headers":undefined,"#,
            r#""includeTools":undefined,"protocolVersion":undefined,"#,
            r#""requestHeadersCommand":undefined,"socket":undefined,"url":undefined}"#
        );
        const UPSTREAM_DIGEST: &str =
            "2190558e470a75c0f992989bd1799b374e669deecb8093e4118a1a9419068cf4";

        let ours = server_identity_pre_image(&entry);

        // The difference is exactly one member, and it is `socket`.
        assert_ne!(ours, UPSTREAM_PRE_IMAGE, "if these are equal, `socket` landed");
        assert!(!ours.contains("\"socket\""), "we omit the key entirely: {ours}");
        assert!(UPSTREAM_PRE_IMAGE.contains(r#""socket":undefined"#));
        assert_eq!(
            UPSTREAM_PRE_IMAGE.replace(r#""socket":undefined,"#, ""),
            ours,
            "removing upstream's `socket` member must yield our pre-image exactly — any other \
             difference is a second, unrecorded divergence"
        );
        assert_ne!(compute_mcp_server_hash(&entry), UPSTREAM_DIGEST);
    }

    /// The cross-crate golden vector, asserted here and in
    /// `cyrup_mcp::dirs::tests::golden_vector_stdio_server`.
    ///
    /// **Read the caveat before trusting the word "golden".** These constants are upstream's
    /// `stableStringify` + `computeServerHash` run on node 22 **with the `socket` key removed from
    /// the identity** — the deviation the writer's own vectors document and this one inherits. They
    /// are therefore byte-compatible with *cyrup*, not with pi: upstream's real digest for this very
    /// fixture is `2190558e…`, pinned in
    /// `the_socket_key_is_the_one_divergence_from_upstreams_own_digest`. What this test proves is
    /// that the two Rust implementations agree on *what* they hash; it does not prove either matches
    /// upstream.
    ///
    /// The cross-crate test above proves the two implementations agree; this proves *what* they
    /// agree on. Note what a plain stdio server's pre-image is mostly made of: nine `undefined`
    /// tokens. That is the whole of MCP-142 in one string.
    #[test]
    fn pre_image_matches_the_upstream_generated_golden_vector() {
        let entry: ServerEntry = serde_json::from_str(
            r#"{
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
                "env": { "NODE_ENV": "production", "API_TOKEN": "s3cret" },
                "cwd": "/home/u/work",
                "exposeResources": false,
                "excludeTools": ["danger_*"],
                "lifecycle": "eager",
                "requestTimeoutMs": 30000,
                "debug": true
            }"#,
        )
        .expect("entry");
        assert_eq!(
            server_identity_pre_image(&entry),
            concat!(
                r#"{"args":["-y","@modelcontextprotocol/server-filesystem","/tmp"],"#,
                r#""auth":undefined,"bearerToken":undefined,"bearerTokenEnv":undefined,"#,
                r#""command":"npx","cwd":"/home/u/work","#,
                r#""env":{"API_TOKEN":"s3cret","NODE_ENV":"production"},"#,
                r#""excludeTools":["danger_*"],"exposeResources":false,"headers":undefined,"#,
                r#""includeTools":undefined,"protocolVersion":undefined,"#,
                r#""requestHeadersCommand":undefined,"url":undefined}"#
            )
        );
        assert_eq!(
            compute_mcp_server_hash(&entry),
            "4dd46c1fd26680867fe6c5ffdde2ab0f0a35972cd9c211bf6dd68d1f304eb277"
        );

        // The HTTP half, which pins the two enum-valued keys and a non-empty `includeTools` next to
        // an EMPTY `excludeTools` — `[]` is not absent, and hashing it as absent would let an edit
        // that empties the list go unnoticed.
        let http: ServerEntry = serde_json::from_str(
            r#"{
                "url": "https://api.example.com/mcp",
                "headers": { "X-Api-Key": "k", "Accept": "application/json" },
                "auth": "bearer",
                "protocolVersion": "2026-07-28",
                "bearerToken": "!tok",
                "bearerTokenEnv": "TOK_ENV",
                "exposeResources": true,
                "includeTools": ["a", "b"],
                "excludeTools": []
            }"#,
        )
        .expect("http entry");
        assert_eq!(
            compute_mcp_server_hash(&http),
            "572dcbaa24a3a82d42f90a86ebb8039227f999a1b14078b35dfa9f40cb872356"
        );
    }

    /// MCP-142 pinned at the token level and at the digest level.
    ///
    /// `stable_stringify` mapped `Value::Null => "null"` and had no way to say `undefined` at all,
    /// so the empty definition — fourteen absent fields — hashed as fourteen `null`s. This is the
    /// case where the divergence is total: every single key disagreed with the writer.
    #[test]
    fn an_absent_field_is_undefined_and_an_explicit_null_is_null() {
        assert_eq!(
            stable_stringify(&HashValue::Object(vec![
                ("b".to_string(), HashValue::Undefined),
                ("a".to_string(), HashValue::Null),
            ])),
            "{\"a\":null,\"b\":undefined}"
        );
        // An `undefined` ELEMENT survives in an array, where `JSON.stringify` would write `null`.
        assert_eq!(
            stable_stringify(&HashValue::Array(vec![
                HashValue::Number(1.0),
                HashValue::Undefined,
                HashValue::Null,
            ])),
            "[1,undefined,null]"
        );

        let empty = ServerEntry::default();
        let pre_image = server_identity_pre_image(&empty);
        assert!(!pre_image.contains("null"), "{pre_image}");
        assert_eq!(pre_image.matches("undefined").count(), 14, "{pre_image}");
        assert_eq!(
            compute_mcp_server_hash(&empty),
            "671c1578e5f4c763aac0deb77dc2a99f55688f80ed687a652e5259319937794e"
        );
        let writer_empty = WriterServerEntry::default();
        assert_eq!(
            compute_mcp_server_hash(&empty),
            compute_server_hash(&writer_empty, &ResolvedIdentity::verbatim(&writer_empty))
        );
    }

    /// The three keys MCP-141 added: editing any one of them must change the digest, or a config
    /// edit leaves a stale tool list looking valid. Each of these three assertions failed before.
    #[test]
    fn the_three_added_identity_keys_evict_the_cache() {
        let base: ServerEntry = serde_json::from_str(r#"{ "command": "x" }"#).expect("base");
        for changed_json in [
            r#"{ "command": "x", "protocolVersion": "auto" }"#,
            r#"{ "command": "x", "includeTools": ["a"] }"#,
            r#"{ "command": "x", "requestHeadersCommand": { "command": "sign" } }"#,
        ] {
            let changed: ServerEntry = serde_json::from_str(changed_json).expect("changed");
            assert_ne!(
                compute_mcp_server_hash(&base),
                compute_mcp_server_hash(&changed),
                "{changed_json}"
            );
            let writer_base: WriterServerEntry =
                serde_json::from_str(r#"{ "command": "x" }"#).expect("writer base");
            let writer_changed: WriterServerEntry =
                serde_json::from_str(changed_json).expect("writer changed");
            assert_eq!(
                compute_mcp_server_hash(&changed),
                compute_server_hash(&writer_changed, &ResolvedIdentity::verbatim(&writer_changed)),
                "{changed_json}"
            );
            assert_eq!(
                compute_mcp_server_hash(&base),
                compute_server_hash(&writer_base, &ResolvedIdentity::verbatim(&writer_base))
            );
        }
    }

    /// `url` is hashed **resolved**, not as written (MCP-141's fourth gap) — the property that makes
    /// the cache track an `$API_HOST` change. `CYRUP_HOME`/`HOME` is always set in this process, so
    /// it is the one variable a hermetic test can rely on.
    #[test]
    fn the_url_is_hashed_after_interpolation() {
        let literal: ServerEntry =
            serde_json::from_str(r#"{ "url": "https://x.example/${HOME}/mcp" }"#).expect("literal");
        let pre_image = server_identity_pre_image(&literal);
        assert!(!pre_image.contains("${HOME}"), "{pre_image}");
        let home = std::env::var("HOME").unwrap_or_default();
        assert!(pre_image.contains(&format!("https://x.example/{home}/mcp")), "{pre_image}");
    }

    /// MCP-370's shared table: the same `(server, tool, mode)` triples through this module's
    /// formatter and through `cyrup_mcp::registration`'s — the one that actually names the
    /// registered tool. Hyphens, a non-ASCII code point, a dotted tool name, the `-mcp` suffix, and
    /// all four modes.
    ///
    /// Before this change three of the four modes produced the wrong name for any hyphenated server
    /// and the fourth mode did not exist, so this table could not even be written.
    #[test]
    fn reader_and_writer_agree_on_every_prefix_mode() {
        let modes = [
            (ToolPrefix::Server, WriterToolPrefix::Server),
            (ToolPrefix::None, WriterToolPrefix::None),
            (ToolPrefix::Short, WriterToolPrefix::Short),
            (ToolPrefix::Mcp, WriterToolPrefix::Mcp),
        ];
        for (server, tool) in [
            ("chrome-devtools", "take_screenshot"),
            ("linear-mcp", "list_issues"),
            ("github", "search.repositories"),
            ("naïve", "click"),
            ("mcp", "ping"),
        ] {
            for (reader_mode, writer_mode) in modes {
                assert_eq!(
                    get_server_prefix(server, reader_mode),
                    cyrup_mcp::registration::server_prefix(server, writer_mode),
                    "prefix {server} / {reader_mode:?}"
                );
                assert_eq!(
                    format_tool_name(tool, server, reader_mode),
                    cyrup_mcp::registration::format_tool_name(tool, server, writer_mode),
                    "name {server}/{tool} / {reader_mode:?}"
                );
            }
        }
        // Spelled out, so the table above cannot agree on something wrong in both crates.
        assert_eq!(
            format_tool_name("take_screenshot", "chrome-devtools", ToolPrefix::Server),
            "chrome-devtools_take_screenshot"
        );
        assert_eq!(
            format_tool_name("list_issues", "linear-mcp", ToolPrefix::Mcp),
            "mcp__linear-mcp_list_issues"
        );
        // `sanitizeServerPrefix` hex-escapes what it cannot keep: `ï` is U+00EF.
        assert_eq!(get_server_prefix("naïve", ToolPrefix::Server), "na_ef_ve");
        // …and `formatToolName` folds dots in the TOOL name, hyphens survive.
        assert_eq!(
            format_tool_name("search.repositories", "github", ToolPrefix::Server),
            "github_search_repositories"
        );
        assert_eq!(
            format_tool_name("web-search", "github", ToolPrefix::Server),
            "github_web-search"
        );
    }

    /// The whole contract, end to end: `cyrup-mcp` writes `mcp-cache.json` — its own
    /// `ServerCacheEntry`, its own `configHash` — and this resolver reads it.
    ///
    /// This is the production failure the four units describe. Before the change the entry was
    /// rejected outright by `is_server_cache_valid` (the digests could not match), so the selector
    /// expanded to nothing; and had it matched, the resource tool would have come out as
    /// `browser_mcp_get_console_logs` — a name `cyrup_mcp::registration` never registers.
    #[test]
    fn a_cache_written_by_cyrup_mcp_resolves_through_this_reader() {
        let fixture = make_fixture();
        // `includeTools` participates in the digest (MCP-141); this resolver does not yet FILTER on
        // it (the remaining half of MCP-370), which is why every cached tool below still appears.
        let definition = serde_json::json!({
            "command": "npx",
            "args": ["browser-mcp"],
            "protocolVersion": "auto",
            "includeTools": ["click", "navigate"]
        });
        write_json(
            &fixture.agent_dir.join("mcp.json"),
            &serde_json::json!({ "mcpServers": { "browser-mcp": definition } }),
        );

        let writer_entry: WriterServerEntry =
            serde_json::from_value(definition).expect("writer entry");
        let mut cache = cyrup_mcp::dirs::MetadataCache::default();
        cache.servers.insert(
            "browser-mcp".to_string(),
            cyrup_mcp::dirs::ServerCacheEntry {
                config_hash: compute_server_hash(
                    &writer_entry,
                    &ResolvedIdentity::verbatim(&writer_entry),
                ),
                tools: vec![cyrup_mcp::dirs::CachedTool {
                    name: "click".to_string(),
                    ..cyrup_mcp::dirs::CachedTool::default()
                }],
                resources: vec![cyrup_mcp::dirs::CachedResource {
                    uri: "resource://console".to_string(),
                    name: "Console Logs".to_string(),
                    ..cyrup_mcp::dirs::CachedResource::default()
                }],
                cached_at: now_ms(),
                ..cyrup_mcp::dirs::ServerCacheEntry::default()
            },
        );
        cyrup_mcp::dirs::save_metadata_cache(&fixture.agent_dir.join("mcp-cache.json"), &cache)
            .expect("save cache");

        let resolved = resolve(&fixture, &["browser-mcp"]);
        assert_eq!(
            resolved,
            vec![
                "browser-mcp_click".to_string(),
                "browser-mcp_read_console_logs".to_string()
            ]
        );
        // …and those are exactly the names the writer would register for the same metadata.
        assert_eq!(
            resolved,
            vec![
                cyrup_mcp::registration::format_tool_name(
                    "click",
                    "browser-mcp",
                    WriterToolPrefix::Server
                ),
                cyrup_mcp::registration::format_tool_name(
                    "read_console_logs",
                    "browser-mcp",
                    WriterToolPrefix::Server
                ),
            ]
        );
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
