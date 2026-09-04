//! `<agent_dir>`, every adapter-owned path under it, and the `mcp-cache.json` metadata cache —
//! `agent-dir.ts` (13a §26, MCP-048), the path half of `config.ts` / `utils.ts` / `mcp-auth.ts`
//! (MCP-084, MCP-068), and `metadata-cache.ts` (13b §17, MCP-070, MCP-077).
//!
//! # Do not re-implement the resolution
//!
//! Upstream's `getAgentDir()` (`agent-dir.ts:10`) reads `PI_CODING_AGENT_DIR` (trimmed, with `~`
//! expansion) and defaults to `join(homedir(), ".pi", "agent")`. cyrup already resolves the same
//! concept, better: `cyrup_config::ConfigDirs::agent_dir` is a `PathBuf` **field** populated by
//! `ConfigDirs::resolve` from the CLI flag, then the shared agent-dir ladder
//! (`cyrup_config::paths::ENV_AGENT_DIR_KEYS` — `$CYRUP_AGENT_DIR`, `$CYRUP_CODING_AGENT_DIR`,
//! `$PI_CODING_AGENT_DIR`), then
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
//! **The reciprocal change has landed (MCP-070 option (b), MCP-094, MCP-141/142/143/144/145).**
//! `cyrup_ext_subagents::exec::mcp_direct_tools` now builds the same fifteen keys in the same
//! resolved forms, renders an absent field as the bare `undefined` token, and carries the same
//! `try`/`catch` on an unhashable definition. Neither side owns the contract alone: that crate takes
//! `cyrup-mcp` as a **dev-dependency** and asserts its pre-image, its digest and its tool names
//! against the functions below rather than against constants copied out of them, so the two cannot
//! drift apart without a test failing.
//!
//! That residue is CLOSED. [`home_dir`] here and the in-tree reader both call
//! [`cyrup_config::paths::cyrup_home_dir_from`], so a `cwd` beginning with `~` hashes identically
//! under any `CYRUP_HOME`. This was MCP-139's agent-dir axis 3, whose fix its own note specified as
//! *"a single shared resolver spanning this crate, `cyrup_ext`'s `npx_resolver` and that reader"* —
//! that resolver now exists in `cyrup-config`, and all three call it.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::{HttpRequestHeadersCommand, ServerEntry};
use crate::credentials::EnvFn;
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
    ///
    /// Called by `crate::commands`' `/mcp` and `/mcp setup` arms and by
    /// [`crate::panel_host::SetupCallbacks`], all of which reach `crate::onboarding` through it.
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
    ///
    /// **No caller yet.** The trace writer is `mcp-trace.ts` — **MCP-133, unported** (see
    /// `crate::runtime`'s step-4 note); this is the destination waiting for it.
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

/// node's `os.homedir()` — the home `resolveConfigPath` expands `~` against (`utils.ts:190`).
///
/// **Not a fourth agent-dir resolver.** This is `<home>`, not `<agent_dir>`; [`McpDirs`] still
/// takes its agent dir from `cyrup_config::ConfigDirs` and this module still refuses to re-resolve
/// it (see the module header). The one thing that needs a home and cannot be handed one is
/// [`crate::registration::default_server_hasher`]: [`ServerHasher`](crate::registration::ServerHasher)
/// is a bare `fn` pointer with nowhere to carry state, and it exists precisely so
/// `is_server_cache_valid` stays a two-argument predicate.
///
/// The workspace's one home ladder ([`cyrup_config::paths::cyrup_home_dir_from`]) — `CYRUP_HOME`
/// -> `HOME` -> the OS home — with this module's own terminal.
///
/// The empty terminal is deliberate and stays: an empty home only matters for a `cwd` that starts
/// with `~`, and there it yields the remainder unrooted rather than a wrong absolute path. That is
/// this module's answer to "no home resolvable"; the ladder returns `Option` precisely so each
/// caller can give its own (`ConfigDirs::resolve` errors, `cyrup_ext_subagents` falls to
/// `temp_dir`).
///
/// # This closes MCP-139's agent-dir axis 3
///
/// This resolver used to read `$HOME` -> `$USERPROFILE` -> empty with **no `CYRUP_HOME` rung**,
/// and its own doc said so: *"This is not the home the in-tree reader uses… the two differ exactly
/// when `CYRUP_HOME` is set to something other than `HOME` — MCP-139's agent-dir axis 3, whose fix
/// is the single shared resolver that unit specifies and that lives outside this crate."* That
/// resolver now exists, in `cyrup-config`, and this is it.
#[must_use]
pub fn home_dir() -> PathBuf {
    cyrup_config::paths::cyrup_home_dir_from(&|key| std::env::var_os(key)).unwrap_or_default()
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
    if let Some(rest) = interpolated
        .strip_prefix("~/")
        .or_else(|| interpolated.strip_prefix("~\\"))
    {
        return node_path_join(home, rest);
    }
    PathBuf::from(interpolated)
}

/// node's `path.join(base, rest)`, which is NOT `Path::join`. Upstream reaches `resolveConfigPath`'s
/// `~` arm through it (`utils.ts:187`), and the two differ in three ways that all change the digest:
///
/// * **`rest` is never absolute.** `path.join("/home/u", "/x")` is `/home/u/x`; `Path::join` throws
///   the base away and yields `/x`. `"~//x"` strips to `rest = "/x"`, so this was silently hashing a
///   `cwd` of `/x` — the home directory gone entirely, for a perfectly ordinary config typo.
/// * **The result is normalized.** node collapses repeated separators and folds `.` and `..`
///   lexically, so `"~/a//b"` is `/home/u/a/b` and `"~/.."` is `/home`. `Path::join` preserves all
///   of it verbatim.
/// * **A trailing separator is dropped** unless the result is the root, so `"~/"` is `/home/u`, not
///   `/home/u/`.
///
/// Lexical only — it never touches the filesystem, exactly as node's does, so it cannot follow a
/// symlink or differ between a machine where the path exists and one where it does not.
///
/// `cyrup_ext_subagents::exec::mcp_direct_tools::resolve_config_path` carries the same logic and
/// must keep carrying it: that crate depends on this one only as a `[dev-dependency]`, so the
/// production reader cannot call this function. The cross-crate conformance tests are what hold the
/// two copies together.
#[must_use]
pub fn node_path_join(base: &Path, rest: &str) -> PathBuf {
    let base = base.to_string_lossy().replace('\\', "/");
    let rest = rest.replace('\\', "/");
    let absolute = base.starts_with('/');

    let mut parts: Vec<&str> = Vec::new();
    for segment in base.split('/').chain(rest.split('/')) {
        match segment {
            // `path.join` drops empty segments (so `a//b` collapses) and `.` outright.
            "" | "." => {}
            ".." => {
                // Pop a real segment; on a relative path with nothing to pop, `..` survives, which
                // is what node does (`path.join("a", "../../b")` is `../b`).
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
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base.join(candidate)
    };
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
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
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
    #[serde(default)]
    pub name: String,
    /// The tool description, verbatim from `tools/list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The raw JSON Schema, stored unnormalised — `normalizeDirectToolInputSchema` runs at
    /// registration, not at cache time (MCP-087).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    /// **Cut 2**, retained in the schema. MCP Apps' `ui://` resource for this tool.
    ///
    /// **Never written and never read**: [`serialize_tools`] always writes `None` because MCP Apps is
    /// Cut 2. The field is retained only so `mcp-cache.json` keeps the shape the in-tree reader
    /// `cyrup_ext_subagents::exec::mcp_direct_tools` deserialises.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_resource_uri: Option<String>,
    /// **Cut 2**, retained in the schema. `UiToolVisibility[]` — an array upstream, not a scalar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_visibility: Option<Vec<String>>,
    /// **Cut 2**, retained in the schema. `"eager" | "stream-first"`.
    ///
    /// **Never written and never read** — Cut 2, as [`Self::ui_resource_uri`]; retained for
    /// `mcp-cache.json` shape compatibility only.
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
    #[serde(default)]
    pub uri: String,
    /// The resource name, which `resourceNameToToolName` sanitises into the tool name.
    #[serde(default)]
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
    #[serde(default)]
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
    #[serde(default)]
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
    ///
    /// `#[serde(default)]` on this and the two below is **not** cosmetic. `metadata-cache.ts`
    /// reads every member with `??`, so a cache written by an older build — or by a version that
    /// had not yet learned to write `resources` — still loads. Without the defaults a single
    /// missing key makes [`load_metadata_cache`] return `None` for the **whole file**, while
    /// [`crate::registration`]'s lenient reader over the same bytes returns everything: the `/mcp`
    /// panel would show no cached data for servers whose tools are registered and working. See the
    /// report's note on unifying the two cache readers.
    #[serde(default)]
    pub config_hash: String,
    /// Every tool the server advertised, unfiltered — `includeTools`/`excludeTools` are applied at
    /// reconstruction so a config edit does not require a reconnect.
    #[serde(default)]
    pub tools: Vec<CachedTool>,
    /// Every resource the server advertised.
    #[serde(default)]
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
    ///
    /// The custom deserialiser is MCP-145's `typeof entry.cachedAt !== "number"` arm made
    /// non-destructive: a plain `i64` makes `"cachedAt": "1760000000000"` a **parse error**, which
    /// [`load_metadata_cache`] turns into `None` for the whole file, where upstream rejects only
    /// that one entry. Anything that is not a JSON number lands on `0`, which the falsy test
    /// already rejects. See `crate::registration::lenient_epoch_ms`, its twin on the lenient
    /// reader over the same bytes.
    #[serde(default, deserialize_with = "lenient_epoch_ms")]
    pub cached_at: i64,
}

/// `i64` epoch milliseconds that answers `0` for **any** non-number instead of failing the parse —
/// see [`ServerCacheEntry::cached_at`]. `0` is upstream's falsy `cachedAt`, i.e. "invalid entry".
fn lenient_epoch_ms<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Value>::deserialize(deserializer)?
        .and_then(|value| value.as_i64())
        .unwrap_or(0))
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
        Self {
            version: CACHE_VERSION,
            servers: IndexMap::new(),
        }
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
        std::fs::create_dir_all(parent).map_err(|source| McpError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
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

    std::fs::write(&temp, body).map_err(|source| McpError::Io {
        path: temp.clone(),
        source,
    })?;
    std::fs::rename(&temp, path).map_err(|source| {
        // A failed rename leaves the temp file behind; upstream's `renameSync` throw does too, but
        // there is no reason to keep it.
        let _ = std::fs::remove_file(&temp);
        McpError::Io {
            path: path.to_path_buf(),
            source,
        }
    })
}

// -------------------------------------------------------------------------------------------
// The serialisers — live discovery result → cache entry (`metadata-cache.ts`; MCP-140)
// -------------------------------------------------------------------------------------------

/// `extractUiToolVisibility(meta)` (`ui-tool-visibility.ts:3`) — the `_meta.ui.visibility` array,
/// normalised.
///
/// Five arms, and the two that return an **empty** vector rather than `None` are the load-bearing
/// ones: `Some(vec![])` means "the server said something about visibility that this client does not
/// understand", and [`crate::registration::is_ui_tool_visible_to_model`] answers `false` for it —
/// fail-closed. `None` means "the server said nothing", which is visible. Collapsing the two would
/// expose to the model every tool a server marked app-only with a spelling we do not know.
///
/// * no `_meta`, or `_meta.ui` missing / not an object / an array ⇒ `None`
/// * `_meta.ui.visibility` absent ⇒ `None`
/// * `visibility` present but not an array ⇒ `Some(vec![])`
/// * any element that is neither `"model"` nor `"app"` ⇒ `Some(vec![])` — the **whole** list is
///   discarded on one bad element, not just that element
/// * otherwise the elements, deduplicated, in first-occurrence order
///
/// **Cut 2 keeps this.** MCP Apps went, but the *filter* stays: a cache written by an
/// upstream-compatible producer still has to hide what the server hid (§3.17's "Cut-2 seam inside
/// the schema").
#[must_use]
pub fn extract_ui_tool_visibility(meta: Option<&Value>) -> Option<Vec<String>> {
    let ui = meta?.as_object()?.get("ui")?;
    // `Array.isArray(ui)` is excluded explicitly upstream; `as_object` already rejects an array,
    // and it also rejects the `null` that `!ui` catches.
    let visibility = ui.as_object()?.get("visibility")?;
    let Some(entries) = visibility.as_array() else {
        return Some(Vec::new());
    };
    let mut values: Vec<String> = Vec::new();
    for entry in entries {
        let Some(name) = entry.as_str().filter(|n| *n == "model" || *n == "app") else {
            return Some(Vec::new());
        };
        if !values.iter().any(|seen| seen == name) {
            values.push(name.to_string());
        }
    }
    Some(values)
}

/// `serializeTools(tools)` (`metadata-cache.ts:281`) — `tools/list` → [`CachedTool`].
///
/// `filter(t => t?.name)` is a **truthiness** test, so a tool whose name is the empty string is
/// dropped as well as one with no name at all; `rmcp`'s `Tool::name` is a non-optional
/// `Cow<'static, str>`, so emptiness is the whole of that filter here.
///
/// Every optional field is emitted **only when defined** — the object-spread form
/// `...(x !== undefined ? { x } : {})`, which never writes an explicit `null`. In Rust that is
/// `Option` plus the `skip_serializing_if` already on [`CachedTool`].
///
/// **Two Cut-2 fields are never written.** `uiResourceUri` (upstream's `tryGetToolUiResourceUri`)
/// and `uiStreamMode` (`extractToolUiStreamMode`) belong to MCP Apps, which is cut whole; their
/// *names* stay reserved in the on-disk schema so a pi-written cache round-trips, and
/// [`CACHE_VERSION`] is deliberately not bumped to drop them. `uiVisibility` is **not** cut — see
/// [`extract_ui_tool_visibility`].
///
/// `inputSchema` is always present on `rmcp`'s `Tool` (an `Arc<JsonObject>`, not an `Option`), so
/// the "only when defined" rule has nothing to suppress; it is written raw and unnormalised,
/// because `normalizeDirectToolInputSchema` runs at registration, not at cache time (MCP-087).
#[must_use]
pub fn serialize_tools(tools: &[rmcp::model::Tool]) -> Vec<CachedTool> {
    tools
        .iter()
        .filter(|tool| !tool.name.is_empty())
        .map(|tool| CachedTool {
            name: tool.name.to_string(),
            description: tool
                .description
                .as_ref()
                .map(std::string::ToString::to_string),
            input_schema: Some(Value::Object((*tool.input_schema).clone())),
            ui_resource_uri: None,
            ui_visibility: extract_ui_tool_visibility(
                tool.meta
                    .as_ref()
                    .map(|meta| Value::Object(meta.0.clone()))
                    .as_ref(),
            ),
            ui_stream_mode: None,
        })
        .collect()
}

/// `serializeResources(resources)` (`metadata-cache.ts:299`) — `resources/list` → [`CachedResource`].
///
/// `filter(r => r?.name && r?.uri)` needs **both** truthy: a resource missing either one would
/// produce a `read_` tool that cannot be named or cannot be read, so it is dropped here rather than
/// at reconstruction. `description` is the only optional member; the
/// `` `Read resource: ${uri}` `` fallback belongs to the *reconstructor*, not here, so an absent
/// description round-trips as absent.
#[must_use]
pub fn serialize_resources(resources: &[rmcp::model::Resource]) -> Vec<CachedResource> {
    resources
        .iter()
        .filter(|resource| !resource.name.is_empty() && !resource.uri.is_empty())
        .map(|resource| CachedResource {
            uri: resource.uri.clone(),
            name: resource.name.clone(),
            description: resource.description.clone(),
        })
        .collect()
}

/// `serializePrompts(prompts)` (`metadata-cache.ts:309`) — `prompts/list` → [`CachedPrompt`].
///
/// `(prompts ?? [])` then `filter(prompt => prompt?.name)`, and each argument filtered on
/// `argument?.name` in turn — a nameless argument is dropped, it does not drop the prompt.
///
/// **`arguments` is emitted only when `Array.isArray(prompt.arguments)`**, which is why
/// [`CachedPrompt::arguments`] is `Option<Vec<_>>` and not `Vec<_>`: a server that sends no
/// `arguments` key round-trips as absent, while one that sends `[]` round-trips as an empty array.
/// Flattening the two would be invisible in the file and visible in the slash command's help.
#[must_use]
pub fn serialize_prompts(prompts: &[rmcp::model::Prompt]) -> Vec<CachedPrompt> {
    prompts
        .iter()
        .filter(|prompt| !prompt.name.is_empty())
        .map(|prompt| CachedPrompt {
            name: prompt.name.clone(),
            title: prompt.title.clone(),
            description: prompt.description.clone(),
            arguments: prompt.arguments.as_ref().map(|arguments| {
                arguments
                    .iter()
                    .filter(|argument| !argument.name.is_empty())
                    .map(|argument| CachedPromptArgument {
                        name: argument.name.clone(),
                        description: argument.description.clone(),
                        required: argument.required,
                    })
                    .collect()
            }),
        })
        .collect()
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
///
/// **No `Eq`.** [`Self::request_headers_command`] carries `timeoutMs`, which is `f64` because it
/// arrived from `JSON.parse` — the same reason [`ServerEntry`] itself is `PartialEq` only.
#[derive(Debug, Clone, Default, PartialEq)]
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
    /// `definition.requestHeadersCommand` with `command`, every `args` element and every `env`
    /// value `interpolateEnvVars`'d, `timeoutMs` copied through (`metadata-cache.ts:94-101`,
    /// upstream v2.26.0). Interpolated for the same reason `headers` is: the digest has to change
    /// when `$MCP_SIGNER_ACTOR` changes even though the config text did not, which is precisely what
    /// upstream's `"hashes the effective per-request header command"` test asserts
    /// (`__tests__/direct-tools.test.ts:357-379`).
    pub request_headers_command: Option<HttpRequestHeadersCommand>,
}

impl ResolvedIdentity {
    /// The six fields copied **verbatim** off the definition, with no resolver applied.
    ///
    /// Correct only for a definition in which none of them contains an interpolation token
    /// (`${VAR}`, `$env:VAR`, `{env:VAR}`), a `~`, or a `!`/`!!` secret marker — and wrong,
    /// silently, for any that does.
    ///
    /// **Kept, no longer a placeholder (MCP-141).** [`Self::resolve`] below is the real constructor
    /// and is what every production caller now uses; this one survives as the *fixture* form, so a
    /// golden vector can pin a pre-image without depending on the ambient environment or on a home
    /// directory. For a definition with no token, no `~` and no `!`, the two are equal by
    /// construction — which is exactly what `resolve_and_verbatim_agree_when_nothing_needs_resolving`
    /// asserts, and why the golden vectors below stayed byte-identical across MCP-141.
    ///
    /// It also cannot express upstream's **throw**: it is infallible, so an entry whose `env` or
    /// `headers` carries a non-string member (`interpolateEnvRecord`'s `value.startsWith` TypeError
    /// — see [`crate::config::StringRecord`]) hashes here as its string members alone. That is the
    /// second thing this constructor is wrong about, and for the same reason as the first: it does
    /// not run the resolvers. [`Self::resolve`] does both.
    #[must_use]
    pub fn verbatim(entry: &ServerEntry) -> Self {
        Self {
            env: entry.env.as_deref().cloned(),
            cwd: entry.cwd.clone(),
            url: entry.url.clone(),
            headers: entry.headers.as_deref().cloned(),
            bearer_token: entry.bearer_token.clone(),
            request_headers_command: entry.request_headers_command.clone(),
        }
    }

    /// `computeServerHash`'s six resolver calls, run for real (`metadata-cache.ts:86-108` @
    /// v2.26.1) — MCP-141 leg (b), which closes the split where the in-tree *reader* resolved and
    /// this *writer* did not.
    ///
    /// | field | upstream | here |
    /// |---|---|---|
    /// | `env` | `interpolateEnvRecord` | [`crate::secrets::interpolate_env_record`] |
    /// | `cwd` | `resolveConfigPath` | [`crate::credentials::interpolate_env_vars`] + [`resolve_config_path_tail`] |
    /// | `url` | `resolveServerUrl` — **throws** | [`crate::credentials::resolve_server_url`] — the one `Err` arm |
    /// | `headers` | `interpolateEnvRecord` | [`crate::secrets::interpolate_env_record`] |
    /// | `bearerToken` | `resolveBearerToken` | [`crate::credentials::resolve_bearer_token`] |
    /// | `requestHeadersCommand` | `interpolateEnvVars` ×3 + `interpolateEnvRecord` | same, below |
    ///
    /// **`env`/`headers` go through `interpolateSecretExpression`, not plain interpolation**
    /// (MCP-144). `!!x` is un-escaped to the literal `!x` — the value the child will actually see —
    /// while a bare `!cmd` is hashed as its own text and **never executed**: hashing happens on the
    /// discovery, merge, preview and panel paths, and a resolver that spawned there would run
    /// arbitrary shell out of a repo's `.mcp.json` merely because the user listed their config.
    /// [`crate::secrets::resolve_command_secret`] — the form that does spawn — is unreachable from
    /// here by construction.
    ///
    /// **Two things can fail, and both are upstream throws.** `resolveServerUrl` rejects a URL
    /// naming an unset variable or one that no longer parses after interpolation; and
    /// `interpolateEnvRecord` rejects an `env`, `headers` or `requestHeadersCommand.env` block
    /// carrying a non-string member, because `interpolateSecretExpression` calls `value.startsWith`
    /// on it unconditionally ([`crate::config::StringRecord`] carries that throw, and
    /// `interpolate_env_record` raises it). Either way
    /// [`crate::registration::is_server_cache_valid`] must answer `false` (MCP-145). Between them
    /// they are the whole mechanism keeping a `url: "https://x/${MISSING}"` or an `env: { N: 5 }`
    /// server out of the cold-start direct tool surface.
    ///
    /// The second arm is new. While `env`/`headers` were a `BTreeMap<String, String>` behind
    /// `lenient`, a non-string member dropped the entire map, this constructor succeeded, and the
    /// entry was called VALID — the opposite of the answer
    /// `cyrup_ext_subagents::exec::mcp_direct_tools` gave for the same bytes.
    ///
    /// `env` and `home` are arguments rather than ambient reads for the reason every other seam in
    /// this crate takes them: edition 2024 makes `std::env::set_var` `unsafe`, so a test that pinned
    /// a variable could not undo it, and a hash fixture that read the real environment would not be
    /// a fixture. Production passes [`crate::secrets::PROCESS_ENV`] and
    /// `cyrup_config::ConfigDirs::home`.
    pub fn resolve(entry: &ServerEntry, env: &EnvFn, home: &Path) -> McpResult<Self> {
        Ok(Self {
            env: interpolate_env_record(entry.env.as_ref(), env)?,
            cwd: entry
                .cwd
                .as_deref()
                .map(|raw| resolve_config_path(raw, env, home)),
            url: crate::credentials::resolve_server_url(entry.url.as_deref(), env)?,
            headers: interpolate_env_record(entry.headers.as_ref(), env)?,
            bearer_token: crate::credentials::resolve_bearer_token(
                entry.bearer_token.as_deref(),
                entry.bearer_token_env.as_deref(),
                env,
            ),
            request_headers_command: entry
                .request_headers_command
                .as_ref()
                .map(|command| -> McpResult<HttpRequestHeadersCommand> {
                    // `metadata-cache.ts:94-101` — `command` and each `args` element go through
                    // plain `interpolateEnvVars` (NOT the secret grammar: upstream does not call
                    // `interpolateSecretExpression` here), `env` through `interpolateEnvRecord` —
                    // which is why the nested `env` throws on a non-string member exactly as the
                    // outer one does — and `timeoutMs` is copied through untouched.
                    Ok(HttpRequestHeadersCommand {
                        command: command
                            .command
                            .as_deref()
                            .map(|c| crate::credentials::interpolate_env_vars(c, env)),
                        args: command.args.as_ref().map(|args| {
                            args.iter()
                                .map(|a| crate::credentials::interpolate_env_vars(a, env))
                                .collect()
                        }),
                        env: interpolate_env_record(command.env.as_ref(), env)?
                            .map(crate::config::StringRecord::from),
                        timeout_ms: command.timeout_ms,
                    })
                })
                .transpose()?,
        })
    }
}

/// `interpolateEnvRecord(values)` (`utils.ts:107`) **with upstream's throw** — the resolver
/// `computeServerHash` applies to `env`, `headers` and `requestHeadersCommand.env`
/// (`metadata-cache.ts:90`, `:93`, `:98`).
///
/// [`crate::secrets::interpolate_env_record`] is the value half and is total, because by the time it
/// is called every member is already a [`String`]. The half that can fail is the one the *type*
/// carries: [`crate::config::StringRecord::unhashable`] is `Some` exactly when upstream's
/// `value.startsWith(…)` would have been called on something that is not a string, and upstream's
/// TypeError is what it holds. Raising it here — rather than silently hashing the string members —
/// is what makes [`crate::registration::is_server_cache_valid`] answer `false` for such an entry,
/// which is the answer `cyrup_ext_subagents::exec::mcp_direct_tools` has always given.
///
/// # Errors
///
/// The block carried a member that is not a string.
fn interpolate_env_record(
    values: Option<&crate::config::StringRecord>,
    env: &EnvFn,
) -> McpResult<Option<BTreeMap<String, String>>> {
    if let Some(message) = values.and_then(crate::config::StringRecord::unhashable) {
        return Err(McpError::Config(message.to_string()));
    }
    Ok(crate::secrets::interpolate_env_record(
        values.map(crate::config::StringRecord::values),
        env,
    ))
}

/// `resolveConfigPath(value)` (`utils.ts:187`) **whole** — interpolate, then expand `~`.
///
/// The two halves live apart in this crate ([`crate::credentials::interpolate_env_vars`] is the
/// crate's single interpolation engine, [`resolve_config_path_tail`] is the `~` rule), and every
/// caller that wants upstream's function wants both in that order. Returns a `String`, not a
/// `PathBuf`, because the hash pre-image is a **JSON string** and a lossy `PathBuf` round-trip on a
/// non-UTF-8 path would change the digest.
#[must_use]
pub fn resolve_config_path(raw: &str, env: &EnvFn, home: &Path) -> String {
    let interpolated = crate::credentials::interpolate_env_vars(raw, env);
    resolve_config_path_tail(&interpolated, home)
        .to_string_lossy()
        .into_owned()
}

/// [`compute_server_hash`] with the resolvers applied — `computeServerHash(definition)` end to end
/// (MCP-141), and the function [`crate::registration::is_server_cache_valid`]'s default hasher is.
///
/// `Err` is upstream's `throw`, which every caller maps to "this cache entry is not valid"
/// (MCP-145). It has **two** sources, both of them resolvers upstream lets throw out of the identity
/// literal: [`crate::credentials::resolve_server_url`], and `interpolateEnvRecord` on an `env`,
/// `headers` or `requestHeadersCommand.env` block carrying a non-string member
/// ([`crate::config::StringRecord`]).
pub fn try_compute_server_hash(entry: &ServerEntry, env: &EnvFn, home: &Path) -> McpResult<String> {
    Ok(compute_server_hash(
        entry,
        &ResolvedIdentity::resolve(entry, env, home)?,
    ))
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
    /// switches to exponential notation — unreachable in this pre-image, whose one numeric field
    /// (`requestHeadersCommand.timeoutMs`) is bounded at 60000 by its own read-site validation.
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
            Value::Object(map) => HashValue::Object(
                map.into_iter()
                    .map(|(k, v)| (k, HashValue::from_json(v)))
                    .collect(),
            ),
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

/// `computeServerHash`'s identity object (`metadata-cache.ts:82`).
///
/// **Fifteen fields, and the list is the specification.** Everything is verbatim, including the
/// fields that are **not** here: `lifecycle`, `idleTimeout`, `requestTimeoutMs` and `debug` are
/// runtime behaviour, they do not change which tools a server exposes, and hashing them would evict
/// every cache entry on an unrelated edit.
///
/// # `socket` is the fifteenth, and it is emitted `undefined` unconditionally
///
/// Upstream's third key is `socket: resolveConfigPath(definition.socket)`
/// (`metadata-cache.ts:89`), and `stableStringify` walks `Object.keys()` — which includes keys
/// holding `undefined` — so upstream emits `"socket":undefined` for **every** definition, including
/// one that never mentions a socket. This pre-image omitted the key entirely, on the argument that
/// Cut 3 removed the field; the consequence was that every digest cyrup produced differed from pi's
/// by exactly that one member, for every server in every config, forever.
///
/// Emitting the key unconditionally is both correct and complete here, and not a compromise:
/// [`crate::config::to_server_entries`] **rejects** any entry carrying `socket` with a named Cut-3
/// diagnostic, so a `ServerEntry` that reached this function can only ever have had
/// `socket: undefined` — which is precisely `resolveConfigPath(undefined)`.
///
/// Measured, not argued. Upstream's own digest for the plain-stdio golden fixture below is
/// `2190558e470a75c0f992989bd1799b374e669deecb8093e4118a1a9419068cf4`; before this key landed cyrup
/// produced `4dd46c1f…`, and deleting upstream's `socket` member from its pre-image yielded cyrup's
/// byte for byte. The two now agree; `golden_vector_stdio_server` pins it.
///
/// # `requestHeadersCommand` is v2.26.0's addition, and it invalidated every cache entry once
///
/// `stableStringify` emits a key whose value is absent as the literal `undefined` rather than
/// dropping it, so widening the object changes the digest of **every** server, not just of one that
/// configures a signing command. Upstream took exactly that hit at v2.26.0 (`metadata-cache.ts:94`);
/// the alternative — omitting the key when the field is absent — would have left two adapters
/// disagreeing about the digest of the same config, which is worse than one cold cache.
///
/// Note the nested object is emitted **whole** whenever the field is present: `args`, `env` and
/// `timeoutMs` each render as `undefined` inside it when they are absent, and
/// [`stable_stringify`] sorts its four keys just as it sorts the outer ones.
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
        // `socket: resolveConfigPath(definition.socket)` (`metadata-cache.ts:89`). Always
        // `undefined`: `to_server_entries` rejects any entry that configures a socket (Cut 3), so
        // the field cannot be present, and `resolveConfigPath(undefined)` is `undefined`. The KEY
        // must still be emitted — `stableStringify` walks `Object.keys()`, so upstream writes
        // `"socket":undefined` rather than dropping it, and omitting it moved every cyrup digest.
        ("socket".to_string(), HashValue::Undefined),
        ("url".to_string(), opt_string(resolved.url.as_deref())),
        (
            "headers".to_string(),
            opt_string_map(resolved.headers.as_ref()),
        ),
        (
            "requestHeadersCommand".to_string(),
            opt_request_headers_command(resolved.request_headers_command.as_ref()),
        ),
        ("auth".to_string(), opt_serde(entry.auth.as_ref())),
        (
            "protocolVersion".to_string(),
            opt_serde(entry.protocol_version.as_ref()),
        ),
        (
            "bearerToken".to_string(),
            opt_string(resolved.bearer_token.as_deref()),
        ),
        (
            "bearerTokenEnv".to_string(),
            opt_string(entry.bearer_token_env.as_deref()),
        ),
        (
            "exposeResources".to_string(),
            entry
                .expose_resources
                .map_or(HashValue::Undefined, HashValue::Bool),
        ),
        (
            "includeTools".to_string(),
            opt_string_list(entry.include_tools.as_ref()),
        ),
        (
            "excludeTools".to_string(),
            opt_string_list(entry.exclude_tools.as_ref()),
        ),
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
///
/// Crate-visible because it has a second caller: [`crate::state::approval_cache_key`] takes the same
/// digest over the same [`stable_stringify`] pre-image (`tool-approval.ts:151`), and a second
/// hand-rolled hex loop is the kind of near-duplicate that drifts.
pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
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

/// `undefined` for absent, the four-key nested object otherwise (`metadata-cache.ts:94-101`).
///
/// The values arrive **already interpolated** on [`ResolvedIdentity::request_headers_command`] —
/// this function only shapes them, exactly as its `headers` sibling does.
fn opt_request_headers_command(value: Option<&HttpRequestHeadersCommand>) -> HashValue {
    value.map_or(HashValue::Undefined, |command| {
        HashValue::Object(vec![
            (
                "command".to_string(),
                opt_string(command.command.as_deref()),
            ),
            ("args".to_string(), opt_string_list(command.args.as_ref())),
            ("env".to_string(), opt_string_map(command.env.as_deref())),
            (
                "timeoutMs".to_string(),
                command
                    .timeout_ms
                    .map_or(HashValue::Undefined, HashValue::Number),
            ),
        ])
    })
}

/// `undefined` for absent, an object otherwise. [`BTreeMap`] iterates in key order, which is the
/// order [`stable_stringify`] would impose anyway.
fn opt_string_map(value: Option<&BTreeMap<String, String>>) -> HashValue {
    value.map_or(HashValue::Undefined, |map| {
        HashValue::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), HashValue::String(v.clone())))
                .collect(),
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
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    /// node's `path.join` semantics, pinned against values measured by running upstream's own
    /// `resolveConfigPath` on node 22 (pi-mcp-adapter @ v2.26.1, `fafae21`) with `HOME=/home/u`.
    ///
    /// Every one of these hashed differently before `node_path_join` existed, because the `~` arm
    /// was a bare `Path::join`. The first row is the one that matters most: `"~//x"` strips to
    /// `rest = "/x"`, and `Path::join` treats an absolute `rest` as a REPLACEMENT — so a `cwd` of
    /// `~//x` hashed as `/x`, with the user's home directory silently gone.
    #[test]
    fn the_tilde_arm_has_node_path_join_semantics_not_rust_ones() {
        let home = Path::new("/home/u");
        // (input, what upstream's resolveConfigPath returns on node 22)
        for (input, expected) in [
            ("~//x", "/home/u/x"),
            ("~/", "/home/u"),
            ("~\\", "/home/u"),
            ("~/..", "/home"),
            ("~/./x", "/home/u/x"),
            ("~/a//b", "/home/u/a/b"),
            // Unchanged by the fix, kept so a future rewrite cannot regress the ordinary cases.
            ("~", "/home/u"),
            ("~/work", "/home/u/work"),
            ("/absolute", "/absolute"),
            ("relative/x", "relative/x"),
        ] {
            assert_eq!(
                resolve_config_path_tail(input, home),
                PathBuf::from(expected),
                "resolveConfigPath({input:?}) must match node"
            );
        }
    }

    /// `..` may not climb out of an absolute root, and survives on a relative path — both node
    /// behaviours, and both reachable from a config that over-uses `../`.
    #[test]
    fn node_path_join_clamps_dotdot_at_the_root_and_keeps_it_when_relative() {
        assert_eq!(
            node_path_join(Path::new("/home/u"), "../../../.."),
            PathBuf::from("/")
        );
        assert_eq!(
            node_path_join(Path::new("a"), "../../b"),
            PathBuf::from("../b")
        );
        assert_eq!(node_path_join(Path::new("/"), "x"), PathBuf::from("/x"));
    }

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
        assert_eq!(
            resolve_config_path_tail("~", home),
            PathBuf::from("/home/u")
        );
        assert_eq!(
            resolve_config_path_tail("~/x", home),
            PathBuf::from("/home/u/x")
        );
        // `utils.ts:192` tests BOTH prefixes; `agent-dir.ts` tests only `~/`.
        assert_eq!(
            resolve_config_path_tail("~\\x", home),
            PathBuf::from("/home/u/x")
        );
        // …and it never trims, because a config value is used as written.
        assert_eq!(
            resolve_config_path_tail(" ~/x", home),
            PathBuf::from(" ~/x")
        );
    }

    #[test]
    fn adapter_owned_paths_hang_off_agent_dir() {
        let dirs = McpDirs::new(PathBuf::from("/a/agent"), PathBuf::from("/w"));
        assert_eq!(dirs.global_config(), PathBuf::from("/a/agent/mcp.json"));
        assert_eq!(
            dirs.metadata_cache(),
            PathBuf::from("/a/agent/mcp-cache.json")
        );
        assert_eq!(
            dirs.onboarding_state(),
            PathBuf::from("/a/agent/mcp-onboarding.json")
        );
        assert_eq!(
            dirs.agent_plugin_data("p"),
            PathBuf::from("/a/agent/agent-plugin-data/p")
        );
        assert_eq!(
            dirs.default_oauth_dir(),
            PathBuf::from("/a/agent/mcp-oauth")
        );
        assert_eq!(dirs.trace_dir(), PathBuf::from("/w/.cyrup/mcp-traces"));
    }

    #[test]
    fn ladder_paths_match_config_ts() {
        let dirs = McpDirs::new(PathBuf::from("/a/agent"), PathBuf::from("/w"));
        let home = Path::new("/home/u");
        assert_eq!(
            shared_global_config(home),
            PathBuf::from("/home/u/.config/mcp/mcp.json")
        );
        assert_eq!(
            agents_global_configs(home),
            [
                PathBuf::from("/home/u/.agents/mcp.json"),
                PathBuf::from("/home/u/.agents/mcp/mcp.json"),
            ]
        );
        assert_eq!(dirs.project_shared_config(), PathBuf::from("/w/.mcp.json"));
        assert_eq!(
            dirs.project_agent_config(),
            PathBuf::from("/w/.cyrup/mcp.json")
        );
        // `getPiGlobalConfigPath(undefined)` vs `resolve(overridePath)`.
        assert_eq!(dirs.user_config(None), PathBuf::from("/a/agent/mcp.json"));
        assert_eq!(
            dirs.user_config(Some("mcp.json")),
            PathBuf::from("/w/mcp.json")
        );
        assert_eq!(
            dirs.user_config(Some("/etc/mcp.json")),
            PathBuf::from("/etc/mcp.json")
        );
    }

    #[test]
    fn resolve_from_folds_dot_segments_lexically() {
        let base = Path::new("/w/project");
        assert_eq!(resolve_from(base, "a/../b"), PathBuf::from("/w/project/b"));
        assert_eq!(
            resolve_from(base, "./a/./b/"),
            PathBuf::from("/w/project/a/b")
        );
        assert_eq!(
            resolve_from(base, "../sibling"),
            PathBuf::from("/w/sibling")
        );
        // `path.resolve("/a/../..") === "/"` — `..` cannot escape the root.
        assert_eq!(resolve_from(Path::new("/a"), "../.."), PathBuf::from("/"));
        // node's resolve does NOT expand `~`; a port that "helpfully" did would relocate secrets.
        assert_eq!(
            resolve_from(base, "~/creds"),
            PathBuf::from("/w/project/~/creds")
        );
    }

    #[test]
    fn oauth_dir_precedence_matches_get_auth_base_dir() {
        let dirs = McpDirs::new(PathBuf::from("/a/agent"), PathBuf::from("/w"));
        let none = |_: &str| None;
        let set = |_: &str| Some("  /env/oauth  ".to_string());
        let blank = |_: &str| Some("   ".to_string());

        // Tier 3: the default.
        assert_eq!(
            resolve_auth_base_dir(&dirs, None, &none),
            PathBuf::from("/a/agent/mcp-oauth")
        );
        // Tier 2: `settings.oauthDir`, resolved against cwd.
        let configured = resolve_configured_oauth_dir(" creds ", dirs.cwd()).unwrap();
        assert_eq!(configured, PathBuf::from("/w/creds"));
        assert_eq!(
            resolve_auth_base_dir(&dirs, Some(&configured), &none),
            configured
        );
        // Tier 1: the env var outranks both, trimmed but otherwise verbatim.
        assert_eq!(
            resolve_auth_base_dir(&dirs, Some(&configured), &set),
            PathBuf::from("/env/oauth")
        );
        // A whitespace-only override is not an override.
        assert_eq!(
            resolve_auth_base_dir(&dirs, Some(&configured), &blank),
            configured
        );
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
        assert_eq!(
            stable_stringify(&HashValue::String("a\"b\n".to_string())),
            "\"a\\\"b\\n\""
        );
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
    /// 64-hex `configHash`.
    ///
    /// **Provenance.** Both constants are **upstream's own**, produced by running
    /// `metadata-cache.ts`'s `stableStringify` + `computeServerHash` on node 22 against
    /// `tmp/pi-mcp-adapter` at tag `v2.26.1` (`fafae21`), and they now **include the `socket` key**
    /// — the fifteenth member, which upstream emits as `"socket":undefined` for every definition
    /// and which this pre-image used to drop. Before that key landed this vector read
    /// `4dd46c1fd26680867fe6c5ffdde2ab0f0a35972cd9c211bf6dd68d1f304eb277`, which was cyrup's digest
    /// and nobody else's; it is now byte-identical to pi's for the same definition.
    ///
    /// `cyrup_ext_subagents::exec::mcp_direct_tools::compute_mcp_server_hash` asserts the same two
    /// constants in `pre_image_matches_the_upstream_generated_golden_vector`.
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
                r#""includeTools":undefined,"protocolVersion":undefined,"#,
                r#""requestHeadersCommand":undefined,"socket":undefined,"url":undefined}"#
            )
        );
        assert_eq!(
            compute_server_hash(&entry, &resolved),
            "2190558e470a75c0f992989bd1799b374e669deecb8093e4118a1a9419068cf4"
        );
    }

    /// The HTTP half of the golden vector — `auth`, `protocolVersion`, `headers` and a `bearerToken`
    /// all present, so the two enum-valued identity fields are pinned on the wire.
    ///
    /// Upstream-generated on node 22 the same way as the vector above, and **including `socket`**
    /// (it read `572dcbaa…` while the key was missing).
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
                r#""requestHeadersCommand":undefined,"socket":undefined,"#,
                r#""url":"https://api.example.com/mcp"}"#
            )
        );
        assert_eq!(
            compute_server_hash(&entry, &resolved),
            "5ee9972ca350254322d2c8aa519f273144f1f80b256e1cfce5a1af21e3e75970"
        );
    }

    /// The empty definition — the case that makes the `undefined`-vs-`null` divergence total. Every
    /// one of the fifteen fields is absent, so a writer that emitted `null` would differ in fifteen
    /// places at once.
    ///
    /// Upstream-generated on node 22 and **including `socket`**: fifteen `undefined` tokens, not
    /// fourteen (the digest read `671c1578…` while the key was missing).
    #[test]
    fn golden_vector_empty_definition() {
        let entry = ServerEntry::default();
        let resolved = ResolvedIdentity::verbatim(&entry);
        let pre_image = server_identity_pre_image(&entry, &resolved);
        assert!(!pre_image.contains("null"));
        assert_eq!(pre_image.matches("undefined").count(), 15, "{pre_image}");
        assert!(pre_image.contains(r#""socket":undefined"#), "{pre_image}");
        assert_eq!(
            compute_server_hash(&entry, &resolved),
            "a04128961dff1d77f5ea95dd5ddb01415888636efe2d32cf950c78b34e54c3fa"
        );
    }

    /// The field v2.26.0 added (`metadata-cache.ts:94-101`) — the only nested object and the
    /// only *number* in the whole pre-image.
    ///
    /// Upstream-generated on node 22 against upstream's own `stableStringify(identity)`, the same
    /// way the three vectors above are, and **including `socket`** (it read `bace6621…` while the
    /// key was missing). The two properties it pins are the nested
    /// object's shape (four keys, sorted, `undefined` for each absent member) and the fact that the
    /// pre-image carries the **interpolated** command — which is what upstream's
    /// `"hashes the effective per-request header command"` test asserts
    /// (`__tests__/direct-tools.test.ts:357-379`).
    #[test]
    fn golden_vector_request_headers_command() {
        let entry: ServerEntry = serde_json::from_str(
            r#"{
                "url": "https://api.example.com/mcp",
                "requestHeadersCommand": {
                    "command": "node",
                    "args": ["sign.mjs", "${MCP_SIGNER_ACTOR}"],
                    "env": { "ACTOR": "$env:MCP_SIGNER_ACTOR" },
                    "timeoutMs": 2500
                }
            }"#,
        )
        .unwrap();

        let with_actor = |actor: &str| ResolvedIdentity {
            request_headers_command: Some(HttpRequestHeadersCommand {
                command: Some("node".to_string()),
                args: Some(vec!["sign.mjs".to_string(), actor.to_string()]),
                env: Some(BTreeMap::from([("ACTOR".to_string(), actor.to_string())]).into()),
                timeout_ms: Some(2500.0),
            }),
            ..ResolvedIdentity::verbatim(&entry)
        };
        let resolved = with_actor("actor-one");

        assert_eq!(
            server_identity_pre_image(&entry, &resolved),
            concat!(
                r#"{"args":undefined,"auth":undefined,"bearerToken":undefined,"#,
                r#""bearerTokenEnv":undefined,"command":undefined,"cwd":undefined,"#,
                r#""env":undefined,"excludeTools":undefined,"exposeResources":undefined,"#,
                r#""headers":undefined,"includeTools":undefined,"protocolVersion":undefined,"#,
                r#""requestHeadersCommand":{"args":["sign.mjs","actor-one"],"command":"node","#,
                r#""env":{"ACTOR":"actor-one"},"timeoutMs":2500},"socket":undefined,"#,
                r#""url":"https://api.example.com/mcp"}"#
            )
        );
        assert_eq!(
            compute_server_hash(&entry, &resolved),
            "7adc765f97381e1b0635190f397ef162364d648de67dc6c2be17942f6c0e3179"
        );

        // `timeoutMs` renders as `2500`, never `2500.0` — the pre-image is JS numbers.
        assert!(server_identity_pre_image(&entry, &resolved).contains(r#""timeoutMs":2500}"#));

        // A change to `$MCP_SIGNER_ACTOR` alone must evict the cached metadata, even though the
        // config text is untouched.
        assert_ne!(
            compute_server_hash(&entry, &resolved),
            compute_server_hash(&entry, &with_actor("actor-two"))
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
        let entry: ServerEntry =
            serde_json::from_str(r#"{ "url": "https://${HOST}/mcp" }"#).unwrap();
        let literal = ResolvedIdentity::verbatim(&entry);
        let resolved = ResolvedIdentity {
            url: Some("https://a.example/mcp".to_string()),
            ..literal.clone()
        };
        assert_ne!(
            compute_server_hash(&entry, &literal),
            compute_server_hash(&entry, &resolved)
        );
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
        assert!(!is_server_cache_valid_at(
            &fresh,
            "other",
            CACHE_MAX_AGE_MS,
            now
        ));
        // `!entry.cachedAt` rejects 0 as well as absent.
        assert!(!is_server_cache_valid_at(
            &entry_with("h", 0),
            "h",
            CACHE_MAX_AGE_MS,
            now
        ));
        // Exactly at the boundary is still valid — upstream's test is `>`, not `>=`.
        let boundary = entry_with("h", now - CACHE_MAX_AGE_MS);
        assert!(is_server_cache_valid_at(
            &boundary,
            "h",
            CACHE_MAX_AGE_MS,
            now
        ));
        let stale = entry_with("h", now - CACHE_MAX_AGE_MS - 1);
        assert!(!is_server_cache_valid_at(
            &stale,
            "h",
            CACHE_MAX_AGE_MS,
            now
        ));
        // `maxAgeMs <= 0` disables the age check entirely.
        assert!(is_server_cache_valid_at(&stale, "h", 0, now));
    }

    fn cache_with(names: &[&str]) -> MetadataCache {
        let mut cache = MetadataCache::default();
        for name in names {
            cache
                .servers
                .insert((*name).to_string(), entry_with("h", 42));
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
        second
            .servers
            .insert("beta".to_string(), entry_with("h2", 43));
        second
            .servers
            .insert("gamma".to_string(), entry_with("h3", 44));
        save_metadata_cache(&path, &second).unwrap();

        let loaded = load_metadata_cache(&path).unwrap();
        assert_eq!(loaded.version, CACHE_VERSION);
        // Existing keys keep their position; new keys append — JS object-spread order.
        assert_eq!(
            loaded
                .servers
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["alpha", "beta", "gamma"]
        );
        assert_eq!(
            loaded.servers.get("beta").map(|e| e.config_hash.as_str()),
            Some("h2")
        );

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

        assert!(
            load_metadata_cache(&path).is_none(),
            "a missing file is not an error"
        );
        std::fs::write(&path, "{{{").unwrap();
        assert!(load_metadata_cache(&path).is_none());
        std::fs::write(&path, r#"{"version":2,"servers":{}}"#).unwrap();
        assert!(load_metadata_cache(&path).is_none());
        std::fs::write(&path, r#"{"version":1}"#).unwrap();
        assert!(
            load_metadata_cache(&path).is_none(),
            "a missing `servers` is a rejected file"
        );
    }

    /// The on-disk shape `cyrup_ext_subagents::exec::mcp_direct_tools` reads: `version`,
    /// `servers.<name>.{configHash,tools[].name,resources[].{uri,name},cachedAt}`, 2-space indent,
    /// no trailing newline. Optional members are omitted rather than emitted as `null` — the
    /// reader's `Option` fields tolerate both, but pi omits, and a byte-comparable file is worth
    /// more than a tolerant one (MCP-077, MCP-094).
    ///
    /// The two readers of `mcp-cache.json` must agree about what is present. Before integration
    /// this file's reader was strict where `crate::registration`'s is lenient, so a cache missing
    /// one key made the panel see nothing while the registered surface saw everything.
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
    // -------------------------------------------------------------------------------------
    // MCP-141 leg (b) / MCP-143 / MCP-144 — the resolvers actually run now
    // -------------------------------------------------------------------------------------

    /// The fixture environment the resolved vectors below were generated against.
    fn vector_env() -> EnvFn {
        std::sync::Arc::new(|name: &str| match name {
            "HOST" => Some("a.example".to_string()),
            "TOK_ENV" => Some("from-env".to_string()),
            _ => None,
        })
    }

    /// `/home/u` — the `homedir()` the node run that produced the vectors below saw
    /// (`process.env.HOME = "/home/u"` before importing, asserted by printing `homedir()`).
    fn vector_home() -> PathBuf {
        PathBuf::from("/home/u")
    }

    /// The four golden vectors above use [`ResolvedIdentity::verbatim`]; this proves that is not a
    /// weaker assertion. None of their fields carries an interpolation token, a `~` or a `!`, so
    /// the real resolvers are the identity on them — which is why MCP-141 leg (b) could land
    /// without moving a single pinned digest.
    #[test]
    fn resolve_and_verbatim_agree_when_nothing_needs_resolving() {
        for json in [
            r#"{ "command": "npx", "args": ["-y", "x"], "env": { "A": "1" }, "cwd": "/home/u/work",
                 "exposeResources": false, "excludeTools": ["danger_*"] }"#,
            r#"{ "url": "https://api.example.com/mcp", "headers": { "X": "k" }, "auth": "bearer",
                 "protocolVersion": "2026-07-28", "bearerToken": "!tok", "bearerTokenEnv": "TOK_ENV" }"#,
            r#"{}"#,
        ] {
            let entry: ServerEntry = serde_json::from_str(json).unwrap();
            let resolved =
                ResolvedIdentity::resolve(&entry, &vector_env(), &vector_home()).expect("hashable");
            assert_eq!(resolved, ResolvedIdentity::verbatim(&entry), "{json}");
        }
    }

    /// **The vector that proves the resolvers run**, and the one MCP-141 leg (b) exists for.
    ///
    /// Every resolver is exercised at once: all three interpolation forms (MCP-143) in `env`, the
    /// `!!` escape and the `!` marker (MCP-144), a `~`-prefixed and interpolated `cwd`, an
    /// interpolated `url`, an interpolated header beside a literal-`!` header, and a `bearerToken`
    /// supplied by `bearerTokenEnv`.
    ///
    /// **Provenance.** Both constants come from *running upstream* on node 22 against
    /// `tmp/pi-mcp-adapter` at tag `v2.26.1` (`fafae21`): `computeServerHash` for the digest, and
    /// `metadata-cache.ts:344`'s `stableStringify` over `computeServerHash`'s own identity literal —
    /// built from upstream's real `interpolateEnvRecord` / `resolveConfigPath` / `resolveServerUrl` /
    /// `resolveBearerToken` — for the pre-image. The reconstruction was proved faithful by asserting
    /// `sha256(preImage) === computeServerHash(definition)` for this and every other fixture before
    /// the `socket` member landed.
    ///
    /// **Both constants include `socket`**, so they are upstream's, unqualified: `ac61954a…` is the
    /// digest a stock `pi-mcp-adapter` computes for this definition, and it is the digest cyrup
    /// computes for it. While the key was missing this vector read
    /// `c273715eef4b2fb58f5db61d54793b01abd262edd7e59a5c7b189fddf910bd3c`, which differed from
    /// upstream by exactly the `"socket":undefined` member.
    #[test]
    fn the_resolved_identity_golden_vector() {
        let entry: ServerEntry = serde_json::from_str(
            r#"{
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
            }"#,
        )
        .unwrap();
        let resolved =
            ResolvedIdentity::resolve(&entry, &vector_env(), &vector_home()).expect("hashable");

        assert_eq!(
            server_identity_pre_image(&entry, &resolved),
            concat!(
                r#"{"args":["-y","srv"],"auth":undefined,"bearerToken":"from-env","#,
                r#""bearerTokenEnv":"TOK_ENV","command":"npx","cwd":"/home/u/work/a.example","#,
                r#""env":{"BRACE":"a.example","DOLLAR":"a.example","ESCAPED":"!a.example","#,
                r#""INTERP":"a.example","MARKER":"!op read x","PLAIN":"p"},"#,
                r#""excludeTools":["b"],"exposeResources":false,"#,
                r#""headers":{"X-Host":"a.example","X-Lit":"!keep"},"includeTools":["a"],"#,
                r#""protocolVersion":undefined,"requestHeadersCommand":undefined,"#,
                r#""socket":undefined,"url":"https://a.example/mcp"}"#
            )
        );
        assert_eq!(
            try_compute_server_hash(&entry, &vector_env(), &vector_home()).expect("hashable"),
            "ac61954adda845c50a6c691e7ac291e2546dfcc6158b8d6a1b7785ce47356de3"
        );

        // …and the same definition under `verbatim` is a DIFFERENT digest — which is the whole
        // point: the writer used to stamp that one while the reader validated against this one.
        assert_ne!(
            compute_server_hash(&entry, &ResolvedIdentity::verbatim(&entry)),
            try_compute_server_hash(&entry, &vector_env(), &vector_home()).expect("hashable")
        );
    }

    /// MCP-144 on the two `bearerToken` arms, pinned by digest.
    ///
    /// `!!${HOST}` loses exactly **one** `!` and interpolates the rest, so it hashes as the value
    /// the transport will send; `!op read tok` hashes as its own text and is never executed. Both
    /// digests are upstream-generated on node 22 the same way as the vector above, and both
    /// **include `socket`** (they read `bdc09d45…` and `ffcfe72a…` while the key was missing).
    #[test]
    fn the_secret_expression_grammar_is_in_the_digest() {
        let escaped: ServerEntry = serde_json::from_str(
            r#"{ "url": "https://api.example.com/mcp", "bearerToken": "!!${HOST}" }"#,
        )
        .unwrap();
        let resolved =
            ResolvedIdentity::resolve(&escaped, &vector_env(), &vector_home()).expect("hashable");
        assert_eq!(resolved.bearer_token.as_deref(), Some("!a.example"));
        assert_eq!(
            compute_server_hash(&escaped, &resolved),
            "1369c625f92c11b211de41532e1f86e9395a38c2afcb2a9d0f11b1a75d6a80bb"
        );

        let marker: ServerEntry = serde_json::from_str(
            r#"{ "url": "https://api.example.com/mcp", "bearerToken": "!op read tok" }"#,
        )
        .unwrap();
        let resolved =
            ResolvedIdentity::resolve(&marker, &vector_env(), &vector_home()).expect("hashable");
        assert_eq!(resolved.bearer_token.as_deref(), Some("!op read tok"));
        assert_eq!(
            compute_server_hash(&marker, &resolved),
            "b47379b5afec6de30b426546be3fa984693110d3d3e7fa412b94747e359a5d2b"
        );
    }

    /// MCP-141's stated behaviour — "a URL server referencing a missing variable must never be
    /// cache-valid" — and MCP-145's catcher.
    ///
    /// All three messages are byte-exact against a node 22 run of `utils.ts` @ v2.26.1: the singular
    /// and plural forms of the missing-variable throw (`"https://x/${NOPE}"` and
    /// `"https://x/${NOPE}/${ALSONOPE}"`) and the post-interpolation parse failure (`"${HOST}"`,
    /// which interpolates to the bare host `a.example` and is not a URL).
    #[test]
    fn an_unresolvable_url_throws_and_the_throw_reaches_the_cache_predicate() {
        let cases = [
            (
                r#"{ "url": "https://x.example/${NOPE}/mcp" }"#,
                "Missing environment variable in MCP server URL: NOPE",
            ),
            (
                r#"{ "url": "https://x.example/${NOPE}/{env:ALSONOPE}" }"#,
                "Missing environment variables in MCP server URL: NOPE, ALSONOPE",
            ),
            (
                r#"{ "url": "$env:HOST" }"#,
                "Invalid MCP server URL after environment interpolation: a.example",
            ),
        ];
        for (json, message) in cases {
            let entry: ServerEntry = serde_json::from_str(json).unwrap();
            let error = try_compute_server_hash(&entry, &vector_env(), &vector_home())
                .expect_err(json)
                .to_string();
            assert_eq!(error, message, "{json}");
        }

        // The third pattern raises the throw too — `getMissingEnvVars` scans all three forms
        // (MCP-143), so `{env:MISSING}` in a URL is as fatal as `${MISSING}`.
        let brace: ServerEntry =
            serde_json::from_str(r#"{ "url": "https://x.example/{env:NOPE}" }"#).unwrap();
        assert!(try_compute_server_hash(&brace, &vector_env(), &vector_home()).is_err());
    }

    // -------------------------------------------------------------------------------------
    // MCP-140 — the serialisers
    // -------------------------------------------------------------------------------------

    #[test]
    fn serialize_tools_drops_nameless_tools_and_writes_only_defined_fields() {
        let mut described = rmcp::model::Tool::new(
            "search",
            "find things",
            std::sync::Arc::new(
                serde_json::json!({ "type": "object" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        );
        described.meta = Some(rmcp::model::MetaObject(
            serde_json::json!({ "ui": { "visibility": ["model", "model", "app"] } })
                .as_object()
                .unwrap()
                .clone(),
        ));
        let bare = rmcp::model::Tool::new("bare", "", std::sync::Arc::new(serde_json::Map::new()));
        let nameless =
            rmcp::model::Tool::new("", "dropped", std::sync::Arc::new(serde_json::Map::new()));

        let cached = serialize_tools(&[described, bare, nameless]);
        assert_eq!(cached.len(), 2, "the nameless tool is filtered out");
        assert_eq!(cached[0].name, "search");
        assert_eq!(cached[0].description.as_deref(), Some("find things"));
        // Deduplicated, in first-occurrence order.
        assert_eq!(
            cached[0].ui_visibility.as_deref(),
            Some(["model".to_string(), "app".to_string()].as_slice())
        );
        // Both Cut-2 fields stay absent — never `null`, and never a bumped CACHE_VERSION.
        assert!(cached[0].ui_resource_uri.is_none());
        assert!(cached[0].ui_stream_mode.is_none());
        assert!(
            cached[1].ui_visibility.is_none(),
            "no `_meta` means visible, not hidden"
        );

        let json = serde_json::to_string(&cached[1]).unwrap();
        assert!(!json.contains("uiVisibility"), "{json}");
        assert!(
            !json.contains("null"),
            "an absent field is omitted, never written as null: {json}"
        );
    }

    /// `extractUiToolVisibility`'s fail-closed arms — the two that answer `Some(vec![])` rather
    /// than `None`, which [`crate::registration::is_ui_tool_visible_to_model`] reads as "hidden".
    #[test]
    fn ui_visibility_extraction_fails_closed_on_anything_unrecognised() {
        let visibility = |value: serde_json::Value| extract_ui_tool_visibility(Some(&value));
        assert_eq!(visibility(serde_json::json!({})), None, "no `ui` key");
        assert_eq!(
            visibility(serde_json::json!({ "ui": ["model"] })),
            None,
            "`ui` is an array"
        );
        assert_eq!(visibility(serde_json::json!({ "ui": null })), None);
        assert_eq!(
            visibility(serde_json::json!({ "ui": { "other": 1 } })),
            None
        );
        assert_eq!(
            visibility(serde_json::json!({ "ui": { "visibility": "model" } })),
            Some(Vec::new()),
            "present but not an array is NOT absent"
        );
        assert_eq!(
            visibility(serde_json::json!({ "ui": { "visibility": ["model", "future"] } })),
            Some(Vec::new()),
            "one unrecognised element discards the whole list"
        );
        assert_eq!(
            visibility(serde_json::json!({ "ui": { "visibility": [] } })),
            Some(Vec::new())
        );
        for empty in [Some(Vec::new()), None] {
            assert_eq!(
                crate::registration::is_ui_tool_visible_to_model(
                    empty
                        .as_ref()
                        .map(|v: &Vec<String>| serde_json::json!(v))
                        .as_ref()
                ),
                empty.is_none(),
                "an empty list must hide, an absent list must show"
            );
        }
    }

    #[test]
    fn serialize_resources_needs_both_a_name_and_a_uri() {
        let good =
            rmcp::model::Resource::new("file:///a", "My File").with_description("the a file");
        let no_description = rmcp::model::Resource::new("file:///b", "B");
        let no_name = rmcp::model::Resource::new("file:///c", "");
        let no_uri = rmcp::model::Resource::new("", "D");

        let cached = serialize_resources(&[good, no_description, no_name, no_uri]);
        assert_eq!(cached.len(), 2);
        assert_eq!(cached[0].uri, "file:///a");
        assert_eq!(cached[0].name, "My File");
        assert_eq!(cached[0].description.as_deref(), Some("the a file"));
        // The `Read resource: {uri}` fallback belongs to the reconstructor, not here.
        assert!(cached[1].description.is_none());
    }

    #[test]
    fn serialize_prompts_keeps_absent_and_empty_arguments_distinct() {
        let no_arguments = rmcp::model::Prompt::new("plain", Some("d"), None);
        let empty_arguments = rmcp::model::Prompt::new("empty", None::<String>, Some(Vec::new()));
        let with_arguments = rmcp::model::Prompt::new(
            "args",
            None::<String>,
            Some(vec![
                rmcp::model::PromptArgument::new("topic")
                    .with_description("what about")
                    .with_required(true),
                rmcp::model::PromptArgument::new(""),
            ]),
        );
        let nameless = rmcp::model::Prompt::new("", None::<String>, None);

        let cached = serialize_prompts(&[no_arguments, empty_arguments, with_arguments, nameless]);
        assert_eq!(cached.len(), 3, "only the nameless prompt is dropped");
        assert!(cached[0].arguments.is_none(), "absent stays absent");
        assert_eq!(
            cached[1].arguments.as_deref().map(<[_]>::len),
            Some(0),
            "`[]` stays `[]`"
        );
        let arguments = cached[2].arguments.as_deref().expect("arguments");
        assert_eq!(
            arguments.len(),
            1,
            "the nameless argument is dropped, the prompt is not"
        );
        assert_eq!(arguments[0].name, "topic");
        assert_eq!(arguments[0].required, Some(true));

        // Absent and empty are different BYTES, which is the only place the difference shows.
        assert!(
            !serde_json::to_string(&cached[0])
                .unwrap()
                .contains("arguments")
        );
        assert!(
            serde_json::to_string(&cached[1])
                .unwrap()
                .contains(r#""arguments":[]"#)
        );
    }

    // -------------------------------------------------------------------------------------
    // MCP-139 / MCP-145 — save over a corrupt file, and the two `cachedAt` rules
    // -------------------------------------------------------------------------------------

    /// §3.17's save contract on the path the merge cannot take: a file that does not parse, or
    /// declares a foreign version, is **replaced** rather than merged, so the result holds only the
    /// new servers — and it is a valid file afterwards.
    #[test]
    fn a_save_over_a_corrupt_file_replaces_it_with_only_the_new_servers() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("mcp-cache.json");

        for corrupt in [
            "{ not json",
            "",
            "null",
            r#"{"version":9,"servers":{"old":{}}}"#,
        ] {
            std::fs::write(&path, corrupt).unwrap();
            save_metadata_cache(&path, &cache_with(&["fresh"])).unwrap();

            let loaded = load_metadata_cache(&path).expect("valid after the replace");
            assert_eq!(loaded.version, CACHE_VERSION);
            assert_eq!(loaded.servers.len(), 1, "{corrupt}");
            assert!(loaded.servers.contains_key("fresh"), "{corrupt}");
            assert!(
                !loaded.servers.contains_key("old"),
                "a foreign version is not merged"
            );
            assert!(
                std::fs::read_dir(temp.path()).unwrap().count() == 1,
                "the pid-suffixed temp file must be gone"
            );
        }
    }

    /// MCP-145's two `cachedAt` rules on the writer's own reader.
    #[test]
    fn cached_at_must_be_a_number_and_max_age_zero_disables_the_age_check() {
        let year = 365 * 24 * 60 * 60 * 1000;
        let old = entry_with("h", now_ms() - year);
        assert!(
            !is_server_cache_valid(&old, "h", CACHE_MAX_AGE_MS),
            "a year is older than 7 days"
        );
        assert!(
            is_server_cache_valid(&old, "h", 0),
            "`maxAgeMs = 0` disables the age check"
        );

        // A JSON string `cachedAt` must invalidate THIS entry without costing the file its others.
        let file = serde_json::json!({
            "version": CACHE_VERSION,
            "servers": {
                "bad": { "configHash": "h", "cachedAt": "1760000000000", "tools": [], "resources": [] },
                "good": { "configHash": "h", "cachedAt": 1_760_000_000_000_i64, "tools": [], "resources": [] }
            }
        });
        let parsed: MetadataCache = serde_json::from_str(&file.to_string()).expect("still parses");
        assert_eq!(
            parsed.servers.len(),
            2,
            "one bad entry must not lose the whole file"
        );
        assert_eq!(
            parsed.servers["bad"].cached_at, 0,
            "a non-number lands on the falsy value"
        );
        assert!(!is_server_cache_valid(&parsed.servers["bad"], "h", 0));
        assert!(is_server_cache_valid(&parsed.servers["good"], "h", 0));
    }

    #[test]
    fn a_cache_missing_optional_keys_still_loads_here_and_in_registration() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = McpDirs::new(dir.path().to_path_buf(), dir.path().to_path_buf());
        let path = dirs.metadata_cache();
        // No `resources`, no `configHash`, a tool with only a name.
        std::fs::write(
            &path,
            format!(
                r#"{{"version":{CACHE_VERSION},"servers":{{"linear":{{"tools":[{{"name":"list_issues"}}],"cachedAt":1}}}}}}"#
            ),
        )
        .unwrap();

        let cache = load_metadata_cache(&path).expect("a partial cache still loads");
        let entry = cache.servers.get("linear").expect("the server survives");
        assert_eq!(entry.tools.len(), 1);
        assert!(entry.resources.is_empty());
        assert_eq!(entry.config_hash, "");

        // And the lenient reader agrees, which is the invariant that matters.
        let other = crate::registration::load_metadata_cache(&dirs).expect("both readers agree");
        assert_eq!(
            other.servers.get("linear").map(|e| e.tools().len()),
            Some(1)
        );
    }
}
