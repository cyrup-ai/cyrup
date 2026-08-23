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
//! `cyrup_mcp::dirs::server_identity_pre_image` (the 15-key `configHash` pre-image),
//! `cyrup_mcp::dirs::stable_stringify` (the token grammar that pre-image is rendered in) and
//! `cyrup_mcp::registration::{server_prefix, format_tool_name}` (the name a direct tool is actually
//! registered under). The writer was retargeted at upstream v2.26.1 and this reader was not, so the
//! two disagreed **silently**: every cached entry failed hash validation (so `mcp:` selectors
//! resolved to nothing) and every resource-backed tool resolved to a name the adapter never
//! registers. This module now mirrors all three:
//!
//! * MCP-141 — the identity pre-image is the same **15** keys, in the same *resolved* forms, as
//!   `server_identity_pre_image` (upstream `computeServerHash`, `metadata-cache.ts:82` @ v2.26.1).
//! * MCP-142 — `stable_stringify` renders an **absent** field as the bare nine-character token
//!   `undefined` (`metadata-cache.ts:344`), which is why `HashValue` exists instead of
//!   `serde_json::Value`.
//! * MCP-146 — a resource-backed tool is `read_<name>`, never `get_<name>`
//!   (`tool-metadata.ts:46` and `:119`).
//! * MCP-370 — `ToolPrefix` carries upstream's **four** modes and the server prefix goes through
//!   `sanitizeServerPrefix` (`types.ts:667`/`:675`), which **preserves hyphens**.
//! * MCP-143 — `interpolate_env_vars` runs upstream's **three** chained passes; the third,
//!   `{env:NAME}`, was missing.
//! * MCP-144 — hashed values go through the `!`/`!!` secret grammar
//!   ([`interpolate_secret_expression`]), so `!!x` un-escapes to the literal `!x` and a bare `!cmd`
//!   is hashed verbatim and **never executed**.
//! * MCP-145 — the hash is fallible and [`is_server_cache_valid_with_age`] is upstream's
//!   `try`/`catch`: an unresolvable `url` or a non-string `env` value makes the entry invalid
//!   rather than producing a digest upstream would never produce.
//!
//! The writer's side of the same contract closed with them: `ResolvedIdentity::resolve`
//! (MCP-141 leg (b)) replaced `ResolvedIdentity::verbatim` at every production call site, so both
//! sides now hash the *resolved* forms and `reader_and_writer_agree_once_every_resolver_actually_runs`
//! asserts one node-generated vector through both implementations.
//!
//! ## The four hashing divergences that used to live here are closed
//!
//! Three of them were the writer's config types discarding what upstream keeps, and the fourth was
//! a key both sides dropped. All four moved every digest, and all four were free to fix only while
//! `cyrup_mcp::dirs::save_metadata_cache` still had no production call site — no deployed cache had
//! to be invalidated.
//!
//! 1. **`socket`.** Upstream's identity object has **15** keys, and its third is
//!    `socket: resolveConfigPath(definition.socket)` (`metadata-cache.ts:89`). `stableStringify`
//!    walks `Object.keys()`, so an absent `socket` is emitted as `"socket":undefined` rather than
//!    dropped. Both Rust pre-images omitted the key, so every cyrup digest differed from pi's by
//!    exactly that member. Both now emit it unconditionally — `socket` is Cut 3 and
//!    `cyrup_mcp::config::to_server_entries` rejects any entry configuring one, so the value can
//!    only be `undefined`. `the_socket_key_is_no_longer_a_divergence_from_upstream` pins cyrup's
//!    digest against upstream's own, measured on node 22.
//! 2. **`auth`** — the writer's `AuthMode` gained a verbatim `Other` arm, so `auth: "basic"` reaches
//!    the pre-image instead of being dropped to `undefined` by `lenient`.
//! 3. **`protocolVersion`** — likewise `ProtocolVersionSetting::Other`, so a real revision such as
//!    `"2025-06-18"` is hashed as written. `Invalid MCP protocolVersion` is still raised, at
//!    **connect**, by `cyrup_mcp::runtime::version_negotiation` — which is where upstream raises it,
//!    and which the deserialiser must not pre-empt.
//! 4. **`env` / `headers`** — the worst of the four, because the two crates returned *opposite*
//!    answers rather than merely different digests. Upstream's `interpolateEnvRecord` throws on a
//!    non-string member and `isServerCacheValid` catches it to `false`, which is what this reader
//!    has always done; the writer's `lenient` dropped the whole map, hashed `"env":undefined` and
//!    called the entry VALID. `cyrup_mcp::config::StringRecord` now carries the throw.
//!
//! A **fifth** turned up while measuring the four, and it was this module's: `auth` and
//! `protocolVersion` are `Option<Value>` here, and serde's derived `Option<T>` reads a JSON `null`
//! as `None` — so `auth: null` hashed as `"auth":undefined` where upstream hashes `"auth":null`,
//! and the writer (whose `lenient` keeps the raw value) was the correct side. `present_or_absent`
//! is the fix. It was found by `reader_writer_and_upstream_agree_across_the_edge_cases`, which
//! compares both implementations against *upstream's* digest rather than against each other — the
//! reader-versus-writer tables agreed happily while both were one key away from pi.
//!
//! ## What this module is still NOT byte-identical to, stated exactly
//!
//! **A narrow divergence remains, and it is a path one, not a digest one.** The home this
//! module anchors `~` against ([`home_dir`]: `CYRUP_HOME` → `HOME` → tempdir) is not the home the
//! writer's default hasher uses (`cyrup_mcp::dirs::home_dir`: `HOME` → `USERPROFILE`), so with
//! `CYRUP_HOME` set to something other than `HOME` the two disagree — but only for a server whose
//! `cwd` actually starts with `~`. That is the narrow tail of MCP-139's agent-dir axis 3, whose fix
//! is the one shared agent-dir resolver that unit specifies, spanning `cyrup_ext`'s `npx_resolver`
//! as well as this file.

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
/// a key of the config-identity pre-image (MCP-141) — this resolver never runs it, and the
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
    /// `Record<string, unknown>` in pi, so this is held as raw JSON. A non-string value makes
    /// `interpolateEnvRecord` **throw** (`value.startsWith is not a function`), which
    /// [`is_server_cache_valid_with_age`] turns into "not cache-valid" — it is not filtered out
    /// (MCP-144).
    #[serde(default)]
    pub env: Option<BTreeMap<String, Value>>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, Value>>,
    /// Per-request header-signing command (upstream v2.26.0). Hashed, never executed here — see
    /// [`RequestHeadersCommand`]. One of the fifteen identity keys (MCP-141).
    #[serde(default, rename = "requestHeadersCommand")]
    pub request_headers_command: Option<RequestHeadersCommand>,
    /// `"oauth" | "bearer" | false` in pi — held raw and folded verbatim into the identity hash.
    ///
    /// `present_or_absent`, not a bare `#[serde(default)]`: serde's own `Option<T>` impl maps a
    /// JSON `null` to `None`, and this field is one of the two where the difference is a digest.
    #[serde(default, deserialize_with = "present_or_absent")]
    pub auth: Option<Value>,
    /// `"legacy" | "auto" | "2026-07-28"` in pi — held raw and folded verbatim into the identity
    /// hash, exactly as [`Self::auth`] is. Part of the config identity (MCP-141) because pinning a
    /// protocol revision changes what the server advertises; not otherwise used by this resolver.
    #[serde(default, rename = "protocolVersion", deserialize_with = "present_or_absent")]
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

/// `Some(value)` for a key that is **present**, including one whose value is JSON `null` — the
/// distinction `stableStringify` renders as `null` versus the bare token `undefined` (MCP-142).
///
/// Serde's derived `Option<T>` reads a JSON `null` as `None`, which collapses the two. That is
/// harmless for a field nobody hashes and wrong for [`ServerEntry::auth`] and
/// [`ServerEntry::protocol_version`], which `computeServerHash` folds in verbatim
/// (`metadata-cache.ts:103-104`): measured on node 22 @ `v2.26.1`, `computeServerHash({command:"x",
/// auth:null})` is `d5e9d0fe71ad5cc5d6a82b93d537f69ee59809f7f10e1f5c1f26c1d0a97e28e4` over a
/// pre-image carrying `"auth":null`, while the same definition without the key hashes a pre-image
/// carrying `"auth":undefined`. This reader produced the second for both.
///
/// `#[serde(default)]` still supplies `None` for an **absent** key, because serde calls a field's
/// `deserialize_with` only when the key is there.
fn present_or_absent<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Value::deserialize(deserializer)?))
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
    /// Epoch milliseconds. Upstream rejects a falsy or non-numeric value
    /// (`!entry.cachedAt || typeof entry.cachedAt !== "number"`), and so does
    /// [`is_server_cache_valid_with_age`].
    ///
    /// The custom deserialiser is that `typeof` arm made non-destructive: a plain `Option<i64>`
    /// turns `"cachedAt": "1760000000000"` into a serde **error**, which fails
    /// `serde_json::from_str::<MetadataCache>` and costs every other server in the file its cached
    /// tools. Upstream casts the parsed JSON without validating it and rejects only the one bad
    /// entry. Anything that is not a JSON integer becomes `None` here, which this entry — and only
    /// this entry — fails on. (MCP-145; `cyrup_mcp::dirs` and `cyrup_mcp::registration` carry the
    /// same deserialiser over the same bytes.)
    #[serde(default, rename = "cachedAt", deserialize_with = "lenient_epoch_ms")]
    cached_at: Option<i64>,
}

/// `Option<i64>` epoch milliseconds that answers `None` for **any** non-integer instead of failing
/// the parse — see [`ServerCacheEntry::cached_at`].
fn lenient_epoch_ms<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Value>::deserialize(deserializer)?.and_then(|value| value.as_i64()))
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
/// `crate::spawn::depth::resolve_effective_depth_from`'s injectable-lookup pattern.
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

/// `isServerCacheValid(entry, definition, maxAgeMs)` (`metadata-cache.ts:114` @ v2.26.1) at the
/// module's own [`CACHE_MAX_AGE_MS`] — the arity every call site here uses.
fn is_server_cache_valid(entry: &ServerCacheEntry, definition: &ServerEntry) -> bool {
    is_server_cache_valid_with_age(entry, definition, CACHE_MAX_AGE_MS)
}

/// `isServerCacheValid` with upstream's third parameter, and with its **throw-to-false** rule
/// (MCP-145).
///
/// ```text
/// let configHash; try { configHash = computeServerHash(definition) } catch { return false }
/// if (!entry || entry.configHash !== configHash) return false;
/// if (!entry.cachedAt || typeof entry.cachedAt !== "number") return false;
/// if (maxAgeMs > 0 && Date.now() - entry.cachedAt > maxAgeMs) return false;
/// return true;
/// ```
///
/// Four rejections, in upstream's order:
///
/// 1. **The hash throws.** `computeServerHash` runs inside a `try`, and *any* throw means invalid.
///    [`compute_mcp_server_hash`] now has that error channel — `resolveServerUrl` rejects a URL
///    naming an unset variable or one that no longer parses after interpolation, and
///    `interpolateEnvRecord` rejects a non-string `env`/`headers` value. Until MCP-145 this
///    function had no such arm at all, because the hash could not fail: a
///    `url: "https://x/${MISSING}"` server therefore hashed its *raw* text, and if a cache entry
///    for it existed the subagent registered direct tools for a server no call could ever reach.
/// 2. A `configHash` mismatch — absent counts as a mismatch.
/// 3. A **falsy or non-numeric** `cachedAt`. `!entry.cachedAt` rejects `0` as well as absent (this
///    accepted `0`), and the `typeof` test rejects a JSON string, which
///    [`ServerCacheEntry::cached_at`]'s deserialiser turns into `None`.
/// 4. An age over `max_age_ms`, checked **only when that limit is positive** — `0` disables the age
///    check entirely, which is upstream's documented way to validate a definition without regard to
///    freshness. This module had no parameter at all and treated the constant as a hard floor.
fn is_server_cache_valid_with_age(
    entry: &ServerCacheEntry,
    definition: &ServerEntry,
    max_age_ms: i64,
) -> bool {
    // The `catch { return false }`. This is the whole of MCP-145: a definition whose identity
    // cannot be computed is never cache-valid, and it is never an error the caller sees.
    let Ok(config_hash) = compute_mcp_server_hash(definition) else {
        return false;
    };
    if entry.config_hash.as_deref() != Some(config_hash.as_str()) {
        return false;
    }
    // `!entry.cachedAt` is a falsy test on a number, so `0` is rejected alongside absent.
    let Some(cached_at) = entry.cached_at.filter(|ms| *ms != 0) else {
        return false;
    };
    if max_age_ms > 0 && now_ms().saturating_sub(cached_at) > max_age_ms {
        return false;
    }
    true
}

/// The config-identity **pre-image** for a server definition — the exact byte sequence
/// [`compute_mcp_server_hash`] digests, and the shared contract with the cache writer's
/// `cyrup_mcp::dirs::server_identity_pre_image` (upstream `computeServerHash`,
/// `metadata-cache.ts:82` @ v2.26.1).
///
/// # Fifteen keys, and the list is the specification (MCP-141)
///
/// This function built **eleven**: `protocolVersion`, `includeTools` and `requestHeadersCommand`
/// were missing, so editing any of them left a stale tool list looking valid — and, far worse, the
/// eleven-key pre-image is one the writer never produces, so no `cyrup-mcp`-written cache entry
/// could validate here at all.
///
/// The fifteenth is `socket: resolveConfigPath(definition.socket)` (`metadata-cache.ts:89`), and it
/// is emitted **`undefined` unconditionally**. `stableStringify` walks `Object.keys()`, which
/// includes keys holding `undefined`, so upstream writes `"socket":undefined` for every definition
/// — one that never mentions a socket included. Both Rust pre-images used to drop the key, which
/// made every cyrup digest differ from pi's by exactly that member, for every server, forever; the
/// standing note in 13c-mcp-servers.md:1753 ("Keep `socket` … in the pre-image despite Cut 3") is
/// what this now satisfies. There is nothing to read: `socket` is Cut 3, neither
/// `cyrup_mcp::config::ServerEntry` nor [`ServerEntry`] has the field, and
/// `cyrup_mcp::config::to_server_entries` rejects any entry that configures one — so
/// `resolveConfigPath(definition.socket)` can only ever be `resolveConfigPath(undefined)`, which is
/// `undefined`.
///
/// # Five of the fifteen are hashed in their *resolved* form
///
/// `env` and `headers` (`interpolateEnvRecord`), `cwd` (`resolveConfigPath`), `url`
/// (`resolveServerUrl` — MCP-141's fourth gap: this took `definition.url` **raw**) and `bearerToken`
/// (`resolveBearerToken`). The digest has to change when `$API_HOST` changes even though the config
/// text did not; that is the entire point of hashing the resolved form.
///
/// Returned separately from the digest for the reason the writer returns it separately: a hash
/// mismatch tells you nothing about *which* field disagreed, and the conformance tests below assert
/// the bytes.
pub fn server_identity_pre_image(definition: &ServerEntry) -> Result<String, IdentityError> {
    server_identity_pre_image_with(definition, &|name| std::env::var(name).ok(), &home_dir())
}

/// [`server_identity_pre_image`] against an injected environment and home directory.
///
/// The production arity above reads `std::env` and [`home_dir`]; this one exists because edition
/// 2024 makes `std::env::set_var` `unsafe` (so a test cannot pin `$API_HOST` and put it back) and
/// because the cross-crate conformance tests must drive **both** implementations from one table —
/// `cyrup_mcp::dirs::ResolvedIdentity::resolve` takes the same two seams for the same reason.
pub fn server_identity_pre_image_with(
    definition: &ServerEntry,
    env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
) -> Result<String, IdentityError> {
    let identity = HashValue::Object(vec![
        ("command".to_string(), opt_string(definition.command.as_deref())),
        ("args".to_string(), opt_string_list(definition.args.as_ref())),
        ("env".to_string(), interpolate_env_record(definition.env.as_ref(), env)?),
        (
            "cwd".to_string(),
            opt_string(resolve_config_path(definition.cwd.as_deref(), home, env).as_deref()),
        ),
        // `socket: resolveConfigPath(definition.socket)` (`metadata-cache.ts:89`) — always
        // `undefined`, and the KEY is what matters: `stableStringify` emits a key whose value is
        // `undefined` rather than dropping it, so omitting it moved every digest off upstream's.
        ("socket".to_string(), HashValue::Undefined),
        (
            "url".to_string(),
            opt_string(resolve_server_url(definition.url.as_deref(), env)?.as_deref()),
        ),
        ("headers".to_string(), interpolate_env_record(definition.headers.as_ref(), env)?),
        (
            "requestHeadersCommand".to_string(),
            request_headers_command_value(definition.request_headers_command.as_ref(), env)?,
        ),
        ("auth".to_string(), HashValue::from_optional_json(definition.auth.clone())),
        (
            "protocolVersion".to_string(),
            HashValue::from_optional_json(definition.protocol_version.clone()),
        ),
        (
            "bearerToken".to_string(),
            opt_string(resolve_bearer_token(definition, env).as_deref()),
        ),
        ("bearerTokenEnv".to_string(), opt_string(definition.bearer_token_env.as_deref())),
        (
            "exposeResources".to_string(),
            definition.expose_resources.map_or(HashValue::Undefined, HashValue::Bool),
        ),
        ("includeTools".to_string(), opt_string_list(definition.include_tools.as_ref())),
        ("excludeTools".to_string(), opt_string_list(definition.exclude_tools.as_ref())),
    ]);
    Ok(stable_stringify(&identity))
}

/// One of `computeServerHash`'s `throw`s, carrying upstream's own message.
///
/// Every producer is a resolver upstream lets throw out of the identity literal, and every consumer
/// is [`is_server_cache_valid_with_age`]'s `catch`, which answers `false`. It is a distinct type
/// rather than a `String` so a caller cannot accidentally render it as a user-facing failure: there
/// is no path in this module on which an unhashable server is an *error* — it is simply a server
/// with no usable cache entry, exactly as upstream treats it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityError(pub String);

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Compute the stable config-identity hash for a server definition (pi `computeMcpServerHash`, and
/// the twin of `cyrup_mcp::dirs::compute_server_hash`): SHA-256 over
/// [`server_identity_pre_image`], hex-encoded. Exported so a cache writer (and this crate's tests)
/// stamp the SAME `configHash` the resolver validates.
pub fn compute_mcp_server_hash(definition: &ServerEntry) -> Result<String, IdentityError> {
    Ok(hex_sha256(&server_identity_pre_image(definition)?))
}

/// [`compute_mcp_server_hash`] against an injected environment and home — see
/// [`server_identity_pre_image_with`].
pub fn compute_mcp_server_hash_with(
    definition: &ServerEntry,
    env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
) -> Result<String, IdentityError> {
    Ok(hex_sha256(&server_identity_pre_image_with(definition, env, home)?))
}

/// `createHash("sha256").update(preImage).digest("hex")`.
fn hex_sha256(pre_image: &str) -> String {
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    hasher.update(pre_image.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // Writing to a `String` is infallible; the result is discarded rather than unwrapped.
        let _ = write!(hex, "{byte:02x}");
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

/// The `requestHeadersCommand` identity key (`metadata-cache.ts:94-101`): `undefined` when the field is absent,
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
) -> Result<HashValue, IdentityError> {
    let Some(command) = value else {
        return Ok(HashValue::Undefined);
    };
    Ok(HashValue::Object(vec![
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
        // `interpolateEnvRecord`, so a non-string value throws here too — the nested `env` is not a
        // laxer map than the outer one.
        ("env".to_string(), interpolate_env_record(command.env.as_ref(), env)?),
        (
            "timeoutMs".to_string(),
            command.timeout_ms.map_or(HashValue::Undefined, HashValue::Number),
        ),
    ]))
}

/// `resolveServerUrl(definition)` (`utils.ts:167`) — interpolate, validate, **or throw** (MCP-145).
///
/// Wave 1 landed the value half only: an absent `url` stayed absent and a present one was
/// interpolated, so the digest tracked `$API_HOST` (MCP-141's fourth gap). The error channel was
/// deliberately omitted then, on the argument that a definition the *writer* cannot hash never gets
/// a cache entry, so the worst this reader could do was compute a digest matching nothing. That
/// argument does not hold: a cache entry can also come from a co-installed `pi-mcp-adapter`, or
/// from cyrup itself *before* the variable was unset, and in either case this reader would validate
/// it against a raw-text digest and register direct tools for a server no call can reach — exactly
/// what upstream's `try`/`catch` exists to prevent. MCP-145 is the catcher; this is what it catches.
///
/// Three arms:
///
/// * absent ⇒ `Ok(None)` (upstream's `definition.url == null`, which is `null` *and* `undefined`);
/// * any placeholder naming an **unset** variable ⇒
///   `Missing environment variable{s} in MCP server URL: {names}`, singular/plural on the count,
///   names in first-occurrence order;
/// * a resolved string `new URL()` rejects ⇒
///   `Invalid MCP server URL after environment interpolation: {resolved}`.
///
/// Upstream's fourth arm, `MCP server URL must be a string`, is absorbed by the type system:
/// [`ServerEntry::url`] is `Option<String>`, so a non-string `url` is dropped by serde before it
/// reaches here. All three message forms were produced by **running `utils.ts` on node 22** at tag
/// `v2.26.1` (`fafae21`), not transcribed.
///
/// `url::Url::parse` is the WHATWG parser `new URL()` is; measured against node on the cases that
/// matter, both accept `unix:///tmp/s.sock`, `x:y` and `mailto:a@b` and both reject `//x/y` and
/// `/abs/path`.
fn resolve_server_url(
    url: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<String>, IdentityError> {
    let Some(url) = url else {
        return Ok(None);
    };
    let missing = missing_env_vars(url, env);
    if !missing.is_empty() {
        let plural = if missing.len() == 1 { "" } else { "s" };
        return Err(IdentityError(format!(
            "Missing environment variable{plural} in MCP server URL: {}",
            missing.join(", ")
        )));
    }
    let resolved = interpolate_env_vars(url, env);
    if url::Url::parse(&resolved).is_err() {
        return Err(IdentityError(format!(
            "Invalid MCP server URL after environment interpolation: {resolved}"
        )));
    }
    Ok(Some(resolved))
}

/// `getMissingEnvVars(value)` (`utils.ts:83`) — every placeholder name whose variable is unset, in
/// first-occurrence order, deduplicated.
///
/// **One scan over the three alternatives, not three chained passes.** That asymmetry with
/// [`interpolate_env_vars`] is upstream's: this function *scans* where the interpolator
/// *substitutes*, so it must never see a later pass's output. The scanner is written on top of
/// [`expand_pattern`]'s own name grammar so the two cannot disagree about what a name is — a
/// second, hand-rolled parser here is precisely how `{env:NAME}` came to be missing from one and
/// not the other.
///
/// `undefined`-vs-empty matters: upstream tests `process.env[name] === undefined`, so a variable set
/// to the empty string counts as **present** and is not reported.
///
/// The scan is one left-to-right pass that tries the three alternatives **in order at each start
/// position** and skips one character when none matches — which is what a regex alternation does,
/// and it is why this is not three [`expand_pattern`] passes: three passes would report every
/// `${…}` name before every `{env:…}` name, and upstream joins the names in the order it *found*
/// them, so `https://x/{env:B}/${A}` must say `B, A` and not `A, B`.
fn missing_env_vars(value: &str, env: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    let mut rest = value;
    while !rest.is_empty() {
        if let Some((name, consumed)) = match_placeholder(rest) {
            if env(name).is_none() && !missing.iter().any(|seen| seen == name) {
                missing.push(name.to_string());
            }
            rest = rest.get(consumed..).unwrap_or("");
        } else {
            let step = rest.chars().next().map_or(1, char::len_utf8);
            rest = rest.get(step..).unwrap_or("");
        }
    }
    missing
}

/// The three alternatives of `getMissingEnvVars`'s regex, tried in source order at one position:
/// `${NAME}`, `$env:NAME`, `{env:NAME}`. Returns the captured name and the byte length consumed.
///
/// `\w+` is greedy but must be followed by the closing delimiter, and `\w` never matches `}` — so
/// "the maximal run of name characters, then the delimiter" is the same language as "everything up
/// to the first `}`, all of it name characters".
fn match_placeholder(rest: &str) -> Option<(&str, usize)> {
    for (open, close) in [("${", true), ("$env:", false), ("{env:", true)] {
        let Some(after) = rest.strip_prefix(open) else {
            continue;
        };
        let name_len = after.find(|c: char| !is_word_char(c)).unwrap_or(after.len());
        if name_len == 0 {
            continue;
        }
        if close && after.get(name_len..name_len + 1) != Some("}") {
            continue;
        }
        let name = after.get(..name_len)?;
        return Some((name, open.len() + name_len + usize::from(close)));
    }
    None
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

/// `interpolateEnvRecord(values)` (`utils.ts:107`) — [`interpolate_secret_expression`] applied per
/// value, `undefined` in / `undefined` out. An **absent** map is `undefined` (MCP-142), never
/// `null`; a present map is an object.
///
/// **Two changes, both MCP-144, both digest-visible.**
///
/// 1. Each value goes through the `!`/`!!` grammar, not plain interpolation. `!!x` un-escapes to the
///    literal `!x` — the value the child will actually see — and a bare `!cmd` is hashed as its own
///    text and never executed. This function ran plain `interpolate_env_vars`, so `!!${HOME}` hashed
///    as `!!${HOME}`-interpolated rather than as `!` + home, and disagreed with the writer for every
///    such value.
/// 2. A **non-string value is a throw**, not a dropped key. Upstream calls `value.startsWith(…)`
///    unconditionally; measured on node 22 against v2.26.1, `interpolateEnvRecord({ k: 5 })` throws
///    `value.startsWith is not a function` and `{ k: null }` throws
///    `Cannot read properties of null (reading 'startsWith')` — either way `computeServerHash`
///    throws and [`is_server_cache_valid_with_age`] answers `false`. This silently *dropped* the
///    key, producing a digest for an object upstream never produces, so an `env: { "N": 5 }` server
///    validated a cache entry it should have been denied.
///
/// [`BTreeMap`] iterates in key order, which is the order [`stable_stringify`] would impose anyway.
fn interpolate_env_record(
    values: Option<&BTreeMap<String, Value>>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<HashValue, IdentityError> {
    let Some(values) = values else {
        return Ok(HashValue::Undefined);
    };
    let mut entries: Vec<(String, HashValue)> = Vec::with_capacity(values.len());
    for (key, value) in values {
        let Some(text) = value.as_str() else {
            // The message is upstream's own TypeError, which reaches nothing but the `catch`.
            return Err(IdentityError(if value.is_null() {
                "Cannot read properties of null (reading 'startsWith')".to_string()
            } else {
                "value.startsWith is not a function".to_string()
            }));
        };
        entries.push((
            key.clone(),
            HashValue::String(interpolate_secret_expression(text, env)),
        ));
    }
    Ok(HashValue::Object(entries))
}

/// `interpolateSecretExpression(value)` (`utils.ts:102`) — the `!` / `!!` command-marker grammar
/// (MCP-144), and the twin of `cyrup_mcp::credentials::interpolate_secret_expression`.
///
/// * `!!x` — an **escaped** literal `!`: upstream is `interpolateEnvVars(value.slice(1))`, which
///   drops exactly **one** `!`, so `!!x` yields `!x` and `!!!x` yields `!!x`. Stripping both — the
///   obvious `strip_prefix("!!")` — deletes the escape and turns an escaped literal into a bare
///   value.
/// * `!x` — a command marker, returned **verbatim**. Hashing must never run a command: this module
///   resolves a subagent's tool selectors from config found on disk, and a resolver that executed
///   `!` here would run arbitrary shell out of a repository's `.mcp.json` merely because a subagent
///   was spawned in that directory. Upstream's executing form, `resolveCommandSecret`, is reachable
///   only from connect/auth paths and has no counterpart in this crate at all.
/// * anything else — interpolated.
fn interpolate_secret_expression(value: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    if value.starts_with("!!") {
        return interpolate_env_vars(value.get(1..).unwrap_or_default(), env);
    }
    if value.starts_with('!') {
        return value.to_string();
    }
    interpolate_env_vars(value, env)
}

/// `interpolateEnvVars(value)` (`utils.ts:74`) — **three** chained passes, `${NAME}` then
/// `$env:NAME` then `{env:NAME}`, each falling back to the empty string on a missing variable.
///
/// **The third pass is MCP-143.** It had two, so `{env:GITHUB_TOKEN}` in an `env` value, a header, a
/// URL, a `cwd` or a `bearerToken` reached the digest as an 18-character literal — and the writer,
/// which has had all three since MCP-082, hashed the expanded value. Two failure modes, and the
/// silent one is the expensive one: the child gets a literal placeholder, *and* the config hash
/// differs, so no cache entry validates and the subagent's `mcp:` selectors resolve to nothing.
///
/// Order matters because each pass runs over the previous pass's **output**, which is observable:
/// with `A="$env:B"` and `B="2"`, `"${A}"` resolves to `"2"`, where a single alternation would leave
/// `$env:B` literal. (Measured on node 22 at v2.26.1 — that is also why the scan in
/// [`missing_env_vars`] is deliberately *not* three passes.)
fn interpolate_env_vars(value: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    let after_braces = expand_pattern(value, "${", Some("}"), env);
    let after_dollar_env = expand_pattern(&after_braces, "$env:", None, env);
    expand_pattern(&after_dollar_env, "{env:", Some("}"), env)
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
        return Some(node_path_join(home, rest).to_string_lossy().into_owned());
    }
    Some(resolved)
}

/// node's `path.join(base, rest)`, which is NOT `Path::join` — see the twin in
/// `cyrup_mcp::dirs::node_path_join`, which this MUST stay byte-identical to.
///
/// Three differences, all of which change the digest: node never lets `rest` act absolute
/// (`path.join("/home/u", "/x")` is `/home/u/x`, where `Path::join` yields `/x` and loses the home
/// directory entirely — `"~//x"` strips to exactly that `rest`); node normalizes, collapsing
/// repeated separators and folding `.`/`..` lexically; and node drops a trailing separator unless
/// the result is the root.
///
/// This is duplicated rather than shared because `cyrup-mcp` is a `[dev-dependency]` of this crate,
/// so production code here cannot call it. The cross-crate conformance tests hold the copies
/// together.
fn node_path_join(base: &Path, rest: &str) -> PathBuf {
    let base = base.to_string_lossy().replace('\\', "/");
    let rest = rest.replace('\\', "/");
    let absolute = base.starts_with('/');

    let mut parts: Vec<&str> = Vec::new();
    for segment in base.split('/').chain(rest.split('/')) {
        match segment {
            "" | "." => {}
            ".." => {
                if matches!(parts.last(), Some(&last) if last != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }

    let joined = parts.join("/");
    if absolute {
        PathBuf::from(format!("/{joined}"))
    } else if joined.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(joined)
    }
}

/// `resolveBearerToken(definition)` (`utils.ts:199`) — `bearerToken` wins over `bearerTokenEnv`.
///
/// `bearerToken` present ⇒ [`interpolate_secret_expression`] (MCP-144: this called plain
/// `interpolate_env_vars`, so `!!x` hashed as `!!x` where the writer hashes `!x`, and a `!cmd`
/// marker was interpolated instead of passed through); else `process.env[bearerTokenEnv]` when
/// `bearerTokenEnv` is **truthy** — an empty string is not, which the `filter` below reproduces
/// rather than relying on `env("")` happening to return `None`.
fn resolve_bearer_token(
    definition: &ServerEntry,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    if let Some(token) = definition.bearer_token.as_deref() {
        return Some(interpolate_secret_expression(token, env));
    }
    definition
        .bearer_token_env
        .as_deref()
        .filter(|name| !name.is_empty())
        .and_then(env)
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
                        "configHash": compute_mcp_server_hash(&entry).expect("fixture definitions are hashable"),
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

    /// All fifteen identity fields set at once, hashed by BOTH implementations (MCP-141/142), and
    /// pinned against upstream's own digest for the same definition — `socket` included.
    ///
    /// Fails on the pre-committed code twice over: this module emitted **eleven** keys (no
    /// `protocolVersion`, no `includeTools`, no `requestHeadersCommand`) and rendered every absent
    /// field as `null` where the writer renders `undefined`. In production that meant
    /// `is_server_cache_valid` rejected every entry `cyrup-mcp` writes, so every `mcp:` selector a
    /// subagent declared resolved to nothing — with no error anywhere.
    ///
    /// The fixture carries no interpolation token and no `!` secret marker on purpose, so it can be
    /// hashed through the writer's `ResolvedIdentity::verbatim` — the resolvers are the identity on
    /// such a definition, which keeps this vector independent of the ambient environment. The case
    /// where they are *not* the identity is
    /// `reader_and_writer_agree_once_every_resolver_actually_runs`, which drives both sides from one
    /// injected environment and home.
    #[test]
    fn reader_and_writer_agree_on_the_fifteen_field_pre_image() {
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

        let pre_image = server_identity_pre_image(&reader).expect("hashable");
        assert_eq!(pre_image, writer_pre_image(&writer, &resolved));
        assert_eq!(compute_mcp_server_hash(&reader).expect("hashable"), compute_server_hash(&writer, &resolved));

        // Fifteen keys, named — a count alone would not say which one went missing.
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
            "socket",
            "url",
        ] {
            assert!(pre_image.contains(&format!("\"{key}\":")), "missing {key} in {pre_image}");
        }
        // Upstream's own digest for this definition, measured on node 22 @ `v2.26.1` (`fafae21`).
        // While `socket` was missing the two crates agreed with each other here and with nobody
        // else, which is exactly the failure mode a reader-versus-writer assertion cannot see.
        assert_eq!(
            compute_mcp_server_hash(&reader).expect("hashable"),
            "fa28f264c176c38a612395e65601f0646a3faba71cdc834b095c488c9a5bd63c"
        );
        // `timeoutMs` renders as a JS number: `2500`, never `2500.0`.
        assert!(pre_image.contains(r#""timeoutMs":2500}"#), "{pre_image}");
        // The three runtime-only keys are outside the identity, so editing them must NOT evict.
        assert!(!pre_image.contains("lifecycle"), "{pre_image}");
        assert!(!pre_image.contains("debug"), "{pre_image}");
    }

    /// **`socket` is no longer a divergence** — cyrup's digest IS upstream's, measured.
    ///
    /// `computeServerHash` (`metadata-cache.ts:86-109` @ v2.26.1) builds a **15**-key identity whose
    /// third key is `socket: resolveConfigPath(definition.socket)`, and `stableStringify` walks
    /// `Object.keys()` — which includes keys holding `undefined` — so upstream emits
    /// `"socket":undefined` even for a definition that never mentions a socket. Neither Rust
    /// pre-image emitted the key, so **every** cyrup digest differed from pi's by exactly that one
    /// member. Both now emit it unconditionally, which is the whole of 13c-mcp-servers.md:1753
    /// ("Keep `socket` … in the pre-image despite Cut 3"): `ServerEntry` has no `socket` field
    /// post-Cut-3 and `cyrup_mcp::config::to_server_entries` rejects any entry that configures one,
    /// so `resolveConfigPath(definition.socket)` can only ever be `resolveConfigPath(undefined)`.
    ///
    /// Both constants below were produced by running upstream's own `stableStringify` +
    /// `computeServerHash` on node 22 against `tmp/pi-mcp-adapter` at tag `v2.26.1` (`fafae21`),
    /// using the same fixture as the cross-crate vector above, and both **include `socket`**. The
    /// reconstruction that produced the pre-image was proved faithful by asserting
    /// `sha256(preImage) === computeServerHash(definition)` against upstream's own exported
    /// function on the same run. This is the positive form of a test that used to assert the two
    /// sides differ; before the key landed cyrup's digest here was
    /// `4dd46c1fd26680867fe6c5ffdde2ab0f0a35972cd9c211bf6dd68d1f304eb277`.
    #[test]
    fn the_socket_key_is_no_longer_a_divergence_from_upstream() {
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

        let ours = server_identity_pre_image(&entry).expect("hashable");
        assert_eq!(ours, UPSTREAM_PRE_IMAGE, "byte-for-byte, or the key moved again");
        assert!(ours.contains(r#""socket":undefined"#), "{ours}");
        assert_eq!(compute_mcp_server_hash(&entry).expect("hashable"), UPSTREAM_DIGEST);

        // The writer agrees with both, which is the property the shared cache file depends on.
        let writer: WriterServerEntry = serde_json::from_str(
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
        .expect("writer entry");
        let resolved = ResolvedIdentity::verbatim(&writer);
        assert_eq!(writer_pre_image(&writer, &resolved), UPSTREAM_PRE_IMAGE);
        assert_eq!(compute_server_hash(&writer, &resolved), UPSTREAM_DIGEST);
    }

    /// The third `lenient` divergence, and the one that used to be worst-behaved: on a non-string
    /// `env`/`headers` member the two crates returned OPPOSITE cache-validity answers, not merely
    /// different digests. They now give the same answer, and it is upstream's.
    ///
    /// Measured on node 22 @ v2.26.1: `computeServerHash({command:"x",env:{K:5}})` THROWS
    /// `value.startsWith is not a function`, `{command:"x",headers:{K:null}}` THROWS
    /// `Cannot read properties of null (reading 'startsWith')`, and a non-string member in
    /// `requestHeadersCommand.env` throws the same way — `isServerCacheValid` catches every one of
    /// them and returns `false`. This reader always reproduced that. The writer held `env`/`headers`
    /// as `Option<BTreeMap<String, String>>` behind `deserialize_with = "lenient"`, which dropped
    /// the whole map when any member failed to fit — so it hashed `"env":undefined`, produced a
    /// digest, and called the entry VALID.
    ///
    /// `cyrup_mcp::config::StringRecord` is the fix: it keeps the string members for every consumer
    /// *and* the throw for the hash, and `ResolvedIdentity::resolve` raises it. This is the positive
    /// form of a test that used to assert the two sides disagree.
    #[test]
    fn a_non_string_record_member_makes_both_crates_refuse_the_hash() {
        let home = Path::new("/home/u");
        let env = |_: &str| None;
        let writer_env: cyrup_mcp::credentials::EnvFn = std::sync::Arc::new(|_: &str| None);

        for (json, message) in [
            (r#"{"command":"x","env":{"K":5}}"#, "value.startsWith is not a function"),
            (
                r#"{"command":"x","headers":{"K":null}}"#,
                "Cannot read properties of null (reading 'startsWith')",
            ),
            (
                r#"{"url":"https://api.example.com/mcp",
                    "requestHeadersCommand":{"command":"sign","env":{"K":5}}}"#,
                "value.startsWith is not a function",
            ),
        ] {
            // The reader refuses to hash it at all — upstream's throw.
            let reader: ServerEntry = serde_json::from_str(json).expect("reader entry");
            assert_eq!(
                server_identity_pre_image_with(&reader, &env, home)
                    .expect_err(json)
                    .to_string(),
                message,
                "{json}"
            );

            // …and so does the writer, with the same sentence.
            let writer: WriterServerEntry = serde_json::from_str(json).expect("writer entry");
            assert_eq!(
                ResolvedIdentity::resolve(&writer, &writer_env, home)
                    .expect_err(json)
                    .to_string(),
                message,
                "{json}"
            );

            // The consequence that actually matters: `isServerCacheValid` answers `false` on BOTH
            // sides, for an entry whose stamped hash could not have come from anywhere.
            let entry = ServerCacheEntry {
                config_hash: Some("whatever".to_string()),
                cached_at: Some(now_ms()),
                ..ServerCacheEntry::default()
            };
            assert!(!is_server_cache_valid_with_age(&entry, &reader, 0), "{json}");
            assert!(
                cyrup_mcp::dirs::try_compute_server_hash(&writer, &writer_env, home).is_err(),
                "{json}"
            );
        }

        // And the string members still reach every consumer — the map is not dropped, only the
        // digest is refused.
        let mixed: WriterServerEntry =
            serde_json::from_str(r#"{"command":"x","env":{"GOOD":"1","BAD":5}}"#)
                .expect("writer entry");
        let record = mixed.env.as_ref().expect("the map survives");
        assert_eq!(record.get("GOOD").map(String::as_str), Some("1"));
        assert_eq!(record.len(), 1, "only the string members are usable");
        assert_eq!(record.unhashable(), Some("value.startsWith is not a function"));
    }

    /// The `protocolVersion` digest divergence, closed and pinned against upstream's own digest.
    ///
    /// Upstream hashes the key verbatim (`metadata-cache.ts:104`) and `config.ts` validates it not
    /// at all; `Invalid MCP protocolVersion` is thrown at CONNECT time, long after the digest. This
    /// reader always held the field raw and so always matched upstream. The writer typed it behind
    /// `deserialize_with = "lenient"`, which silently dropped any value its enum rejected, and
    /// `ProtocolVersionSetting` knew only `"legacy" | "auto" | "2026-07-28"`.
    ///
    /// `"2025-06-18"` is a real MCP protocol revision and is exactly the shape of value a user pins.
    /// For such a server the two sides hashed different pre-images, so `is_server_cache_valid`
    /// rejected every entry and the server re-discovered its whole surface every session —
    /// silently, and forever. It failed SAFE (a re-discovery, never a wrong tool), which is
    /// precisely why it survived two separate audits without anything visibly breaking.
    ///
    /// `ProtocolVersionSetting::Other` closes it. The constants are upstream's own, from node 22 @
    /// `v2.26.1`, and include `socket`. **The deserialiser still validates nothing** — the throw
    /// moved to connect, where upstream performs it; see
    /// `cyrup_mcp::runtime`'s `wire_tests::an_unknown_revision_throws_upstreams_sentence_at_connect`.
    #[test]
    fn a_protocol_revision_the_writer_used_to_reject_now_agrees_on_the_digest() {
        const JSON: &str = r#"{"url":"https://api.example.com/mcp","protocolVersion":"2025-06-18"}"#;
        const UPSTREAM_PRE_IMAGE: &str = concat!(
            r#"{"args":undefined,"auth":undefined,"bearerToken":undefined,"#,
            r#""bearerTokenEnv":undefined,"command":undefined,"cwd":undefined,"env":undefined,"#,
            r#""excludeTools":undefined,"exposeResources":undefined,"headers":undefined,"#,
            r#""includeTools":undefined,"protocolVersion":"2025-06-18","#,
            r#""requestHeadersCommand":undefined,"socket":undefined,"#,
            r#""url":"https://api.example.com/mcp"}"#
        );
        const UPSTREAM_DIGEST: &str =
            "9825a7ed2a651688c432bdab4dbbf2139581f641cad0c97ee3052cc64336ec81";
        let env = |_: &str| None;
        let home = Path::new("/home/u");

        let reader: ServerEntry = serde_json::from_str(JSON).expect("reader entry");
        let ours = server_identity_pre_image_with(&reader, &env, home).expect("reader pre-image");

        let writer: WriterServerEntry = serde_json::from_str(JSON).expect("writer entry");
        let writer_env: cyrup_mcp::credentials::EnvFn = std::sync::Arc::new(|_: &str| None);
        let resolved =
            ResolvedIdentity::resolve(&writer, &writer_env, home).expect("writer resolve");
        let theirs = writer_pre_image(&writer, &resolved);

        // Both keep what the user wrote, and it is what upstream keeps.
        assert!(ours.contains(r#""protocolVersion":"2025-06-18""#), "{ours}");
        assert!(theirs.contains(r#""protocolVersion":"2025-06-18""#), "{theirs}");
        assert_eq!(ours, theirs, "if these differ, the passthrough arm regressed");
        assert_eq!(ours, UPSTREAM_PRE_IMAGE);
        assert_eq!(compute_mcp_server_hash(&reader).expect("hashable"), UPSTREAM_DIGEST);
        assert_eq!(compute_server_hash(&writer, &resolved), UPSTREAM_DIGEST);
    }

    /// The `auth` half of the same divergence, and its own upstream-generated vector.
    ///
    /// `auth` is hashed verbatim (`metadata-cache.ts:103`) and validated nowhere; every consumer is
    /// a `===` comparison an unknown value simply fails. Behind `lenient`, the writer's two-variant
    /// `AuthMode` turned `auth: "basic"` into `None` and hashed `"auth":undefined`, while this
    /// reader — holding the field raw, as upstream does — hashed the value. `AuthMode::Other` closes
    /// it. Constants from node 22 @ `v2.26.1`, `socket` included.
    #[test]
    fn an_auth_value_the_writer_used_to_reject_now_agrees_on_the_digest() {
        const JSON: &str = r#"{"url":"https://api.example.com/mcp","auth":"basic"}"#;
        const UPSTREAM_DIGEST: &str =
            "0926c8b8e6711e6d59143d0a409ea51a3ef76a1006057ffa28d4be20835137b8";
        let home = Path::new("/home/u");

        let reader: ServerEntry = serde_json::from_str(JSON).expect("reader entry");
        let writer: WriterServerEntry = serde_json::from_str(JSON).expect("writer entry");
        let writer_env: cyrup_mcp::credentials::EnvFn = std::sync::Arc::new(|_: &str| None);
        let resolved =
            ResolvedIdentity::resolve(&writer, &writer_env, home).expect("writer resolve");
        let theirs = writer_pre_image(&writer, &resolved);

        assert!(theirs.contains(r#""auth":"basic""#), "{theirs}");
        assert_eq!(
            server_identity_pre_image_with(&reader, &|_: &str| None, home).expect("hashable"),
            theirs
        );
        assert_eq!(compute_server_hash(&writer, &resolved), UPSTREAM_DIGEST);
        assert_eq!(compute_mcp_server_hash(&reader).expect("hashable"), UPSTREAM_DIGEST);

        // `"oauth"`, `"bearer"` and the two booleans still land on the arms that have meaning, so
        // the passthrough widened nothing: `Other` catches only what those reject.
        for (json, expected) in [
            (r#"{"auth":"oauth"}"#, cyrup_mcp::config::AuthMode::Named(cyrup_mcp::config::AuthKind::Oauth)),
            (r#"{"auth":"bearer"}"#, cyrup_mcp::config::AuthMode::Named(cyrup_mcp::config::AuthKind::Bearer)),
            (r#"{"auth":false}"#, cyrup_mcp::config::AuthMode::Disabled(false)),
        ] {
            let parsed: WriterServerEntry = serde_json::from_str(json).expect("writer entry");
            assert_eq!(parsed.auth, Some(expected), "{json}");
        }
    }

    /// The cross-crate golden vector, asserted here and in
    /// `cyrup_mcp::dirs::tests::golden_vector_stdio_server`.
    ///
    /// **Upstream-generated, and no longer carrying a caveat.** Both constants are upstream's own
    /// `stableStringify` + `computeServerHash` run on node 22 against `tmp/pi-mcp-adapter` at tag
    /// `v2.26.1` (`fafae21`), and they now **include the `socket` key** — so they are byte-compatible
    /// with pi as well as with `cyrup_mcp::dirs`. While the key was missing this vector read
    /// `4dd46c1f…`, a digest only cyrup produced.
    ///
    /// The cross-crate test above proves the two implementations agree; this proves *what* they
    /// agree on. Note what a plain stdio server's pre-image is mostly made of: ten `undefined`
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
            server_identity_pre_image(&entry).expect("hashable"),
            concat!(
                r#"{"args":["-y","@modelcontextprotocol/server-filesystem","/tmp"],"#,
                r#""auth":undefined,"bearerToken":undefined,"bearerTokenEnv":undefined,"#,
                r#""command":"npx","cwd":"/home/u/work","#,
                r#""env":{"API_TOKEN":"s3cret","NODE_ENV":"production"},"#,
                r#""excludeTools":["danger_*"],"exposeResources":false,"headers":undefined,"#,
                r#""includeTools":undefined,"protocolVersion":undefined,"#,
                r#""requestHeadersCommand":undefined,"socket":undefined,"url":undefined}"#
            )
        );
        assert_eq!(
            compute_mcp_server_hash(&entry).expect("hashable"),
            "2190558e470a75c0f992989bd1799b374e669deecb8093e4118a1a9419068cf4"
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
            compute_mcp_server_hash(&http).expect("hashable"),
            "5ee9972ca350254322d2c8aa519f273144f1f80b256e1cfce5a1af21e3e75970"
        );
    }

    /// The differential table: one fixture set, three implementations, one digest each.
    ///
    /// Every constant below is `computeServerHash(definition)` evaluated on node 22 against
    /// `tmp/pi-mcp-adapter` at tag `v2.26.1` (`fafae21`), with `HOME=/home/u` and no other variable
    /// set — **upstream's answer**, `socket` included. The test asserts that this reader and
    /// `cyrup_mcp::dirs` both reproduce it. It exists because the four divergences this wave closed
    /// were each invisible on the *other* implementations' fixtures: a table that only ever compared
    /// reader against writer agreed happily while both were one key away from pi.
    ///
    /// The rows are chosen for the edges, not the middle: an **empty** map (`{}` is not `undefined`),
    /// an explicit JSON `null` on `auth` (which `null` renders as, and `undefined` does not), the
    /// tolerated `auth: true`, an `auth` that is an object, a numeric `protocolVersion`, and an `env`
    /// carrying non-ASCII and both characters `JSON.stringify` escapes.
    #[test]
    fn reader_writer_and_upstream_agree_across_the_edge_cases() {
        const HOME: &str = "/home/u";
        for (json, upstream_digest) in [
            (
                r#"{"command":"x","env":{}}"#,
                "1d224401e4ab9a3e11e3490649da48a3fd946b49869464320b97b423c7f2893b",
            ),
            (
                r#"{"command":"x","auth":null}"#,
                "d5e9d0fe71ad5cc5d6a82b93d537f69ee59809f7f10e1f5c1f26c1d0a97e28e4",
            ),
            (
                r#"{"command":"x","auth":true}"#,
                "ae49caf49c8f178b01c20e367d4d4bd5efa81862550adefca79f016e243fde43",
            ),
            (
                r#"{"command":"x","auth":{"mode":"custom"}}"#,
                "e20d021bf5d47780b216e45f26e49802817d04ebfc3421ece1bb56f9e7d0aa32",
            ),
            (
                r#"{"command":"x","protocolVersion":5}"#,
                "df7fbe03ab78e1275d5feac1fcd776d4360c04f291ce8a569c6cb65ad241a150",
            ),
            (
                r#"{"command":"x","protocolVersion":"auto"}"#,
                "4aa154797d547787f9172441c48461ecaaf4483f8dc0071fb5fd4fc60fc62d2d",
            ),
            (
                r#"{"url":"https://a.example/mcp","headers":{}}"#,
                "2a32f29f637d9dfda066a90f3c4991bf25c5fcea8d9f8d17f92924328a7f1a27",
            ),
            (
                "{\"command\":\"x\",\"env\":{\"K\":\"café ☃\",\"Q\":\"a\\\"b\\nc\"}}",
                "c05ec96dfb2a8e5f33558d675c5a4d0d62dfbb41ab77728fe5edb8260a2fd1ec",
            ),
        ] {
            let reader: ServerEntry = serde_json::from_str(json).expect("reader entry");
            let writer: WriterServerEntry = serde_json::from_str(json).expect("writer entry");
            let reader_env = |_: &str| None;
            let writer_env: cyrup_mcp::credentials::EnvFn = std::sync::Arc::new(|_: &str| None);
            let resolved = ResolvedIdentity::resolve(&writer, &writer_env, Path::new(HOME))
                .expect("writer resolve");

            let ours = server_identity_pre_image_with(&reader, &reader_env, Path::new(HOME))
                .expect("reader pre-image");
            assert_eq!(ours, writer_pre_image(&writer, &resolved), "{json}");
            assert_eq!(
                compute_mcp_server_hash_with(&reader, &reader_env, Path::new(HOME))
                    .expect("hashable"),
                upstream_digest,
                "reader disagrees with upstream on {json}\n  pre-image: {ours}"
            );
            assert_eq!(
                compute_server_hash(&writer, &resolved),
                upstream_digest,
                "writer disagrees with upstream on {json}"
            );
        }
    }

    /// MCP-142 pinned at the token level and at the digest level.
    ///
    /// `stable_stringify` mapped `Value::Null => "null"` and had no way to say `undefined` at all,
    /// so the empty definition — fifteen absent fields — hashed as fifteen `null`s. This is the
    /// case where the divergence is total: every single key disagreed with the writer. The digest is
    /// upstream's own, from node 22 @ `v2.26.1`, and includes `socket` (it read `671c1578…` while
    /// the key was missing).
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
        let pre_image = server_identity_pre_image(&empty).expect("hashable");
        assert!(!pre_image.contains("null"), "{pre_image}");
        assert_eq!(pre_image.matches("undefined").count(), 15, "{pre_image}");
        assert_eq!(
            compute_mcp_server_hash(&empty).expect("hashable"),
            "a04128961dff1d77f5ea95dd5ddb01415888636efe2d32cf950c78b34e54c3fa"
        );
        let writer_empty = WriterServerEntry::default();
        assert_eq!(
            compute_mcp_server_hash(&empty).expect("hashable"),
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
                compute_mcp_server_hash(&base).expect("hashable"),
                compute_mcp_server_hash(&changed).expect("hashable"),
                "{changed_json}"
            );
            let writer_base: WriterServerEntry =
                serde_json::from_str(r#"{ "command": "x" }"#).expect("writer base");
            let writer_changed: WriterServerEntry =
                serde_json::from_str(changed_json).expect("writer changed");
            assert_eq!(
                compute_mcp_server_hash(&changed).expect("hashable"),
                compute_server_hash(&writer_changed, &ResolvedIdentity::verbatim(&writer_changed)),
                "{changed_json}"
            );
            assert_eq!(
                compute_mcp_server_hash(&base).expect("hashable"),
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
        let pre_image = server_identity_pre_image(&literal).expect("hashable");
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

    /// The fixture environment every resolved vector below was generated against, and the `homedir()`
    /// the node run saw (`process.env.HOME = "/home/u"`, asserted by printing `homedir()`).
    fn vector_env(name: &str) -> Option<String> {
        match name {
            "HOST" => Some("a.example".to_string()),
            "TOK_ENV" => Some("from-env".to_string()),
            "A" => Some("1".to_string()),
            "B" => Some("2".to_string()),
            "C" => Some("3".to_string()),
            "A_B" => None,
            "CHAIN" => Some("$env:B".to_string()),
            "CHAIN2" => Some("{env:C}".to_string()),
            "café" => Some("unicode".to_string()),
            _ => None,
        }
    }

    fn vector_home() -> PathBuf {
        PathBuf::from("/home/u")
    }

    /// MCP-143 — `interpolateEnvVars`'s **three** chained passes.
    ///
    /// Every expectation below is the literal output of upstream's own `interpolateEnvVars`
    /// (`utils.ts:74` @ v2.26.1) run on node 22 with `A=1 B=2 C=3` and `café=unicode` set; nothing
    /// here is reasoned about. Three of them are worth naming:
    ///
    /// * `"a${A}-b$env:B-c{env:C}-d"` needs the third pass, which this function did not have. (The
    ///   version of this case in 13c's MCP-143 text, `"a${A}b$env:Bc{env:C}d"` → `"a1b2c3d"`, is
    ///   **wrong**: `\w+` is greedy, so `$env:Bc` names the unset variable `Bc` and node actually
    ///   returns `"a1b3d"`. Measured, not argued.)
    /// * `"${CHAIN}"` with `CHAIN="$env:B"` resolves to `"2"` — pass 2 runs over pass 1's output.
    /// * `"$env:café"` resolves to `"é"`: JavaScript's `\w` is ASCII-only, so the name is `caf`.
    ///   Rust's `regex` makes `\w` Unicode-aware, which is why the writer spells the class out and
    ///   why this module's `is_word_char` is ASCII.
    #[test]
    fn interpolate_env_vars_expands_all_three_forms() {
        let cases = [
            ("a${A}b", "a1b"),
            ("$env:A", "1"),
            ("{env:A}", "1"),
            ("a${A}-b$env:B-c{env:C}-d", "a1-b2-c3-d"),
            ("a${A}b$env:Bc{env:C}d", "a1b3d"),
            ("${NOPE}", ""),
            ("$env:NOPE", ""),
            ("{env:NOPE}", ""),
            ("{env:}", "{env:}"),
            ("{env:-}", "{env:-}"),
            ("${}", "${}"),
            ("$env:", "$env:"),
            ("{env:A", "{env:A"),
            ("env:A}", "env:A}"),
            ("${A}${B}{env:C}", "123"),
            ("{env:A}{env:B}", "12"),
            ("$ENV:A", "$ENV:A"),
            ("{ENV:A}", "{ENV:A}"),
            ("${A_B}", ""),
            ("prefix{env:A}suffix", "prefix1suffix"),
            ("${CHAIN}", "2"),
            ("${CHAIN2}", "3"),
            ("${café}", "${café}"),
            ("$env:café", "é"),
            ("{env:café}", "{env:café}"),
        ];
        for (input, expected) in cases {
            assert_eq!(interpolate_env_vars(input, &vector_env), expected, "input {input:?}");
        }
    }

    /// MCP-144 — `interpolateSecretExpression`'s three arms, and the two call sites that bypassed
    /// it.
    ///
    /// `!!x` drops exactly **one** `!` (so `!!!x` becomes `!!x`), a bare `!` is returned verbatim
    /// and never executed, and anything else interpolates. All four outputs are node-measured
    /// through upstream's `interpolateEnvRecord`.
    #[test]
    fn the_secret_expression_grammar_covers_both_hashed_call_sites() {
        assert_eq!(interpolate_secret_expression("!!${HOST}", &vector_env), "!a.example");
        assert_eq!(interpolate_secret_expression("!op read x", &vector_env), "!op read x");
        assert_eq!(interpolate_secret_expression("!!!x", &vector_env), "!!x");
        assert_eq!(interpolate_secret_expression("{env:HOST}", &vector_env), "a.example");

        // Call site 1 — `env` / `headers`.
        let values = BTreeMap::from([
            ("ESCAPED".to_string(), Value::String("!!${HOST}".to_string())),
            ("MARKER".to_string(), Value::String("!op read x".to_string())),
        ]);
        assert_eq!(
            interpolate_env_record(Some(&values), &vector_env).expect("all strings"),
            HashValue::Object(vec![
                ("ESCAPED".to_string(), HashValue::String("!a.example".to_string())),
                ("MARKER".to_string(), HashValue::String("!op read x".to_string())),
            ])
        );

        // Call site 2 — `bearerToken`.
        let escaped: ServerEntry =
            serde_json::from_str(r#"{ "bearerToken": "!!${HOST}" }"#).expect("entry");
        assert_eq!(resolve_bearer_token(&escaped, &vector_env).as_deref(), Some("!a.example"));
        let marker: ServerEntry =
            serde_json::from_str(r#"{ "bearerToken": "!op read tok" }"#).expect("entry");
        assert_eq!(resolve_bearer_token(&marker, &vector_env).as_deref(), Some("!op read tok"));
        // An empty `bearerTokenEnv` is falsy upstream, so it supplies nothing.
        let empty: ServerEntry =
            serde_json::from_str(r#"{ "bearerTokenEnv": "" }"#).expect("entry");
        assert_eq!(resolve_bearer_token(&empty, &vector_env), None);
    }

    /// The whole-identity cross-crate vector with every resolver exercised at once: all three
    /// interpolation forms (MCP-143), the `!!` escape and the `!` marker (MCP-144), a `~`-prefixed
    /// and interpolated `cwd`, an interpolated `url`, and a `bearerToken` supplied by
    /// `bearerTokenEnv`.
    ///
    /// This is the fixture the *previous* wave could not write: `ResolvedIdentity::verbatim` was the
    /// writer's only constructor, so anything carrying a token made the two sides disagree by
    /// construction. `ResolvedIdentity::resolve` (MCP-141 leg (b)) is what makes it assertable.
    ///
    /// **Provenance, stated exactly.** Both constants are upstream's, produced by running
    /// `computeServerHash` on node 22 against `tmp/pi-mcp-adapter` at tag `v2.26.1` (`fafae21`), and
    /// they **include `socket`** — so `ac61954a…` is the digest a stock `pi-mcp-adapter` computes
    /// for this definition, not merely the one cyrup computes. The pre-image came from running
    /// upstream's own `stableStringify` over `computeServerHash`'s identity literal, built from
    /// upstream's real resolvers, and was proved faithful by asserting
    /// `sha256(preImage) === computeServerHash(definition)` against upstream's exported function on
    /// the same run. While the key was missing this vector read `c273715e…`.
    #[test]
    fn reader_and_writer_agree_once_every_resolver_actually_runs() {
        const DEFINITION: &str = r#"{
            "command": "npx",
            "args": ["-y", "srv"],
            "env": {
                "PLAIN": "p",
                "INTERP": "${HOST}",
                "BRACE": "{env:HOST}",
                "DOLLAR": "$env:HOST",
                "ESCAPED": "!!${HOST}",
                "MARKER": "!op read x"
            },
            "cwd": "~/work/${HOST}",
            "url": "https://${HOST}/mcp",
            "headers": { "X-Host": "{env:HOST}", "X-Lit": "!keep" },
            "bearerTokenEnv": "TOK_ENV",
            "exposeResources": false,
            "includeTools": ["a"],
            "excludeTools": ["b"]
        }"#;
        const PRE_IMAGE: &str = concat!(
            r#"{"args":["-y","srv"],"auth":undefined,"bearerToken":"from-env","#,
            r#""bearerTokenEnv":"TOK_ENV","command":"npx","cwd":"/home/u/work/a.example","#,
            r#""env":{"BRACE":"a.example","DOLLAR":"a.example","ESCAPED":"!a.example","#,
            r#""INTERP":"a.example","MARKER":"!op read x","PLAIN":"p"},"#,
            r#""excludeTools":["b"],"exposeResources":false,"#,
            r#""headers":{"X-Host":"a.example","X-Lit":"!keep"},"includeTools":["a"],"#,
            r#""protocolVersion":undefined,"requestHeadersCommand":undefined,"#,
            r#""socket":undefined,"url":"https://a.example/mcp"}"#
        );
        const DIGEST: &str = "ac61954adda845c50a6c691e7ac291e2546dfcc6158b8d6a1b7785ce47356de3";

        let reader: ServerEntry = serde_json::from_str(DEFINITION).expect("reader entry");
        let writer: WriterServerEntry = serde_json::from_str(DEFINITION).expect("writer entry");
        let writer_env: cyrup_mcp::credentials::EnvFn = std::sync::Arc::new(vector_env);
        let resolved = ResolvedIdentity::resolve(&writer, &writer_env, &vector_home())
            .expect("this url resolves");

        let ours = server_identity_pre_image_with(&reader, &vector_env, &vector_home())
            .expect("hashable");
        assert_eq!(ours, PRE_IMAGE);
        assert_eq!(ours, writer_pre_image(&writer, &resolved));
        assert_eq!(
            compute_mcp_server_hash_with(&reader, &vector_env, &vector_home()).expect("hashable"),
            DIGEST
        );
        assert_eq!(compute_server_hash(&writer, &resolved), DIGEST);

        // The digest the writer used to stamp — `verbatim`, no resolvers — is a different one, and
        // that difference is the production bug both sides now avoid.
        assert_ne!(compute_server_hash(&writer, &ResolvedIdentity::verbatim(&writer)), DIGEST);
    }

    /// MCP-145 — the throw arm, on the two things that can throw, plus the two `cachedAt` rules and
    /// the `maxAgeMs` parameter.
    ///
    /// The messages are byte-exact against a node 22 run of `utils.ts` @ v2.26.1.
    #[test]
    fn an_unhashable_definition_is_never_cache_valid() {
        let unresolvable: ServerEntry =
            serde_json::from_str(r#"{ "url": "https://x.example/${NOPE}/mcp" }"#).expect("entry");
        assert_eq!(
            compute_mcp_server_hash_with(&unresolvable, &vector_env, &vector_home())
                .expect_err("must throw")
                .to_string(),
            "Missing environment variable in MCP server URL: NOPE"
        );
        // Two missing names pluralise and keep FIRST-OCCURRENCE order across the three forms, which
        // is why `missing_env_vars` is one scan and not three passes.
        let two: ServerEntry =
            serde_json::from_str(r#"{ "url": "https://x.example/{env:BEE}/${AYE}" }"#)
                .expect("entry");
        assert_eq!(
            compute_mcp_server_hash_with(&two, &vector_env, &vector_home())
                .expect_err("must throw")
                .to_string(),
            "Missing environment variables in MCP server URL: BEE, AYE"
        );
        let unparseable: ServerEntry =
            serde_json::from_str(r#"{ "url": "$env:HOST" }"#).expect("entry");
        assert_eq!(
            compute_mcp_server_hash_with(&unparseable, &vector_env, &vector_home())
                .expect_err("must throw")
                .to_string(),
            "Invalid MCP server URL after environment interpolation: a.example"
        );
        // A non-string `env` value is upstream's other throw — `value.startsWith is not a function`
        // — where this module used to silently drop the key.
        let non_string: ServerEntry =
            serde_json::from_str(r#"{ "command": "x", "env": { "N": 5 } }"#).expect("entry");
        assert_eq!(
            compute_mcp_server_hash_with(&non_string, &vector_env, &vector_home())
                .expect_err("must throw")
                .to_string(),
            "value.startsWith is not a function"
        );

        // …and every one of them makes the entry invalid rather than erroring out of the resolver.
        // The `configHash` is irrelevant: there is no hash to compare it against.
        for definition in [&unresolvable, &two, &unparseable, &non_string] {
            let entry = ServerCacheEntry {
                config_hash: Some("0".repeat(64)),
                cached_at: Some(now_ms()),
                ..ServerCacheEntry::default()
            };
            assert!(!is_server_cache_valid(&entry, definition));
            assert!(!is_server_cache_valid_with_age(&entry, definition, 0), "even with no TTL");
        }
    }

    /// `!entry.cachedAt` is falsy-testing a number, and `maxAgeMs > 0` gates the age check.
    #[test]
    fn cached_at_zero_is_rejected_and_max_age_zero_disables_the_age_check() {
        let definition: ServerEntry =
            serde_json::from_str(r#"{ "command": "x" }"#).expect("entry");
        let hash = compute_mcp_server_hash(&definition).expect("hashable");
        let with_stamp = |cached_at: Option<i64>| ServerCacheEntry {
            config_hash: Some(hash.clone()),
            cached_at,
            ..ServerCacheEntry::default()
        };

        assert!(!is_server_cache_valid(&with_stamp(Some(0)), &definition), "0 is falsy");
        assert!(!is_server_cache_valid(&with_stamp(None), &definition));
        assert!(is_server_cache_valid(&with_stamp(Some(now_ms())), &definition));

        let year = 365 * 24 * 60 * 60 * 1000;
        let ancient = with_stamp(Some(now_ms() - year));
        assert!(!is_server_cache_valid(&ancient, &definition));
        assert!(
            is_server_cache_valid_with_age(&ancient, &definition, 0),
            "`maxAgeMs = 0` disables the age check entirely"
        );

        // A JSON string `cachedAt` must cost this entry and nothing else — a plain `Option<i64>`
        // would have failed the whole file's parse.
        let cache: MetadataCache = serde_json::from_value(serde_json::json!({
            "version": 1,
            "servers": {
                "bad":  { "configHash": hash, "cachedAt": "1760000000000" },
                "good": { "configHash": hash, "cachedAt": now_ms() }
            }
        }))
        .expect("the file still parses");
        assert_eq!(cache.servers.len(), 2);
        assert!(!is_server_cache_valid(&cache.servers["bad"], &definition));
        assert!(is_server_cache_valid(&cache.servers["good"], &definition));
    }
}
