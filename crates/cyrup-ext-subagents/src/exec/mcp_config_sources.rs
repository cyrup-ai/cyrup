//! SUBA-124 — the two MCP-server sources `loadMcpConfig` merges beside the `mcp.json` ladder
//! (settings `packages`, `settings.agentPluginPaths`). Upstream's own file split.
//!
//! SUBA-124 — a faithful port of pi-subagents' `runs/shared/mcp-config-sources.ts` @v0.71.0: the
//! two MCP-server sources `loadMcpConfig` merges BESIDE the `mcp.json` ladder.
//!
//! `crate::exec::mcp_direct_tools::load_mcp_config` used to read only the `mcp.json` files and
//! their imports, so a child granted `mcp:<server>` for a server contributed by a settings
//! `packages` entry or by an `agentPluginPaths` plugin resolved to **no tools at all**, silently:
//! `resolve_direct_tool_names` skips a selection whose server is absent from the config with no
//! warning, and the MCP adapter (`cyrup-mcp`) does load those servers at runtime — so the servers
//! exist, the child simply could not name them. Upstream (`mcp-direct-tool-allowlist.ts:253-262`)
//! merges both loaders' output before resolution:
//!
//! ```ts
//! const packageServers = loadPackageMcpServers(projectRoot);
//! const pluginServers = loadAgentPluginMcpServers(config.settings?.agentPluginPaths, projectRoot);
//! const packageOnlyServers = Object.fromEntries(
//!     Object.entries(packageServers).filter(([name]) => !Object.hasOwn(pluginServers, name)),
//! );
//! return mergeConfigs({ mcpServers: packageOnlyServers }, mergeConfigs({ mcpServers: pluginServers }, config));
//! ```
//!
//! # Why this is a port and not a call into `cyrup-mcp`
//!
//! `cyrup_mcp::agent_plugin` already carries the plugin reader — but as a port of the ADAPTER's own
//! `agent-plugin-loader.ts`, and, decisively, `cyrup-mcp` is a **dev**-dependency of this crate on
//! purpose: resolving a subagent's `mcp:` selectors must not drag the MCP adapter (rmcp, reqwest,
//! oauth2) into a spawn (see this crate's `Cargo.toml`, `[dev-dependencies]`). The shapes the two
//! sides must agree on are therefore asserted by a cross-crate CONFORMANCE test rather than shared
//! by a production edge — the precedent MCP-141/142/146/370 already set for
//! `<agent_dir>/mcp-cache.json` in [`crate::exec::mcp_direct_tools`].
//!
//! # Nothing here executes anything
//!
//! Both loaders are pure reads. A plugin's `command` is resolved and stored, never spawned; a
//! `${PLUGIN_ROOT}` / `${PLUGIN_DATA}` placeholder is expanded textually; a URL is parsed only to
//! be validated. The result is a [`ServerEntry`] map whose only consumer is the direct-tool
//! resolver's `configHash` comparison against the metadata cache.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, MAIN_SEPARATOR, MAIN_SEPARATOR_STR, Path, PathBuf};

use serde_json::Value;
use url::Url;

use super::mcp_direct_tools::{McpDirs, ServerEntry};
use crate::error::SubagentError;

/// `PACKAGE_CONFIG_ROOT` (`:7`) — the subdirectory an `npm:` package source is installed under.
const PACKAGE_CONFIG_ROOT: &str = "npm";
/// `PACKAGE_GIT_ROOT` (`:8`) — the subdirectory a `git:`/URL package source is checked out under.
const PACKAGE_GIT_ROOT: &str = "git";
/// `PLUGIN_SCHEMA` (`:9`) — compared for EQUALITY, not as a prefix or a version range.
const PLUGIN_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
/// `PLUGIN_MCP_SCHEMA` (`:10`).
const PLUGIN_MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";
/// `PLUGIN_STDIO_FIELDS` (`:13`) — the per-transport key allowlist. `url` on a `stdio` entry is
/// therefore an unknown field and rejects the server.
const PLUGIN_STDIO_FIELDS: &[&str] = &["type", "command", "args", "env", "cwd"];
/// `PLUGIN_HTTP_FIELDS` (`:14`).
const PLUGIN_HTTP_FIELDS: &[&str] = &["type", "url", "headers"];
/// The `${PLUGIN_ROOT}` / `${PLUGIN_DATA}` names the loader INJECTS, which a plugin's own `env` may
/// therefore not define (`translatePluginStdioServer`, `:317`).
const PLUGIN_ROOT_VAR: &str = "PLUGIN_ROOT";
/// See [`PLUGIN_ROOT_VAR`].
const PLUGIN_DATA_VAR: &str = "PLUGIN_DATA";
/// `path.join(getAgentDir(), "agent-plugin-data", pluginName)` (`:320`).
const AGENT_PLUGIN_DATA_DIR: &str = "agent-plugin-data";

// -------------------------------------------------------------------------------------------------
// readJson (`:428-441`)
// -------------------------------------------------------------------------------------------------

/// `readJson(filePath)` (`:428`): `undefined` for a MISSING file, and a **throw** for anything else
/// — an unreadable file or invalid JSON.
///
/// The throw matters: upstream's `loadMcpConfig` does not catch it, so a malformed `settings.json`
/// or `package.json` aborts direct-tool resolution entirely (the caller's own `try`/`catch` turns
/// that into an empty allowlist) rather than silently behaving as though the file declared nothing.
/// Degrading to "no servers" would hand the child a DIFFERENT tool set than the operator wrote,
/// with nothing said about it.
///
/// # Errors
///
/// [`SubagentError::Management`] carrying upstream's own sentence verbatim.
fn read_json(file_path: &Path) -> Result<Option<Value>, SubagentError> {
    let content = match std::fs::read_to_string(file_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(SubagentError::Management(format!(
                "Failed to read JSON file '{}': {error}",
                file_path.display()
            )));
        }
    };
    serde_json::from_str::<Value>(&content)
        .map(Some)
        .map_err(|error| {
            SubagentError::Management(format!(
                "Invalid JSON in '{}': {error}",
                file_path.display()
            ))
        })
}

/// The `!value || typeof value !== "object" || Array.isArray(value)` guard both loaders open with.
fn as_object(value: Option<Value>) -> Option<serde_json::Map<String, Value>> {
    match value {
        Some(Value::Object(map)) => Some(map),
        _ => None,
    }
}

// -------------------------------------------------------------------------------------------------
// isMcpServerDefinition (`:45-63`)
// -------------------------------------------------------------------------------------------------

/// `isMcpServerDefinition(value)` (`:45`) — the gate a PACKAGE-contributed server must pass before
/// it is accepted. A plugin server never sees it: `translatePluginServer` builds the definition
/// itself, field by field.
///
/// Every clause is `value[field] !== undefined && <wrong type>` — an ABSENT key is always fine, and
/// an explicit `null` is a wrong type for every field (JSON `null` is neither string, array, record
/// nor boolean), which is upstream's behaviour too because `typeof null === "object"`.
#[must_use]
fn is_mcp_server_definition(value: &Value) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    // `:47-49`.
    for field in [
        "command",
        "socket",
        "cwd",
        "url",
        "bearerToken",
        "bearerTokenEnv",
        "protocolVersion",
        "httpTransport",
        "pluginDataDir",
    ] {
        if map.get(field).is_some_and(|v| !v.is_string()) {
            return false;
        }
    }
    // `:50-52`.
    for field in ["args", "includeTools", "excludeTools"] {
        if map.get(field).is_some_and(|v| !is_string_array(v)) {
            return false;
        }
    }
    // `:53-55`.
    for field in ["env", "headers"] {
        if map.get(field).is_some_and(|v| !is_string_record(v)) {
            return false;
        }
    }
    // `:56-58`.
    for field in ["exposeResources", "literalEnv"] {
        if map.get(field).is_some_and(|v| !v.is_boolean()) {
            return false;
        }
    }
    // `:59`.
    if map
        .get("requestHeadersCommand")
        .is_some_and(|v| !is_request_headers_command(v))
    {
        return false;
    }
    // `:60`.
    if map.get("auth").is_some_and(|value| {
        !matches!(value.as_str(), Some("oauth" | "bearer")) && value != &Value::Bool(false)
    }) {
        return false;
    }
    // `:61`.
    map.get("directTools")
        .is_none_or(|v| v.is_boolean() || is_string_array(v))
}

/// `isStringArray` (`:68`).
fn is_string_array(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|entries| entries.iter().all(Value::is_string))
}

/// `isStringRecord` (`:72`) — a non-array object whose every value is a string.
fn is_string_record(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.values().all(Value::is_string))
}

/// `isRequestHeadersCommand` (`:76`).
fn is_request_headers_command(value: &Value) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    if !map.get("command").is_some_and(Value::is_string) {
        return false;
    }
    if map.get("args").is_some_and(|v| !is_string_array(v)) {
        return false;
    }
    if map.get("env").is_some_and(|v| !is_string_record(v)) {
        return false;
    }
    // `Number.isFinite` — a JSON number is always finite, so the type test is the whole clause.
    map.get("timeoutMs").is_none_or(Value::is_number)
}

/// A gate-passing raw definition, as the [`ServerEntry`] this crate's resolver consumes.
///
/// The five keys [`ServerEntry`] does not carry (`socket`, `directTools`, `httpTransport`,
/// `pluginDataDir`, `literalEnv`) are dropped, which is lossless HERE for a stated reason: none of
/// them is one of `computeMcpServerHash`'s fifteen identity keys except `socket`, and `socket` is
/// emitted unconditionally as `undefined` by
/// [`crate::exec::mcp_direct_tools::compute_mcp_server_hash`] because the writer
/// (`cyrup_mcp::config::to_server_entries`) rejects any entry configuring one. So a definition that
/// round-trips through [`ServerEntry`] hashes to exactly what it hashed before.
fn as_server_entry(value: Value) -> Option<ServerEntry> {
    serde_json::from_value::<ServerEntry>(value).ok()
}

// -------------------------------------------------------------------------------------------------
// loadPackageMcpServers (`:83-119`)
// -------------------------------------------------------------------------------------------------

/// `loadPackageMcpServers(cwd)` (`:83`): every settings `packages` entry's `package.json`, its
/// `pi.mcp` config path(s), and the `mcpServers` those declare — namespaced
/// `` `${package}__${server}` ``, first writer wins.
///
/// # Errors
///
/// [`read_json`]'s throw, from a `package.json` or a `pi.mcp` config that exists and is not valid
/// JSON.
pub(crate) fn load_package_mcp_servers(
    cwd: &Path,
    dirs: &McpDirs,
) -> Result<BTreeMap<String, ServerEntry>, SubagentError> {
    let mut servers: BTreeMap<String, ServerEntry> = BTreeMap::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for package_root in configured_package_roots(cwd, dirs)? {
        // `:85-86`.
        let Some(manifest) = as_object(read_json(&package_root.join("package.json"))?) else {
            continue;
        };
        // `:87-88`.
        let Some(package_name) = manifest
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        // `:89-94` — `manifest.pi?.mcp`, accepted as one string or as an array of strings; anything
        // else contributes no config paths at all (NOT an error).
        let mcp_value = manifest
            .get("pi")
            .and_then(Value::as_object)
            .and_then(|pi| pi.get("mcp"));
        let config_paths: Vec<String> = match mcp_value {
            Some(Value::String(one)) => vec![one.clone()],
            // `Array.isArray(mcpValue) && mcpValue.every(isString)` — ONE non-string member drops
            // the whole list, it does not filter it.
            Some(Value::Array(entries)) if entries.iter().all(Value::is_string) => entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        // `:95`.
        let package_prefix = format_name(package_name, "package");

        for config_path in config_paths {
            // `:98-99`.
            let Some(resolved_path) = resolve_package_config_path(&package_root, &config_path)
            else {
                continue;
            };
            // `:100-101`.
            let Some(config) = as_object(read_json(&resolved_path)?) else {
                continue;
            };
            // `:102-103`.
            let Some(raw_servers) = config.get("mcpServers").and_then(Value::as_object) else {
                continue;
            };
            for (server_name, definition) in raw_servers {
                // `:105`.
                if !is_mcp_server_definition(definition) {
                    continue;
                }
                // `:106`.
                let normalized_name =
                    format!("{package_prefix}__{}", format_name(server_name, "server"));
                // `:107-109` — first writer wins, ACROSS packages as well as within one, because
                // `seen` is scoped to the whole call.
                if !seen.insert(normalized_name.clone()) {
                    continue;
                }
                let Some(entry) = as_server_entry(definition.clone()) else {
                    continue;
                };
                servers.insert(normalized_name, entry);
            }
        }
    }

    Ok(servers)
}

/// `getConfiguredPackageRoots(cwd)` (`:145-169`): the project `settings.json` first, then the agent
/// one, each entry resolved against ITS OWN file's directory.
///
/// # Errors
///
/// [`read_json`]'s throw, from a `settings.json` that exists and is not valid JSON, or
/// [`crate::discovery::find_configured_project_root`]'s malformed-`projectRootResolution` refusal.
fn configured_package_roots(cwd: &Path, dirs: &McpDirs) -> Result<Vec<PathBuf>, SubagentError> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let project_config_dir = project_config_dir(cwd)?;
    // `:147-150` — the ORDER is load-bearing: a project package shadows an agent one of the same
    // normalized name, because `loadPackageMcpServers`' `seen` set is first-writer-wins.
    let sources = [
        (project_config_dir.join("settings.json"), project_config_dir),
        (dirs.agent_dir.join("settings.json"), dirs.agent_dir.clone()),
    ];
    for (settings_path, base_dir) in sources {
        // `:152-153`.
        let Some(settings) = as_object(read_json(&settings_path)?) else {
            continue;
        };
        // `:154-155`.
        let Some(packages) = settings.get("packages").and_then(Value::as_array) else {
            continue;
        };
        for entry in packages {
            // `:157-161` — a bare string, or `{ source: "…" }`; anything else is skipped.
            let package_source = match entry {
                Value::String(source) => Some(source.as_str()),
                Value::Object(map) => map.get("source").and_then(Value::as_str),
                _ => None,
            };
            let Some(package_source) = package_source else {
                continue;
            };
            // `:163-164`.
            if let Some(root) = resolve_package_root(package_source, &base_dir)
                && !roots.contains(&root)
            {
                roots.push(root);
            }
        }
    }
    Ok(roots)
}

/// `findProjectConfigDir(cwd)` (`:171-174`).
///
/// # Errors
///
/// [`crate::discovery::find_configured_project_root`]'s malformed-`projectRootResolution` refusal —
/// upstream lets `readProjectRootResolution` throw straight through here, and degrading to the
/// nearest root would read a DIFFERENT project's `settings.json` than the one the operator named.
fn project_config_dir(cwd: &Path) -> Result<PathBuf, SubagentError> {
    let resolved = lexical_absolute(cwd);
    let project_root =
        crate::discovery::find_configured_project_root(&resolved)?.unwrap_or(resolved);
    Ok(project_root.join(crate::discovery::PROJECT_CONFIG_DIR_SEGMENT))
}

/// `resolvePackageRoot(source, baseDir)` (`:176-192`) — the four source grammars.
fn resolve_package_root(source: &str, base_dir: &Path) -> Option<PathBuf> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return None;
    }
    // `:179-182` — `npm:<name>[@<version>]`, installed under `<baseDir>/npm/node_modules/<name>`.
    if let Some(spec) = trimmed.strip_prefix("npm:") {
        let package_name = parse_npm_package_name(spec)?;
        return resolve_contained_path(
            &base_dir.join(PACKAGE_CONFIG_ROOT).join("node_modules"),
            &package_name,
        );
    }
    // `:184-187` — `git:…`, or a bare URL / `git@host:path`, checked out under
    // `<baseDir>/git/<host>/<repoPath>`.
    if trimmed.starts_with("git:") || is_bare_git_url(trimmed) {
        let spec = if trimmed.starts_with("git:") {
            trimmed.to_string()
        } else {
            format!("git:{trimmed}")
        };
        let parsed = parse_git_package_path(&spec)?;
        return Some(
            base_dir
                .join(PACKAGE_GIT_ROOT)
                .join(parsed.0)
                .join(parsed.1),
        );
    }
    // `:189-192` — a local path, `file:`-prefixed or not, `~`-expanded, relative to `baseDir`.
    let local_path = trimmed.strip_prefix("file:").unwrap_or(trimmed);
    let home = crate::paths::home_dir();
    if local_path == "~" {
        return Some(home);
    }
    if let Some(rest) = local_path.strip_prefix("~/") {
        return Some(home.join(rest));
    }
    Some(if Path::new(local_path).is_absolute() {
        lexical_normalize(Path::new(local_path))
    } else {
        node_resolve(base_dir, Path::new(local_path))
    })
}

/// `/^(?:https?:\/\/|ssh:\/\/|git@[^:]+:)/` (`:184`).
fn is_bare_git_url(spec: &str) -> bool {
    if spec.starts_with("http://") || spec.starts_with("https://") || spec.starts_with("ssh://") {
        return true;
    }
    // `git@[^:]+:` — at least one non-colon character before the first colon. `find` returns the
    // FIRST colon, so everything before it is colon-free by construction.
    spec.strip_prefix("git@")
        .is_some_and(|rest| rest.find(':').is_some_and(|index| index > 0))
}

/// `parseNpmPackageName(source)` (`:194-200`). `source` here is already `slice(4)`'d.
fn parse_npm_package_name(spec: &str) -> Option<String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    // `/^(@?[^@]+(?:\/[^@]+)?)(?:@(.+))?$/` — the name is everything up to a version `@` that is
    // not the leading scope `@`. `match?.[1] ?? spec` means an unmatched spec falls back whole.
    let name = npm_name_capture(spec).unwrap_or(spec);
    // `resolveContainedPath("/", packageName) ? packageName : undefined` — the traversal test.
    resolve_contained_path(Path::new("/"), name).map(|_| name.to_string())
}

/// The first capture group of `/^(@?[^@]+(?:\/[^@]+)?)(?:@(.+))?$/`, transcribed.
fn npm_name_capture(spec: &str) -> Option<&str> {
    let (scope_at, rest) = match spec.strip_prefix('@') {
        Some(rest) => (true, rest),
        None => (false, spec),
    };
    // `[^@]+` is greedy but must leave the optional `(?:@(.+))?` satisfiable, so the name ends at
    // the FIRST `@` in `rest` (each `[^@]+` cannot cross one) — and a trailing `@` with nothing
    // after it fails `(.+)`, so the whole regex fails and the caller falls back to `spec`.
    let name_end = rest.find('@').unwrap_or(rest.len());
    if name_end == 0 {
        return None;
    }
    if name_end < rest.len() && rest.len() == name_end + 1 {
        return None; // a bare trailing `@`: `(.+)` cannot match
    }
    let name = rest.get(..name_end)?;
    // `[^@]+(?:\/[^@]+)?` — at most ONE `/`, and neither side may be empty.
    let mut segments = name.split('/');
    let first = segments.next().unwrap_or_default();
    let second = segments.next();
    if segments.next().is_some() || first.is_empty() || second.is_some_and(str::is_empty) {
        return None;
    }
    // Sliced out of `spec` rather than returned as `name`, so a scoped package keeps its leading
    // `@` — `rest` is `spec` with that `@` already stripped.
    spec.get(..if scope_at { name_end + 1 } else { name_end })
}

/// `parseGitPackagePath(source)` (`:202-231`) — `(host, repoPath)`.
fn parse_git_package_path(source: &str) -> Option<(String, String)> {
    let spec = source.get(4..)?.trim();
    if spec.is_empty() {
        return None;
    }

    let (host, repo_path) = if let Some(rest) = spec.strip_prefix("git@") {
        // `/^git@([^:]+):(.+)$/` — `[^:]+` cannot cross a colon, so the host ends at the FIRST one.
        let index = rest.find(':')?;
        let host = rest.get(..index)?;
        let path = rest.get(index + 1..)?;
        if host.is_empty() || path.is_empty() {
            return None;
        }
        (host.to_string(), path.to_string())
    } else if has_url_scheme(spec) {
        let url = Url::parse(spec).ok()?;
        (
            url.host_str().unwrap_or_default().to_string(),
            url.path().trim_start_matches('/').to_string(),
        )
    } else {
        let index = spec.find('/')?;
        (
            spec.get(..index)?.to_string(),
            spec.get(index + 1..)?.to_string(),
        )
    };

    // `stripGitRef(repoPath).replace(/\.git$/, "").replace(/^\/+/, "")`.
    let stripped = strip_git_ref(&repo_path);
    let without_git = stripped.strip_suffix(".git").unwrap_or(stripped);
    let normalized_path = without_git.trim_start_matches('/');
    if host.is_empty()
        || !is_safe_package_path(&host)
        || !is_safe_package_path(normalized_path)
        || normalized_path.split(['\\', '/']).count() < 2
    {
        return None;
    }
    Some((host, normalized_path.to_string()))
}

/// `/^[a-z][a-z0-9+.-]*:\/\//i` (`:213`).
fn has_url_scheme(spec: &str) -> bool {
    let Some(index) = spec.find("://") else {
        return false;
    };
    let Some(scheme) = spec.get(..index) else {
        return false;
    };
    let mut bytes = scheme.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'))
}

/// `stripGitRef(repoPath)` (`:233-238`) — truncate at the first `@` or `#`, whichever comes first.
fn strip_git_ref(repo_path: &str) -> &str {
    match repo_path.find(['@', '#']) {
        Some(index) => repo_path.get(..index).unwrap_or(repo_path),
        None => repo_path,
    }
}

/// `isSafePackagePath(value)` (`:240-244`).
fn is_safe_package_path(value: &str) -> bool {
    !value.is_empty()
        && !Path::new(value).is_absolute()
        && value
            .split(['\\', '/'])
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// `resolvePackageConfigPath(packageRoot, configuredPath)` (`:246-257`): lexical containment, then
/// `existsSync`, then `isFile`, then containment AGAIN over both realpaths — a `pi.mcp` symlink
/// pointing outside the package is refused.
fn resolve_package_config_path(package_root: &Path, configured_path: &str) -> Option<PathBuf> {
    let lexical_path = resolve_contained_path(package_root, configured_path)?;
    if !lexical_path.exists() || !lexical_path.is_file() {
        return None;
    }
    let package_real = std::fs::canonicalize(package_root).ok()?;
    let config_real = std::fs::canonicalize(&lexical_path).ok()?;
    resolve_contained_path(&package_real, &config_real.to_string_lossy())
}

// -------------------------------------------------------------------------------------------------
// loadAgentPluginMcpServers (`:120-146`)
// -------------------------------------------------------------------------------------------------

/// `loadAgentPluginMcpServers(paths, cwd)` (`:120`): each `agentPluginPaths` entry's `plugin.json`
/// + `mcp.json`, translated, namespaced `` `${plugin}__${server}` ``, first writer wins.
///
/// `paths` is upstream's `unknown` — `config.settings?.agentPluginPaths`, unvalidated at parse time
/// (`parseSettings:309` stores it raw) and filtered HERE: a non-array yields nothing, and a
/// non-string member is skipped.
///
/// # Errors
///
/// [`read_json`]'s throw, from a `plugin.json` or `mcp.json` that exists and is not valid JSON.
pub(crate) fn load_agent_plugin_mcp_servers(
    paths: Option<&Value>,
    cwd: &Path,
    dirs: &McpDirs,
) -> Result<BTreeMap<String, ServerEntry>, SubagentError> {
    let mut servers: BTreeMap<String, ServerEntry> = BTreeMap::new();
    // `:122` — `if (!Array.isArray(paths)) return servers;`
    let Some(entries) = paths.and_then(Value::as_array) else {
        return Ok(servers);
    };

    for configured_path in entries {
        // `:125`.
        let Some(configured_path) = configured_path.as_str() else {
            continue;
        };
        let plugin_root = resolve_plugin_path(configured_path, cwd);
        // `:127-128`.
        let manifest = read_json(&plugin_root.join("plugin.json"))?;
        let Some(plugin_name) = valid_plugin_manifest_name(manifest.as_ref()) else {
            continue;
        };
        // `:129-131`.
        let config = read_json(&plugin_root.join("mcp.json"))?;
        let Some(raw_servers) = valid_plugin_config_servers(config.as_ref()) else {
            continue;
        };

        for (server_name, raw_definition) in &raw_servers {
            // `:134-135`.
            let Some(definition) =
                translate_plugin_server(&plugin_name, &plugin_root, raw_definition, dirs)
            else {
                continue;
            };
            // `:136`.
            let normalized_name = format!(
                "{}__{}",
                format_name(&plugin_name, "plugin"),
                format_name(server_name, "server")
            );
            // `:137` — `if (Object.hasOwn(servers, normalizedName)) continue;` First writer wins,
            // across plugins as well as within one.
            if servers.contains_key(&normalized_name) {
                continue;
            }
            servers.insert(normalized_name, definition);
        }
    }

    Ok(servers)
}

/// `isValidPluginManifest(value)` (`:281-289`), returning the accepted `name`.
fn valid_plugin_manifest_name(value: Option<&Value>) -> Option<String> {
    let map = value?.as_object()?;
    if map.get("$schema").and_then(Value::as_str) != Some(PLUGIN_SCHEMA) {
        return None;
    }
    let name = map.get("name").and_then(Value::as_str)?;
    // `name.length >= 1 && name.length <= 64` — a JS string length is UTF-16 code units, but the
    // character class below admits only ASCII, so byte length is the same number for every name
    // that can pass.
    if name.is_empty() || name.len() > 64 || !is_valid_plugin_name(name) {
        return None;
    }
    Some(name.to_string())
}

/// `PLUGIN_NAME_PATTERN` (`:12`): `/^(?!.*(?:--|\.\.))[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$/`.
fn is_valid_plugin_name(name: &str) -> bool {
    // The negative lookahead applies to the WHOLE string, not to the tail.
    if name.contains("--") || name.contains("..") {
        return false;
    }
    let bytes = name.as_bytes();
    let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    let Some(&first) = bytes.first() else {
        return false;
    };
    if !alnum(first) {
        return false;
    }
    if bytes.len() == 1 {
        return true;
    }
    let Some(&last) = bytes.last() else {
        return false;
    };
    if !alnum(last) {
        return false;
    }
    bytes
        .get(1..bytes.len().saturating_sub(1))
        .is_some_and(|middle| middle.iter().all(|&b| alnum(b) || b == b'.' || b == b'-'))
}

/// `isValidPluginConfig(value)` (`:291-300`) — `$schema` equality, NO key but `$schema`/`mcpServers`
/// (an unknown top-level key discards the whole file), and `mcpServers` a non-array object.
fn valid_plugin_config_servers(value: Option<&Value>) -> Option<serde_json::Map<String, Value>> {
    let map = value?.as_object()?;
    if map.get("$schema").and_then(Value::as_str) != Some(PLUGIN_MCP_SCHEMA) {
        return None;
    }
    if !map
        .keys()
        .all(|key| key == "$schema" || key == "mcpServers")
    {
        return None;
    }
    map.get("mcpServers").and_then(Value::as_object).cloned()
}

/// `translatePluginServer(pluginName, pluginRoot, value)` (`:302-311`).
fn translate_plugin_server(
    plugin_name: &str,
    plugin_root: &Path,
    value: &Value,
    dirs: &McpDirs,
) -> Option<ServerEntry> {
    let raw = value.as_object()?;
    match raw.get("type").and_then(Value::as_str) {
        Some("stdio") => translate_plugin_stdio_server(plugin_name, plugin_root, raw, dirs),
        // Cut 1 kept `sse` alongside `streamable-http`, both carried on `httpTransport`.
        Some("streamable-http" | "sse") => translate_plugin_http_server(raw),
        _ => None,
    }
}

/// `translatePluginStdioServer` (`:313-344`).
fn translate_plugin_stdio_server(
    plugin_name: &str,
    plugin_root: &Path,
    raw: &serde_json::Map<String, Value>,
    dirs: &McpDirs,
) -> Option<ServerEntry> {
    // `:318` — the per-transport key allowlist.
    if raw
        .keys()
        .any(|key| !PLUGIN_STDIO_FIELDS.contains(&key.as_str()))
    {
        return None;
    }
    // `:319-320`.
    let command = raw.get("command").and_then(Value::as_str)?;
    if command.is_empty() {
        return None;
    }
    if !is_bare_command(command) && !command.starts_with("./") {
        return None;
    }
    // `:321-324` — a PRESENT `args`/`env` of the wrong shape rejects the server; an absent one is
    // the empty default.
    let args = match raw.get("args") {
        None => Vec::new(),
        Some(value) if is_string_array(value) => value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.as_str().map(str::to_string))
            .collect(),
        Some(_) => return None,
    };
    let env: BTreeMap<String, String> = match raw.get("env") {
        None => BTreeMap::new(),
        Some(value) if is_string_record(value) => value
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(key, value)| value.as_str().map(|v| (key.clone(), v.to_string())))
            .collect(),
        Some(_) => return None,
    };
    // `:325` — the plugin may not define the two names the loader injects, or env interpolation
    // stops being a read primitive.
    if env.contains_key(PLUGIN_ROOT_VAR) || env.contains_key(PLUGIN_DATA_VAR) {
        return None;
    }

    // `:327`.
    let plugin_data_dir = dirs.agent_dir.join(AGENT_PLUGIN_DATA_DIR).join(plugin_name);
    let plugin_root_text = plugin_root.to_string_lossy().into_owned();
    let plugin_data_text = plugin_data_dir.to_string_lossy().into_owned();
    // `:328-329` — a `./…` command is resolved INSIDE the plugin root or the server is refused.
    let resolved_command = if command.starts_with("./") {
        resolve_contained_path(plugin_root, command)?
            .to_string_lossy()
            .into_owned()
    } else {
        command.to_string()
    };
    // `:330-331`.
    let cwd = resolve_plugin_cwd(raw.get("cwd"), plugin_root, &plugin_data_dir)?;

    // `:333-343`. `PLUGIN_ROOT`/`PLUGIN_DATA` are appended AFTER the plugin's own env, so they win
    // — which is why the shadowing check above exists at all.
    let mut resolved_env: BTreeMap<String, Value> = env
        .into_iter()
        .map(|(key, value)| {
            (
                key,
                Value::String(expand_plugin_placeholders(
                    &value,
                    &plugin_root_text,
                    &plugin_data_text,
                )),
            )
        })
        .collect();
    resolved_env.insert(
        PLUGIN_ROOT_VAR.to_string(),
        Value::String(plugin_root_text.clone()),
    );
    resolved_env.insert(
        PLUGIN_DATA_VAR.to_string(),
        Value::String(plugin_data_text.clone()),
    );

    Some(ServerEntry {
        command: Some(resolved_command),
        args: Some(
            args.iter()
                .map(|value| {
                    expand_plugin_placeholders(value, &plugin_root_text, &plugin_data_text)
                })
                .collect(),
        ),
        env: Some(resolved_env),
        cwd: Some(cwd.to_string_lossy().into_owned()),
        ..ServerEntry::default()
    })
}

/// `translatePluginHttpServer(raw)` (`:346-366`).
fn translate_plugin_http_server(raw: &serde_json::Map<String, Value>) -> Option<ServerEntry> {
    // `:351`.
    if raw
        .keys()
        .any(|key| !PLUGIN_HTTP_FIELDS.contains(&key.as_str()))
    {
        return None;
    }
    // `:352`.
    let url = raw.get("url").and_then(Value::as_str)?;
    if !is_valid_plugin_url(url) {
        return None;
    }
    // `:353-354`.
    let headers: Option<BTreeMap<String, Value>> = match raw.get("headers") {
        None => None,
        Some(value) if is_string_record(value) => Some(
            value
                .as_object()
                .into_iter()
                .flatten()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ),
        Some(_) => return None,
    };
    if let Some(headers) = headers.as_ref() {
        // `:356-361` — two keys differing only in case reject the whole server rather than one
        // winning silently.
        let mut normalized: BTreeSet<String> = BTreeSet::new();
        for key in headers.keys() {
            if !normalized.insert(key.to_lowercase()) {
                return None;
            }
        }
        // `:362-366` — `new Headers(headers)` throws on an invalid field name or value.
        for (key, value) in headers {
            if !is_valid_header_name(key) || !value.as_str().is_some_and(is_valid_header_value) {
                return None;
            }
        }
    }
    Some(ServerEntry {
        url: Some(url.to_string()),
        headers,
        ..ServerEntry::default()
    })
}

/// `resolvePluginCwd(value, pluginRoot, pluginDataDir)` (`:368-376`): absent is the plugin root,
/// and the only accepted spellings are `./…`, `${PLUGIN_ROOT}…` and `${PLUGIN_DATA}…`, each
/// contained in its own anchor.
fn resolve_plugin_cwd(
    value: Option<&Value>,
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
    if value == "${PLUGIN_ROOT}" || value.starts_with("${PLUGIN_ROOT}/") {
        return resolve_contained_path(plugin_root, &value.replacen("${PLUGIN_ROOT}", ".", 1));
    }
    if value == "${PLUGIN_DATA}" || value.starts_with("${PLUGIN_DATA}/") {
        return resolve_contained_path(plugin_data_dir, &value.replacen("${PLUGIN_DATA}", ".", 1));
    }
    None
}

/// `expandPluginPlaceholders(value, pluginRoot, pluginDataDir)` (`:378-380`).
fn expand_plugin_placeholders(value: &str, plugin_root: &str, plugin_data_dir: &str) -> String {
    value
        .replace("${PLUGIN_ROOT}", plugin_root)
        .replace("${PLUGIN_DATA}", plugin_data_dir)
}

/// `isBareCommand(command)` (`:382-387`).
fn is_bare_command(command: &str) -> bool {
    !command.contains('/')
        && !command.contains('\\')
        && !command.contains("${PLUGIN_ROOT}")
        && !command.contains("${PLUGIN_DATA}")
}

/// `isValidPluginUrl(value)` (`:389-402`) — no interpolation syntax, `http`/`https` only, no
/// userinfo, no fragment, and plaintext only to this machine.
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
    // JS reads `url.username`/`password`/`hash`, all `""` when absent, so the test is
    // non-emptiness: `https://h/#` is ACCEPTED upstream.
    if !url.username().is_empty()
        || url.password().is_some_and(|password| !password.is_empty())
        || url.fragment().is_some_and(|fragment| !fragment.is_empty())
    {
        return false;
    }
    if url.scheme() == "https" {
        return true;
    }
    is_loopback_host(url.host_str().unwrap_or_default())
}

/// The loopback test of `isValidPluginUrl` (`:401`).
fn is_loopback_host(hostname: &str) -> bool {
    let host = hostname.to_lowercase();
    if host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]" {
        return true;
    }
    // `/^127(?:\.\d{1,3}){3}$/`, transcribed including its looseness.
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

/// A WHATWG header NAME (a non-empty HTTP token) — what `new Headers({…})` enforces at `:363`.
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

/// A WHATWG header VALUE: after stripping leading/trailing HTTP whitespace, no NUL, CR or LF. The
/// STORED string is the caller's original — the trim is for the test only.
fn is_valid_header_value(value: &str) -> bool {
    let trimmed = value.trim_matches(|ch| ch == '\t' || ch == '\n' || ch == '\r' || ch == ' ');
    !trimmed.contains(['\0', '\n', '\r'])
}

/// `formatName(value, fallback)` (`:420-422`).
fn format_name(value: &str, fallback: &str) -> String {
    // `replace(/[^A-Za-z0-9_-]+/g, "_")` — a RUN of disallowed characters collapses to ONE `_`.
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
    // `replace(/^[_-]+|[_-]+$/g, "")`.
    let trimmed = collapsed.trim_matches(|ch| ch == '_' || ch == '-');
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

/// `resolvePluginPath(configuredPath, cwd)` (`:275-279`), reading `$HOME`.
///
/// Anchored on [`crate::paths::home_dir`] rather than a bare `$HOME` read because this crate forbids
/// `unsafe` env mutation and `home_dir` is the ONE home every other path in this module resolves
/// against — including the `~` expansion [`crate::exec::mcp_direct_tools::McpDirs::from_env`]
/// performs. Using a second, different home here would make a `~/…` plugin path and a `~/…` server
/// `cwd` in the same file resolve to different places.
fn resolve_plugin_path(configured_path: &str, cwd: &Path) -> PathBuf {
    let home = crate::paths::home_dir();
    if configured_path == "~" {
        return node_resolve(&home, Path::new("."));
    }
    if let Some(rest) = configured_path.strip_prefix("~/") {
        return node_resolve(&home, Path::new(rest));
    }
    if Path::new(configured_path).is_absolute() {
        return lexical_normalize(Path::new(configured_path));
    }
    node_resolve(cwd, Path::new(configured_path))
}

// -------------------------------------------------------------------------------------------------
// Path containment (`resolveContainedPath`, `:259-265`)
// -------------------------------------------------------------------------------------------------

/// `resolveContainedPath(root, value)` (`:259`).
///
/// NOT `Path::strip_prefix`: that is a component test on the LITERAL path and would accept
/// `root/../../etc`. Node's `path.resolve` normalises `..` lexically first, which is what makes the
/// subsequent `path.relative` string test meaningful — including its consequence that a directory
/// literally named `..foo` inside the root is refused, because `rel.startsWith("..")` is a string
/// test. Symlinks are deliberately NOT resolved here, because upstream does not resolve them and a
/// stricter check is still a divergent one.
fn resolve_contained_path(root: &Path, candidate: &str) -> Option<PathBuf> {
    let resolved = node_resolve(root, Path::new(candidate));
    let relative = lexical_relative(root, &resolved);
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

/// `path.resolve(base, value)`: join, make absolute against the process cwd if it still is not, then
/// normalise lexically. `Path::join` already reproduces Node's "an absolute right-hand side wins".
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

/// `path.resolve(value)` for a single path — `path.resolve(cwd)` in `findProjectConfigDir`.
fn lexical_absolute(value: &Path) -> PathBuf {
    node_resolve(Path::new(""), value)
}

/// Node's `path.normalize`, purely lexically: drop `.`, pop a `..` against a preceding real
/// component, and CLAMP a `..` that would escape the root (`/a/../..` is `/`).
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

/// `path.relative(from, to)` for two already-normalised paths, returned as a **String** because the
/// caller's test is a string test (`rel.startsWith("..")`).
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
