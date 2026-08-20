//! `<agent_dir>`, every adapter-owned path under it, and the `mcp-cache.json` metadata cache —
//! `agent-dir.ts` (13a §26, MCP-048), the path half of `config.ts` / `utils.ts` / `mcp-auth.ts`
//! (MCP-084, MCP-068), and `metadata-cache.ts` (13b §17, MCP-070, MCP-077).
//!
//! # Do not re-implement the resolution
//!
//! Upstream's `getAgentDir()` (`agent-dir.ts:10`) reads `PI_CODING_AGENT_DIR` (trimmed, with `~`
//! expansion) and defaults to `join(homedir(), ".pi", "agent")`. cyrup already resolves the same
//! concept, better: `cyrup_config::ConfigDirs::agent_dir` is a `PathBuf` **field** populated by
//! `ConfigDirs::resolve` from the CLI flag, then `$CYRUP_AGENT_DIR` / `$PI_CODING_AGENT_DIR`, then
//! `<home>/.cyrup/agent`. So [`McpDirs`] takes that resolved path as a constructor argument —
//! exactly the way `cyrup_ext_subagents::extension::subagent_extension_for_env` takes it — and adds
//! only the *filenames* hanging off it.
//!
//! # Why that matters more than it looks (MCP-048)
//!
//! Two in-tree consumers already bind to cyrup's agent dir: `cyrup_ext_subagents::exec::
//! mcp_direct_tools` reads `<agent_dir>/mcp-cache.json`, and `cyrup_permission_system::manager`
//! reads `<agent_dir>/mcp.json` **independently** as its global MCP config path. A `~/.pi/agent`
//! fallback living only inside `cyrup-mcp` would make the permission gate enumerate a different
//! (empty) MCP server set than the extension actually runs — permissions too permissive or too
//! strict, with no error anywhere. So this module reads `~/.cyrup/agent` **only**; the one-way move
//! from `~/.pi/agent` belongs where cyrup already handles migrations, not here. If that ruling is
//! ever reversed, the fallback must live in a shared resolver both crates call.
//!
//! # `getAppName` / `getAppClientUri`
//!
//! Upstream reads `$PI_PACKAGE_DIR/package.json`'s `piConfig.{name,clientUri}` so a rebranded
//! distribution names itself correctly in the OAuth dynamic-client-registration payload. cyrup has
//! no distribution manifest — `ConfigDirs` exposes a package *install root*, not a manifest — so
//! this is a compile-time constant pair. Recorded as a mechanism substitution, not a port.
//!
//! # `<home>` is an argument, never an ambient read
//!
//! Three of the six config sources are home-anchored (`~/.config/mcp/mcp.json`,
//! `~/.agents/mcp.json`, `~/.agents/mcp/mcp.json`) and `resolveConfigPath` expands `~`, but
//! [`McpDirs`] deliberately does **not** carry a `home` field: its constructor signature is fixed by
//! `crates/cyrup/src/main.rs`, and `cyrup_config::ConfigDirs::home` is already the one resolved home
//! in the process (`directories::BaseDirs` → `$HOME`). Every home-anchored path here therefore takes
//! `home: &Path`, which also keeps upstream's two *different* home sources distinguishable:
//! `agent-dir.ts` uses `homedir()` while `agent-plugin-loader.ts`'s `resolvePluginPath` anchors on
//! `process.env.HOME` (MCP-047 care point 4).
//!
//! # `mcp-cache.json` is a shared on-disk contract, and the pre-image is not JSON (§17, MCP-070)
//!
//! `cyrup-mcp` is the **writer** of a file that already has an in-tree **reader**:
//! `cyrup_ext_subagents::exec::mcp_direct_tools` (`load_metadata_cache`, `is_server_cache_valid`,
//! `compute_mcp_server_hash`, `CACHE_VERSION = 1`, `CACHE_MAX_AGE_MS = 7 days`). If the two disagree
//! about one byte of the `configHash` pre-image, every cached entry is rejected and the symptom
//! surfaces three subsystems away as "direct tools silently didn't appear" and as `mcp:` subagent
//! selectors resolving to nothing.
//!
//! [`stable_stringify`] is therefore a literal port of `metadata-cache.ts:344`, including the part
//! that looks like a bug and is not: its scalar branch is `JSON.stringify(value)`, and
//! `JSON.stringify(undefined)` returns the JS value `undefined`, so the fallback emits the **literal
//! nine-character string `undefined`** for an absent field — `"null"` only for an explicit JSON
//! `null`. The resulting pre-image is deliberately not valid JSON. That is why [`HashValue`] has a
//! first-class [`HashValue::Undefined`] variant instead of collapsing absence onto
//! `serde_json::Value::Null`: collapsing it is exactly the divergence MCP-070 exists to close.
//!
//! **Reciprocal change owed (MCP-070 option (b), MCP-094).** The existing reader inserts
//! `Value::Null` for absent fields, renders that as `"null"`, hashes **11** keys rather than the 13
//! below, and uses the **raw** `definition.url` instead of `resolveServerUrl`'s output. It must be
//! upgraded in the same change as the first cache write, or no `cyrup-mcp`-written entry will ever
//! validate. The golden vectors in this module's tests are the shared conformance fixture.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::ServerEntry;
use crate::errors::{McpError, McpResult};

/// `getAppName()`'s value for this distribution. Reaches the wire in the OAuth dynamic client
/// registration `client_name` and in the MCP `initialize` handshake's `clientInfo.name`.
pub const APP_NAME: &str = "cyrup";

/// `getAppClientUri()`'s value — the DCR `client_uri`.
pub const APP_CLIENT_URI: &str = "https://github.com/cyrup-ai/cyrup";

/// The global MCP config filename, in `<agent_dir>`. The **same** file
/// `cyrup_permission_system::manager::read_configured_mcp_server_names` reads.
pub const MCP_CONFIG_FILE: &str = "mcp.json";

/// The tool/resource/prompt metadata cache, in `<agent_dir>`. `cyrup-mcp` is the **writer** of a
/// file that already has a reader (`cyrup_ext_subagents::exec::mcp_direct_tools`), so its digests
/// must stay byte-identical to that reader's `compute_mcp_server_hash`.
pub const METADATA_CACHE_FILE: &str = "mcp-cache.json";

/// The onboarding-state file, in `<agent_dir>` — see [`crate::onboarding`].
pub const ONBOARDING_FILE: &str = "mcp-onboarding.json";

/// The per-plugin data directory root, in `<agent_dir>`. `agent-plugin-loader.ts` hands each plugin
/// `<agent_dir>/agent-plugin-data/<manifest.name>` as `${PLUGIN_DATA}`.
pub const AGENT_PLUGIN_DATA_DIR: &str = "agent-plugin-data";

/// The default OAuth storage root, in `<agent_dir>`. `getAuthBaseDir` prefers
/// `$MCP_OAUTH_DIR` (trimmed), then `settings.oauthDir`, then this.
pub const OAUTH_DIR: &str = "mcp-oauth";

/// The project-local directory the MCP trace writer defaults into: `<cwd>/.cyrup/mcp-traces`.
/// Upstream writes `.pi/mcp-traces`; the rename is the same one applied to every other
/// project-local path in the port.
pub const TRACE_DIR: &str = "mcp-traces";

/// The project-local config directory — upstream `getConfigDirName()` (`agent-dir.ts:5`), whose
/// default is `.pi` and whose cyrup value is `.cyrup`. Both `<cwd>/.cyrup/mcp.json` (config source
/// 6) and `<cwd>/.cyrup/mcp-traces` hang off it.
pub const PROJECT_CONFIG_DIR: &str = ".cyrup";

/// `PROJECT_CONFIG_NAME` (`config.ts:18`) — the *shared*, non-pi-branded project config, read from
/// `<cwd>/.mcp.json`. The leading dot is part of the filename, not the directory.
pub const PROJECT_SHARED_CONFIG_FILE: &str = ".mcp.json";

/// `$MCP_OAUTH_DIR` — read by `getAuthBaseDir` (`mcp-auth.ts:417`) and, unlike the `PI_*` names,
/// **not** pi-branded, so it is preserved verbatim (13b §16, MCP-068).
pub const MCP_OAUTH_DIR_VAR: &str = "MCP_OAUTH_DIR";

/// The resolved directory layout for one MCP runtime: `<agent_dir>` plus the session `cwd` that
/// project-scoped paths (`.cyrup/mcp-traces`, `.mcp.json`, relative `settings.oauthDir`) resolve
/// against.
///
/// Cheap to clone; holds no handles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpDirs {
    agent_dir: PathBuf,
    cwd: PathBuf,
}

impl McpDirs {
    /// Build from an already-resolved `cyrup_config::ConfigDirs::agent_dir` and the session's
    /// working directory. Nothing here re-reads the environment — the caller
    /// (`crates/cyrup/src/main.rs`) has already resolved both.
    #[must_use]
    pub fn new(agent_dir: PathBuf, cwd: PathBuf) -> Self {
        Self { agent_dir, cwd }
    }

    /// `<agent_dir>`.
    #[must_use]
    pub fn agent_dir(&self) -> &Path {
        &self.agent_dir
    }

    /// The session's working directory — `resolveConfigPath`'s base and `loadMcpConfig`'s
    /// project-scope anchor.
    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// `getAgentPath(...segments)` — one segment at a time, which is every real call site.
    #[must_use]
    pub fn agent_path(&self, segment: &str) -> PathBuf {
        self.agent_dir.join(segment)
    }

    /// `<agent_dir>/mcp.json`.
    #[must_use]
    pub fn global_config(&self) -> PathBuf {
        self.agent_path(MCP_CONFIG_FILE)
    }

    /// `<agent_dir>/mcp-cache.json`.
    #[must_use]
    pub fn metadata_cache(&self) -> PathBuf {
        self.agent_path(METADATA_CACHE_FILE)
    }

    /// `<agent_dir>/mcp-onboarding.json`.
    #[must_use]
    pub fn onboarding_state(&self) -> PathBuf {
        self.agent_path(ONBOARDING_FILE)
    }

    /// `<agent_dir>/agent-plugin-data/<plugin>` — the `${PLUGIN_DATA}` a plugin's `cwd` and `env`
    /// may reference and nothing else may escape.
    #[must_use]
    pub fn agent_plugin_data(&self, plugin: &str) -> PathBuf {
        self.agent_path(AGENT_PLUGIN_DATA_DIR).join(plugin)
    }

    /// `<agent_dir>/mcp-oauth` — the last tier of `getAuthBaseDir`.
    #[must_use]
    pub fn default_oauth_dir(&self) -> PathBuf {
        self.agent_path(OAUTH_DIR)
    }

    /// `<cwd>/.cyrup/mcp-traces` — the trace writer's default destination.
    #[must_use]
    pub fn trace_dir(&self) -> PathBuf {
        self.cwd.join(PROJECT_CONFIG_DIR).join(TRACE_DIR)
    }

    /// `getPiGlobalConfigPath(overridePath)` (`config.ts:168`) — config source **4**, the one
    /// source that is always emitted and that every `import`-kind source writes *back* to.
    ///
    /// `Some(p)` is `resolve(overridePath)`: a `--mcp-config` value is absolutised against the
    /// session cwd, so `--mcp-config mcp.json` names the file in the project, not one in
    /// `<agent_dir>`. `None` is `getAgentPath("mcp.json")`.
    #[must_use]
    pub fn user_config(&self, override_path: Option<&str>) -> PathBuf {
        match override_path {
            Some(raw) => resolve_from(&self.cwd, raw),
            None => self.global_config(),
        }
    }

    /// `getProjectConfigPath(cwd)` (`config.ts:175`) — config source **5**, `<cwd>/.mcp.json`, the
    /// `shared` project config every MCP-aware editor understands.
    #[must_use]
    pub fn project_shared_config(&self) -> PathBuf {
        resolve_from(&self.cwd, PROJECT_SHARED_CONFIG_FILE)
    }

    /// `getProjectPiConfigPath(cwd)` (`config.ts:180`) — config source **6**,
    /// `<cwd>/.cyrup/mcp.json`, the cyrup-owned project override that outranks every other source.
    #[must_use]
    pub fn project_agent_config(&self) -> PathBuf {
        resolve_from(&self.cwd, PROJECT_CONFIG_DIR).join(MCP_CONFIG_FILE)
    }
}

/// `GENERIC_GLOBAL_CONFIG_PATH` (`config.ts:13`) — config source **1**, `~/.config/mcp/mcp.json`,
/// the cross-tool standard location. Emitted only when it differs from
/// [`McpDirs::user_config`]'s path.
#[must_use]
pub fn shared_global_config(home: &Path) -> PathBuf {
    home.join(".config").join("mcp").join(MCP_CONFIG_FILE)
}

/// `AGENTS_GLOBAL_CONFIG_PATHS` (`config.ts:14`) — config sources **2** and **3**, in that order:
/// `~/.agents/mcp.json` then `~/.agents/mcp/mcp.json`. Both are `shared` and both write back to
/// [`McpDirs::user_config`].
#[must_use]
pub fn agents_global_configs(home: &Path) -> [PathBuf; 2] {
    [
        home.join(".agents").join(MCP_CONFIG_FILE),
        home.join(".agents").join("mcp").join(MCP_CONFIG_FILE),
    ]
}

/// `agent-dir.ts`'s `~` expansion, reproduced exactly because it is *not* the same as a shell's:
/// a bare `"~"` is the home directory itself, `"~/…"` resolves the remainder against home, and
/// **anything else — including `~user`** — is taken literally and merely made absolute.
///
/// `home` is supplied by the caller rather than read here so the two upstream home sources stay
/// distinguishable: `agent-dir.ts` uses `homedir()` while `agent-plugin-loader.ts`'s
/// `resolvePluginPath` anchors on `process.env.HOME` (MCP-047 care point 4).
#[must_use]
pub fn expand_tilde(raw: &str, home: &Path) -> PathBuf {
    let trimmed = raw.trim();
    if trimmed == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return home.join(rest);
    }
    PathBuf::from(trimmed)
}

/// The tail of `resolveConfigPath(value)` (`utils.ts:187`) — the `~` expansion that runs **after**
/// interpolation, applied to `cwd` (and, before Cut 3, to `socket`).
///
/// Two differences from [`expand_tilde`], both real and both upstream: this form accepts the
/// Windows `~\` prefix as well as `~/`, and it does **not** trim — `agent-dir.ts` trims because the
/// value came from `process.env[…]?.trim()`, whereas a config value is used as written.
///
/// The interpolation half is [`crate::config`]'s (MCP-082): the three syntaxes `${VAR}`, `$VAR` and
/// `{env:VAR}` are one implementation for the whole crate, and duplicating a two-syntax copy here
/// is how `cyrup_ext_subagents::exec::mcp_direct_tools` came to disagree with the adapter.
#[must_use]
pub fn resolve_config_path_tail(interpolated: &str, home: &Path) -> PathBuf {
    if interpolated == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) =
        interpolated.strip_prefix("~/").or_else(|| interpolated.strip_prefix("~\\"))
    {
        return home.join(rest);
    }
    PathBuf::from(interpolated)
}

/// node's `path.resolve(base, raw)` — join when `raw` is relative, then fold `.` and `..`
/// **lexically**, never touching the filesystem and never following a symlink.
///
/// Upstream reaches for `resolve()` at four path sites this module owns: `--mcp-config`
/// (`config.ts:169`), both project configs (`config.ts:176`, `config.ts:181`) and
/// `settings.oauthDir` (`config.ts:1243`). It is deliberately **not**
/// `cyrup_config::paths::resolve_path_from_base`, which additionally expands `~` and `file://`:
/// node's `resolve` does neither, so an `oauthDir: "~/creds"` upstream yields a directory literally
/// named `~` under the cwd, and a port that silently "fixes" that writes the user's tokens
/// somewhere their config does not name.
///
/// `base` is absolute at every call site (it is the session cwd). A relative `base` yields a
/// relative result rather than node's `process.cwd()` prepend, because this crate resolves cwd once,
/// at construction, and never re-reads it.
#[must_use]
pub fn resolve_from(base: &Path, raw: &str) -> PathBuf {
    let candidate = Path::new(raw);
    let joined =
        if candidate.is_absolute() { candidate.to_path_buf() } else { base.join(candidate) };
    normalize_lexically(&joined)
}

/// The `.` / `..` fold node's `path.resolve` performs after joining. `..` at the root is dropped
/// rather than escaping it (`path.resolve("/a/../..") === "/"`); on a still-relative remainder it is
/// kept, because there is nothing to cancel it against.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut stack: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match stack.last() {
                Some(Component::Normal(_)) => {
                    stack.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => stack.push(component),
            },
            other => stack.push(other),
        }
    }
    let mut out = PathBuf::new();
    for component in stack {
        out.push(component.as_os_str());
    }
    if out.as_os_str().is_empty() { PathBuf::from(".") } else { out }
}

/// `resolveConfiguredOAuthDir(raw, cwd)` (`config.ts:1235`) — the `settings.oauthDir` half of the
/// OAuth storage root.
///
/// Blank after `.trim()` is `undefined`, not `<cwd>`; anything else is `resolve(cwd, trimmed)`, so a
/// relative `oauthDir` is project-scoped. Upstream's two other arms are absorbed by the type system:
/// `undefined`/`null` cannot reach here, and the non-string arm's
/// `settings.oauthDir must be a string` throw is the deserialiser's job (MCP-066).
#[must_use]
pub fn resolve_configured_oauth_dir(raw: &str, cwd: &Path) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(resolve_from(cwd, trimmed))
}

/// `getAuthBaseDir(options)` (`mcp-auth.ts:416`) — the three-tier OAuth storage root:
/// `$MCP_OAUTH_DIR` (trimmed, non-empty) outranks the configured
/// [`resolve_configured_oauth_dir`] value, which outranks `<agent_dir>/mcp-oauth`.
///
/// The env override is used **verbatim** — not absolutised and not `~`-expanded — because upstream
/// applies nothing but `.trim()` to it. `env` is injected rather than read here: edition 2024 makes
/// `std::env::set_var` `unsafe`, so a test that pinned the variable could not undo it; production
/// passes `&|key| std::env::var(key).ok()` (MCP-068).
#[must_use]
pub fn resolve_auth_base_dir(
    dirs: &McpDirs,
    configured: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
) -> PathBuf {
    if let Some(raw) = env(MCP_OAUTH_DIR_VAR) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    configured.map_or_else(|| dirs.default_oauth_dir(), Path::to_path_buf)
}

// -------------------------------------------------------------------------------------------
// `mcp-cache.json` — the metadata cache (`metadata-cache.ts`; MCP-070, MCP-077)
// -------------------------------------------------------------------------------------------

/// `CACHE_VERSION` (`metadata-cache.ts:34`). A file declaring any other version is treated as
/// **absent**, by the writer here and by `mcp_direct_tools::load_metadata_cache` alike.
///
/// Do **not** bump this to drop the now-dead `uiResourceUri` / `uiVisibility` / `uiStreamMode`
/// fields that Cut 2 orphaned: the schema is a live cross-crate contract, and the cost of a bump is
/// every user's direct tools disappearing for one session. They stay absent and ignored (MCP-077).
pub const CACHE_VERSION: u32 = 1;

/// `CACHE_MAX_AGE_MS` (`metadata-cache.ts:35`) — 7 days. An entry older than this is stale even
/// when its `configHash` still matches, because a server's tool list can change without its
/// definition changing.
pub const CACHE_MAX_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// One cached tool descriptor — `types.ts:617` `CachedTool`.
///
/// `description` and `inputSchema` are what let [`crate::registration`] register a direct tool with
/// its real schema before any server is contacted. The three `ui*` fields are Cut 2 casualties kept
/// in the on-disk schema: a pi-written cache carries them and must round-trip, and a
/// `cyrup-mcp`-written cache simply omits them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedTool {
    /// The server-side tool name, unprefixed. `serializeTools` drops any tool without one.
    pub name: String,
    /// The tool description, verbatim from `tools/list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The raw JSON Schema, stored unnormalised — `normalizeDirectToolInputSchema` runs at
    /// registration, not at cache time (MCP-087).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    /// **Cut 2**, retained in the schema. MCP Apps' `ui://` resource for this tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_resource_uri: Option<String>,
    /// **Cut 2**, retained in the schema. `UiToolVisibility[]` — an array upstream, not a scalar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_visibility: Option<Vec<String>>,
    /// **Cut 2**, retained in the schema. `"eager" | "stream-first"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_stream_mode: Option<String>,
}

/// One cached resource descriptor — `types.ts:626` `CachedResource`. Each becomes a `read_<name>`
/// direct tool unless the server sets `exposeResources: false`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedResource {
    /// The resource URI — the `read_*` tool's payload. `serializeResources` drops a resource
    /// missing either this or `name`.
    pub uri: String,
    /// The resource name, which `resourceNameToToolName` sanitises into the tool name.
    pub name: String,
    /// Falls back to `` `Read resource: ${uri}` `` at reconstruction time, not here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One argument of a cached prompt — the inline object type in `types.ts:636`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedPromptArgument {
    /// The argument name. An argument without one is dropped by `serializePrompts`.
    pub name: String,
    /// Shown in the slash command's help.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether the prompt refuses to render without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// One cached prompt descriptor — `types.ts:632` `CachedPrompt`. Each becomes a slash command.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedPrompt {
    /// The server-side prompt name, which `formatPromptCommandName` turns into the command.
    pub name: String,
    /// The human-facing title, if the server sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Defaults to `""` at reconstruction, not here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Absent and empty are **different** on the wire: `serializePrompts` emits the key only when
    /// `prompt.arguments` is an array, so a server that sends no `arguments` at all round-trips as
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<CachedPromptArgument>>,
}

/// One server's cached metadata — `types.ts:639` `ServerCacheEntry`.
///
/// `tools` and `resources` are required (`[]` when empty); `prompts` and `instructions` are
/// optional, and the reader in `cyrup-ext-subagents` models neither — it has no
/// `deny_unknown_fields`, so they round-trip harmlessly (MCP-077).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCacheEntry {
    /// [`compute_server_hash`] over the server definition this metadata was captured from. The
    /// entry is discarded the moment it stops matching.
    pub config_hash: String,
    /// Every tool the server advertised, unfiltered — `includeTools`/`excludeTools` are applied at
    /// reconstruction so a config edit does not require a reconnect.
    pub tools: Vec<CachedTool>,
    /// Every resource the server advertised.
    pub resources: Vec<CachedResource>,
    /// Every prompt the server advertised, when the server implements `prompts/list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts: Option<Vec<CachedPrompt>>,
    /// The `initialize` result's `instructions`, replayed into the system prompt without a
    /// connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// `Date.now()` at capture, in **milliseconds**. `0` is treated as absent — upstream's guard is
    /// the falsy test `!entry.cachedAt`, which rejects `0` as well as `undefined`.
    #[serde(default)]
    pub cached_at: i64,
}

/// The whole on-disk cache — `types.ts:648` `MetadataCache`.
///
/// `servers` is an [`IndexMap`] for the same reason `McpConfig::mcp_servers` is: a `BTreeMap` would
/// re-sort the file on every write, producing a diff for every user on the first save and
/// destroying the correspondence with `mcp.json`'s own order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataCache {
    /// Always [`CACHE_VERSION`] on write; any other value makes the file absent on read.
    pub version: u32,
    /// Server name → its cached metadata, in file order.
    pub servers: IndexMap<String, ServerCacheEntry>,
}

impl Default for MetadataCache {
    fn default() -> Self {
        Self { version: CACHE_VERSION, servers: IndexMap::new() }
    }
}

/// `loadMetadataCache()` (`metadata-cache.ts:43`) — read `<agent_dir>/mcp-cache.json`, or `None`.
///
/// Every failure mode collapses to `None`, exactly as upstream's `try { … } catch { return null }`
/// does: a missing file, unreadable bytes, malformed JSON, a version mismatch, a missing `servers`
/// object. This runs on the synchronous registration path, so it is `std::fs`, never `tokio::fs` —
/// nothing may block a session build on the reactor (MCP-003).
#[must_use]
pub fn load_metadata_cache(path: &Path) -> Option<MetadataCache> {
    let text = std::fs::read_to_string(path).ok()?;
    let parsed: MetadataCache = serde_json::from_str(&text).ok()?;
    if parsed.version != CACHE_VERSION {
        return None;
    }
    Some(parsed)
}

/// `saveMetadataCache(cache)` (`metadata-cache.ts:57`) — merge over whatever is on disk, then write
/// atomically.
///
/// The merge is `{ ...existing.servers, ...cache.servers }`: a caller that connected one server
/// must not erase the other twelve entries. [`IndexMap`]'s `insert` keeps an existing key at its
/// existing position while replacing its value, and appends genuinely new keys — which is precisely
/// JS object-spread order. An unreadable or version-mismatched existing file is silently replaced,
/// not merged.
///
/// The write is `<path>.<pid>.tmp` + `rename`, upstream's `writeFileSync` + `renameSync`, so a crash
/// mid-write can never leave a truncated cache that the reader would then reject. **No lock** —
/// upstream takes none, and adding one here would change the concurrency contract with the reader
/// (MCP-061's ruling, applied to the second file this crate owns).
///
/// The body is `JSON.stringify(merged, null, 2)` with **no** trailing newline. `mcp.json` gets one
/// (`writeRawConfigObject` appends `"\n"`); this file does not, and the asymmetry is upstream's.
pub fn save_metadata_cache(path: &Path, cache: &MetadataCache) -> McpResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| McpError::Io { path: parent.to_path_buf(), source })?;
    }

    let mut merged = load_metadata_cache(path).unwrap_or_default();
    merged.version = CACHE_VERSION;
    for (name, entry) in &cache.servers {
        merged.servers.insert(name.clone(), entry.clone());
    }

    let body = serde_json::to_string_pretty(&merged)
        .map_err(|e| McpError::Config(format!("serialising MCP metadata cache: {e}")))?;

    let mut temp = path.as_os_str().to_os_string();
    temp.push(format!(".{}.tmp", std::process::id()));
    let temp = PathBuf::from(temp);

    std::fs::write(&temp, body).map_err(|source| McpError::Io { path: temp.clone(), source })?;
    std::fs::rename(&temp, path).map_err(|source| {
        // A failed rename leaves the temp file behind; upstream's `renameSync` throw does too, but
        // there is no reason to keep it.
        let _ = std::fs::remove_file(&temp);
        McpError::Io { path: path.to_path_buf(), source }
    })
}

/// `isServerCacheValid(entry, definition, maxAgeMs)` (`metadata-cache.ts:114`), with the digest
/// passed in rather than recomputed.
///
/// Upstream computes the hash inside a `try` and returns `false` when it **throws** — which it can,
/// because `resolveServerUrl` throws on a missing interpolation variable or an unparseable URL. The
/// split is deliberate: the caller owns that fallible resolution (MCP-084) and maps its error onto
/// `false`, so this function stays total.
///
/// Three rejections, in upstream's order: a hash mismatch; a falsy `cachedAt` (which is `0` as well
/// as absent — `!entry.cachedAt`); and an age over `max_age_ms`, checked only when that limit is
/// positive, so `0` disables the age check entirely.
#[must_use]
pub fn is_server_cache_valid(entry: &ServerCacheEntry, config_hash: &str, max_age_ms: i64) -> bool {
    is_server_cache_valid_at(entry, config_hash, max_age_ms, now_ms())
}

/// [`is_server_cache_valid`] against an injected `Date.now()`, so the 7-day boundary is testable
/// without sleeping or mutating the clock.
#[must_use]
pub fn is_server_cache_valid_at(
    entry: &ServerCacheEntry,
    config_hash: &str,
    max_age_ms: i64,
    now_ms: i64,
) -> bool {
    if entry.config_hash != config_hash {
        return false;
    }
    if entry.cached_at == 0 {
        return false;
    }
    if max_age_ms > 0 && now_ms.saturating_sub(entry.cached_at) > max_age_ms {
        return false;
    }
    true
}

/// `Date.now()` — milliseconds since the Unix epoch, saturating rather than panicking on a clock
/// before 1970.
#[must_use]
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// The five identity fields `computeServerHash` does **not** read straight off the definition:
/// each one is passed through a resolver first, and the resolved value — not the config text — is
/// what the digest covers.
///
/// | field | upstream resolver | unit |
/// |---|---|---|
/// | `env` | `interpolateEnvRecord` (`utils.ts:107`) → `interpolateSecretExpression` per value | MCP-082/083 |
/// | `cwd` | `resolveConfigPath` (`utils.ts:187`) | MCP-084 |
/// | `url` | `resolveServerUrl` (`utils.ts:167`) — **throws** on a missing var or a bad URL | MCP-084 |
/// | `headers` | `interpolateEnvRecord` | MCP-082/083 |
/// | `bearerToken` | `resolveBearerToken` (`utils.ts`) — `interpolateSecretExpression`, then `$bearerTokenEnv` | MCP-084 |
///
/// Why they arrive resolved rather than being resolved here: the digest must change when
/// `$API_HOST` changes, even though the config text did not — that is the entire point of hashing
/// the resolved form. And `resolveBearerToken` uses `interpolateSecretExpression`, which unescapes
/// `!!x` to `!x` **without spawning anything**; a hash-site copy that used plain interpolation would
/// hash `"!!x"` and disagree with the connect path forever. That is exactly the bug
/// `cyrup_ext_subagents::exec::mcp_direct_tools::resolve_bearer_token` has today (13b §6).
///
/// **No resolver is reached at hash time.** `resolveCommandSecret` — the `!`-prefixed form that
/// *does* spawn a process — is reached only at connect/auth time, never during discovery, merge,
/// preview, hashing or rendering. Keeping that timing is MCP-083's security property.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedIdentity {
    /// `interpolateEnvRecord(definition.env)`.
    pub env: Option<BTreeMap<String, String>>,
    /// `resolveConfigPath(definition.cwd)`.
    pub cwd: Option<String>,
    /// `resolveServerUrl(definition)`.
    pub url: Option<String>,
    /// `interpolateEnvRecord(definition.headers)`.
    pub headers: Option<BTreeMap<String, String>>,
    /// `resolveBearerToken(definition)`.
    pub bearer_token: Option<String>,
}

impl ResolvedIdentity {
    /// The five fields copied **verbatim** off the definition, with no resolver applied.
    ///
    /// Correct only for a definition in which none of them contains an interpolation token
    /// (`${VAR}`, `$VAR`, `{env:VAR}`), a `~`, or a `!`/`!!` secret marker — and wrong, silently,
    /// for any that does. It exists so the cache writer and these tests have a call site that
    /// compiles before MCP-082 and MCP-084 land; every production caller must replace it with the
    /// real resolvers. TODO(MCP-082, MCP-084): delete this constructor once they exist.
    #[must_use]
    pub fn verbatim(entry: &ServerEntry) -> Self {
        Self {
            env: entry.env.clone(),
            cwd: entry.cwd.clone(),
            url: entry.url.clone(),
            headers: entry.headers.clone(),
            bearer_token: entry.bearer_token.clone(),
        }
    }
}

/// One node of `stableStringify`'s input (`metadata-cache.ts:344`).
///
/// This is not `serde_json::Value` and must not become it: JS distinguishes an **absent** property
/// from one explicitly set to `null`, `stableStringify` renders the first as the literal
/// `undefined` and the second as `null`, and the digest differs. [`Undefined`](Self::Undefined) is
/// that distinction made expressible in a language that has no `undefined`.
#[derive(Debug, Clone, PartialEq)]
pub enum HashValue {
    /// JS `undefined` — an absent property. Renders as the nine characters `undefined`.
    Undefined,
    /// JS `null` — a property explicitly set to null. Renders as `null`.
    Null,
    /// A boolean.
    Bool(bool),
    /// A number. `f64` is the faithful model: every number in the pre-image arrived through
    /// `JSON.parse`, which produces IEEE-754 doubles, and Rust's `f64` `Display` is the same
    /// shortest-round-trip form `String(n)` produces. It diverges only at |n| ≥ 1e21, where JS
    /// switches to exponential notation — unreachable in this pre-image, which carries no numbers
    /// at all post-Cut-3.
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
    #[must_use]
    pub fn from_json(value: Value) -> Self {
        match value {
            Value::Null => HashValue::Null,
            Value::Bool(b) => HashValue::Bool(b),
            Value::Number(n) => n.as_f64().map_or(HashValue::Null, HashValue::Number),
            Value::String(s) => HashValue::String(s),
            Value::Array(items) => {
                HashValue::Array(items.into_iter().map(HashValue::from_json).collect())
            }
            Value::Object(map) => {
                HashValue::Object(map.into_iter().map(|(k, v)| (k, HashValue::from_json(v))).collect())
            }
        }
    }

    /// `undefined` for `None`, [`from_json`](Self::from_json) for `Some` — the shape every optional
    /// identity field takes.
    #[must_use]
    pub fn from_optional_json(value: Option<Value>) -> Self {
        value.map_or(HashValue::Undefined, HashValue::from_json)
    }
}

/// `stableStringify(value)` (`metadata-cache.ts:344`) — the deterministic byte sequence the config
/// digest is taken over.
///
/// Deliberately **not valid JSON**: an absent property renders as the bare word `undefined`.
/// Object keys are sorted (`Object.keys(obj).sort()`); arrays keep their order. Strings go through
/// `serde_json`, whose escaping matches `JSON.stringify` byte for byte for every `str` — same short
/// forms for `\b \t \n \f \r`, same `\u00XX` for the other C0 controls, no escaping of `/` or of
/// non-ASCII, and Rust's `String` cannot hold the lone surrogate that is JS's only other case.
///
/// The key sort is byte-wise UTF-8 where JS sorts by UTF-16 code unit. The two agree on every
/// ASCII key — which is every key in the identity object — and can differ only between a
/// `U+E000..U+FFFF` key and an astral-plane one inside a user's `env` or `headers` map.
#[must_use]
pub fn stable_stringify(value: &HashValue) -> String {
    match value {
        HashValue::Undefined => "undefined".to_string(),
        HashValue::Null => "null".to_string(),
        HashValue::Bool(true) => "true".to_string(),
        HashValue::Bool(false) => "false".to_string(),
        HashValue::Number(n) => {
            // `JSON.stringify(NaN) === "null"`, and likewise for ±Infinity.
            if n.is_finite() { format!("{n}") } else { "null".to_string() }
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

/// `computeServerHash`'s identity object (`metadata-cache.ts:82`), post-Cut-3.
///
/// **Thirteen fields, and the list is the specification.** Upstream builds fourteen; `socket` is
/// gone with Cut 3 (a config that could carry it no longer exists in the port, so no cyrup hash
/// needs to encode it) and that is a *recorded* divergence from the v2.25.0 pre-image, not an
/// oversight. Everything else is verbatim, including the fields that are **not** here: `lifecycle`,
/// `idleTimeout`, `requestTimeoutMs` and `debug` are runtime behaviour, they do not change which
/// tools a server exposes, and hashing them would evict every cache entry on an unrelated edit.
///
/// Upstream v2.26.0 adds a fifteenth field, `requestHeadersCommand`. The plan is pinned at v2.25.0
/// and neither [`ServerEntry`] nor this pre-image carries it; adding it later is an
/// on-disk-contract change that must land in the reader at the same time.
///
/// Returned as the pre-image rather than the digest so a conformance test can assert the bytes —
/// a hash mismatch tells you nothing about *which* field disagreed.
#[must_use]
pub fn server_identity_pre_image(entry: &ServerEntry, resolved: &ResolvedIdentity) -> String {
    let identity = HashValue::Object(vec![
        ("command".to_string(), opt_string(entry.command.as_deref())),
        ("args".to_string(), opt_string_list(entry.args.as_ref())),
        ("env".to_string(), opt_string_map(resolved.env.as_ref())),
        ("cwd".to_string(), opt_string(resolved.cwd.as_deref())),
        ("url".to_string(), opt_string(resolved.url.as_deref())),
        ("headers".to_string(), opt_string_map(resolved.headers.as_ref())),
        ("auth".to_string(), opt_serde(entry.auth.as_ref())),
        ("protocolVersion".to_string(), opt_serde(entry.protocol_version.as_ref())),
        ("bearerToken".to_string(), opt_string(resolved.bearer_token.as_deref())),
        ("bearerTokenEnv".to_string(), opt_string(entry.bearer_token_env.as_deref())),
        (
            "exposeResources".to_string(),
            entry.expose_resources.map_or(HashValue::Undefined, HashValue::Bool),
        ),
        ("includeTools".to_string(), opt_string_list(entry.include_tools.as_ref())),
        ("excludeTools".to_string(), opt_string_list(entry.exclude_tools.as_ref())),
    ]);
    stable_stringify(&identity)
}

/// `computeServerHash(definition)` (`metadata-cache.ts:82`) — the 64-hex `configHash` stamped into
/// every [`ServerCacheEntry`] and re-checked on every read, here and in
/// `cyrup_ext_subagents::exec::mcp_direct_tools`.
#[must_use]
pub fn compute_server_hash(entry: &ServerEntry, resolved: &ResolvedIdentity) -> String {
    hex_sha256(server_identity_pre_image(entry, resolved).as_bytes())
}

/// `createHash("sha256").update(bytes).digest("hex")`.
fn hex_sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // Writing to a `String` is infallible; the result is discarded rather than unwrapped.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// `undefined` for absent, a JSON string otherwise.
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

/// `undefined` for absent, an object otherwise. [`BTreeMap`] iterates in key order, which is the
/// order [`stable_stringify`] would impose anyway.
fn opt_string_map(value: Option<&BTreeMap<String, String>>) -> HashValue {
    value.map_or(HashValue::Undefined, |map| {
        HashValue::Object(
            map.iter().map(|(k, v)| (k.clone(), HashValue::String(v.clone()))).collect(),
        )
    })
}

/// `undefined` for absent, the value's JSON form otherwise — used for the two identity fields whose
/// wire form is an enum: `auth` (`"oauth"` / `"bearer"` / `false`) and `protocolVersion`
/// (`"legacy"` / `"auto"` / `"2026-07-28"`). Serialising either cannot fail; the fallback keeps the
/// function total.
fn opt_serde<T: Serialize>(value: Option<&T>) -> HashValue {
    value.map_or(HashValue::Undefined, |v| {
        serde_json::to_value(v).map_or(HashValue::Undefined, HashValue::from_json)
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn tilde_forms_match_agent_dir_ts() {
        let home = Path::new("/home/u");
        assert_eq!(expand_tilde("~", home), PathBuf::from("/home/u"));
        assert_eq!(expand_tilde("~/x/y", home), PathBuf::from("/home/u/x/y"));
        // `~other` is NOT a home reference upstream — `configured.startsWith("~/")` is the only
        // prefix tested, so this stays literal.
        assert_eq!(expand_tilde("~other/x", home), PathBuf::from("~other/x"));
        assert_eq!(expand_tilde("  /abs/p  ", home), PathBuf::from("/abs/p"));
    }

    #[test]
    fn config_path_tilde_takes_the_windows_form_and_does_not_trim() {
        let home = Path::new("/home/u");
        assert_eq!(resolve_config_path_tail("~", home), PathBuf::from("/home/u"));
        assert_eq!(resolve_config_path_tail("~/x", home), PathBuf::from("/home/u/x"));
        // `utils.ts:192` tests BOTH prefixes; `agent-dir.ts` tests only `~/`.
        assert_eq!(resolve_config_path_tail("~\\x", home), PathBuf::from("/home/u/x"));
        // …and it never trims, because a config value is used as written.
        assert_eq!(resolve_config_path_tail(" ~/x", home), PathBuf::from(" ~/x"));
    }

    #[test]
    fn adapter_owned_paths_hang_off_agent_dir() {
        let dirs = McpDirs::new(PathBuf::from("/a/agent"), PathBuf::from("/w"));
        assert_eq!(dirs.global_config(), PathBuf::from("/a/agent/mcp.json"));
        assert_eq!(dirs.metadata_cache(), PathBuf::from("/a/agent/mcp-cache.json"));
        assert_eq!(dirs.onboarding_state(), PathBuf::from("/a/agent/mcp-onboarding.json"));
        assert_eq!(dirs.agent_plugin_data("p"), PathBuf::from("/a/agent/agent-plugin-data/p"));
        assert_eq!(dirs.default_oauth_dir(), PathBuf::from("/a/agent/mcp-oauth"));
        assert_eq!(dirs.trace_dir(), PathBuf::from("/w/.cyrup/mcp-traces"));
    }

    #[test]
    fn ladder_paths_match_config_ts() {
        let dirs = McpDirs::new(PathBuf::from("/a/agent"), PathBuf::from("/w"));
        let home = Path::new("/home/u");
        assert_eq!(shared_global_config(home), PathBuf::from("/home/u/.config/mcp/mcp.json"));
        assert_eq!(
            agents_global_configs(home),
            [
                PathBuf::from("/home/u/.agents/mcp.json"),
                PathBuf::from("/home/u/.agents/mcp/mcp.json"),
            ]
        );
        assert_eq!(dirs.project_shared_config(), PathBuf::from("/w/.mcp.json"));
        assert_eq!(dirs.project_agent_config(), PathBuf::from("/w/.cyrup/mcp.json"));
        // `getPiGlobalConfigPath(undefined)` vs `resolve(overridePath)`.
        assert_eq!(dirs.user_config(None), PathBuf::from("/a/agent/mcp.json"));
        assert_eq!(dirs.user_config(Some("mcp.json")), PathBuf::from("/w/mcp.json"));
        assert_eq!(dirs.user_config(Some("/etc/mcp.json")), PathBuf::from("/etc/mcp.json"));
    }

    #[test]
    fn resolve_from_folds_dot_segments_lexically() {
        let base = Path::new("/w/project");
        assert_eq!(resolve_from(base, "a/../b"), PathBuf::from("/w/project/b"));
        assert_eq!(resolve_from(base, "./a/./b/"), PathBuf::from("/w/project/a/b"));
        assert_eq!(resolve_from(base, "../sibling"), PathBuf::from("/w/sibling"));
        // `path.resolve("/a/../..") === "/"` — `..` cannot escape the root.
        assert_eq!(resolve_from(Path::new("/a"), "../.."), PathBuf::from("/"));
        // node's resolve does NOT expand `~`; a port that "helpfully" did would relocate secrets.
        assert_eq!(resolve_from(base, "~/creds"), PathBuf::from("/w/project/~/creds"));
    }

    #[test]
    fn oauth_dir_precedence_matches_get_auth_base_dir() {
        let dirs = McpDirs::new(PathBuf::from("/a/agent"), PathBuf::from("/w"));
        let none = |_: &str| None;
        let set = |_: &str| Some("  /env/oauth  ".to_string());
        let blank = |_: &str| Some("   ".to_string());

        // Tier 3: the default.
        assert_eq!(resolve_auth_base_dir(&dirs, None, &none), PathBuf::from("/a/agent/mcp-oauth"));
        // Tier 2: `settings.oauthDir`, resolved against cwd.
        let configured = resolve_configured_oauth_dir(" creds ", dirs.cwd()).unwrap();
        assert_eq!(configured, PathBuf::from("/w/creds"));
        assert_eq!(resolve_auth_base_dir(&dirs, Some(&configured), &none), configured);
        // Tier 1: the env var outranks both, trimmed but otherwise verbatim.
        assert_eq!(
            resolve_auth_base_dir(&dirs, Some(&configured), &set),
            PathBuf::from("/env/oauth")
        );
        // A whitespace-only override is not an override.
        assert_eq!(resolve_auth_base_dir(&dirs, Some(&configured), &blank), configured);
        // Blank after trim is `undefined`, not the cwd.
        assert_eq!(resolve_configured_oauth_dir("   ", dirs.cwd()), None);
    }

    /// `stableStringify`'s scalar branch, cross-checked against node 22 running the upstream
    /// function verbatim.
    #[test]
    fn stable_stringify_emits_undefined_for_absent() {
        assert_eq!(stable_stringify(&HashValue::Undefined), "undefined");
        assert_eq!(stable_stringify(&HashValue::Null), "null");
        assert_eq!(stable_stringify(&HashValue::Bool(false)), "false");
        assert_eq!(stable_stringify(&HashValue::String("a\"b\n".to_string())), "\"a\\\"b\\n\"");
        assert_eq!(
            stable_stringify(&HashValue::Array(vec![
                HashValue::Number(1.0),
                HashValue::Undefined,
                HashValue::Null,
            ])),
            "[1,undefined,null]"
        );
        // Keys sort; an `undefined` VALUE is still emitted, unlike `JSON.stringify`, which would
        // drop the whole key.
        assert_eq!(
            stable_stringify(&HashValue::Object(vec![
                ("b".to_string(), HashValue::Undefined),
                ("a".to_string(), HashValue::Number(1.0)),
            ])),
            "{\"a\":1,\"b\":undefined}"
        );
    }

    /// The golden vector MCP-070 asks for: a fixed `ServerEntry`, its exact pre-image, and its exact
    /// 64-hex `configHash`, generated by running `metadata-cache.ts`'s `stableStringify` +
    /// `computeServerHash` verbatim on node 22 with `socket` unset.
    ///
    /// `cyrup_ext_subagents::exec::mcp_direct_tools::compute_mcp_server_hash` must assert the same
    /// two constants once MCP-070 option (b) lands there. Today it produces neither: it writes
    /// `null` where this writes `undefined`, and it hashes 11 keys.
    #[test]
    fn golden_vector_stdio_server() {
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
        .unwrap();
        let resolved = ResolvedIdentity::verbatim(&entry);

        assert_eq!(
            server_identity_pre_image(&entry, &resolved),
            concat!(
                r#"{"args":["-y","@modelcontextprotocol/server-filesystem","/tmp"],"#,
                r#""auth":undefined,"bearerToken":undefined,"bearerTokenEnv":undefined,"#,
                r#""command":"npx","cwd":"/home/u/work","#,
                r#""env":{"API_TOKEN":"s3cret","NODE_ENV":"production"},"#,
                r#""excludeTools":["danger_*"],"exposeResources":false,"headers":undefined,"#,
                r#""includeTools":undefined,"protocolVersion":undefined,"url":undefined}"#
            )
        );
        assert_eq!(
            compute_server_hash(&entry, &resolved),
            "a615fb56a8e4e0b32fc0fe8d4020422f2b68fe70f3758a29a655f02597d985cd"
        );
    }

    /// The HTTP half of the golden vector — `auth`, `protocolVersion`, `headers` and a `bearerToken`
    /// all present, so the two enum-valued identity fields are pinned on the wire.
    #[test]
    fn golden_vector_http_server() {
        let entry: ServerEntry = serde_json::from_str(
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
        .unwrap();
        let resolved = ResolvedIdentity::verbatim(&entry);

        assert_eq!(
            server_identity_pre_image(&entry, &resolved),
            concat!(
                r#"{"args":undefined,"auth":"bearer","bearerToken":"!tok","#,
                r#""bearerTokenEnv":"TOK_ENV","command":undefined,"cwd":undefined,"#,
                r#""env":undefined,"excludeTools":[],"exposeResources":true,"#,
                r#""headers":{"Accept":"application/json","X-Api-Key":"k"},"#,
                r#""includeTools":["a","b"],"protocolVersion":"2026-07-28","#,
                r#""url":"https://api.example.com/mcp"}"#
            )
        );
        assert_eq!(
            compute_server_hash(&entry, &resolved),
            "3132fc7312000677fa760f26236fb4bc534d5bb7a080c72e80416198781087fa"
        );
    }

    /// The empty definition — the case that makes the `undefined`-vs-`null` divergence total. Every
    /// one of the thirteen fields is absent, so a writer that emitted `null` would differ in
    /// thirteen places at once.
    #[test]
    fn golden_vector_empty_definition() {
        let entry = ServerEntry::default();
        let resolved = ResolvedIdentity::verbatim(&entry);
        assert!(!server_identity_pre_image(&entry, &resolved).contains("null"));
        assert_eq!(
            compute_server_hash(&entry, &resolved),
            "0a0f1e413316895cefba6c3dcdcb44d3b9c0b8e91591be79d39fc0afe2d089ef"
        );
    }

    /// The three fields deliberately excluded from the identity object: a change to any of them
    /// must NOT evict cached metadata.
    #[test]
    fn runtime_only_fields_are_outside_the_identity() {
        let base: ServerEntry = serde_json::from_str(r#"{ "command": "x" }"#).unwrap();
        let noisy: ServerEntry = serde_json::from_str(
            r#"{ "command": "x", "lifecycle": "keep-alive", "idleTimeout": 3,
                 "requestTimeoutMs": 1000, "debug": true, "directTools": true }"#,
        )
        .unwrap();
        assert_eq!(
            compute_server_hash(&base, &ResolvedIdentity::verbatim(&base)),
            compute_server_hash(&noisy, &ResolvedIdentity::verbatim(&noisy))
        );
    }

    /// The resolved form is what is hashed, not the config text — the property that makes the cache
    /// track `$API_HOST` changes.
    #[test]
    fn resolution_changes_the_digest() {
        let entry: ServerEntry = serde_json::from_str(r#"{ "url": "https://${HOST}/mcp" }"#).unwrap();
        let literal = ResolvedIdentity::verbatim(&entry);
        let resolved =
            ResolvedIdentity { url: Some("https://a.example/mcp".to_string()), ..literal.clone() };
        assert_ne!(compute_server_hash(&entry, &literal), compute_server_hash(&entry, &resolved));
    }

    /// An empty list is not an absent list — the pre-image distinguishes them, so the digest must.
    #[test]
    fn empty_list_differs_from_absent_list() {
        let absent = ServerEntry::default();
        let empty: ServerEntry = serde_json::from_str(r#"{ "excludeTools": [] }"#).unwrap();
        assert_ne!(
            compute_server_hash(&absent, &ResolvedIdentity::verbatim(&absent)),
            compute_server_hash(&empty, &ResolvedIdentity::verbatim(&empty))
        );
    }

    fn entry_with(config_hash: &str, cached_at: i64) -> ServerCacheEntry {
        ServerCacheEntry {
            config_hash: config_hash.to_string(),
            cached_at,
            ..ServerCacheEntry::default()
        }
    }

    #[test]
    fn cache_validity_rejects_mismatch_falsy_timestamp_and_age() {
        let now = 1_000_000_000_000;
        let fresh = entry_with("h", now - 1000);
        assert!(is_server_cache_valid_at(&fresh, "h", CACHE_MAX_AGE_MS, now));
        assert!(!is_server_cache_valid_at(&fresh, "other", CACHE_MAX_AGE_MS, now));
        // `!entry.cachedAt` rejects 0 as well as absent.
        assert!(!is_server_cache_valid_at(&entry_with("h", 0), "h", CACHE_MAX_AGE_MS, now));
        // Exactly at the boundary is still valid — upstream's test is `>`, not `>=`.
        let boundary = entry_with("h", now - CACHE_MAX_AGE_MS);
        assert!(is_server_cache_valid_at(&boundary, "h", CACHE_MAX_AGE_MS, now));
        let stale = entry_with("h", now - CACHE_MAX_AGE_MS - 1);
        assert!(!is_server_cache_valid_at(&stale, "h", CACHE_MAX_AGE_MS, now));
        // `maxAgeMs <= 0` disables the age check entirely.
        assert!(is_server_cache_valid_at(&stale, "h", 0, now));
    }

    fn cache_with(names: &[&str]) -> MetadataCache {
        let mut cache = MetadataCache::default();
        for name in names {
            cache.servers.insert((*name).to_string(), entry_with("h", 42));
        }
        cache
    }

    #[test]
    fn save_merges_over_disk_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(METADATA_CACHE_FILE);

        save_metadata_cache(&path, &cache_with(&["alpha", "beta"])).unwrap();
        // A second writer that only reconnected `beta` must not erase `alpha`.
        let mut second = MetadataCache::default();
        second.servers.insert("beta".to_string(), entry_with("h2", 43));
        second.servers.insert("gamma".to_string(), entry_with("h3", 44));
        save_metadata_cache(&path, &second).unwrap();

        let loaded = load_metadata_cache(&path).unwrap();
        assert_eq!(loaded.version, CACHE_VERSION);
        // Existing keys keep their position; new keys append — JS object-spread order.
        assert_eq!(
            loaded.servers.keys().map(String::as_str).collect::<Vec<_>>(),
            ["alpha", "beta", "gamma"]
        );
        assert_eq!(loaded.servers.get("beta").map(|e| e.config_hash.as_str()), Some("h2"));

        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "atomic write left a temp file behind");
    }

    #[test]
    fn load_rejects_a_foreign_version_and_a_malformed_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(METADATA_CACHE_FILE);

        assert!(load_metadata_cache(&path).is_none(), "a missing file is not an error");
        std::fs::write(&path, "{{{").unwrap();
        assert!(load_metadata_cache(&path).is_none());
        std::fs::write(&path, r#"{"version":2,"servers":{}}"#).unwrap();
        assert!(load_metadata_cache(&path).is_none());
        std::fs::write(&path, r#"{"version":1}"#).unwrap();
        assert!(load_metadata_cache(&path).is_none(), "a missing `servers` is a rejected file");
    }

    /// The on-disk shape `cyrup_ext_subagents::exec::mcp_direct_tools` reads: `version`,
    /// `servers.<name>.{configHash,tools[].name,resources[].{uri,name},cachedAt}`, 2-space indent,
    /// no trailing newline. Optional members are omitted rather than emitted as `null` — the
    /// reader's `Option` fields tolerate both, but pi omits, and a byte-comparable file is worth
    /// more than a tolerant one (MCP-077, MCP-094).
    #[test]
    fn written_file_matches_the_readers_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(METADATA_CACHE_FILE);

        let mut cache = MetadataCache::default();
        cache.servers.insert(
            "fs".to_string(),
            ServerCacheEntry {
                config_hash: "abc".to_string(),
                tools: vec![CachedTool {
                    name: "read_file".to_string(),
                    description: Some("Read a file".to_string()),
                    input_schema: Some(serde_json::json!({ "type": "object" })),
                    ..CachedTool::default()
                }],
                resources: vec![CachedResource {
                    uri: "file:///a".to_string(),
                    name: "a".to_string(),
                    description: None,
                }],
                prompts: None,
                instructions: Some("be careful".to_string()),
                cached_at: 42,
            },
        );
        save_metadata_cache(&path, &cache).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            body,
            concat!(
                "{\n",
                "  \"version\": 1,\n",
                "  \"servers\": {\n",
                "    \"fs\": {\n",
                "      \"configHash\": \"abc\",\n",
                "      \"tools\": [\n",
                "        {\n",
                "          \"name\": \"read_file\",\n",
                "          \"description\": \"Read a file\",\n",
                "          \"inputSchema\": {\n",
                "            \"type\": \"object\"\n",
                "          }\n",
                "        }\n",
                "      ],\n",
                "      \"resources\": [\n",
                "        {\n",
                "          \"uri\": \"file:///a\",\n",
                "          \"name\": \"a\"\n",
                "        }\n",
                "      ],\n",
                "      \"instructions\": \"be careful\",\n",
                "      \"cachedAt\": 42\n",
                "    }\n",
                "  }\n",
                "}"
            )
        );
    }
}
