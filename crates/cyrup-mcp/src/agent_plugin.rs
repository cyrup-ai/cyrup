//! The Agent Plugin translator — `agent-plugin-loader.ts` (13a §25, MCP-047).
//!
//! A vendor-neutral plugin format: a third-party directory carrying a manifest that contributes MCP
//! servers. **Every rule in this file is a containment boundary around third-party content**, which
//! is why MCP-047 is one of the port's fourteen criticals. A plugin must not be able to run an
//! arbitrary binary from outside its own directory, read an environment variable through
//! interpolation, or reach a non-loopback plaintext HTTP endpoint.
//!
//! (MCP-047's prose names the destination `cyrup_mcp::config::agent_plugin`; MCP-001's enumerated
//! module set puts it at the crate root. Same code, one level up.)
//!
//! # The ruleset, so it is not re-derived from the diff
//!
//! * two `$schema` **equality** checks — manifest and `mcp.json`;
//! * a plugin-name regex with a 1..64 length bound;
//! * four field allowlists;
//! * an unknown top-level key in the plugin's `mcp.json` discards the **whole file**, not the key;
//! * `command` must be bare or `./…`, resolved through `resolveContainedPath`;
//! * `cwd` restricted to `./…` / `${PLUGIN_ROOT}…` / `${PLUGIN_DATA}…`;
//! * `env` may not define `PLUGIN_ROOT` or `PLUGIN_DATA` — the loader injects those itself,
//!   together with `literalEnv: true`, and that pair is what stops env interpolation from being a
//!   read primitive;
//! * URLs reject any `${` / `$env:` / `{env:`, any userinfo and any fragment, and permit plain
//!   `http:` **only** for loopback;
//! * headers are deduplicated case-insensitively and validated by constructing real header
//!   name/value types;
//! * servers are namespaced `` `${plugin}__${server}` ``;
//! * **warn and skip, everywhere. Nothing here ever throws.** First writer wins on a duplicate.
//!
//! # Four Rust-specific care points (MCP-047)
//!
//! 1. **`resolveContainedPath`.** Node's `path.relative` plus the `..`/separator/absolute test must
//!    be reproduced by normalising `..` components **lexically** before comparing, because Rust's
//!    `Path` does not normalise and `strip_prefix` would happily accept `root/../../etc`. Do **not**
//!    resolve symlinks — upstream does not, and resolving them would be a stricter and therefore
//!    divergent check. See [`resolve_contained_path`], plus `lexical_normalize` / `lexical_relative`.
//! 2. **The plugin-name regex uses a negative lookahead**, which the `regex` crate rejects. It is
//!    expressed as `!name.contains("--") && !name.contains("..")` plus the plain character class.
//! 3. **Header validation** needs real header parsers. The plan named
//!    `reqwest::header::{HeaderName, HeaderValue}`; `reqwest` is **not** a direct dependency of this
//!    crate (it arrives only transitively through `rmcp`), and — more to the point — `http`'s
//!    validation is *not* the rule upstream applies. Upstream constructs a WHATWG `new Headers(...)`,
//!    whose rules are narrower on names and wider on values than `http`'s. Both are reproduced here
//!    directly against the Fetch spec: see `is_valid_header_name` / `is_valid_header_value`.
//! 4. **`resolvePluginPath` anchors `~` on `std::env::var_os("HOME")`** — a *different* source from
//!    [`crate::dirs`]'s home. The split is reproduced: this module never calls
//!    [`crate::dirs::expand_tilde`], which trims its input and is anchored on the OS home.
//!
//! Cut 1 narrows the accepted `type` set to `{"stdio", "streamable-http"}`; a `type: "sse"` entry is
//! skipped with the existing `unsupported type` reason. That case is **live**, not theoretical:
//! `agent-plugin-loader.ts:185` sets `httpTransport` directly from the manifest's `type`, so silently
//! ignoring it would produce a server that appears configured and never connects.
//!
//! # What the returned skip tuple carries
//!
//! Upstream's only output for a rejection is a `console.warn` line. The port keeps that line — it is
//! the `String` half of every `(String, SkipReason)` this module returns, formatted **byte-for-byte**
//! as upstream formats it, and simultaneously emitted through `tracing::warn!`. The [`SkipReason`]
//! half is the machine-readable category the `/mcp` panel groups by. There is one `SkipReason` per
//! distinct upstream `console.warn` site, so the mapping is 1:1 and auditable against
//! `agent-plugin-loader.ts` line by line.
//!
//! # Ordering is load-bearing
//!
//! `mcpServers` is iterated with `Object.entries`, i.e. **insertion order**, and first-writer-wins
//! on a normalised-name collision reads directly off that order — upstream's own test asserts that
//! `"tools.db"` beats `"tools_db"` because it is written first, not because it sorts first. Both the
//! server map and each entry's own key map are therefore [`IndexMap`], never `serde_json::Map`
//! (which is a `BTreeMap` under this workspace's feature set and would silently sort both).

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf, MAIN_SEPARATOR, MAIN_SEPARATOR_STR};
use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::config::{HttpTransport, ServerEntry};
use crate::dirs::McpDirs;

/// The environment variable name a plugin's `cwd`/`env` may reference for its own root, and which
/// its `env` may **not** define.
pub const PLUGIN_ROOT_VAR: &str = "PLUGIN_ROOT";

/// As [`PLUGIN_ROOT_VAR`], for the plugin's writable data directory
/// (`<agent_dir>/agent-plugin-data/<name>`).
pub const PLUGIN_DATA_VAR: &str = "PLUGIN_DATA";

/// The separator between the plugin name and the server name in the namespaced key —
/// `` `${plugin}__${server}` `` (`agent-plugin-loader.ts:253`).
pub const NAMESPACE_SEPARATOR: &str = "__";

/// `agent-plugin-loader.ts:6`. Compared for **equality**, never as a prefix or a version range: a
/// plugin declaring `…/1.1.0/plugin.schema.json` is refused rather than best-effort parsed.
const PLUGIN_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";

/// `agent-plugin-loader.ts:7`. Equality, as [`PLUGIN_SCHEMA`].
const MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

/// `agent-plugin-loader.ts:9-20`. An unknown manifest key is **warned about only** — unlike the
/// `mcp.json` allowlist below, which discards the whole file.
const PLUGIN_MANIFEST_FIELDS: &[&str] = &[
    "$schema",
    "name",
    "version",
    "description",
    "author",
    "homepage",
    "repository",
    "license",
    "keywords",
    "extensions",
];

/// `agent-plugin-loader.ts:21`. An unknown key here discards **every** server in the file.
///
/// [`RawPluginMcpDocument`] declares both of these as real fields, so `serde`'s flatten catch-all
/// never sees them and the filter in [`translate_plugin_mcp_config`] is a no-op *today*. It is kept
/// because the constant — not the struct — is the stated allowlist: if the upstream schema grows a
/// key this loader does not yet read, adding it here is the one-line change that tolerates it, and
/// the check then does real work.
const MCP_CONFIG_FIELDS: &[&str] = &["$schema", "mcpServers"];

/// `agent-plugin-loader.ts:22`. Note what is *absent*: no `lifecycle`, no `approveTools`, no
/// `directTools`, no `debug`. A plugin may not opt itself out of any gate.
const STDIO_FIELDS: &[&str] = &["type", "command", "args", "env", "cwd"];

/// `agent-plugin-loader.ts:23`. Note the absence of `auth`, `bearerToken` and `oauth`: a plugin
/// cannot bind itself to the user's credential store.
const HTTP_FIELDS: &[&str] = &["type", "url", "headers"];

/// `resolve(pluginRoot, "plugin.json")` — `agent-plugin-loader.ts:92`.
const PLUGIN_MANIFEST_FILE: &str = "plugin.json";

/// `resolve(pluginRoot, "mcp.json")` — `agent-plugin-loader.ts:73`.
const PLUGIN_MCP_FILE: &str = "mcp.json";

/// The character-class half of `PLUGIN_NAME_PATTERN` (`agent-plugin-loader.ts:8`). The full upstream
/// pattern is `/^(?!.*(?:--|\.\.))[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$/`; the `regex` crate refuses the
/// negative lookahead by design (it would cost the linear-time guarantee), so the lookahead is
/// hoisted into two `contains` calls in [`is_valid_plugin_name`] — care point 2.
///
/// `Regex::new` on a literal pattern cannot fail; the `Option` exists so the impossible branch is a
/// **refusal** rather than an `expect`, which the crate lints deny.
static PLUGIN_NAME_CLASS: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new("^[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$").ok());

/// A plugin's manifest, as far as this loader reads it.
///
/// Upstream returns only `{name}` (`agent-plugin-loader.ts:132`); `schema` and `version` are carried
/// here for the `/mcp` panel and are never acted upon. Both are populated **only** when the JSON
/// value is a string — a numeric `version` leaves this `None` rather than failing the plugin,
/// because upstream never reads the field at all and must not start rejecting plugins over it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPluginManifest {
    /// Checked for **equality** against the expected schema URL — not a prefix match, not a
    /// version range.
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// The plugin's identity. Bounded 1..64, restricted character class, and neither `--` nor `..`
    /// may appear (upstream expresses that with a negative lookahead the `regex` crate rejects).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Human-readable version, carried for diagnostics only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// One server a plugin contributed, plus the reason it was accepted or the reason it was skipped.
#[derive(Debug, Clone)]
pub struct LoadedPluginServer {
    /// The namespaced key: `` `${plugin}__${server}` ``.
    pub name: String,
    /// The translated entry, already carrying the loader's injected `pluginDataDir` and
    /// `literalEnv: true`.
    pub entry: ServerEntry,
    /// The plugin directory this came from.
    pub root: PathBuf,
}

/// `AgentPluginSummary` (`agent-plugin-loader.ts:29-33`) — what `getConfigSummary` shows in the
/// onboarding panel and folds into its discovery fingerprint (`config.ts:254`, `config.ts:406`).
#[derive(Debug, Clone)]
pub struct AgentPluginSummary {
    /// The **resolved** plugin root, not the configured string.
    pub path: PathBuf,
    /// `None` when the manifest was missing or invalid — which is also when
    /// [`Self::server_count`] is forced to zero.
    pub name: Option<String>,
    /// Servers this plugin contributed *after* its own intra-plugin collision pass and *before*
    /// the cross-plugin one, exactly as upstream counts them.
    pub server_count: usize,
}

/// Why a plugin, or one of its servers, was skipped.
///
/// One variant per `console.warn` site in `agent-plugin-loader.ts`, so the taxonomy can be audited
/// against the upstream file rather than inferred. Nothing here ever throws; every arm is a warn and
/// a skip. `#[non_exhaustive]` because the upstream diagnostic set is the only thing that bounds it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkipReason {
    /// `plugin.json` is absent, or is not a regular file (`agent-plugin-loader.ts:93-100`).
    MissingManifest,
    /// `plugin.json` did not parse, or did not parse to a non-array object
    /// (`agent-plugin-loader.ts:102-112`).
    UnreadableManifest,
    /// The manifest's or `mcp.json`'s `$schema` did not match exactly
    /// (`agent-plugin-loader.ts:120`, `:148`).
    SchemaMismatch,
    /// The plugin name failed the character class, the length bound, or the `--`/`..` test
    /// (`agent-plugin-loader.ts:124`).
    InvalidPluginName,
    /// `mcp.json` exists but is not a regular file, did not parse, was not an object, or its
    /// `mcpServers` was not an object (`agent-plugin-loader.ts:75`, `:83`, `:136`, `:152`).
    UnreadableMcpConfig,
    /// An unknown top-level key in the plugin's `mcp.json` — which discards the **whole file**
    /// (`agent-plugin-loader.ts:143-146`; a `return`, not a `continue`).
    UnknownTopLevelKey(String),
    /// A `mcpServers` value that is not a non-array object (`agent-plugin-loader.ts:178`).
    NotAnObject,
    /// A server key outside `STDIO_FIELDS` / `HTTP_FIELDS` (`agent-plugin-loader.ts:198`,
    /// `:236`). The allowlist is per-transport, so `url` on a `stdio` entry lands here.
    UnknownField(String),
    /// `type` outside `{"stdio", "streamable-http"}` — which after Cut 1 includes `"sse"`
    /// (`agent-plugin-loader.ts:187`). The payload is the offending value for machine consumers;
    /// upstream's warning text does not name it.
    UnsupportedType(String),
    /// `command` was not a non-empty string, or was neither bare nor `./…`
    /// (`agent-plugin-loader.ts:200-201`).
    InvalidCommand,
    /// A `./…` `command` that escapes the plugin root (`agent-plugin-loader.ts:209`).
    CommandEscapesPluginRoot,
    /// `args` was present but was not an array of strings (`agent-plugin-loader.ts:263`).
    InvalidArgs,
    /// `env` was present but was not an object of strings (`agent-plugin-loader.ts:272`, `:282`).
    InvalidEnv,
    /// `env` attempted to define [`PLUGIN_ROOT_VAR`] or [`PLUGIN_DATA_VAR`]
    /// (`agent-plugin-loader.ts:278`). Half of what stops env interpolation being a read primitive.
    EnvShadowsInjectedVar(String),
    /// A `cwd` outside `./…` / `${PLUGIN_ROOT}…` / `${PLUGIN_DATA}…`
    /// (`agent-plugin-loader.ts:213`).
    CwdOutsidePlugin,
    /// `url` was not a non-empty string (`agent-plugin-loader.ts:238`).
    InvalidUrl,
    /// A URL carrying interpolation syntax, userinfo, a fragment, or plain `http:` to a
    /// non-loopback host (`agent-plugin-loader.ts:239`).
    UnsafeUrl,
    /// `headers` was not an object of strings, or was not a valid set of HTTP fields
    /// (`agent-plugin-loader.ts:293`, `:300`, `:315`).
    InvalidHeaders,
    /// Two header keys differing only in case (`agent-plugin-loader.ts:305`). Upstream rejects the
    /// whole server rather than picking a winner.
    DuplicateHeader(String),
    /// A namespaced name this load already produced, intra-plugin
    /// (`agent-plugin-loader.ts:163`) or cross-plugin (`agent-plugin-loader.ts:41`).
    /// **First writer wins** in both cases.
    DuplicateServer(String),
}

/// The `(warning, category)` pair every rejection produces. See the module docs.
type SkipOutcome = (String, SkipReason);

// ---------------------------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------------------------

/// `loadAgentPluginConfigs(paths, cwd = process.cwd())` — `agent-plugin-loader.ts:35`.
///
/// This is the defaulted-`cwd` arm: relative plugin paths resolve against the **process** working
/// directory, exactly as upstream's default parameter does. The config ladder should call
/// [`load_agent_plugins_in`] instead, which takes the session cwd explicitly — `config.ts:311`
/// threads its own `cwd` through and never relies on the default.
///
/// `getPluginPaths` (`agent-plugin-loader.ts:64`) filters a `unknown` value down to its `string`
/// members and yields `[]` for a non-array; `&[String]` makes both halves structurally impossible,
/// so the filter has no port.
#[must_use]
pub fn load_agent_plugins(
    plugin_paths: &[String],
    agent_dir: &Path,
) -> (Vec<LoadedPluginServer>, Vec<SkipOutcome>) {
    let cwd = std::env::current_dir().unwrap_or_default();
    load_agent_plugins_in(plugin_paths, &McpDirs::new(agent_dir.to_path_buf(), cwd))
}

/// [`load_agent_plugins`] with both anchors named: `dirs.cwd()` resolves relative plugin paths and
/// `dirs.agent_dir()` is where each plugin's `${PLUGIN_DATA}` lands.
///
/// Servers come back in file order, which is the order they must keep: it is the order they connect
/// in, the order `/mcp` lists them, and the tie-break the tool-name collision resolver uses.
#[must_use]
pub fn load_agent_plugins_in(
    plugin_paths: &[String],
    dirs: &McpDirs,
) -> (Vec<LoadedPluginServer>, Vec<SkipOutcome>) {
    let mut servers: Vec<LoadedPluginServer> = Vec::new();
    let mut claimed: HashSet<String> = HashSet::new();
    let mut skips: Vec<SkipOutcome> = Vec::new();

    for raw_path in plugin_paths {
        let root = resolve_plugin_path(raw_path, dirs.cwd());
        let Some((_name, loaded)) = load_plugin(&root, dirs, &mut skips) else {
            continue;
        };
        for (name, entry) in loaded {
            // `agent-plugin-loader.ts:41-45` — the cross-plugin pass. First writer wins; the
            // second plugin to claim a namespaced name is told so and dropped.
            if !claimed.insert(name.clone()) {
                skips.push(warn_skip(
                    format!(
                        "Agent Plugin at {} skips duplicate normalized MCP server {name}",
                        root.display()
                    ),
                    SkipReason::DuplicateServer(name),
                ));
                continue;
            }
            servers.push(LoadedPluginServer {
                name,
                entry,
                root: root.clone(),
            });
        }
    }

    (servers, skips)
}

/// `getAgentPluginSummaries(paths, cwd)` — `agent-plugin-loader.ts:51`.
///
/// Upstream loads the config, then re-reads the manifest with `report = false` purely to recover the
/// name. The double read is elided here (one pass yields both); the *warnings* are unaffected,
/// because the load half of that pair reports with `report = true` either way.
///
/// Note this counts each plugin's servers **before** the cross-plugin collision pass, so a summary
/// total can exceed the number of servers `loadMcpConfig` ends up with. That is upstream's
/// arithmetic (`config.ts:255`) and the onboarding fingerprint is computed from it.
#[must_use]
pub fn agent_plugin_summaries(plugin_paths: &[String], dirs: &McpDirs) -> Vec<AgentPluginSummary> {
    plugin_paths
        .iter()
        .map(|raw_path| {
            let path = resolve_plugin_path(raw_path, dirs.cwd());
            let mut skips = Vec::new();
            match load_plugin(&path, dirs, &mut skips) {
                Some((name, servers)) => AgentPluginSummary {
                    path,
                    name: Some(name),
                    server_count: servers.len(),
                },
                None => AgentPluginSummary {
                    path,
                    name: None,
                    server_count: 0,
                },
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// One plugin
// ---------------------------------------------------------------------------------------------

/// `loadAgentPluginMcpConfig` (`agent-plugin-loader.ts:68`) fused with the manifest read, so the
/// validated plugin name comes back with the servers instead of being recovered by a second read.
///
/// `None` means **the manifest was invalid**, which is upstream's only `null` return and the exact
/// condition [`agent_plugin_summaries`] tests. A missing or broken `mcp.json` is *not* that: it
/// yields `Some((name, vec![]))`, because the plugin itself is well-formed and the panel should say
/// so.
fn load_plugin(
    plugin_root: &Path,
    dirs: &McpDirs,
    skips: &mut Vec<SkipOutcome>,
) -> Option<(String, Vec<(String, ServerEntry)>)> {
    let (_manifest, plugin_name) = read_plugin_manifest(plugin_root, skips)?;

    let mcp_path = plugin_root.join(PLUGIN_MCP_FILE);
    // `existsSync` + `statSync(...).isFile()`. Both follow symlinks, so `fs::metadata` — not
    // `symlink_metadata` — is the faithful call, and a broken symlink lands in the "missing" arm
    // exactly as `existsSync` puts it there.
    let Ok(meta) = std::fs::metadata(&mcp_path) else {
        // `agent-plugin-loader.ts:74` — a plugin with no `mcp.json` contributes nothing and is
        // NOT an error. Plugins carry more than MCP servers.
        return Some((plugin_name, Vec::new()));
    };
    if !meta.is_file() {
        skips.push(warn_skip(
            format!(
                "Agent Plugin {plugin_name} has invalid MCP config: mcp.json is not a regular file"
            ),
            SkipReason::UnreadableMcpConfig,
        ));
        return Some((plugin_name, Vec::new()));
    }

    let raw = match std::fs::read_to_string(&mcp_path) {
        Ok(raw) => raw,
        Err(error) => {
            // Node reads with `"utf8"`, which substitutes replacement characters and then almost
            // always fails in `JSON.parse`. Rust's read fails first; the observable outcome — warn,
            // and an empty server set for this plugin — is identical.
            skips.push(warn_skip(
                format!(
                    "Agent Plugin {plugin_name} has invalid MCP config: failed to parse mcp.json: {error}"
                ),
                SkipReason::UnreadableMcpConfig,
            ));
            return Some((plugin_name, Vec::new()));
        }
    };

    let servers = translate_plugin_mcp_config(&raw, &plugin_name, plugin_root, dirs, skips);
    Some((plugin_name, servers))
}

/// `readPluginManifest(pluginRoot, report)` — `agent-plugin-loader.ts:91`.
///
/// Returns the manifest **and** its validated name, so no caller has to re-derive the invariant that
/// a manifest which got this far always has one.
///
/// The document is walked as a `serde_json::Value` rather than deserialised into
/// [`AgentPluginManifest`] directly, and that is deliberate: a typed `Option<String> version` makes a
/// numeric `version` a *parse error*, which would reject a plugin upstream happily loads — upstream
/// never reads the field. Key order is irrelevant here (it only sequences warnings), so the
/// `BTreeMap` behind `Value::Object` costs nothing.
fn read_plugin_manifest(
    plugin_root: &Path,
    skips: &mut Vec<SkipOutcome>,
) -> Option<(AgentPluginManifest, String)> {
    let root_display = plugin_root.display();
    let manifest_path = plugin_root.join(PLUGIN_MANIFEST_FILE);

    let Ok(meta) = std::fs::metadata(&manifest_path) else {
        skips.push(warn_skip(
            format!("Agent Plugin at {root_display} is invalid: missing plugin.json"),
            SkipReason::MissingManifest,
        ));
        return None;
    };
    if !meta.is_file() {
        skips.push(warn_skip(
            format!("Agent Plugin at {root_display} is invalid: plugin.json is not a regular file"),
            SkipReason::MissingManifest,
        ));
        return None;
    }

    let raw = match std::fs::read_to_string(&manifest_path) {
        Ok(raw) => raw,
        Err(error) => {
            skips.push(warn_skip(
                format!(
                    "Agent Plugin at {root_display} is invalid: failed to parse plugin.json: {error}"
                ),
                SkipReason::UnreadableManifest,
            ));
            return None;
        }
    };
    let parsed: Value = match serde_json::from_str(&raw) {
        Ok(parsed) => parsed,
        Err(error) => {
            skips.push(warn_skip(
                format!(
                    "Agent Plugin at {root_display} is invalid: failed to parse plugin.json: {error}"
                ),
                SkipReason::UnreadableManifest,
            ));
            return None;
        }
    };
    // `!raw || typeof raw !== "object" || Array.isArray(raw)` — `Value::Object` is precisely the
    // set that survives all three, since `null` and arrays are separate `Value` arms.
    let Value::Object(fields) = parsed else {
        skips.push(warn_skip(
            format!("Agent Plugin at {root_display} is invalid: plugin.json must be an object"),
            SkipReason::UnreadableManifest,
        ));
        return None;
    };

    // `agent-plugin-loader.ts:115-119` — unknown manifest keys WARN and are then ignored. Contrast
    // `mcp.json`, where the same situation discards the file. The asymmetry is upstream's.
    for key in fields.keys() {
        if !PLUGIN_MANIFEST_FIELDS.contains(&key.as_str()) {
            tracing::warn!(
                "Agent Plugin at {root_display} ignores unknown plugin.json field: {key}"
            );
        }
    }

    // `manifest.$schema !== PLUGIN_SCHEMA`. A non-string `$schema` fails this the same way a
    // mismatched string does — hence the `as_str()` rather than a type-error branch.
    if fields.get("$schema").and_then(Value::as_str) != Some(PLUGIN_SCHEMA) {
        skips.push(warn_skip(
            format!("Agent Plugin at {root_display} is invalid: unsupported plugin.json $schema"),
            SkipReason::SchemaMismatch,
        ));
        return None;
    }

    let name = fields.get("name").and_then(Value::as_str).unwrap_or_default();
    if !is_valid_plugin_name(name) {
        skips.push(warn_skip(
            format!("Agent Plugin at {root_display} is invalid: plugin.json name is invalid"),
            SkipReason::InvalidPluginName,
        ));
        return None;
    }
    let name = name.to_owned();

    // `agent-plugin-loader.ts:128-130` — a malformed `extensions` warns and is otherwise ignored;
    // this loader does not read extensions at all. `undefined` is silent, `null` is not.
    if let Some(extensions) = fields.get("extensions")
        && !matches!(extensions, Value::Object(_))
    {
        tracing::warn!("Agent Plugin {name} ignores non-object plugin.json extensions");
    }

    let manifest = AgentPluginManifest {
        schema: Some(PLUGIN_SCHEMA.to_owned()),
        name: Some(name.clone()),
        version: fields
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_owned),
    };
    Some((manifest, name))
}

/// The plugin's `mcp.json`, shaped so that **server order survives**.
///
/// `mcpServers` is an [`IndexMap`], never a `serde_json::Map`: under this workspace's feature set
/// that map is a `BTreeMap`, and a round-trip through it would sort the servers and silently change
/// which one wins a normalised-name collision.
#[derive(Debug, Deserialize)]
struct RawPluginMcpDocument {
    /// Untyped so a non-string `$schema` fails the *equality* check rather than the *parse*, which
    /// is what upstream does.
    #[serde(rename = "$schema", default)]
    schema: Option<Value>,
    #[serde(rename = "mcpServers", default)]
    mcp_servers: Option<IndexMap<String, RawJson>>,
    /// Everything else in the document, in file order. A single entry here discards the file.
    #[serde(flatten)]
    unknown: IndexMap<String, Value>,
}

/// A JSON value that keeps **every** object's key order, at every depth.
///
/// `serde_json::Value` cannot be used anywhere a plugin's own key order is observable, and in this
/// file it is observable three levels down. `Object` is tried first by `untagged`, so any JSON
/// object lands in an [`IndexMap`]; everything else — scalars, `null`, arrays — falls through to
/// `Other` verbatim.
///
/// Three rules read that order, and each of them is a diagnostic a plugin author has to act on:
///
/// * the **entry**'s own keys decide which unknown field the allowlist names (`for…return`);
/// * **`headers`** decides which of two case-colliding names `duplicate header ${key}` reports;
/// * **`env`** decides which injected variable `env must not define ${key}` reports.
///
/// The `Other` arm is separately load-bearing: it is what keeps one malformed `mcpServers` value
/// from failing the whole document. A map typed on the value would abort the parse and take every
/// sibling server with it, where upstream warns per entry and carries on.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawJson {
    Object(IndexMap<String, RawJson>),
    Other(Value),
}

impl RawJson {
    /// `typeof value === "object" && !Array.isArray(value) && value !== null`, which the `Object`
    /// arm already encodes exactly.
    fn as_object(&self) -> Option<&IndexMap<String, RawJson>> {
        match self {
            Self::Object(fields) => Some(fields),
            Self::Other(_) => None,
        }
    }

    /// `typeof value === "string"`.
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Other(value) => value.as_str(),
            Self::Object(_) => None,
        }
    }

    /// `Array.isArray(value)`. Elements stay `serde_json::Value` because no rule reads inside an
    /// object nested in an array.
    fn as_array(&self) -> Option<&[Value]> {
        match self {
            Self::Other(Value::Array(items)) => Some(items),
            _ => None,
        }
    }

    /// A label for the machine-readable half of [`SkipReason::UnsupportedType`], following JS's own
    /// string coercion so the payload reads the way the plugin author's console would.
    fn label(&self) -> String {
        match self {
            Self::Other(value) => value.to_string(),
            Self::Object(_) => "[object Object]".to_owned(),
        }
    }
}

/// `translateAgentPluginMcpConfig` — `agent-plugin-loader.ts:135`.
fn translate_plugin_mcp_config(
    raw: &str,
    plugin_name: &str,
    plugin_root: &Path,
    dirs: &McpDirs,
    skips: &mut Vec<SkipOutcome>,
) -> Vec<(String, ServerEntry)> {
    let document: RawPluginMcpDocument = match serde_json::from_str(raw) {
        Ok(document) => document,
        Err(error) => {
            // Three upstream warnings collapse onto one Rust parse failure, so the failure is
            // re-classified here to keep the diagnostics distinguishable: a document that is valid
            // JSON but not an object is `mcp.json must be an object` (`:137`); a document that IS
            // an object can only have failed on `mcpServers` not being one (`:153`), because every
            // other field in `RawPluginMcpDocument` is untyped.
            let (message, reason) = match serde_json::from_str::<Value>(raw) {
                Ok(Value::Object(_)) => (
                    format!(
                        "Agent Plugin {plugin_name} has invalid MCP config: mcpServers must be an object"
                    ),
                    SkipReason::UnreadableMcpConfig,
                ),
                Ok(_) => (
                    format!(
                        "Agent Plugin {plugin_name} has invalid MCP config: mcp.json must be an object"
                    ),
                    SkipReason::UnreadableMcpConfig,
                ),
                Err(_) => (
                    format!(
                        "Agent Plugin {plugin_name} has invalid MCP config: failed to parse mcp.json: {error}"
                    ),
                    SkipReason::UnreadableMcpConfig,
                ),
            };
            skips.push(warn_skip(message, reason));
            return Vec::new();
        }
    };

    // `agent-plugin-loader.ts:142-147`. THE WHOLE FILE, not the key: an unrecognised top-level
    // field is evidence the file was written against a schema this loader does not implement, and
    // partially honouring it is how a containment rule gets skipped by accident.
    if let Some(key) = document
        .unknown
        .keys()
        .find(|key| !MCP_CONFIG_FIELDS.contains(&key.as_str()))
    {
        skips.push(warn_skip(
            format!(
                "Agent Plugin {plugin_name} has invalid MCP config: unknown top-level field {key}"
            ),
            SkipReason::UnknownTopLevelKey(key.clone()),
        ));
        return Vec::new();
    }

    if document.schema.as_ref().and_then(Value::as_str) != Some(MCP_SCHEMA) {
        skips.push(warn_skip(
            format!(
                "Agent Plugin {plugin_name} has invalid MCP config: unsupported mcp.json $schema"
            ),
            SkipReason::SchemaMismatch,
        ));
        return Vec::new();
    }

    let Some(entries) = document.mcp_servers else {
        // `!mcpConfig.mcpServers` — an absent or `null` map. A non-object one already failed the
        // parse above and was re-classified to this same message.
        skips.push(warn_skip(
            format!("Agent Plugin {plugin_name} has invalid MCP config: mcpServers must be an object"),
            SkipReason::UnreadableMcpConfig,
        ));
        return Vec::new();
    };

    let plugin_data_dir = dirs.agent_plugin_data(plugin_name);
    let mut out: Vec<(String, ServerEntry)> = Vec::new();
    let mut claimed: HashSet<String> = HashSet::new();

    for (server_name, entry) in &entries {
        let translated = translate_plugin_server(
            plugin_name,
            plugin_root,
            &plugin_data_dir,
            server_name,
            entry,
        );
        let entry = match translated {
            Ok(entry) => entry,
            Err(skip) => {
                skips.push(warn_skip(skip.0, skip.1));
                continue;
            }
        };

        let normalized = format_plugin_server_name(plugin_name, server_name);
        // `agent-plugin-loader.ts:163-166` — the intra-plugin pass. Two distinct raw names can
        // normalise onto one key (`tools.db` and `tools_db` both become `tools_db`); the first one
        // written to the file keeps it.
        if !claimed.insert(normalized.clone()) {
            skips.push(warn_skip(
                format!(
                    "Agent Plugin {plugin_name} skips invalid MCP server {server_name}: normalized server name {normalized} already exists"
                ),
                SkipReason::DuplicateServer(normalized),
            ));
            continue;
        }
        out.push((normalized, entry));
    }

    out
}

/// `translateAgentPluginServer` — `agent-plugin-loader.ts:172`.
fn translate_plugin_server(
    plugin_name: &str,
    plugin_root: &Path,
    plugin_data_dir: &Path,
    server_name: &str,
    entry: &RawJson,
) -> Result<ServerEntry, SkipOutcome> {
    let Some(raw) = entry.as_object() else {
        return Err((
            skip_message(plugin_name, server_name, "entry must be an object"),
            SkipReason::NotAnObject,
        ));
    };

    match raw.get("type").and_then(RawJson::as_str) {
        Some("stdio") => {
            translate_stdio_server(plugin_name, plugin_root, plugin_data_dir, server_name, raw)
        }
        // **Cut 1.** Upstream accepts `"streamable-http" | "sse"` here and assigns `httpTransport`
        // straight from the value (`agent-plugin-loader.ts:185`, `:245`). rmcp 3.1.2 ships no SSE
        // client transport, so `"sse"` falls through to the `unsupported type` arm below —
        // a NAMED diagnostic. Accepting it silently would register a server that can never connect.
        Some("streamable-http") => translate_http_server(plugin_name, server_name, raw),
        other => Err((
            skip_message(plugin_name, server_name, "unsupported type"),
            SkipReason::UnsupportedType(
                other
                    .map(str::to_owned)
                    .or_else(|| raw.get("type").map(RawJson::label))
                    .unwrap_or_else(|| "undefined".to_owned()),
            ),
        )),
    }
}

/// `translateStdioServer` — `agent-plugin-loader.ts:191`.
///
/// The check order is upstream's and is preserved on purpose: allowlist, `command` shape, `args`,
/// `env`, *then* command containment, *then* `cwd`. A plugin with both a malformed `args` and an
/// escaping `command` is told about `args`, which is what upstream's tests observe.
fn translate_stdio_server(
    plugin_name: &str,
    plugin_root: &Path,
    plugin_data_dir: &Path,
    server_name: &str,
    raw: &IndexMap<String, RawJson>,
) -> Result<ServerEntry, SkipOutcome> {
    for key in raw.keys() {
        if !STDIO_FIELDS.contains(&key.as_str()) {
            return Err((
                skip_message(plugin_name, server_name, &format!("unknown field {key}")),
                SkipReason::UnknownField(key.clone()),
            ));
        }
    }

    let command = raw.get("command").and_then(RawJson::as_str).unwrap_or_default();
    if command.is_empty() {
        return Err((
            skip_message(
                plugin_name,
                server_name,
                "command must be a non-empty string",
            ),
            SkipReason::InvalidCommand,
        ));
    }
    // Bare, or explicitly plugin-relative. Everything else — an absolute path, a `../` escape, a
    // `${PLUGIN_DATA}/bin` that would run a binary the plugin itself wrote — is refused here,
    // before any path resolution happens.
    if !is_bare_command(command) && !command.starts_with("./") {
        return Err((
            skip_message(
                plugin_name,
                server_name,
                "command must be bare or plugin-relative",
            ),
            SkipReason::InvalidCommand,
        ));
    }

    let args = translate_string_array(raw.get("args"), plugin_name, server_name, "args")?;
    let env = translate_env(raw.get("env"), plugin_name, server_name)?;

    let plugin_root_str = plugin_root.to_string_lossy().into_owned();
    let plugin_data_str = plugin_data_dir.to_string_lossy().into_owned();

    let resolved_command = if command.starts_with("./") {
        let Some(resolved) = resolve_contained_path(plugin_root, command) else {
            return Err((
                skip_message(
                    plugin_name,
                    server_name,
                    "command must stay inside the plugin directory",
                ),
                SkipReason::CommandEscapesPluginRoot,
            ));
        };
        resolved.to_string_lossy().into_owned()
    } else {
        command.to_owned()
    };

    let Some(cwd) = resolve_plugin_cwd(raw.get("cwd"), plugin_root, plugin_data_dir) else {
        return Err((
            skip_message(
                plugin_name,
                server_name,
                "cwd must be plugin-relative, PLUGIN_ROOT-rooted, or PLUGIN_DATA-rooted",
            ),
            SkipReason::CwdOutsidePlugin,
        ));
    };

    // The injected pair is written LAST so it cannot be shadowed by ordering; `translate_env` has
    // already refused any entry that names either variable, so this is belt and braces.
    let mut resolved_env: BTreeMap<String, String> = env
        .into_iter()
        .map(|(key, value)| {
            (
                key,
                expand_plugin_placeholders(&value, &plugin_root_str, &plugin_data_str),
            )
        })
        .collect();
    resolved_env.insert(PLUGIN_ROOT_VAR.to_owned(), plugin_root_str.clone());
    resolved_env.insert(PLUGIN_DATA_VAR.to_owned(), plugin_data_str.clone());

    Ok(ServerEntry {
        command: Some(resolved_command),
        args: Some(
            args.iter()
                .map(|value| expand_plugin_placeholders(value, &plugin_root_str, &plugin_data_str))
                .collect(),
        ),
        env: Some(resolved_env),
        cwd: Some(cwd.to_string_lossy().into_owned()),
        plugin_data_dir: Some(plugin_data_str),
        // The other half of the containment: with `literalEnv` set, the connect path performs no
        // `$VAR` interpolation and no `!secret` resolution on these values, so a plugin cannot use
        // its own `env` as a read primitive against the user's environment or credential store.
        literal_env: Some(true),
        ..ServerEntry::default()
    })
}

/// `translateHttpServer` — `agent-plugin-loader.ts:229`.
fn translate_http_server(
    plugin_name: &str,
    server_name: &str,
    raw: &IndexMap<String, RawJson>,
) -> Result<ServerEntry, SkipOutcome> {
    for key in raw.keys() {
        if !HTTP_FIELDS.contains(&key.as_str()) {
            return Err((
                skip_message(plugin_name, server_name, &format!("unknown field {key}")),
                SkipReason::UnknownField(key.clone()),
            ));
        }
    }

    let url = raw.get("url").and_then(RawJson::as_str).unwrap_or_default();
    if url.is_empty() {
        return Err((
            skip_message(plugin_name, server_name, "url must be a non-empty string"),
            SkipReason::InvalidUrl,
        ));
    }
    if !is_valid_plugin_url(url) {
        return Err((
            skip_message(
                plugin_name,
                server_name,
                "url must be an allowed absolute HTTP(S) URL",
            ),
            SkipReason::UnsafeUrl,
        ));
    }

    let headers = translate_headers(raw.get("headers"), plugin_name, server_name)?;

    Ok(ServerEntry {
        url: Some(url.to_owned()),
        // Set EXPLICITLY, never left to default: the connect path must not be able to fall back to
        // another transport for a third-party endpoint.
        http_transport: Some(HttpTransport::StreamableHttp),
        headers,
        ..ServerEntry::default()
    })
}

// ---------------------------------------------------------------------------------------------
// Field translators
// ---------------------------------------------------------------------------------------------

/// `translateStringArray` — `agent-plugin-loader.ts:261`. Absent is `[]`; anything that is not an
/// array of strings rejects the server.
fn translate_string_array(
    value: Option<&RawJson>,
    plugin_name: &str,
    server_name: &str,
    field: &str,
) -> Result<Vec<String>, SkipOutcome> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let invalid = || {
        (
            skip_message(
                plugin_name,
                server_name,
                &format!("{field} must be an array of strings"),
            ),
            SkipReason::InvalidArgs,
        )
    };
    let Some(items) = value.as_array() else {
        return Err(invalid());
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Value::String(item) = item else {
            return Err(invalid());
        };
        out.push(item.clone());
    }
    Ok(out)
}

/// `translateEnv` — `agent-plugin-loader.ts:270`.
///
/// The `PLUGIN_ROOT`/`PLUGIN_DATA` refusal is the containment rule, not a naming convention: those
/// two names are the plugin's only handles on the filesystem, and letting a plugin define them would
/// let it point `${PLUGIN_DATA}` at an arbitrary directory that `resolvePluginCwd` would then
/// happily "contain" a `cwd` inside.
fn translate_env(
    value: Option<&RawJson>,
    plugin_name: &str,
    server_name: &str,
) -> Result<BTreeMap<String, String>, SkipOutcome> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let Some(fields) = value.as_object() else {
        return Err((
            skip_message(
                plugin_name,
                server_name,
                "env must be an object of strings",
            ),
            SkipReason::InvalidEnv,
        ));
    };

    let mut out = BTreeMap::new();
    for (key, entry) in fields {
        if key == PLUGIN_ROOT_VAR || key == PLUGIN_DATA_VAR {
            return Err((
                skip_message(
                    plugin_name,
                    server_name,
                    &format!("env must not define {key}"),
                ),
                SkipReason::EnvShadowsInjectedVar(key.clone()),
            ));
        }
        let Some(entry) = entry.as_str() else {
            return Err((
                skip_message(plugin_name, server_name, "env values must be strings"),
                SkipReason::InvalidEnv,
            ));
        };
        out.insert(key.clone(), entry.to_owned());
    }
    Ok(out)
}

/// `translateHeaders` — `agent-plugin-loader.ts:291`.
///
/// `Ok(None)` is upstream's `undefined` (absent, or present-and-empty); `Err` is its `null`, which
/// rejects the server. Original key casing is preserved in the output, exactly as upstream keeps
/// the caller's record rather than the normalised `Headers` view.
fn translate_headers(
    value: Option<&RawJson>,
    plugin_name: &str,
    server_name: &str,
) -> Result<Option<BTreeMap<String, String>>, SkipOutcome> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(fields) = value.as_object() else {
        return Err((
            skip_message(
                plugin_name,
                server_name,
                "headers must be an object of strings",
            ),
            SkipReason::InvalidHeaders,
        ));
    };

    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (key, entry) in fields {
        let Some(entry) = entry.as_str() else {
            return Err((
                skip_message(plugin_name, server_name, "header values must be strings"),
                SkipReason::InvalidHeaders,
            ));
        };
        // Case-insensitive, because HTTP field names are — two spellings of one header is a
        // request the plugin author did not mean and the server would resolve unpredictably.
        let normalized = key.to_lowercase();
        if !seen.insert(normalized) {
            return Err((
                skip_message(
                    plugin_name,
                    server_name,
                    &format!("duplicate header {key}"),
                ),
                SkipReason::DuplicateHeader(key.clone()),
            ));
        }
        out.insert(key.clone(), entry.to_owned());
    }

    // `new Headers(headers)` — upstream's validation is a throw-or-not construction, reproduced
    // field by field below (care point 3).
    for (key, value) in &out {
        if !is_valid_header_name(key) || !is_valid_header_value(value) {
            return Err((
                skip_message(
                    plugin_name,
                    server_name,
                    "headers are not valid HTTP fields",
                ),
                SkipReason::InvalidHeaders,
            ));
        }
    }

    if out.is_empty() {
        return Ok(None);
    }
    Ok(Some(out))
}

/// `resolvePluginCwd` — `agent-plugin-loader.ts:331`.
///
/// Absent is the plugin root. The three accepted spellings each resolve through
/// [`resolve_contained_path`] against the root they name, so `${PLUGIN_DATA}/../..` is refused just
/// as `./../..` is. Anything else — including an absolute path and including a bare relative path
/// with no `./` — is refused outright.
fn resolve_plugin_cwd(
    value: Option<&RawJson>,
    plugin_root: &Path,
    plugin_data_dir: &Path,
) -> Option<PathBuf> {
    let Some(value) = value else {
        return Some(plugin_root.to_path_buf());
    };
    let value = value.as_str()?;

    if value.starts_with("./") {
        return resolve_contained_path(plugin_root, value);
    }
    // `value.replace(pattern, ".")` in JS replaces the FIRST occurrence only, hence `replacen(_, 1)`.
    // The rewritten value is `"."` or `"./…"`, which is what makes the containment check identical
    // to the `./…` arm above.
    if value == "${PLUGIN_ROOT}" || value.starts_with("${PLUGIN_ROOT}/") {
        return resolve_contained_path(plugin_root, &value.replacen("${PLUGIN_ROOT}", ".", 1));
    }
    if value == "${PLUGIN_DATA}" || value.starts_with("${PLUGIN_DATA}/") {
        return resolve_contained_path(plugin_data_dir, &value.replacen("${PLUGIN_DATA}", ".", 1));
    }
    None
}

/// `expandPluginPlaceholders` — `agent-plugin-loader.ts:351`. `PLUGIN_ROOT` first, then
/// `PLUGIN_DATA`; the order is upstream's and is preserved because it is observable if one
/// expansion's output contains the other's marker.
fn expand_plugin_placeholders(value: &str, plugin_root: &str, plugin_data_dir: &str) -> String {
    value
        .replace("${PLUGIN_ROOT}", plugin_root)
        .replace("${PLUGIN_DATA}", plugin_data_dir)
}

/// `isBareCommand` — `agent-plugin-loader.ts:327`. Note the placeholder tests: a bare command may
/// not smuggle a path in through `${PLUGIN_ROOT}`, which would otherwise expand *after* the
/// containment check that this predicate is what routes around.
fn is_bare_command(command: &str) -> bool {
    !command.contains('/')
        && !command.contains('\\')
        && !command.contains("${PLUGIN_ROOT}")
        && !command.contains("${PLUGIN_DATA}")
}

/// `formatAgentPluginServerName` — `agent-plugin-loader.ts:250`.
///
/// Each half is `/[^A-Za-z0-9_-]+/g → "_"` then `/^[_-]+|[_-]+$/g → ""`, falling back to
/// `"plugin"`/`"server"` when the result is empty. Runs collapse to a **single** underscore, which
/// is exactly why two distinct raw names can land on one normalised key.
fn format_plugin_server_name(plugin_name: &str, server_name: &str) -> String {
    let plugin_part = sanitize_name_part(plugin_name, "plugin");
    let server_part = sanitize_name_part(server_name, "server");
    format!("{plugin_part}{NAMESPACE_SEPARATOR}{server_part}")
}

/// The two-`replace` pipeline of [`format_plugin_server_name`], hand-rolled: the character class is
/// ASCII-only and the run-collapse plus edge-trim are literal transcriptions, so a regex would add a
/// dependency on `LazyLock` fallibility for no behavioural gain.
fn sanitize_name_part(value: &str, fallback: &str) -> String {
    let mut collapsed = String::with_capacity(value.len());
    let mut in_run = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            collapsed.push(ch);
            in_run = false;
        } else if !in_run {
            collapsed.push('_');
            in_run = true;
        }
    }
    let trimmed = collapsed.trim_matches(|ch| ch == '_' || ch == '-');
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// `PLUGIN_NAME_PATTERN.test(name)` plus the 1..64 length bound
/// (`agent-plugin-loader.ts:8`, `:124`).
///
/// The negative lookahead `(?!.*(?:--|\.\.))` is hoisted into two `contains` calls — care point 2.
/// It bans `--` and `..` **anywhere**, not just at the edges, which is what stops a plugin name from
/// becoming a path traversal once it is joined onto `<agent_dir>/agent-plugin-data/`.
fn is_valid_plugin_name(name: &str) -> bool {
    // `manifest.name.length` is UTF-16 code units in JS. The character class admits ASCII only, so
    // any string that could pass the class has an identical length in bytes, code points and code
    // units — `len()` is exact for every accepted input and only over-counts rejected ones.
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    if name.contains("--") || name.contains("..") {
        return false;
    }
    PLUGIN_NAME_CLASS
        .as_ref()
        .is_some_and(|pattern| pattern.is_match(name))
}

// ---------------------------------------------------------------------------------------------
// URL containment
// ---------------------------------------------------------------------------------------------

/// `isValidAgentPluginUrl` — `agent-plugin-loader.ts:357`.
///
/// The interpolation test comes **first and unconditionally**: a plugin URL is never interpolated,
/// so any marker in it is evidence the author expected a substitution that will not happen, and a
/// URL that silently keeps a literal `${…}` is a request to the wrong host.
fn is_valid_plugin_url(value: &str) -> bool {
    if value.contains("${") || value.contains("$env:") || value.contains("{env:") {
        return false;
    }
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    if url.scheme() != "http" && url.scheme() != "https" {
        return false;
    }
    // JS reads `url.username`/`url.password`/`url.hash`, all of which are `""` when absent — so the
    // test is non-emptiness, not presence. `https://h/#` has an empty fragment and is ACCEPTED
    // upstream; `url::Url` reports it as `Some("")`, hence the explicit emptiness check.
    if !url.username().is_empty()
        || url.password().is_some_and(|password| !password.is_empty())
        || url.fragment().is_some_and(|fragment| !fragment.is_empty())
    {
        return false;
    }
    if url.scheme() == "https" {
        return true;
    }
    // Plaintext to anywhere but this machine would put a third-party plugin's traffic — including
    // whatever the model sends it — on the wire in the clear.
    is_loopback_host(url.host_str().unwrap_or_default())
}

/// `isLoopbackHost` — `agent-plugin-loader.ts:372`.
///
/// `[::1]` is listed alongside `::1` because `URL.hostname` keeps the brackets for IPv6, and
/// `url::Url::host_str` does too. The `127.x` test is upstream's regex transcribed literally,
/// including its looseness: it accepts `127.999.999.999`, which the WHATWG parser will already have
/// refused before this is reached.
fn is_loopback_host(hostname: &str) -> bool {
    let host = hostname.to_lowercase();
    if host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]" {
        return true;
    }
    // `/^127(?:\.\d{1,3}){3}$/` — four dot-separated parts, the first literally `127`, the rest 1..3
    // ASCII digits each.
    let mut parts = host.split('.');
    if parts.next() != Some("127") {
        return false;
    }
    let mut octets = 0usize;
    for part in parts {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        octets += 1;
    }
    octets == 3
}

// ---------------------------------------------------------------------------------------------
// Header field validation (care point 3)
// ---------------------------------------------------------------------------------------------

/// A WHATWG *header name*: a non-empty HTTP token
/// (<https://fetch.spec.whatwg.org/#header-name>).
///
/// This is what `new Headers({…})` enforces at `agent-plugin-loader.ts:313`, and it is **not** what
/// `http::HeaderName` enforces — `http` additionally lower-cases and accepts a slightly different
/// byte set. The spec rule is transcribed directly so the port neither over- nor under-accepts.
fn is_valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// A WHATWG *header value* (<https://fetch.spec.whatwg.org/#header-value>): after stripping leading
/// and trailing HTTP whitespace, it must contain no NUL, CR or LF.
///
/// `Headers.append` *normalises* the value by trimming but the record upstream returns is the
/// caller's original, so the trim happens here for the test only — the stored string is untouched.
fn is_valid_header_value(value: &str) -> bool {
    let trimmed = value.trim_matches(|ch| ch == '\t' || ch == '\n' || ch == '\r' || ch == ' ');
    !trimmed.contains(['\0', '\n', '\r'])
}

// ---------------------------------------------------------------------------------------------
// Path containment (care point 1)
// ---------------------------------------------------------------------------------------------

/// `resolveContainedPath(root, value, containmentRoot)` — `agent-plugin-loader.ts:344`.
///
/// Upstream takes a third `containmentRoot` argument; **all three call sites pass the same value as
/// `root`** (`:208`, `:336`, `:339`), so the port collapses it and the two-argument signature is
/// exact rather than a simplification.
///
/// # Why this is not `strip_prefix`
///
/// `Path::strip_prefix` is a component-prefix test on the *literal* path: it would accept
/// `root/../../etc` unchanged, because `..` is a component like any other to Rust. Node's
/// `path.resolve` normalises `..` **lexically** first, which is what makes the subsequent
/// `path.relative` test meaningful. So this normalises first (`lexical_normalize`), then
/// reproduces the string test on the relative path exactly as upstream writes it — including the
/// consequence that a directory literally named `..foo` inside the root is refused, because
/// `rel.startsWith("..")` is a string test and not a component test.
///
/// # Why symlinks are not resolved
///
/// Upstream does not resolve them, and a port that did would refuse plugin layouts upstream accepts
/// — a stricter check is still a divergent one. It would also make the result depend on the
/// filesystem at load time, so a plugin could pass validation and fail on the next run.
#[must_use]
pub fn resolve_contained_path(root: &Path, candidate: &str) -> Option<PathBuf> {
    let resolved = node_resolve(root, Path::new(candidate));
    let relative = lexical_relative(root, &resolved);
    // `rel === "" || (!rel.startsWith("..") && !rel.startsWith(sep) && !isAbsolute(rel))`.
    if relative.is_empty()
        || (!relative.starts_with("..")
            && !relative.starts_with(MAIN_SEPARATOR)
            && !Path::new(&relative).is_absolute())
    {
        Some(resolved)
    } else {
        None
    }
}

/// `path.resolve(base, value)`: join, make absolute against the process cwd if the result still is
/// not, then normalise lexically. `Path::join` already reproduces Node's "an absolute right-hand
/// side wins" rule.
///
/// A failing `current_dir()` leaves the path relative rather than panicking; [`lexical_relative`]
/// still compares the two consistently, so containment holds either way.
fn node_resolve(base: &Path, value: &Path) -> PathBuf {
    let joined = base.join(value);
    let absolute = if joined.is_absolute() {
        joined
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(joined),
            Err(_) => joined,
        }
    };
    lexical_normalize(&absolute)
}

/// Node's `path.normalize` semantics, purely lexically: drop `.`, pop a `..` against a preceding
/// real component, and **clamp** a `..` that would escape the root (`/a/../..` is `/`).
///
/// A leading `..` on a *relative* path is kept, because there is nothing to clamp against — that
/// case only arises when `current_dir()` failed.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => out.push(component),
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(component),
            },
        }
    }
    out.into_iter().collect()
}

/// `path.relative(from, to)` for two already-normalised paths: strip the common component prefix,
/// emit one `..` per component left on `from`, then the remainder of `to`.
///
/// Returned as a `String` and not a `PathBuf` because the caller's test is a **string** test —
/// `rel.startsWith("..")` — and that distinction is load-bearing (see [`resolve_contained_path`]).
fn lexical_relative(from: &Path, to: &Path) -> String {
    let mut from_components = from.components().peekable();
    let mut to_components = to.components().peekable();
    while let (Some(left), Some(right)) = (from_components.peek(), to_components.peek()) {
        if left == right {
            from_components.next();
            to_components.next();
        } else {
            break;
        }
    }

    let ups = from_components.count();
    let mut parts: Vec<String> = Vec::with_capacity(ups);
    parts.resize(ups, "..".to_owned());
    for component in to_components {
        parts.push(component.as_os_str().to_string_lossy().into_owned());
    }
    parts.join(MAIN_SEPARATOR_STR)
}

/// `resolvePluginPath(path, cwd)` — `agent-plugin-loader.ts:321`, reading `$HOME`.
///
/// **Care point 4.** This is deliberately *not* [`crate::dirs::expand_tilde`]: that function is
/// `agent-dir.ts`'s expansion, which trims its input and is anchored on the OS home directory, while
/// this one does neither and anchors on `$HOME`. On a machine where `HOME` is unset or disagrees
/// with the OS home the two resolve differently, and unifying them would move where a user's plugins
/// are looked for.
fn resolve_plugin_path(raw: &str, cwd: &Path) -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(PathBuf::new, PathBuf::from);
    resolve_plugin_path_with_home(raw, cwd, &home)
}

/// [`resolve_plugin_path`] with `$HOME` passed in, so the rule is testable without mutating
/// process-global state that every other test in this binary shares.
///
/// An empty `home` reproduces `process.env.HOME ?? ""`: `resolve("", ".")` is the process cwd, and
/// [`node_resolve`] gets there by the same route.
fn resolve_plugin_path_with_home(raw: &str, cwd: &Path, home: &Path) -> PathBuf {
    if raw == "~" {
        return node_resolve(home, Path::new("."));
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return node_resolve(home, Path::new(rest));
    }
    if Path::new(raw).is_absolute() {
        return lexical_normalize(Path::new(raw));
    }
    node_resolve(cwd, Path::new(raw))
}

// ---------------------------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------------------------

/// `skipServer(manifest, serverName, reason)` — `agent-plugin-loader.ts:256`. The message shape is
/// upstream's, verbatim, because it is what a user greps for.
fn skip_message(plugin_name: &str, server_name: &str, reason: &str) -> String {
    format!("Agent Plugin {plugin_name} skips invalid MCP server {server_name}: {reason}")
}

/// `console.warn(message)` plus the tuple the caller collects. Every rejection in this file goes
/// through here, so "warn and skip, nothing throws" is enforced by construction rather than by
/// review.
fn warn_skip(message: String, reason: SkipReason) -> SkipOutcome {
    tracing::warn!("{message}");
    (message, reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The eight rows of MCP-047's **verify** table, plus the two behaviours
    /// `__tests__/config.test.ts` asserts directly, are all driven through this fixture: a real
    /// plugin directory on disk, because every rule in this file is a filesystem containment rule
    /// and a fixture that fakes the filesystem would not exercise it.
    struct Fixture {
        _tmp: tempfile::TempDir,
        agent_dir: PathBuf,
        cwd: PathBuf,
    }

    impl Fixture {
        fn new() -> Option<Self> {
            let tmp = tempfile::tempdir().ok()?;
            let agent_dir = tmp.path().join("agent");
            let cwd = tmp.path().join("project");
            fs::create_dir_all(&agent_dir).ok()?;
            fs::create_dir_all(&cwd).ok()?;
            Some(Self {
                _tmp: tmp,
                agent_dir,
                cwd,
            })
        }

        fn dirs(&self) -> McpDirs {
            McpDirs::new(self.agent_dir.clone(), self.cwd.clone())
        }

        /// Writes `plugin.json` (valid unless `name` is deliberately bad) and `mcp.json`, returning
        /// the plugin root as the string a `settings.agentPluginPaths` entry would carry.
        fn plugin(&self, name: &str, mcp_json: &str) -> Option<String> {
            self.plugin_with_manifest(
                name,
                &format!(r#"{{"$schema":"{PLUGIN_SCHEMA}","name":"{name}"}}"#),
                Some(mcp_json),
            )
        }

        fn plugin_with_manifest(
            &self,
            dir: &str,
            manifest: &str,
            mcp_json: Option<&str>,
        ) -> Option<String> {
            let root = self.cwd.join("plugins").join(dir);
            fs::create_dir_all(&root).ok()?;
            fs::write(root.join(PLUGIN_MANIFEST_FILE), manifest).ok()?;
            if let Some(mcp_json) = mcp_json {
                fs::write(root.join(PLUGIN_MCP_FILE), mcp_json).ok()?;
            }
            Some(root.to_string_lossy().into_owned())
        }
    }

    fn names(servers: &[LoadedPluginServer]) -> Vec<&str> {
        servers.iter().map(|server| server.name.as_str()).collect()
    }

    fn reasons(skips: &[SkipOutcome]) -> Vec<SkipReason> {
        skips.iter().map(|skip| skip.1.clone()).collect()
    }

    // -----------------------------------------------------------------------------------------
    // resolveContainedPath — care point 1, and the row the plan calls out as the permission bypass
    // -----------------------------------------------------------------------------------------

    #[test]
    fn contained_path_normalises_parent_components_lexically() {
        let root = Path::new("/plugins/acme");
        // The row MCP-047 singles out: `strip_prefix` alone accepts this, because `..` is just a
        // component to Rust. Normalising first is what refuses it.
        assert_eq!(resolve_contained_path(root, "./bin/../../evil"), None);
        assert_eq!(resolve_contained_path(root, "../evil"), None);
        assert_eq!(resolve_contained_path(root, "/etc/passwd"), None);
        assert_eq!(
            resolve_contained_path(root, "./bin/server"),
            Some(PathBuf::from("/plugins/acme/bin/server"))
        );
        // `rel === ""` — the root itself is contained.
        assert_eq!(
            resolve_contained_path(root, "."),
            Some(PathBuf::from("/plugins/acme"))
        );
        // A `..` chain that walks out and back in is contained, because containment is decided on
        // the RESOLVED path and not on the spelling.
        assert_eq!(
            resolve_contained_path(root, "./bin/../lib"),
            Some(PathBuf::from("/plugins/acme/lib"))
        );
        // `rel.startsWith("..")` is a STRING test upstream, so a directory literally named `..foo`
        // is refused. Divergence here would be silent, hence the assertion.
        assert_eq!(resolve_contained_path(root, "./..foo"), None);
    }

    #[test]
    fn lexical_normalise_clamps_at_root() {
        assert_eq!(
            lexical_normalize(Path::new("/a/b/../../../../c")),
            PathBuf::from("/c")
        );
        assert_eq!(lexical_normalize(Path::new("/a/./b/")), PathBuf::from("/a/b"));
    }

    #[test]
    fn lexical_relative_matches_node() {
        assert_eq!(lexical_relative(Path::new("/a"), Path::new("/a")), "");
        assert_eq!(lexical_relative(Path::new("/a"), Path::new("/a/b/c")), "b/c");
        assert_eq!(lexical_relative(Path::new("/a/b"), Path::new("/a/c")), "../c");
    }

    #[test]
    fn plugin_path_tilde_anchors_on_the_supplied_home() {
        let home = Path::new("/home/u");
        let cwd = Path::new("/proj");
        assert_eq!(
            resolve_plugin_path_with_home("~", cwd, home),
            PathBuf::from("/home/u")
        );
        assert_eq!(
            resolve_plugin_path_with_home("~/p/./x", cwd, home),
            PathBuf::from("/home/u/p/x")
        );
        // `~other` is not a home reference upstream either — only the `~/` prefix is tested.
        assert_eq!(
            resolve_plugin_path_with_home("~other", cwd, home),
            PathBuf::from("/proj/~other")
        );
        assert_eq!(
            resolve_plugin_path_with_home("./p", cwd, home),
            PathBuf::from("/proj/p")
        );
        assert_eq!(
            resolve_plugin_path_with_home("/abs/p/..", cwd, home),
            PathBuf::from("/abs")
        );
    }

    // -----------------------------------------------------------------------------------------
    // The name rule
    // -----------------------------------------------------------------------------------------

    #[test]
    fn plugin_name_rule_bans_double_dash_and_dot_anywhere() {
        assert!(is_valid_plugin_name("acme"));
        assert!(is_valid_plugin_name("acme.tools-1"));
        assert!(is_valid_plugin_name("a"));
        // The negative lookahead, hoisted (care point 2).
        assert!(!is_valid_plugin_name("a--b"));
        assert!(!is_valid_plugin_name("a..b"));
        // Character class + edge anchors.
        assert!(!is_valid_plugin_name("Acme"));
        assert!(!is_valid_plugin_name("-acme"));
        assert!(!is_valid_plugin_name("acme-"));
        assert!(!is_valid_plugin_name("acme/evil"));
        assert!(!is_valid_plugin_name(""));
        assert!(!is_valid_plugin_name(&"a".repeat(65)));
        assert!(is_valid_plugin_name(&"a".repeat(64)));
    }

    #[test]
    fn server_name_normalisation_collapses_runs() {
        assert_eq!(format_plugin_server_name("acme", "db"), "acme__db");
        // Both of these normalise onto one key, which is what makes first-writer-wins observable.
        assert_eq!(format_plugin_server_name("acme", "tools.db"), "acme__tools_db");
        assert_eq!(format_plugin_server_name("acme", "tools_db"), "acme__tools_db");
        assert_eq!(format_plugin_server_name("acme", "  "), "acme__server");
        assert_eq!(format_plugin_server_name("...", "x"), "plugin__x");
    }

    // -----------------------------------------------------------------------------------------
    // URL containment
    // -----------------------------------------------------------------------------------------

    #[test]
    fn plugin_urls_reject_interpolation_userinfo_and_plaintext_offbox() {
        assert!(is_valid_plugin_url("https://example.test/mcp"));
        assert!(is_valid_plugin_url("http://127.0.0.1:8080/mcp"));
        assert!(is_valid_plugin_url("http://localhost:3000/mcp"));
        assert!(is_valid_plugin_url("http://[::1]:3000/mcp"));
        assert!(is_valid_plugin_url("http://127.1.2.3/mcp"));

        assert!(!is_valid_plugin_url("http://example.test/mcp"));
        assert!(!is_valid_plugin_url("https://${HOST}/mcp"));
        assert!(!is_valid_plugin_url("https://example.test/$env:TOKEN"));
        assert!(!is_valid_plugin_url("https://example.test/{env:TOKEN}"));
        assert!(!is_valid_plugin_url("https://user@example.test/mcp"));
        assert!(!is_valid_plugin_url("https://user:pw@example.test/mcp"));
        assert!(!is_valid_plugin_url("https://example.test/mcp#frag"));
        assert!(!is_valid_plugin_url("ftp://example.test/mcp"));
        assert!(!is_valid_plugin_url("/relative/mcp"));
        // An EMPTY fragment is `""` in JS and is accepted; only a non-empty one is refused.
        assert!(is_valid_plugin_url("https://example.test/mcp#"));
    }

    #[test]
    fn header_field_validation_follows_the_fetch_spec() {
        assert!(is_valid_header_name("X-Test"));
        assert!(!is_valid_header_name(""));
        assert!(!is_valid_header_name("X Test"));
        assert!(!is_valid_header_name("X:Test"));
        assert!(is_valid_header_value("  bearer abc  "));
        assert!(!is_valid_header_value("a\r\nX-Injected: 1"));
        assert!(!is_valid_header_value("a\0b"));
    }

    // -----------------------------------------------------------------------------------------
    // End-to-end: the verify table
    // -----------------------------------------------------------------------------------------

    #[test]
    fn rejects_every_unsafe_server_and_keeps_the_valid_one() {
        // This is `__tests__/config.test.ts`'s "bad-plugin" case, extended with the Cut-1 `sse`
        // row and the `./bin/../../evil` row MCP-047 adds.
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let Some(path) = fixture.plugin(
            "bad-plugin",
            &format!(
                r#"{{"$schema":"{MCP_SCHEMA}","mcpServers":{{
                  "unsafeCommand":   {{"type":"stdio","command":"../bin/server"}},
                  "escapedCommand":  {{"type":"stdio","command":"./../bin/server"}},
                  "lexicalEscape":   {{"type":"stdio","command":"./bin/../../evil"}},
                  "reservedEnv":     {{"type":"stdio","command":"node","env":{{"PLUGIN_ROOT":"override"}}}},
                  "reservedData":    {{"type":"stdio","command":"node","env":{{"PLUGIN_DATA":"override"}}}},
                  "badArgs":         {{"type":"stdio","command":"node","args":[1]}},
                  "unknownField":    {{"type":"stdio","command":"node","lifecycle":"eager"}},
                  "badCwd":          {{"type":"stdio","command":"node","cwd":"/etc"}},
                  "escapingCwd":     {{"type":"stdio","command":"node","cwd":"${{PLUGIN_ROOT}}/../.."}},
                  "insecureRemote":  {{"type":"streamable-http","url":"http://example.test/mcp"}},
                  "duplicateHeader": {{"type":"streamable-http","url":"https://example.test/mcp","headers":{{"X-Test":"one","x-test":"two"}}}},
                  "legacySse":       {{"type":"sse","url":"https://example.test/sse"}},
                  "notAnObject":     "nope",
                  "valid":           {{"type":"stdio","command":"node"}}
                }}}}"#
            ),
        ) else {
            return;
        };

        let (servers, skips) = load_agent_plugins_in(&[path], &fixture.dirs());
        assert_eq!(names(&servers), vec!["bad-plugin__valid"]);

        let found = reasons(&skips);
        for expected in [
            SkipReason::InvalidCommand,
            SkipReason::CommandEscapesPluginRoot,
            SkipReason::EnvShadowsInjectedVar(PLUGIN_ROOT_VAR.to_owned()),
            SkipReason::EnvShadowsInjectedVar(PLUGIN_DATA_VAR.to_owned()),
            SkipReason::InvalidArgs,
            SkipReason::UnknownField("lifecycle".to_owned()),
            SkipReason::CwdOutsidePlugin,
            SkipReason::UnsafeUrl,
            SkipReason::DuplicateHeader("x-test".to_owned()),
            // Cut 1: NAMED, not silent.
            SkipReason::UnsupportedType("sse".to_owned()),
            SkipReason::NotAnObject,
        ] {
            assert!(found.contains(&expected), "missing skip {expected:?}");
        }
        // `./bin/../../evil` and `./../bin/server` both land on the containment refusal, so the
        // count is what proves the lexical normaliser caught the first one.
        assert_eq!(
            found
                .iter()
                .filter(|reason| **reason == SkipReason::CommandEscapesPluginRoot)
                .count(),
            2
        );
    }

    #[test]
    fn stdio_entry_carries_injected_containment_fields() {
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let Some(path) = fixture.plugin(
            "acme",
            &format!(
                r#"{{"$schema":"{MCP_SCHEMA}","mcpServers":{{
                  "db": {{"type":"stdio","command":"./bin/server","args":["--root","${{PLUGIN_ROOT}}/x"],
                          "env":{{"CACHE":"${{PLUGIN_DATA}}/cache"}},"cwd":"${{PLUGIN_DATA}}/x"}}
                }}}}"#
            ),
        ) else {
            return;
        };
        let dirs = fixture.dirs();
        let (servers, _) = load_agent_plugins_in(std::slice::from_ref(&path), &dirs);
        assert_eq!(servers.len(), 1, "expected exactly one server");
        let Some(server) = servers.first() else { return };

        let root = PathBuf::from(&path);
        let data = dirs.agent_plugin_data("acme");
        assert_eq!(server.name, "acme__db");
        assert_eq!(
            server.entry.command.as_deref(),
            Some(root.join("bin/server").to_string_lossy().as_ref())
        );
        assert_eq!(
            server.entry.args.as_deref(),
            Some(
                [
                    "--root".to_owned(),
                    root.join("x").to_string_lossy().into_owned()
                ]
                .as_slice()
            )
        );
        // `cwd: "${PLUGIN_DATA}/x"` resolves UNDER the data dir — MCP-047's verify row.
        assert_eq!(
            server.entry.cwd.as_deref(),
            Some(data.join("x").to_string_lossy().as_ref())
        );
        assert_eq!(
            server.entry.plugin_data_dir.as_deref(),
            Some(data.to_string_lossy().as_ref())
        );
        // The pair that makes env interpolation inert, and the pair it injects.
        assert_eq!(server.entry.literal_env, Some(true));
        let env = server.entry.env.as_ref().map_or_else(BTreeMap::new, Clone::clone);
        assert_eq!(
            env.get(PLUGIN_ROOT_VAR).map(String::as_str),
            Some(root.to_string_lossy().as_ref())
        );
        assert_eq!(
            env.get(PLUGIN_DATA_VAR).map(String::as_str),
            Some(data.to_string_lossy().as_ref())
        );
        assert_eq!(
            env.get("CACHE").map(String::as_str),
            Some(data.join("cache").to_string_lossy().as_ref())
        );
    }

    #[test]
    fn http_entry_pins_the_transport_and_keeps_header_casing() {
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let Some(path) = fixture.plugin(
            "acme",
            &format!(
                r#"{{"$schema":"{MCP_SCHEMA}","mcpServers":{{
                  "remote": {{"type":"streamable-http","url":"http://127.0.0.1:8080/mcp","headers":{{"X-Test":"one"}}}},
                  "bare":   {{"type":"streamable-http","url":"https://example.test/mcp"}}
                }}}}"#
            ),
        ) else {
            return;
        };
        let (servers, _) = load_agent_plugins_in(&[path], &fixture.dirs());
        assert_eq!(names(&servers), vec!["acme__remote", "acme__bare"]);
        assert_eq!(servers.len(), 2, "expected two servers");
        let (Some(remote), Some(bare)) = (servers.first(), servers.get(1)) else {
            return;
        };
        assert_eq!(remote.entry.http_transport, Some(HttpTransport::StreamableHttp));
        assert_eq!(
            remote
                .entry
                .headers
                .as_ref()
                .and_then(|headers| headers.get("X-Test"))
                .map(String::as_str),
            Some("one")
        );
        // An absent `headers` stays absent rather than becoming an empty map.
        assert!(bare.entry.headers.is_none());
        // The plugin's own key order — not a sorted map's — decides which colliding name the
        // warning reports. Reversed from the test above, the reported key reverses with it.
        assert_eq!(
            translate_headers(
                Some(&RawJson::Object(
                    [
                        ("x-test".to_owned(), RawJson::Other(Value::from("one"))),
                        ("X-Test".to_owned(), RawJson::Other(Value::from("two"))),
                    ]
                    .into_iter()
                    .collect()
                )),
                "acme",
                "remote"
            )
            .err()
            .map(|skip| skip.1),
            Some(SkipReason::DuplicateHeader("X-Test".to_owned()))
        );
        // Nothing a plugin writes can reach the credential store.
        assert!(remote.entry.auth.is_none() && remote.entry.oauth.is_none());
    }

    #[test]
    fn unknown_top_level_key_discards_the_whole_file() {
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let Some(path) = fixture.plugin(
            "acme",
            &format!(
                r#"{{"$schema":"{MCP_SCHEMA}","settings":{{"toolPrefix":"none"}},
                    "mcpServers":{{"db":{{"type":"stdio","command":"node"}}}}}}"#
            ),
        ) else {
            return;
        };
        let (servers, skips) = load_agent_plugins_in(&[path], &fixture.dirs());
        assert!(servers.is_empty());
        assert_eq!(
            reasons(&skips),
            vec![SkipReason::UnknownTopLevelKey("settings".to_owned())]
        );
    }

    #[test]
    fn schema_and_name_are_plugin_level_refusals() {
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let dirs = fixture.dirs();

        let Some(bad_name) = fixture.plugin_with_manifest(
            "bad-name",
            &format!(r#"{{"$schema":"{PLUGIN_SCHEMA}","name":"a--b"}}"#),
            Some(&format!(
                r#"{{"$schema":"{MCP_SCHEMA}","mcpServers":{{"db":{{"type":"stdio","command":"node"}}}}}}"#
            )),
        ) else {
            return;
        };
        let (servers, skips) = load_agent_plugins_in(&[bad_name], &dirs);
        assert!(servers.is_empty());
        assert_eq!(reasons(&skips), vec![SkipReason::InvalidPluginName]);

        let Some(bad_schema) = fixture.plugin_with_manifest(
            "bad-schema",
            r#"{"$schema":"https://agent-plugins.org/schemas/9.9.9/plugin.schema.json","name":"ok"}"#,
            Some("{}"),
        ) else {
            return;
        };
        let (servers, skips) = load_agent_plugins_in(&[bad_schema], &dirs);
        assert!(servers.is_empty());
        assert_eq!(reasons(&skips), vec![SkipReason::SchemaMismatch]);

        let Some(no_mcp) = fixture.plugin_with_manifest(
            "no-mcp",
            &format!(r#"{{"$schema":"{PLUGIN_SCHEMA}","name":"no-mcp","version":"1.0.0"}}"#),
            None,
        ) else {
            return;
        };
        // A plugin with no `mcp.json` is well-formed and simply contributes nothing.
        let (servers, skips) = load_agent_plugins_in(std::slice::from_ref(&no_mcp), &dirs);
        assert!(servers.is_empty() && skips.is_empty());
        let summaries = agent_plugin_summaries(&[no_mcp], &dirs);
        assert_eq!(summaries.len(), 1, "expected one summary");
        let Some(summary) = summaries.first() else { return };
        assert_eq!(summary.name.as_deref(), Some("no-mcp"));
        assert_eq!(summary.server_count, 0);
    }

    #[test]
    fn first_writer_wins_within_and_across_plugins() {
        let Some(fixture) = Fixture::new() else {
            return;
        };
        // `tools.db` and `tools_db` normalise onto one key; the FILE order decides, which is why
        // the server map must never round-trip through a sorting map.
        let Some(collision) = fixture.plugin(
            "collision-plugin",
            &format!(
                r#"{{"$schema":"{MCP_SCHEMA}","mcpServers":{{
                  "tools.db": {{"type":"stdio","command":"node","args":["first.js"]}},
                  "tools_db": {{"type":"stdio","command":"node","args":["second.js"]}}
                }}}}"#
            ),
        ) else {
            return;
        };
        let (servers, skips) =
            load_agent_plugins_in(std::slice::from_ref(&collision), &fixture.dirs());
        assert_eq!(names(&servers), vec!["collision-plugin__tools_db"]);
        assert_eq!(
            servers.first().and_then(|server| server.entry.args.as_ref()),
            Some(&vec!["first.js".to_owned()])
        );
        assert_eq!(
            reasons(&skips),
            vec![SkipReason::DuplicateServer(
                "collision-plugin__tools_db".to_owned()
            )]
        );

        // Cross-plugin: two plugin directories whose manifests claim the same name.
        let Some(second) = fixture.plugin_with_manifest(
            "collision-plugin-copy",
            &format!(r#"{{"$schema":"{PLUGIN_SCHEMA}","name":"collision-plugin"}}"#),
            Some(&format!(
                r#"{{"$schema":"{MCP_SCHEMA}","mcpServers":{{"tools_db":{{"type":"stdio","command":"node","args":["third.js"]}}}}}}"#
            )),
        ) else {
            return;
        };
        let (servers, skips) = load_agent_plugins_in(&[collision, second], &fixture.dirs());
        assert_eq!(names(&servers), vec!["collision-plugin__tools_db"]);
        assert_eq!(
            servers.first().and_then(|server| server.entry.args.as_ref()),
            Some(&vec!["first.js".to_owned()])
        );
        assert!(reasons(&skips).contains(&SkipReason::DuplicateServer(
            "collision-plugin__tools_db".to_owned()
        )));
    }

    #[test]
    fn server_file_order_survives_the_parse() {
        // The contract's `serde_json::Value` trap, asserted for the plugin loader specifically:
        // these four names are deliberately in reverse-sorted order.
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let Some(path) = fixture.plugin(
            "acme",
            &format!(
                r#"{{"$schema":"{MCP_SCHEMA}","mcpServers":{{
                  "zeta":  {{"type":"stdio","command":"node"}},
                  "omega": {{"type":"stdio","command":"node"}},
                  "beta":  {{"type":"stdio","command":"node"}},
                  "alpha": {{"type":"stdio","command":"node"}}
                }}}}"#
            ),
        ) else {
            return;
        };
        let (servers, _) = load_agent_plugins_in(&[path], &fixture.dirs());
        assert_eq!(
            names(&servers),
            vec!["acme__zeta", "acme__omega", "acme__beta", "acme__alpha"]
        );
    }

    #[test]
    fn malformed_entries_do_not_take_their_siblings_with_them() {
        // A `serde` map typed on the value would abort the whole document here; upstream warns per
        // entry and carries on, so the `RawServerEntry::Other` arm has to exist.
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let Some(path) = fixture.plugin(
            "acme",
            &format!(
                r#"{{"$schema":"{MCP_SCHEMA}","mcpServers":{{
                  "scalar": 7,
                  "nulled": null,
                  "listed": [1,2],
                  "good":   {{"type":"stdio","command":"node"}}
                }}}}"#
            ),
        ) else {
            return;
        };
        let (servers, skips) = load_agent_plugins_in(&[path], &fixture.dirs());
        assert_eq!(names(&servers), vec!["acme__good"]);
        assert_eq!(
            reasons(&skips),
            vec![
                SkipReason::NotAnObject,
                SkipReason::NotAnObject,
                SkipReason::NotAnObject
            ]
        );
    }

    #[test]
    fn broken_files_degrade_to_an_empty_surface() {
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let dirs = fixture.dirs();

        let Some(bad_json) = fixture.plugin("acme", "{{{") else {
            return;
        };
        let (servers, skips) = load_agent_plugins_in(&[bad_json], &dirs);
        assert!(servers.is_empty());
        assert_eq!(reasons(&skips), vec![SkipReason::UnreadableMcpConfig]);

        let Some(not_object) = fixture.plugin_with_manifest(
            "arr",
            &format!(r#"{{"$schema":"{PLUGIN_SCHEMA}","name":"arr"}}"#),
            Some("[]"),
        ) else {
            return;
        };
        let (servers, skips) = load_agent_plugins_in(&[not_object], &dirs);
        assert!(servers.is_empty());
        assert_eq!(reasons(&skips), vec![SkipReason::UnreadableMcpConfig]);

        let Some(bad_servers) = fixture.plugin_with_manifest(
            "srv",
            &format!(r#"{{"$schema":"{PLUGIN_SCHEMA}","name":"srv"}}"#),
            Some(&format!(r#"{{"$schema":"{MCP_SCHEMA}","mcpServers":[]}}"#)),
        ) else {
            return;
        };
        let (servers, skips) = load_agent_plugins_in(&[bad_servers], &dirs);
        assert!(servers.is_empty());
        assert!(skips
            .iter()
            .any(|skip| skip.0.contains("mcpServers must be an object")));

        // A path that is not a plugin at all.
        let missing = fixture.cwd.join("nope").to_string_lossy().into_owned();
        let (servers, skips) = load_agent_plugins_in(&[missing], &dirs);
        assert!(servers.is_empty());
        assert_eq!(reasons(&skips), vec![SkipReason::MissingManifest]);

        // A numeric `version` is IGNORED, not fatal — upstream never reads the field.
        let Some(odd_version) = fixture.plugin_with_manifest(
            "ver",
            &format!(r#"{{"$schema":"{PLUGIN_SCHEMA}","name":"ver","version":3,"nonsense":true}}"#),
            Some(&format!(
                r#"{{"$schema":"{MCP_SCHEMA}","mcpServers":{{"db":{{"type":"stdio","command":"node"}}}}}}"#
            )),
        ) else {
            return;
        };
        let (servers, skips) = load_agent_plugins_in(&[odd_version], &dirs);
        assert_eq!(names(&servers), vec!["ver__db"]);
        assert!(skips.is_empty());
    }

    #[test]
    fn warning_text_matches_upstream() {
        // The `String` half of the tuple is what a user greps for, so its shape is asserted rather
        // than left to drift. `__tests__/config.test.ts` asserts this exact substring.
        assert_eq!(
            skip_message("acme", "db", "unsupported type"),
            "Agent Plugin acme skips invalid MCP server db: unsupported type"
        );
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let Some(path) = fixture.plugin(
            "collision-plugin",
            &format!(
                r#"{{"$schema":"{MCP_SCHEMA}","mcpServers":{{
                  "tools.db": {{"type":"stdio","command":"node"}},
                  "tools_db": {{"type":"stdio","command":"node"}}
                }}}}"#
            ),
        ) else {
            return;
        };
        let (_, skips) = load_agent_plugins_in(&[path], &fixture.dirs());
        assert!(skips.iter().any(|skip| skip
            .0
            .contains("normalized server name collision-plugin__tools_db already exists")));
    }
}
